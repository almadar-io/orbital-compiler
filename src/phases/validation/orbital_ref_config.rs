//! Call-site rules for `orbital X = Alias.orbitals.Y { … }` references and
//! their feeding `uses A from "…" { config { … } }` overrides
//! (`docs/Almadar_Orbital_Import.md` §4.5). Mirrors `trait_ref_config.rs`:
//! the raw call-site keys are gone once inline folds/drops them into the
//! resolved schema, so these convert the [`OrbitalRefDiagnostic`]s /
//! [`UsesConfigDiagnostic`]s collected DURING inlining
//! (`phases::inline::inline_schema_with_orbital_diagnostics`) into proper
//! `ValidationError`s.
//!
//! - `ORB_O_CONFIG_UNKNOWN_KEY`: an orbital reference's call-site `config {}`
//!   names a key the upstream orbital never declared. The override silently
//!   no-ops.
//! - `ORB_O_OMIT_EMBEDDED_TRAIT`: an orbital reference's `omit {}` / `only {}`
//!   drops a trait that a surviving trait still embeds via `@trait.<X>` — the
//!   embed token has nothing left to render.
//! - `ORB_O_ROLE_TARGET_NOT_MEMBER`: a `roles {}` remap target is not a
//!   member of the CONSUMER roster's vocabulary.
//! - `ORB_O_ROLE_UNKNOWN_UPSTREAM`: a `roles {}` key names no value in any
//!   UPSTREAM identity field's vocabulary.
//! - `ORB_O_ENTITY_UNMAPPED`: a kept trait/relation/page references an
//!   out-of-orbital entity that `entities {}` never maps.
//! - `ORB_O_ENTITY_UNKNOWN_KEY`: an `entities {}` key names no entity in the
//!   upstream closure's out-of-orbital universe.
//! - `ORB_O_ENTITY_TARGET_UNKNOWN`: an `entities {}` value names no entity
//!   the consumer schema declares.
//! - `ORB_O_PAGE_UNKNOWN_PATH`: a `pages {}` / `mounts {}` key names no page
//!   path the imported orbital declares.
//! - `ORB_U_CONFIG_UNKNOWN_KEY`: a `uses A from "…" { config { … } }`
//!   override names a key the imported behavior's app-level `config {}`
//!   never declared.

use std::collections::{HashMap, HashSet};

use orbital_core::schema::OrbitalSchema;

use super::error::{ErrorCode, ValidationError};
use super::user_identity::{identity_entities, vocabulary};
use super::ValidationResult;
use crate::phases::inline::{OrbitalRefDiagnostic, UsesConfigDiagnostic};

/// The CONSUMER roster's own vocabulary — every identity entity's name
/// (`identity_entities(schema)`, post-inline so siblings/splices are already
/// resolved) alongside, PER FIELD NAME, the union of that field's declared
/// values across every identity entity. A `roles {}` target is a member iff
/// the consumer roster has a field with the SAME NAME as the upstream field
/// the literal came from, and that field's values contain the target — a
/// target must not pass by matching some OTHER field's vocabulary (e.g. a
/// status field's `active` passing as a role).
fn consumer_role_vocabulary(schema: &OrbitalSchema) -> (Vec<&str>, HashMap<&str, HashSet<&str>>) {
    let roster = identity_entities(schema);
    let mut entity_names: Vec<&str> = Vec::new();
    let mut fields: HashMap<&str, HashSet<&str>> = HashMap::new();
    for identity in &roster {
        entity_names.push(identity.name.as_str());
        for field in &identity.fields {
            let Some(name) = field.name.as_deref() else {
                continue;
            };
            if let Some(vocab) = vocabulary(field) {
                fields
                    .entry(name)
                    .or_default()
                    .extend(vocab.iter().map(String::as_str));
            }
        }
    }
    (entity_names, fields)
}

