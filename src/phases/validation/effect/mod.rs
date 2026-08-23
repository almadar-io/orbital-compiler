//! Effect Validation
//!
//! Validates effects: call_service, set, fetch, emit, persist, etc.
//! Ported from `builder/packages/shared/src/orbitals/validation/effect-validator.ts`.

use super::{PageInitPayload, ValidationContext, ValidationResult};
use crate::embedded::ServiceRegistryData;
use orbital_core::schema::{EntityDefinition, OrbitalSchema, TraitDefinition};
use std::collections::{HashMap, HashSet};

mod call_service;
mod emit;
mod emit_declaration;
mod fetch;
mod navigate;
mod os_watch;
mod persist;
mod r#ref;
mod set;
mod utils;

pub(crate) use call_service::{validate_call_service, validate_call_service_object};
pub(crate) use emit::{extract_emit_object_ref, validate_emit, validate_emit_config};
pub(crate) use emit_declaration::{
    collect_declared_emits, validate_effect, validate_emit_against_declaration,
};
pub(crate) use fetch::{validate_fetch, validate_fetch_object};
pub(crate) use navigate::{validate_navigate, validate_send_server};
pub(crate) use os_watch::validate_os_watch;
pub(crate) use persist::validate_persist;
pub(crate) use r#ref::validate_ref;
pub(crate) use set::{
    entity_field_type_map, expr_return_incompatible, validate_set, validate_set_enum_value,
    validate_set_object, validate_set_return_type, validate_set_value_type, EntityFieldMeta,
};
pub use utils::effect_operators;
pub(crate) use utils::{extract_payload_fields, is_valid_event_identifier};

/// Validate all effects in a schema
pub fn validate_all_effects(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate effects in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_effects(
            trait_def,
            &path,
            &ctx.entity_names,
            &ctx.entity_definitions,
            &ctx.service_names,
            &ctx.page_init_payloads,
            &ctx.service_registry,
        );
        ctx.merge(result);
    }
}

/// Validate all effects in a trait definition
pub fn validate_trait_effects(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    known_services: &HashSet<String>,
    page_init_payloads: &HashMap<String, PageInitPayload>,
    service_registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Collect declared emits for this trait
    let declared_emits = collect_declared_emits(trait_def);

    // SET-EMITS-PAYLOAD backstop inputs: the linked entity's field types and
    // the trait's declared payload field types. Both are `{ name -> type }`
    // maps over the lowercase primitive vocabulary. Payload types come from
    // the typed `emits[].payload_schema`; listens carry payload field NAMES
    // (via `payload_mapping`) but no declared types, so they cannot seed a
    // type-comparison entry — the check fires only where a declared type
    // exists on both sides.
    let entity_field_types = trait_def
        .linked_entity
        .as_ref()
        .and_then(|e| entity_definitions.get(e))
        .map(entity_field_type_map)
        .unwrap_or_default();
    let per_event_payloads = collect_per_event_payload_field_types(trait_def);
    let payload_field_types = collect_payload_field_types(trait_def);

    // Validate effects in transitions — scope payload types to the trigger event
    // so `?result` resolves against the correct event's payload schema.
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let scoped_payload = per_event_payloads
            .get(&transition.event)
            .cloned()
            .unwrap_or_else(|| payload_field_types.clone());
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path =
                    format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
                let effect_result = validate_effect(
                    effect,
                    &effect_path,
                    known_entities,
                    entity_definitions,
                    known_services,
                    page_init_payloads,
                    service_registry,
                    Some(&declared_emits),
                    &entity_field_types,
                    &scoped_payload,
                );
                result.merge(effect_result);

                // Validate emit effects against declared emits
                let emit_result =
                    validate_emit_against_declaration(effect, &effect_path, &declared_emits);
                result.merge(emit_result);
            }
        }
    }

    // Validate effects in ticks
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                let effect_result = validate_effect(
                    effect,
                    &effect_path,
                    known_entities,
                    entity_definitions,
                    known_services,
                    page_init_payloads,
                    service_registry,
                    Some(&declared_emits),
                    &entity_field_types,
                    &payload_field_types,
                );
                result.merge(effect_result);

                // Validate emit effects against declared emits
                let emit_result =
                    validate_emit_against_declaration(effect, &effect_path, &declared_emits);
                result.merge(emit_result);
            }
        }
    }

    result
}

/// Collect the `{ payloadFieldName -> declaredType }` map for a trait,
/// sourced from every `emits[].payload_schema` field. This is the typed
/// payload surface the SET-EMITS-PAYLOAD backstop compares against. Listens
/// carry payload field NAMES (via `payload_mapping`) but no declared types,
/// so they cannot contribute a type entry; the backstop only fires where a
/// declared type exists on both the entity and the payload side.
///
/// Per-event payload field types: `{ event_name -> { field_name -> type } }`.
/// Each event gets its own scope so that `?result` resolves against the
/// triggering event's payload, not a flat merge of all emits.
pub(crate) fn collect_per_event_payload_field_types(
    trait_def: &orbital_core::schema::TraitDefinition,
) -> HashMap<String, HashMap<String, String>> {
    let mut map: HashMap<String, HashMap<String, String>> = HashMap::new();
    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            if let Some(schema) = &emit.payload_schema {
                let mut fields: HashMap<String, String> = HashMap::new();
                for field in schema {
                    fields.insert(field.name.clone(), field.field_type.clone());
                }
                map.insert(emit.event.clone(), fields);
            }
        }
    }
    map
}

/// Flat (legacy) payload field types — kept for ticks and contexts that
/// lack a triggering event. On duplicate field names across multiple
/// emits, the first declared type wins.
pub(crate) fn collect_payload_field_types(
    trait_def: &orbital_core::schema::TraitDefinition,
) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            if let Some(schema) = &emit.payload_schema {
                for field in schema {
                    map.entry(field.name.clone())
                        .or_insert_with(|| field.field_type.clone());
                }
            }
        }
    }
    map
}

/// Information about a declared emit
#[derive(Clone)]
pub(crate) struct DeclaredEmit {
    pub(crate) event: String,
    pub(crate) is_external: bool,
    #[allow(dead_code)] // collected + constructed by tests; not read by validators yet
    pub(crate) payload_fields: Vec<String>,
    pub(crate) required_fields: Vec<String>,
}
