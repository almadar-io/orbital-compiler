use super::collect_all_traits;
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{EmitDefinition, EventScope, OrbitalSchema, TraitReference};
use std::collections::HashMap;

// =============================================================================
// COMP-7: validate_scope_boundary
// =============================================================================

/// Validate that composing traits only remap events declared `scope: 'external'`.
///
/// A `TraitReference::Reference` can carry `events: { SRC: DST }` (event-key
/// rename map) or `listens: [...]` (replacement listens list). Each entry in
/// those maps is a "wire" into the imported trait. If the wire touches an
/// event the imported atom declared `scope: 'internal'`, the composition is
/// reaching past the atom's encapsulation and exposing an internal event to
/// cross-orbital traffic.
///
/// This validator is intentionally best-effort:
///   * When the reference's target trait can be resolved **within the same
///     schema** (another orbital inlines a same-named trait, or the author
///     hand-inlined it), we consult its `emits[]` and warn on internal-scoped
///     overrides.
///   * When the reference targets a std behavior (`from: "std/behaviors/..."`)
///     we parse the embedded .orb registry to read the imported trait's
///     `emits[]`. This covers the most common composition path.
///   * When neither applies (an external .orb file that hasn't been loaded by
///     the time this validator runs, e.g. pre-inline CLI validation of a
///     workspace-local import), we skip the check. A future phase that wires
///     the loader into validation can graduate these skips into checks.
///
/// Severity: warning. The plan graduates this to an error after Phase 4 once
/// every atom declares its `scope` explicitly
/// (see `docs/Almadar_Agent_Behaviors_Plan.md`).
pub(crate) fn validate_scope_boundary(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build a fast trait lookup: trait name -> its emits[] (if any). This
    // covers the in-schema case: the `Alias.traits.X` target also appears
    // inline in one of the schema's orbitals (true after the inline phase
    // for anything referenced from another orbital in the same workspace).
    let mut in_schema_emits: HashMap<String, &Vec<EmitDefinition>> = HashMap::new();
    for trait_def in collect_all_traits(schema) {
        if let Some(emits) = &trait_def.emits {
            in_schema_emits.insert(trait_def.name.clone(), emits);
        }
    }

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        // Build alias -> source path map from `uses:` so we can try to
        // resolve std-behavior references. Only std paths
        // ("std/behaviors/...") are resolvable here without the full
        // loader.
        let mut alias_source: HashMap<&str, &str> = HashMap::new();
        if let Some(uses) = &orbital.uses {
            for decl in uses {
                alias_source.insert(decl.alias.as_str(), decl.from.as_str());
            }
        }

        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Reference {
                reference,
                events,
                listens,
                ..
            } = trait_ref
            else {
                continue;
            };

            // Nothing to check if there are no overrides that can pierce
            // scope.
            if events.is_none() && listens.is_none() {
                continue;
            }

            // Parse "Alias.traits.TraitName" — only this shape exposes a
            // per-trait scope surface. Other shapes (bare name fallback)
            // are skipped.
            let Some(ref_str) = reference.as_deref() else {
                continue;
            };
            let parts: Vec<&str> = ref_str.split('.').collect();
            if parts.len() != 3 || parts[1] != "traits" {
                continue;
            }
            let alias = parts[0];
            let target_trait_name = parts[2];

            // Try to get the target trait's emits from one of two sources.
            // `owned_holder` keeps the owned Vec alive when we read it
            // from an embedded std behavior; the in-schema path uses a
            // borrow from `in_schema_emits`.
            let owned_holder: Option<Vec<EmitDefinition>>;
            let target_emits: &Vec<EmitDefinition> =
                if let Some(emits) = in_schema_emits.get(target_trait_name) {
                    *emits
                } else if let Some(src) = alias_source.get(alias) {
                    // Attempt to resolve std-behavior content. Other
                    // sources (local .orb paths) aren't loaded here —
                    // skip silently.
                    if let Some(content) = crate::embedded::get_std_behavior_content(src) {
                        owned_holder = parse_emits_from_std_content(content, target_trait_name);
                        match &owned_holder {
                            Some(e) => e,
                            // Target trait found but has no emits[] —
                            // nothing internal to guard against.
                            None => continue,
                        }
                    } else {
                        continue;
                    }
                } else {
                    // No `uses:` entry for the alias and not in-schema —
                    // we can't verify scope. Skip silently.
                    continue;
                };

            let path_base = format!("orbitals[{}].traits[{}]", oi, ti);

            // Check `events:` rename map entries. The SOURCE key is the
            // upstream atom's event name — that's what we look up in
            // emits[].
            if let Some(map) = events {
                for (source_event, target_event) in map {
                    if let Some(emit) = target_emits.iter().find(|e| &e.event == source_event) {
                        if emit.scope == Some(EventScope::Internal) {
                            result.add_error(
                                ValidationError::warning(
                                    ErrorCode::InternalEventExposed,
                                    format!("{}.events", path_base),
                                    format!(
                                        "Event '{}' on trait '{}' is declared scope: 'internal'; \
                                         composing traits can only remap events declared scope: 'external'. \
                                         Remove the rename '{}': '{}' or change the upstream atom's scope.",
                                        source_event, target_trait_name, source_event, target_event
                                    ),
                                )
                                .with_suggestion(
                                    "Promote the upstream event to scope: 'external' if it is truly \
                                     part of the atom's external surface, or drop the override.",
                                ),
                            );
                        }
                    }
                }
            }

            // Check `listens:` replacement entries. Each entry names an
            // event the composed trait consumes. If the upstream atom's
            // declaration of that same event key is internal-scoped, the
            // override is reaching into private traffic.
            if let Some(listen_overrides) = listens {
                for listen in listen_overrides {
                    if let Some(emit) = target_emits.iter().find(|e| e.event == listen.event) {
                        if emit.scope == Some(EventScope::Internal) {
                            result.add_error(
                                ValidationError::warning(
                                    ErrorCode::InternalEventExposed,
                                    format!("{}.listens", path_base),
                                    format!(
                                        "Listen override for event '{}' touches trait '{}', \
                                         which declares that event as scope: 'internal'. \
                                         Composing traits can only wire events declared scope: 'external'.",
                                        listen.event, target_trait_name
                                    ),
                                )
                                .with_suggestion(
                                    "Promote the upstream event to scope: 'external' or remove this \
                                     listen override.",
                                ),
                            );
                        }
                    }
                }
            }
        }
    }

    result
}

/// Parse an embedded std-behavior JSON blob and extract the `emits[]` for a
/// specific trait name. Returns `None` when parsing fails or the trait isn't
/// present. Failures are treated as "skip the check" rather than errors so a
/// malformed embedded payload doesn't derail unrelated validation; if that
/// ever happens the embedded-registry tests will flag it.
pub(crate) fn parse_emits_from_std_content(
    content: &str,
    trait_name: &str,
) -> Option<Vec<EmitDefinition>> {
    let schema: OrbitalSchema = serde_json::from_str(content).ok()?;
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                if trait_def.name == trait_name {
                    return trait_def.emits.clone();
                }
            }
        }
    }
    None
}