/// Convert inline-collected orbital-reference diagnostics into
/// `ORB_O_*` validation errors (`Almadar_Orbital_Import.md` §4.5 / Stage B).
pub fn validate_orbital_ref_diagnostics(
    diagnostics: &[OrbitalRefDiagnostic],
    schema: &OrbitalSchema,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let (roster_entities, roster_fields) = consumer_role_vocabulary(schema);

    for diag in diagnostics {
        for key in &diag.unknown_config_keys {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefConfigUnknownKey,
                    format!("{}.config({})", diag.path, key),
                    format!(
                        "Call-site config key '{}' does not match any config knob declared by upstream orbital '{}'. It silently no-ops.",
                        key, diag.reference
                    ),
                )
                .with_suggestion(format!(
                    "Remove '{}' or declare it in the upstream orbital's `config {{}}` block.",
                    key
                )),
            );
        }

        for (surviving_trait, omitted_trait) in &diag.omitted_but_embedded {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefOmitEmbeddedTrait,
                    format!("{}.omit({})", diag.path, omitted_trait),
                    format!(
                        "Orbital reference '{}' omits trait '{}', but trait '{}' still embeds it via '@trait.{}'. The embed has nothing left to render.",
                        diag.reference, omitted_trait, surviving_trait, omitted_trait
                    ),
                )
                .with_suggestion(format!(
                    "Keep '{}' (drop it from omit, or add it to only), or remove the '@trait.{}' embed from '{}'.",
                    omitted_trait, omitted_trait, surviving_trait
                )),
            );
        }

        for literal in &diag.role_unknown_upstream {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefRoleUnknownUpstream,
                    format!("{}.roles({})", diag.path, literal),
                    format!(
                        "`roles {{}}` key '{}' names no value in any of upstream orbital '{}''s identity field vocabularies (or it declares no roster).",
                        literal, diag.reference
                    ),
                )
                .with_suggestion(format!(
                    "Match '{}' to a value the upstream `[identity]` entity's role-bearing field actually declares.",
                    literal
                )),
            );
        }

        for (field, literal, targets) in &diag.role_targets {
            let field_values = roster_fields.get(field.as_str());
            for target in targets {
                if field_values.is_some_and(|vals| vals.contains(target.as_str())) {
                    continue;
                }
                let allowed = match field_values {
                    Some(vals) => {
                        let mut allowed: Vec<&str> = vals.iter().copied().collect();
                        allowed.sort_unstable();
                        allowed.join(", ")
                    }
                    // The consumer roster declares no field named `field` at
                    // all — every target for this upstream field is a miss.
                    None => String::new(),
                };
                result.add_error(
                    ValidationError::error(
                        ErrorCode::OrbitalRefRoleTargetNotMember,
                        format!("{}.roles({})", diag.path, literal),
                        format!(
                            "`roles {{ {}: {} }}` target '{}' is not a member of consumer field `{}` on the roster ({}); allowed values: [{}]",
                            literal, target, target, field, roster_entities.join(", "), allowed
                        ),
                    )
                    .with_suggestion(format!(
                        "Map '{}' to one of `{}`'s declared values, or add '{}' to it.",
                        literal, field, target
                    )),
                );
            }
        }

        for (site, entity) in &diag.entity_unmapped {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefEntityUnmapped,
                    format!("{}.{}", diag.path, site),
                    format!(
                        "'{}' references out-of-orbital entity '{}', but `entities {{}}` on '{}' never maps it — the import splice has nothing to rebind it to.",
                        site, entity, diag.reference
                    ),
                )
                .with_suggestion(format!("Add '{}: <consumer entity>' to `entities {{}}`.", entity)),
            );
        }

        for up_name in &diag.entity_unknown_key {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefEntityUnknownKey,
                    format!("{}.entities({})", diag.path, up_name),
                    format!(
                        "`entities {{}}` key '{}' names no entity in upstream orbital '{}''s out-of-orbital closure.",
                        up_name, diag.reference
                    ),
                )
                .with_suggestion(format!(
                    "Remove '{}' or match it to an entity the upstream closure actually references.",
                    up_name
                )),
            );
        }

        for (up_name, target) in &diag.entity_target_unknown {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefEntityTargetUnknown,
                    format!("{}.entities({})", diag.path, up_name),
                    format!(
                        "`entities {{ {}: {} }}` names no entity the consumer schema declares.",
                        up_name, target
                    ),
                )
                .with_suggestion(format!(
                    "Declare '{}' in the consumer schema, or fix the typo.",
                    target
                )),
            );
        }

        for key in &diag.page_unknown_path {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefPageUnknownPath,
                    format!("{}.pages({})", diag.path, key),
                    format!(
                        "`pages {{}}` / `mounts {{}}` key '{}' names no page path upstream orbital '{}' declares.",
                        key, diag.reference
                    ),
                )
                .with_suggestion(format!(
                    "Match '{}' to a path the upstream orbital's `pages {{}}` actually declares.",
                    key
                )),
            );
        }

        for field in &diag.mock_unknown_field {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefMockUnknownField,
                    format!("{}.mock({})", diag.path, field),
                    format!(
                        "`mock {{}}` names '{}', which the entity materialized from '{}' does not declare.",
                        field, diag.reference
                    ),
                )
                .with_suggestion(
                    "Name a field of the imported primary entity as it is after `fields {}` renames, or an `extend {}` field.".to_string(),
                ),
            );
        }

        for field in &diag.retype_unknown_field {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefRetypeUnknownField,
                    format!("{}.retype({})", diag.path, field),
                    format!(
                        "`retype {{}}` names '{}', which the entity materialized from '{}' does not declare.",
                        field, diag.reference
                    ),
                )
                .with_suggestion(
                    "Name a field the upstream primary entity already declares, as it is after `fields {}` renames; add a new field with `extend {}`.".to_string(),
                ),
            );
        }

        for (field, reason) in &diag.retype_conflict {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefRetypeFieldConflict,
                    format!("{}.retype({})", diag.path, field),
                    format!("`retype {{}}` field '{}' is {}.", field, reason),
                )
                .with_suggestion(
                    "Declare the field once: either `extend {}` it or `retype {}` it, and list it once.".to_string(),
                ),
            );
        }

        if let Some(mode) = &diag.entity_persistence_locked {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefEntityPersistenceLocked,
                    format!("{}.entity", diag.path),
                    format!(
                        "The `entity [..]` persistence/collection override has no effect: the primary entity of '{}' is `{}`, and only a `persistent` entity allows its persistence or collection to be overridden.",
                        diag.reference, mode
                    ),
                )
                .with_suggestion(
                    "Remove the persistence/collection from the `entity` line, or import an orbital whose entity is persistent.".to_string(),
                ),
            );
        }

        for name in &diag.traits_unknown_trait {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefTraitsUnknownTrait,
                    format!("{}.traits({})", diag.path, name),
                    format!(
                        "`traits {{}}` key '{}' names no trait of upstream orbital '{}'.",
                        name, diag.reference
                    ),
                )
                .with_suggestion(
                    "Key each entry by the trait's UPSTREAM name (before the import prefix).".to_string(),
                ),
            );
        }

        for name in &diag.traits_omitted_trait {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefTraitsOmittedTrait,
                    format!("{}.traits({})", diag.path, name),
                    format!(
                        "`traits {{}}` overrides '{}', but `omit {{}}` / `only {{}}` drops that trait from the import.",
                        name
                    ),
                )
                .with_suggestion(format!(
                    "Remove the `traits {{ {name} {{ … }} }}` entry, or keep '{name}' in the import."
                )),
            );
        }

        for (trait_name, key) in &diag.traits_unknown_config_key {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefConfigUnknownKey,
                    format!("{}.traits({}).config({})", diag.path, trait_name, key),
                    format!(
                        "`traits {{ {} }}` config key '{}' does not match any knob the upstream trait declares. It silently no-ops.",
                        trait_name, key
                    ),
                )
                .with_suggestion(format!(
                    "Remove '{}' or use a knob declared in '{}''s `config {{}}` block.",
                    key, trait_name
                )),
            );
        }

        for (position, name) in &diag.field_rename_unapplied {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefFieldRenameUnapplied,
                    position.clone(),
                    format!(
                        "`fields {{}}` rename on '{}' left '{}' unapplied at {} — the pre-rename \
                         upstream field name is still referenced here.",
                        diag.reference, name, position
                    ),
                )
                .with_suggestion(format!(
                    "'{}' is a struct member this crate does not yet recognize as a field-name \
                     reference (outside the `key`/`name`/`field` set); rename it by hand to the \
                     `fields {{}}` target, or widen the recognized member set.",
                    name
                )),
            );
        }
    }

    result
}

