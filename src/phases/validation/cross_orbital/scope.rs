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
    for (_, trait_def) in collect_all_traits(schema) {
        if let Some(emits) = &trait_def.emits {
            in_schema_emits.insert(trait_def.name.clone(), emits);
        }
    }

    // Cache embedded std-behavior parses keyed by source path. The same
    // `std/behaviors/X` content was previously re-parsed for every trait
    // reference into it; parse each distinct behavior at most once per run.
    // `None` records a parse failure so we don't retry it.
    let mut parsed_std_cache: HashMap<&str, Option<OrbitalSchema>> = HashMap::new();

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
            let target_emits: &Vec<EmitDefinition> = if let Some(emits) =
                in_schema_emits.get(target_trait_name)
            {
                emits
            } else if let Some(src) = alias_source.get(alias) {
                // Attempt to resolve std-behavior content. Other
                // sources (local .orb paths) aren't loaded here —
                // skip silently. The parsed schema is cached per src so
                // repeated references don't re-parse the same content.
                if crate::embedded::get_std_behavior_content(src).is_some() {
                    let parsed = parsed_std_cache.entry(*src).or_insert_with(|| {
                        crate::embedded::get_std_behavior_content(src)
                            .and_then(|content| serde_json::from_str::<OrbitalSchema>(content).ok())
                    });
                    owned_holder = parsed
                        .as_ref()
                        .and_then(|schema| emits_for_trait(schema, target_trait_name));
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

            let path_base = crate::phases::validation::trait_path(oi, &orbital.name, ti, trait_ref);

            // Check `events:` rename map entries. The SOURCE key is the
            // upstream atom's event name — that's what we look up in
            // emits[].
            if let Some(map) = events {
                for (source_event, target_event) in map {
                    if let Some(emit) = target_emits.iter().find(|e| &e.event == source_event) {
                        if emit.scope == Some(EventScope::Internal) {
                            result.add_error(
                                ValidationError::error(
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
                                ValidationError::error(
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

/// Read a named inline trait's `emits[]` from an already-parsed schema.
fn emits_for_trait(schema: &OrbitalSchema, trait_name: &str) -> Option<Vec<EmitDefinition>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EntityDefinition, EntityRef, Orbital, StateMachine, TraitDefinition,
    };
    use std::collections::HashMap;

    // =========================================================================
    // COMP-7: validate_scope_boundary
    // =========================================================================

    /// Helper: build an orbital that declares an atom trait with a given
    /// emit scope, and a second orbital that composes it via an
    /// `Alias.traits.Atom` reference with an `events:` rename override.
    fn build_scope_boundary_fixture(atom_scope: EventScope) -> OrbitalSchema {
        let atom = TraitDefinition {
            effect_row: None,
            type_params: None,
            type_args: None,
            name: "Atom".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Task".to_string()),
            emits: Some(vec![EmitDefinition {
                definer_knob: None,
                scope_overridden: None,
                event: "OPEN".to_string(),
                scope: Some(atom_scope),
                description: None,
                synonyms: None,
                tier: None,
                payload_schema: None,
                event_id: None,
                payload_entity: None,
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        };
        let atom_orbital = Orbital {
            types: None,
            name: "AtomHost".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "AtomEntity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(atom)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
            reference: None,
            config: None,
        };

        let mut events_rename = HashMap::new();
        events_rename.insert("OPEN".to_string(), "ADD_ITEM".to_string());
        let composer_ref = TraitReference::Reference {
            type_args: None,
            id: None,
            reference: Some("Lib.traits.Atom".to_string()),
            name: Some("Composed".to_string()),
            linked_entity: Some("Cart".to_string()),
            config: None,
            events: Some(events_rename),
            fields: None,
            listens: None,
            emits_scope: None,
            event_ids: None,
            linked_entity_id: None,
            ref_id: None,
        };
        let composer_orbital = Orbital {
            types: None,
            name: "Composer".to_string(),
            uses: Some(vec![orbital_core::schema::UseDeclaration {
                from: "local/does-not-matter".to_string(),
                alias: "Lib".to_string(),
                config: None,
            }]),
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Cart".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
            }),
            auxiliary_entities: None,
            traits: vec![composer_ref],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
            reference: None,
            config: None,
        };

        OrbitalSchema {
            theme: None,
            name: "CompositionApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![atom_orbital, composer_orbital],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
        }
    }

    /// Positive case: the atom declares `scope: 'external'`, so remapping it
    /// is allowed. Expect no ORB_X_INTERNAL_EVENT_EXPOSED warnings.
    #[test]
    fn test_scope_boundary_external_ok() {
        let schema = build_scope_boundary_fixture(EventScope::External);
        let result = validate_scope_boundary(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::InternalEventExposed),
            "External-scoped atom events must allow remapping. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case: the atom declares `scope: 'internal'`, so a composing
    /// `events:` rename touches a private event. Expect exactly one
    /// ORB_X_INTERNAL_EVENT_EXPOSED warning on the Composer orbital.
    #[test]
    fn test_scope_boundary_internal_warns() {
        let schema = build_scope_boundary_fixture(EventScope::Internal);
        let result = validate_scope_boundary(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::InternalEventExposed)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "Exactly one ORB_X_INTERNAL_EVENT_EXPOSED expected. Warnings: {:?}",
            result.errors
        );
        assert!(hits[0].path.contains("traits[0 (") && hits[0].path.contains(".events"));
    }

    /// Bare reference without `events:` / `listens:` overrides must not
    /// produce any COMP-7 warnings regardless of atom scope. This guards
    /// against a naive walker that fires on any `TraitReference::Reference`.
    #[test]
    fn test_scope_boundary_no_override_is_silent() {
        let mut schema = build_scope_boundary_fixture(EventScope::Internal);
        // Drop the events override from the composer trait reference.
        if let Some(TraitReference::Reference {
            events, listens, ..
        }) = schema.orbitals[1].traits.first_mut()
        {
            *events = None;
            *listens = None;
        }
        let result = validate_scope_boundary(&schema);
        assert!(
            result.errors.is_empty(),
            "References without overrides must be silent. Warnings: {:?}",
            result.errors
        );
    }
}
