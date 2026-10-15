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
mod http;
mod keyed_call;
mod navigate;
mod operator_args;
mod os_watch;
mod persist;
mod program_eval;
mod r#ref;
mod set;
mod utils;

pub(crate) use call_service::{validate_call_service, validate_call_service_object};
pub(crate) use http::validate_http_effects;
pub use http::validate_http_secrets;
pub(crate) use keyed_call::validate_cancel_calls_have_a_keyed_call;
pub(crate) use emit::{extract_emit_object_ref, validate_emit, validate_emit_config};
pub(crate) use emit_declaration::{
    collect_declared_emits, validate_effect, validate_emit_against_declaration,
};
pub(crate) use fetch::{
    parse_fetch_include_literals, resolve_fetch_include_field, validate_fetch,
    validate_fetch_object,
};
pub(crate) use navigate::{validate_navigate, validate_send_server};
pub(crate) use operator_args::validate_operator_arg_types;
pub(crate) use os_watch::validate_os_watch;
pub(crate) use persist::{persist_entity_index, validate_persist};
pub(crate) use program_eval::validate_program_eval;
pub(crate) use r#ref::validate_ref;
pub(crate) use set::{
    entity_field_type_map, expr_return_incompatible, validate_set, validate_set_enum_value,
    validate_set_object, validate_set_required_field_from_optional_knob, validate_set_return_type,
    validate_set_value_type, EntityFieldMeta,
};
pub use utils::effect_operators;
pub(crate) use utils::{extract_payload_fields, is_valid_event_identifier};

/// Validate all effects in a schema
pub fn validate_all_effects(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate effects in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_effects(
            schema,
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
    ctx.merge(validate_cancel_calls_have_a_keyed_call(schema));
    ctx.merge(validate_http_effects(schema));
}

/// Validate all effects in a trait definition
pub fn validate_trait_effects(
    schema: &OrbitalSchema,
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
    let per_event_shapes = collect_per_event_payload_shapes(trait_def, schema);
    let mut tick_types = binding_types(trait_def, schema, collect_payload_shapes(trait_def, schema), &per_event_shapes);
    tick_types.optional_payload = optional_payload_fields(trait_def, None);

    // Validate effects in transitions — scope payload types to the trigger event
    // so `?result` resolves against the correct event's payload schema.
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let scoped_payload = per_event_payloads
            .get(&transition.event)
            .cloned()
            .unwrap_or_else(|| payload_field_types.clone());
        let scoped_shapes = match per_event_shapes.get(&transition.event) {
            Some(shapes) => {
                let mut types = binding_types(trait_def, schema, shapes.clone(), &per_event_shapes);
                types.optional_payload = optional_payload_fields(trait_def, Some(&transition.event));
                types
            }
            None => tick_types.clone(),
        };
        if let Some(guard) = &transition.guard {
            result.merge(set::validate_comparison_operands(
                guard,
                &format!("{}.stateMachine.transitions[{}].guard", path, i),
                &entity_field_types,
                &scoped_payload,
            ));
            result.merge(validate_operator_arg_types(
                guard,
                &format!("{}.stateMachine.transitions[{}].guard", path, i),
                &entity_field_types,
                &scoped_payload,
                &scoped_shapes,
                Some(schema),
            ));
        }
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

                result.merge(set::validate_comparison_operands(
                    effect,
                    &effect_path,
                    &entity_field_types,
                    &scoped_payload,
                ));
                result.merge(validate_operator_arg_types(
                    effect,
                    &effect_path,
                    &entity_field_types,
                    &scoped_payload,
                    &scoped_shapes,
                    Some(schema),
                ));

                result.merge(validate_set_required_field_from_optional_knob(
                    effect,
                    &effect_path,
                    &trait_def.name,
                    &entity_field_types,
                ));
            }
        }
    }

    // Validate effects in ticks
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            if let Some(guard) = &tick.guard {
                result.merge(validate_operator_arg_types(
                    guard,
                    &format!("{}.ticks[{}].guard", path, i),
                    &entity_field_types,
                    &payload_field_types,
                    &tick_types,
                    Some(schema),
                ));
            }
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

                result.merge(set::validate_comparison_operands(
                    effect,
                    &effect_path,
                    &entity_field_types,
                    &payload_field_types,
                ));
                result.merge(validate_operator_arg_types(
                    effect,
                    &effect_path,
                    &entity_field_types,
                    &payload_field_types,
                    &tick_types,
                    Some(schema),
                ));

                result.merge(validate_set_required_field_from_optional_knob(
                    effect,
                    &effect_path,
                    &trait_def.name,
                    &entity_field_types,
                ));
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
    // An INBOUND event's declared payload — authored as
    // `listens { EVENT { field : type } }` and lowered onto
    // `stateMachine.events[].payloadSchema` — is the only declared type for an
    // event this trait receives but never emits. It wins over a same-named
    // emits entry, matching L1's `@payload.X` precedence (`checks.rs` resolves
    // listen payloads first, then emits). Without it a `?field` read falls back
    // to the flat trait-wide map and resolves against ANOTHER event's
    // same-named field — the false ORB_X_SET_VALUE_TYPE_MISMATCH class.
    if let Some(events) = &trait_def.state_machine.events {
        for event in events {
            if let Some(schema) = &event.payload_schema {
                let mut fields: HashMap<String, String> = HashMap::new();
                for field in schema {
                    fields.insert(field.name.clone(), field.field_type.clone());
                }
                map.insert(event.key.clone(), fields);
            }
        }
    }
    map
}