/// Convert inline-collected `uses … { config }` diagnostics into
/// `ORB_U_CONFIG_UNKNOWN_KEY` validation errors.
pub fn validate_uses_config_diagnostics(diagnostics: &[UsesConfigDiagnostic]) -> ValidationResult {
    let mut result = ValidationResult::new();

    for diag in diagnostics {
        for key in &diag.unknown_keys {
            result.add_error(
                ValidationError::error(
                    ErrorCode::UsesConfigUnknownKey,
                    format!("{}.uses({}).config({})", diag.path, diag.alias, key),
                    format!(
                        "Call-site config key '{}' on `uses {} from \"…\" {{ config {{ … }} }}` does not match any app-level config knob declared by the imported behavior. It silently no-ops.",
                        key, diag.alias
                    ),
                )
                .with_suggestion(format!(
                    "Remove '{}' or declare it in the imported behavior's app-level `config {{}}` block.",
                    key
                )),
            );
        }
    }

    result
}

/// Every `ORB_O_*`/`ORB_U_*` code this module produces — the ROOT rows a
/// cascade traces back to. Never suppressed, and the only rows a suppressed
/// count is folded into.
fn is_orbital_ref_root(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::OrbitalRefConfigUnknownKey
            | ErrorCode::OrbitalRefOmitEmbeddedTrait
            | ErrorCode::OrbitalRefRoleUnknownUpstream
            | ErrorCode::OrbitalRefRoleTargetNotMember
            | ErrorCode::OrbitalRefEntityUnmapped
            | ErrorCode::OrbitalRefEntityUnknownKey
            | ErrorCode::OrbitalRefEntityTargetUnknown
            | ErrorCode::OrbitalRefPageUnknownPath
            | ErrorCode::OrbitalRefFieldRenameUnapplied
            | ErrorCode::OrbitalRefRetypeUnknownField
            | ErrorCode::OrbitalRefRetypeFieldConflict
            | ErrorCode::OrbitalRefEntityPersistenceLocked
            | ErrorCode::OrbitalRefTraitsUnknownTrait
            | ErrorCode::OrbitalRefTraitsOmittedTrait
            | ErrorCode::UsesConfigUnknownKey
    )
}

/// `diag.path` (`inline/orbital.rs::resolve_orbital_reference`) is
/// `orbitals[{i}]` for a TOP-LEVEL consumer orbital reference — `i` is
/// already the index into `schema.orbitals`, the same index every other
/// validator's `orbitals[{i}]` / `orbitals[{i} ({name})]` path is built
/// from — or `orbitals[name={local}]` when resolving an alias's own
/// reference-form orbital recursively (a different file's index space, no
/// index into THIS schema to give). The digit form resolves directly; the
/// name form falls back to a by-name lookup.
fn local_orbital_index(diag_path: &str, schema: &OrbitalSchema) -> Option<usize> {
    if let Some(rest) = diag_path
        .strip_prefix("orbitals[")
        .and_then(|rest| rest.strip_suffix(']'))
    {
        if let Ok(i) = rest.parse::<usize>() {
            return (i < schema.orbitals.len()).then_some(i);
        }
        if let Some(local_name) = rest.strip_prefix("name=") {
            return schema.orbitals.iter().position(|o| o.name == local_name);
        }
    }
    None
}

/// Does `path` address orbital `index`, in either the bare (`orbitals[3]…`,
/// `entity.rs`/`relations.rs`/`page.rs`) or named (`orbitals[3 (Local)]…`,
/// `id_integrity.rs`/`config_type.rs`/`mod.rs`/`expectations.rs`) spelling
/// every validator uses? Both are followed by `.` or `[` or end-of-string in
/// practice, never by another digit, so a plain prefix check cannot collide
/// `orbitals[1]` with `orbitals[10]` — `]` and `0` differ at that position.
fn path_under_orbital(path: &str, index: usize) -> bool {
    path.starts_with(&format!("orbitals[{index}]"))
        || path.starts_with(&format!("orbitals[{index} ("))
}