/// The `{ event -> { field -> OirType } }` payload shapes of a trait: the full declared type of every payload
/// field (lists, maps, structs, named types resolved), not the bare kind string. The same precedence as
/// [`collect_per_event_payload_field_types`]: an inbound event's declared payload wins over an emit's.
pub(crate) fn collect_per_event_payload_shapes(
    trait_def: &orbital_core::schema::TraitDefinition,
    schema: &OrbitalSchema,
) -> HashMap<String, HashMap<String, orbital_core::ir::OirType>> {
    let shapes = |fields: &[orbital_core::schema::PayloadField]| -> HashMap<String, orbital_core::ir::OirType> {
        fields
            .iter()
            .map(|f| (f.name.clone(), crate::phases::resolve::payload_field_oir_type(f, schema)))
            .collect()
    };
    let mut map = HashMap::new();
    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            if let Some(fields) = &emit.payload_schema {
                map.insert(emit.event.clone(), shapes(fields));
            }
        }
    }
    if let Some(events) = &trait_def.state_machine.events {
        for event in events {
            if let Some(fields) = &event.payload_schema {
                map.insert(event.key.clone(), shapes(fields));
            }
        }
    }
    map
}

/// The full declared types of the bindings one transition (or tick) of `trait_def` reads.
fn binding_types(
    trait_def: &orbital_core::schema::TraitDefinition,
    schema: &OrbitalSchema,
    payload: HashMap<String, orbital_core::ir::OirType>,
    per_event: &HashMap<String, HashMap<String, orbital_core::ir::OirType>>,
) -> set::BindingTypes {
    // The events the trait receives: an earlier delivery in the dispatch is one of them, so its payload is the record
    // carrying every field any of them declares, a field optional when not all declare it. A field the events
    // declare with different types has no one type and is left out.
    let mut merged: HashMap<String, (orbital_core::ir::OirType, usize)> = HashMap::new();
    let mut conflicting: HashSet<String> = HashSet::new();
    let mut deliveries = 0usize;
    let received: std::collections::BTreeSet<&str> = trait_def
        .state_machine
        .transitions
        .iter()
        .map(|t| t.event.as_str())
        .collect();
    for event in received {
        let Some(shape) = per_event.get(event) else {
            continue;
        };
        deliveries += 1;
        for (name, ty) in shape {
            match merged.get_mut(name) {
                Some((prior, seen)) if prior == ty => *seen += 1,
                Some(_) => {
                    conflicting.insert(name.clone());
                }
                None => {
                    merged.insert(name.clone(), (ty.clone(), 1));
                }
            }
        }
    }
    let prev_payload = (deliveries > 0).then(|| {
        set::BindingTypes::record_of(
            &merged
                .into_iter()
                .filter(|(name, _)| !conflicting.contains(name))
                .map(|(name, (ty, seen))| {
                    (name, if seen == deliveries { ty } else { orbital_core::ir::OirType::optional(ty) })
                })
                .collect(),
        )
    });
    set::BindingTypes {
        payload,
        prev_payload,
        optional_payload: Default::default(),
        user: super::user_identity::identity_type(schema, &trait_def.name),
        config: trait_def
            .config
            .iter()
            .flatten()
            .map(|(name, field)| (name.clone(), crate::phases::resolve::config_field_oir_type(field, schema)))
            .collect(),
    }
}