/// Does `message` quote one of `entities` (the out-of-orbital names an
/// `ORB_O_*` diagnostic already flagged), wrapped in `'…'` or `` `…` `` —
/// every message in this validator (and its downstream cousins:
/// `ORB_E_INVALID_RELATION`, `ORB_X_UNRESOLVED_RELATION_TARGET`,
/// `ORB_EFF_FETCH_INVALID_ENTITY`, …) names its offending token that way.
/// Returns the matched entity so the caller can attribute the suppression.
fn quoted_entity<'a>(message: &str, entities: &'a HashSet<String>) -> Option<&'a str> {
    entities.iter().find_map(|e| {
        (message.contains(&format!("'{e}'")) || message.contains(&format!("`{e}`")))
            .then_some(e.as_str())
    })
}

/// Owner ruling (Stage C, 2026-09-06): "suppress any downstream diagnostic at
/// a site/entity an `ORB_O_*` diagnostic already names" — generic, no
/// per-code table. An import that leaves an out-of-orbital entity unmapped
/// (or maps `entities {}`/`roles {}` to something that doesn't exist) is
/// re-reported by every OTHER validator that independently notices the same
/// entity is missing under the same orbital — `ORB_E_INVALID_RELATION` on the
/// relation field, `ORB_X_UNRESOLVED_RELATION_TARGET` on the same field,
/// `ORB_EFF_FETCH_INVALID_ENTITY` on a `fetch` naming it, and (path-matched
/// separately below, since its path carries no orbital index)
/// `ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED` on an access policy against it —
/// every one of these is the SAME root cause the `ORB_O_*` row already names,
/// not independent findings. Called once, after the three
/// `validate_*_diagnostics` merges land in `result`
/// (`merge_post_inline_diagnostics`, `orbital-cli`), so it sees the full
/// validate_comprehensive_full sweep alongside the fresh `ORB_O_*`/`ORB_U_*`
/// rows.
pub fn suppress_cascaded_by_orbital_ref(
    result: &mut ValidationResult,
    diagnostics: &[OrbitalRefDiagnostic],
    schema: &OrbitalSchema,
) {
    if diagnostics.is_empty() {
        return;
    }

    struct Site {
        orbital_index: Option<usize>,
        entities: HashSet<String>,
    }

    let sites: Vec<Site> = diagnostics
        .iter()
        .map(|diag| {
            let orbital_index = local_orbital_index(&diag.path, schema);
            let mut entities: HashSet<String> = HashSet::new();
            entities.extend(diag.entity_unmapped.iter().map(|(_, e)| e.clone()));
            // `entity_target_unknown` is (up_name, target) — an INVALID
            // target means the fold into `subs` never ran (`orbital.rs`'s
            // `entities_map` loop), so the entity stays under its UPSTREAM
            // name unrenamed; downstream validators quote `up_name`, not
            // the (nonexistent) `target`. Add both — `target` never matches
            // anything real, but is harmless to include.
            entities.extend(
                diag.entity_target_unknown
                    .iter()
                    .flat_map(|(up_name, target)| [up_name.clone(), target.clone()]),
            );
            entities.extend(diag.entity_unknown_key.iter().cloned());
            // G3: a stale field name a `fields {}` rename left unapplied is
            // the same kind of root cause — a downstream validator (e.g.
            // `ORB_S_ACCESS_POLICY_UNKNOWN_ENTITY_FIELD`) that independently
            // notices the SAME field name is missing is re-reporting what
            // `ORB_O_FIELD_RENAME_UNAPPLIED` already named, not an
            // independent finding. `quoted_entity`'s matcher is generic over
            // any quoted token, so no separate field-matching path is needed.
            entities.extend(diag.field_rename_unapplied.iter().map(|(_, f)| f.clone()));
            Site {
                orbital_index,
                entities,
            }
        })
        .filter(|s| s.orbital_index.is_some() && !s.entities.is_empty())
        .collect();

    if sites.is_empty() {
        return;
    }

    // (suppressed entity name -> the downstream codes dropped for it), so the
    // root row can name exactly what it absorbed.
    let mut suppressed_by_entity: HashMap<String, Vec<&'static str>> = HashMap::new();

    let mut suppress = |list: &mut Vec<ValidationError>| {
        list.retain(|err| {
            if is_orbital_ref_root(err.code) {
                return true;
            }
            // `ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED`'s path is the bare
            // `{entity}.@{sigil}` (`entity_access.rs:434`) — no orbital index
            // to scope by, so match on the named entity alone: the policy's
            // own entity, or the relation target the message quotes (the
            // un-rebound upstream entity IS the cause).
            if err.code == ErrorCode::SchemaOwnerFieldNotIdentityTyped {
                for site in &sites {
                    let by_path = site
                        .entities
                        .iter()
                        .find(|entity| err.path.starts_with(&format!("{entity}.@")))
                        .map(String::as_str);
                    if let Some(entity) =
                        by_path.or_else(|| quoted_entity(&err.message, &site.entities))
                    {
                        suppressed_by_entity
                            .entry(entity.to_string())
                            .or_default()
                            .push(err.code.as_str());
                        return false;
                    }
                }
                return true;
            }
            for site in &sites {
                let Some(index) = site.orbital_index else {
                    continue;
                };
                if !path_under_orbital(&err.path, index) {
                    continue;
                }
                if let Some(entity) = quoted_entity(&err.message, &site.entities) {
                    suppressed_by_entity
                        .entry(entity.to_string())
                        .or_default()
                        .push(err.code.as_str());
                    return false;
                }
            }
            true
        });
    };

    suppress(&mut result.errors);
    suppress(&mut result.warnings);

    if suppressed_by_entity.is_empty() {
        return;
    }

    // Fold each entity's suppressed count into the first ORB_O_*/ORB_U_*
    // root row that already names it — whichever one fired (unmapped,
    // unknown key, or target unknown), generic over which.
    for row in result.errors.iter_mut().chain(result.warnings.iter_mut()) {
        if !is_orbital_ref_root(row.code) || suppressed_by_entity.is_empty() {
            continue;
        }
        let Some(entity) = suppressed_by_entity
            .keys()
            .find(|e| row.message.contains(e.as_str()))
            .cloned()
        else {
            continue;
        };
        let codes = suppressed_by_entity.remove(&entity).unwrap_or_default();
        row.message.push_str(&format!(
            " ({} downstream finding{} suppressed: {})",
            codes.len(),
            if codes.len() == 1 { "" } else { "s" },
            codes.join(", ")
        ));
    }
}