/// The payload fields the declaration of `event` (or, for a tick, the first emit declaring each field) does not
/// require. The same precedence as [`collect_per_event_payload_shapes`]: an inbound event's declared payload
/// wins over an emit's.
fn optional_payload_fields(
    trait_def: &orbital_core::schema::TraitDefinition,
    event: Option<&str>,
) -> HashSet<String> {
    let optional_of = |fields: &[orbital_core::schema::PayloadField]| -> HashSet<String> {
        fields
            .iter()
            .filter(|f| f.required != Some(true))
            .map(|f| f.name.clone())
            .collect()
    };
    let Some(event) = event else {
        let mut first: HashMap<&str, bool> = HashMap::new();
        for emit in trait_def.emits.iter().flatten() {
            for f in emit.payload_schema.iter().flatten() {
                first.entry(f.name.as_str()).or_insert(f.required != Some(true));
            }
        }
        return first.into_iter().filter(|(_, optional)| *optional).map(|(n, _)| n.to_string()).collect();
    };
    let inbound = trait_def
        .state_machine
        .events
        .iter()
        .flatten()
        .find(|e| e.key == event)
        .and_then(|e| e.payload_schema.as_deref());
    let emitted = || {
        trait_def
            .emits
            .iter()
            .flatten()
            .find(|e| e.event == event)
            .and_then(|e| e.payload_schema.as_deref())
    };
    inbound.or_else(emitted).map(optional_of).unwrap_or_default()
}

/// The payload shapes of a trait with no triggering event (ticks): the first declared emit's type of each field.
pub(crate) fn collect_payload_shapes(
    trait_def: &orbital_core::schema::TraitDefinition,
    schema: &OrbitalSchema,
) -> HashMap<String, orbital_core::ir::OirType> {
    let mut map = HashMap::new();
    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            for field in emit.payload_schema.iter().flatten() {
                map.entry(field.name.clone())
                    .or_insert_with(|| crate::phases::resolve::payload_field_oir_type(field, schema));
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

#[cfg(test)]
mod per_event_payload_tests {
    use super::*;

    fn listening_trait() -> orbital_core::schema::TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Listener",
            "stateMachine": {
                "initial": "idle",
                "states": [{ "name": "idle" }],
                "events": [
                    { "key": "A", "payloadSchema": [{ "name": "id", "type": "string" }, { "name": "kind", "type": "string" }] },
                    { "key": "B", "payloadSchema": [{ "name": "id", "type": "string" }, { "name": "kind", "type": "number" }, { "name": "n", "type": "number" }] }
                ],
                "transitions": [
                    { "from": "idle", "event": "A", "to": "idle" },
                    { "from": "idle", "event": "B", "to": "idle" }
                ]
            },
            "emits": []
        }))
        .expect("fixture deserializes")
    }

    #[test]
    fn an_earlier_delivery_is_the_merge_of_the_listened_payloads() {
        use orbital_core::ir::OirType;
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({ "name": "App", "orbitals": [] })).unwrap();
        let trait_def = listening_trait();
        let per_event = collect_per_event_payload_shapes(&trait_def, &schema);
        let types = binding_types(&trait_def, &schema, HashMap::new(), &per_event);
        let Some(OirType::Struct { fields }) = types.prev_payload else {
            panic!("a trait that listens has a prior delivery payload");
        };
        let by_name = |name: &str| fields.iter().find(|f| f.name == name).map(|f| f.field_type.clone());
        assert_eq!(by_name("id"), Some(OirType::String), "declared by both with one type");
        assert_eq!(by_name("n"), Some(OirType::optional(OirType::Number)), "declared by one of two");
        assert_eq!(by_name("kind"), None, "control: declared with two types, so it has none");
        let silent = binding_types(&listening_trait(), &schema, HashMap::new(), &HashMap::new());
        assert!(silent.prev_payload.is_none(), "control: events with no declared payload give none");
    }

    /// An inbound event declared via `listens { EVENT { field : type } }`
    /// lowers onto `stateMachine.events[].payloadSchema`. Its type must win
    /// for that event, even when an UNRELATED emit declares a same-named
    /// field with a different type — resolving `?value` against the emit was
    /// a false ORB_X_SET_VALUE_TYPE_MISMATCH on every writeback in the trait.
    #[test]
    fn inbound_event_payload_wins_over_same_named_emit_field() {
        let trait_def: orbital_core::schema::TraitDefinition = serde_json::from_value(
            serde_json::json!({
                "name": "SvcTrait",
                "stateMachine": {
                    "initial": "idle",
                    "states": [{ "name": "idle" }],
                    "events": [
                        { "key": "SET_KEY", "payloadSchema": [{ "name": "value", "type": "string" }] }
                    ],
                    "transitions": []
                },
                "emits": [
                    { "event": "GetDone", "payloadSchema": [{ "name": "value", "type": "Map<string,string>" }] }
                ]
            }),
        )
        .expect("fixture deserializes");

        let per_event = collect_per_event_payload_field_types(&trait_def);
        assert_eq!(
            per_event
                .get("SET_KEY")
                .and_then(|f| f.get("value"))
                .map(String::as_str),
            Some("string"),
            "SET_KEY's own declared payload must type its `?value`"
        );
        assert_eq!(
            per_event
                .get("GetDone")
                .and_then(|f| f.get("value"))
                .map(String::as_str),
            Some("Map<string,string>"),
            "the emit keeps its own payload type"
        );
    }
}