#[cfg(test)]
mod suppression_tests {
    use super::*;
    use serde_json::json;

    fn schema_two_orbitals() -> OrbitalSchema {
        let value = json!({
            "name": "test-suppression",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "Local",
                    "entity": {
                        "name": "Widget",
                        "persistence": "persistent",
                        "fields": [
                            { "name": "id", "type": "string", "required": true },
                            { "name": "otherId", "type": "relation", "relation": { "entity": "Other", "cardinality": "one" } }
                        ]
                    },
                    "traits": []
                },
                {
                    "name": "Unrelated",
                    "entity": {
                        "name": "Widget2",
                        "persistence": "persistent",
                        "fields": [
                            { "name": "id", "type": "string", "required": true },
                            { "name": "gizmoId", "type": "relation", "relation": { "entity": "Gizmo", "cardinality": "one" } }
                        ]
                    },
                    "traits": []
                }
            ]
        });
        serde_json::from_value(value).expect("schema fixture must deserialize")
    }

    /// `diag.path` for a TOP-LEVEL consumer orbital reference is the digit
    /// form (`orbitals[{i}]`, `inline/orbital.rs::resolve_orbital_reference`)
    /// — `"Local"` is `schema_two_orbitals()`'s `orbitals[0]`.
    fn orbital_ref_diag(entity: &str) -> OrbitalRefDiagnostic {
        OrbitalRefDiagnostic {
            path: "orbitals[0]".to_string(),
            reference: "Up.orbitals.MainOrbital".to_string(),
            entity_unmapped: vec![("primaryEntity".to_string(), entity.to_string())],
            ..Default::default()
        }
    }

    /// The `orbitals[name={local}]` fallback spelling (recursive alias
    /// resolution, no index into THIS schema).
    fn orbital_ref_diag_named(entity: &str) -> OrbitalRefDiagnostic {
        OrbitalRefDiagnostic {
            path: "orbitals[name=Local]".to_string(),
            reference: "Up.orbitals.MainOrbital".to_string(),
            entity_unmapped: vec![("primaryEntity".to_string(), entity.to_string())],
            ..Default::default()
        }
    }

    fn root_error(entity: &str) -> ValidationError {
        ValidationError::error(
            ErrorCode::OrbitalRefEntityUnmapped,
            "orbitals[0].primaryEntity",
            format!(
                "'primaryEntity' references out-of-orbital entity '{entity}', but `entities {{}}` on 'Up.orbitals.MainOrbital' never maps it — the import splice has nothing to rebind it to."
            ),
        )
    }

    #[test]
    fn drops_downstream_invalid_relation_naming_the_unmapped_entity_in_the_same_orbital() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![orbital_ref_diag("Other")];
        let mut result = ValidationResult::new();
        result.add_error(root_error("Other"));
        result.add_error(ValidationError::error(
            ErrorCode::EntityInvalidRelation,
            "orbitals[0].entity.fields[1].relation.entity",
            "Relation target entity 'Other' does not exist",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert_eq!(
            result.errors.len(),
            1,
            "the downstream ORB_E_INVALID_RELATION must be dropped: {:#?}",
            result.errors
        );
        assert_eq!(result.errors[0].code, ErrorCode::OrbitalRefEntityUnmapped);
        assert!(
            result.errors[0]
                .message
                .contains("1 downstream finding suppressed: ORB_E_INVALID_RELATION"),
            "root message must record what it absorbed: {}",
            result.errors[0].message
        );
    }

    /// Real-world regression (found running the CLI against the PF
    /// `entity_target_unknown.lolo` negative): `entities { Employee: Nobody
    /// }` — the TARGET `Nobody` doesn't exist, so the fold into `subs` never
    /// runs and the entity stays under its UPSTREAM name `Employee`
    /// unrenamed. Every downstream validator therefore quotes `Employee`,
    /// never `Nobody` — the suppression set must include the up_name, not
    /// just the invalid target.
    #[test]
    fn drops_downstream_findings_naming_the_upstream_name_of_an_invalid_target_mapping() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![OrbitalRefDiagnostic {
            path: "orbitals[0]".to_string(),
            reference: "Up.orbitals.MainOrbital".to_string(),
            entity_target_unknown: vec![("Other".to_string(), "Nobody".to_string())],
            ..Default::default()
        }];
        let mut result = ValidationResult::new();
        result.add_error(ValidationError::error(
            ErrorCode::OrbitalRefEntityTargetUnknown,
            "orbitals[0].entities(Other)",
            "`entities { Other: Nobody }` names no entity the consumer schema declares.",
        ));
        result.add_error(ValidationError::error(
            ErrorCode::EntityInvalidRelation,
            "orbitals[0].entity.fields[1].relation.entity",
            "Relation target entity 'Other' does not exist",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert_eq!(
            result.errors.len(),
            1,
            "the downstream ORB_E_INVALID_RELATION (quoting the unrenamed upstream name 'Other') must be dropped: {:#?}",
            result.errors
        );
        assert_eq!(
            result.errors[0].code,
            ErrorCode::OrbitalRefEntityTargetUnknown
        );
    }

    #[test]
    fn drops_downstream_unresolved_relation_target_warning_too() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![orbital_ref_diag("Other")];
        let mut result = ValidationResult::new();
        result.add_error(root_error("Other"));
        result.warnings.push(ValidationError::error(
            ErrorCode::UnresolvedRelationTarget,
            "orbitals[0].entity.fields[1].relation.entity",
            "Relation field 'otherId' on 'Widget' points at 'Other', which is not declared in this schema",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert!(
            result.warnings.is_empty(),
            "the downstream ORB_X_UNRESOLVED_RELATION_TARGET warning must be dropped: {:#?}",
            result.warnings
        );
        assert!(result.errors[0]
            .message
            .contains("ORB_X_UNRESOLVED_RELATION_TARGET"));
    }

    #[test]
    fn drops_downstream_fetch_invalid_entity() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![orbital_ref_diag("Other")];
        let mut result = ValidationResult::new();
        result.add_error(root_error("Other"));
        result.add_error(ValidationError::error(
            ErrorCode::EffectFetchInvalidEntity,
            "orbitals[0 (Local)].traits[0 (OtherFetcher)].stateMachine.transitions[0].effects[0]",
            "Entity 'Other' does not exist",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert_eq!(result.errors.len(), 1, "{:#?}", result.errors);
    }

    #[test]
    fn drops_owner_field_not_identity_typed_matched_by_entity_name_only() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![orbital_ref_diag("Other")];
        let mut result = ValidationResult::new();
        result.add_error(root_error("Other"));
        result.add_error(ValidationError::error(
            ErrorCode::SchemaOwnerFieldNotIdentityTyped,
            "Other.@read",
            "`Other.ownerId` is compared to `@user.id` but is not a relation to the [identity] entity",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert_eq!(result.errors.len(), 1, "{:#?}", result.errors);
    }

    #[test]
    fn unrelated_invalid_relation_in_another_orbital_survives() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![orbital_ref_diag("Other")];
        let mut result = ValidationResult::new();
        result.add_error(root_error("Other"));
        // Same code, but under `orbitals[1]` (Unrelated) and names a
        // different entity ("Gizmo") — a genuine, independent finding.
        result.add_error(ValidationError::error(
            ErrorCode::EntityInvalidRelation,
            "orbitals[1].entity.fields[1].relation.entity",
            "Relation target entity 'Gizmo' does not exist",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert_eq!(
            result.errors.len(),
            2,
            "the unrelated orbital's own finding must survive untouched: {:#?}",
            result.errors
        );
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityInvalidRelation && e.message.contains("Gizmo")));
    }

    #[test]
    fn same_code_same_orbital_different_entity_survives() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![orbital_ref_diag("Other")];
        let mut result = ValidationResult::new();
        result.add_error(root_error("Other"));
        // Under the SAME orbital (`orbitals[0]`) but names an entity the
        // `ORB_O_*` diagnostic never flagged — must not be swept up by a
        // same-orbital-only match.
        result.add_error(ValidationError::error(
            ErrorCode::EntityInvalidRelation,
            "orbitals[0].entity.fields[2].relation.entity",
            "Relation target entity 'SomethingElse' does not exist",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert_eq!(result.errors.len(), 2, "{:#?}", result.errors);
    }

    #[test]
    fn no_orbital_ref_diagnostics_is_a_no_op() {
        let schema = schema_two_orbitals();
        let mut result = ValidationResult::new();
        result.add_error(ValidationError::error(
            ErrorCode::EntityInvalidRelation,
            "orbitals[0].entity.fields[1].relation.entity",
            "Relation target entity 'Other' does not exist",
        ));
        suppress_cascaded_by_orbital_ref(&mut result, &[], &schema);
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn resolves_the_orbitals_name_equals_fallback_path_spelling_too() {
        let schema = schema_two_orbitals();
        let diagnostics = vec![orbital_ref_diag_named("Other")];
        let mut result = ValidationResult::new();
        result.add_error(root_error("Other"));
        result.add_error(ValidationError::error(
            ErrorCode::EntityInvalidRelation,
            "orbitals[0].entity.fields[1].relation.entity",
            "Relation target entity 'Other' does not exist",
        ));

        suppress_cascaded_by_orbital_ref(&mut result, &diagnostics, &schema);

        assert_eq!(result.errors.len(), 1, "{:#?}", result.errors);
    }
}
