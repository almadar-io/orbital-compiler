//! Effect Validation
//!
//! Validates effects: call_service, set, fetch, emit, persist, etc.
//! Ported from `builder/packages/shared/src/orbitals/validation/effect-validator.ts`.

use super::{PageInitPayload, ValidationContext, ValidationResult};
use crate::embedded::ServiceRegistryData;
use orbital_core_base::schema::{EntityDefinition, OrbitalSchema, TraitDefinition};
use std::collections::{HashMap, HashSet};

mod call_service;
mod emit;
mod emit_declaration;
mod fetch;
mod host_effects;
mod http;
mod http_site;
mod inbound;
mod keyed_call;
mod navigate;
mod oauth;
mod cast;
mod operator_args;
mod os_grants;
mod os_watch;
mod persist;
mod postgres;
mod program_eval;
mod redis;
mod set;
mod typed_args;
mod utils;

pub(crate) use call_service::validate_call_service;
pub(crate) use host_effects::validate_host_effects;
pub(crate) use oauth::validate_oauth_effects;
pub(crate) use http::validate_http_effects;
pub(crate) use inbound::validate_inbound;
pub(crate) use http_site::validate_http_auth_placement;
pub(crate) use emit::{extract_emit_object_ref, validate_emit, validate_emit_config};
pub(crate) use emit_declaration::{collect_declared_emits, nested_effects, scope_let_locals};
#[cfg(test)]
pub(crate) use emit_declaration::validate_effect;
pub(crate) use fetch::{
    parse_fetch_include_literals, resolve_fetch_include_field, validate_fetch,
};
pub(crate) use navigate::{validate_navigate, validate_send_server};
pub(crate) use keyed_call::validate_call_key_literals_are_used;
use keyed_call::CallKeyFacts;
pub(crate) use operator_args::{validate_operator_arg_types, validate_operator_arg_types_under};
pub(crate) use os_grants::{validate_final_os_targets, validate_os_grants};
pub(crate) use os_watch::{validate_os_effect, validate_os_watch};
pub(crate) use persist::{persist_entity_index, validate_persist};
pub(crate) use program_eval::validate_program_eval;
pub(crate) use postgres::validate_postgres_effects;
pub(crate) use redis::validate_redis_effects;
pub(crate) use set::{
    entity_field_type_map, expr_return_incompatible_given, expr_return_incompatible_in_ctx, validate_set,
    validate_set_required_field_from_optional_knob, validate_set_return_type,
    validate_set_value_type, BindingTypes, EntityFieldMeta, ExprReturnCtx,
};
pub use typed_args::record_declared_list_types;
pub(crate) use typed_args::needs_declaration;
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
    ctx.merge(validate_call_key_literals_are_used(schema));
    ctx.merge(validate_http_effects(schema));
    ctx.merge(validate_inbound(schema));
    validate_http_auth_placement(schema, ctx);
    ctx.merge(validate_host_effects(schema));
    ctx.merge(validate_redis_effects(schema));
    ctx.merge(validate_postgres_effects(schema));
    ctx.merge(validate_oauth_effects(schema));
    ctx.merge(validate_os_grants(schema));
}

/// The binding types one trait's effects are checked against: the linked entity's fields, and the payload of each
/// triggering event (a tick has none). One construction shared by the per-effect checks and the `--final` key facts.
pub(crate) struct TraitScopes {
    pub(crate) entity_field_types: HashMap<String, EntityFieldMeta>,
    per_event_payloads: HashMap<String, HashMap<String, String>>,
    pub(crate) payload_field_types: HashMap<String, String>,
    per_event_shapes: HashMap<String, HashMap<String, orbital_core_base::ir::OirType>>,
    pub(crate) tick_types: set::BindingTypes,
}

impl TraitScopes {
    pub(crate) fn new(
        schema: &OrbitalSchema,
        trait_def: &TraitDefinition,
        entity_definitions: &HashMap<String, EntityDefinition>,
    ) -> Self {
        let entity_field_types = trait_def
            .linked_entity
            .as_ref()
            .and_then(|e| entity_definitions.get(e))
            .map(entity_field_type_map)
            .unwrap_or_default();
        let per_event_payloads = collect_per_event_payload_field_types(trait_def);
        let payload_field_types = collect_payload_field_types(trait_def);
        let per_event_shapes = collect_per_event_payload_shapes(trait_def, schema);
        let mut tick_types =
            binding_types(trait_def, schema, collect_payload_shapes(trait_def, schema), &per_event_shapes);
        tick_types.optional_payload = optional_payload_fields(trait_def, None);
        Self {
            entity_field_types,
            per_event_payloads,
            payload_field_types,
            per_event_shapes,
            tick_types,
        }
    }

    /// The payload field kinds `?field` resolves against for a transition on `event`.
    pub(crate) fn payload_for(&self, event: &str) -> HashMap<String, String> {
        self.per_event_payloads
            .get(event)
            .cloned()
            .unwrap_or_else(|| self.payload_field_types.clone())
    }

    /// The full binding types for a transition on `event`.
    pub(crate) fn types_for(
        &self,
        schema: &OrbitalSchema,
        trait_def: &TraitDefinition,
        event: &str,
    ) -> set::BindingTypes {
        match self.per_event_shapes.get(event) {
            Some(shapes) => {
                let shapes = hydrate_included_relations(schema, trait_def, event, shapes.clone());
                let mut types = binding_types(trait_def, schema, shapes, &self.per_event_shapes);
                types.optional_payload = optional_payload_fields(trait_def, Some(event));
                types
            }
            None => self.tick_types.clone(),
        }
    }
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
    let scopes = TraitScopes::new(schema, trait_def, entity_definitions);
    let entity_field_types = &scopes.entity_field_types;
    let payload_field_types = &scopes.payload_field_types;
    let tick_types = &scopes.tick_types;

    // Validate effects in transitions — scope payload types to the trigger event
    // so `?result` resolves against the correct event's payload schema.
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let scoped_payload = scopes.payload_for(&transition.event);
        let scoped_shapes = scopes.types_for(schema, trait_def, &transition.event);
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
                let effect_result = emit_declaration::validate_effect_in(
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
                    &scoped_shapes.payload,
                    Some(schema),
                    Some(&scoped_shapes),
                );
                result.merge(effect_result);

                result.merge(set::validate_comparison_operands(
                    effect,
                    &effect_path,
                    &entity_field_types,
                    &scoped_payload,
                ));
                result.merge(validate_operator_arg_types_under(
                    effect,
                    transition.guard.as_ref(),
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
                let effect_result = emit_declaration::validate_effect_in(
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
                    &tick_types.payload,
                    Some(schema),
                    Some(&tick_types),
                );
                result.merge(effect_result);

                result.merge(set::validate_comparison_operands(
                    effect,
                    &effect_path,
                    &entity_field_types,
                    &payload_field_types,
                ));
                result.merge(validate_operator_arg_types_under(
                    effect,
                    tick.guard.as_ref(),
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
    trait_def: &orbital_core_base::schema::TraitDefinition,
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
    trait_def: &orbital_core_base::schema::TraitDefinition,
    schema: &OrbitalSchema,
) -> HashMap<String, HashMap<String, orbital_core_base::ir::OirType>> {
    let shapes = |fields: &[orbital_core_base::schema::PayloadField]| -> HashMap<String, orbital_core_base::ir::OirType> {
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

/// A relation every fetch answering `event` lists in `include` reads as the related row in that transition's payload;
/// otherwise it stays the related row's id (G-CROSS-314).
fn hydrate_included_relations(
    schema: &OrbitalSchema,
    trait_def: &TraitDefinition,
    event: &str,
    mut shapes: HashMap<String, orbital_core_base::ir::OirType>,
) -> HashMap<String, orbital_core_base::ir::OirType> {
    use orbital_core_base::schema::SExpression;
    fn fetches<'a>(expr: &'a SExpression, out: &mut Vec<(&'a str, &'a orbital_core_base::schema::SObject)>) {
        match expr {
            SExpression::Array(items) => {
                if let [SExpression::String(op), SExpression::String(entity), SExpression::Object(options), ..] =
                    items.as_slice()
                {
                    if op == "fetch" {
                        out.push((entity.as_str(), options));
                    }
                }
                for item in items {
                    fetches(item, out);
                }
            }
            SExpression::Object(map) => {
                for value in map.values() {
                    fetches(value, out);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for transition in &trait_def.state_machine.transitions {
        for effect in transition.effects.iter().flatten() {
            fetches(effect, &mut found);
        }
    }
    let answering: Vec<_> = found
        .into_iter()
        .filter(|(_, options)| {
            matches!(options.get("emit"), Some(SExpression::Object(emit))
                if matches!(emit.get("success"), Some(SExpression::String(success)) if success == event))
        })
        .collect();
    let Some((entity, _)) = answering.first().copied() else {
        return shapes;
    };
    if answering.iter().any(|(other, _)| *other != entity) {
        return shapes;
    }
    let Some(def) = schema.orbitals.iter().find_map(|o| {
        std::iter::once(&o.entity).chain(o.auxiliary_entities.iter().flatten()).find_map(|e| match e {
            orbital_core_base::schema::EntityRef::Inline(def) if def.name == entity => Some(def),
            _ => None,
        })
    }) else {
        return shapes;
    };
    let included_by = |options: &orbital_core_base::schema::SObject| -> HashSet<String> {
        let include = match options.get("include") {
            Some(SExpression::String(knob)) => knob
                .strip_prefix("@config.")
                .and_then(|knob| trait_def.config.as_ref()?.get(knob)?.default.as_ref()),
            other => other,
        };
        include
            .and_then(fetch::parse_fetch_include_literals)
            .into_iter()
            .flatten()
            .filter_map(|name| fetch::resolve_fetch_include_field(def, &name))
            .collect()
    };
    let mut included = included_by(answering[0].1);
    for (_, options) in &answering[1..] {
        let other = included_by(options);
        included.retain(|field| other.contains(field));
    }
    if included.is_empty() {
        return shapes;
    }
    fn as_row(ty: orbital_core_base::ir::OirType) -> orbital_core_base::ir::OirType {
        use orbital_core_base::ir::OirType;
        match ty {
            OirType::Reference { entity } => OirType::Row { entity },
            OirType::Optional { inner } => OirType::optional(as_row(*inner)),
            OirType::Array { inner } => OirType::array(as_row(*inner)),
            other => other,
        }
    }
    let mut fields: Vec<orbital_core_base::ir::OirField> = def
        .fields
        .iter()
        .filter_map(|f| {
            let name = f.name.clone()?;
            let shape = orbital_core_base::ir::types::field_shape(f);
            let shape = if included.contains(&name) { as_row(shape) } else { shape };
            Some(orbital_core_base::ir::OirField::new(name, shape))
        })
        .collect();
    for (system, ty) in [
        ("id", orbital_core_base::ir::OirType::String),
        ("createdAt", orbital_core_base::ir::OirType::DateTime),
        ("updatedAt", orbital_core_base::ir::OirType::DateTime),
    ] {
        if !fields.iter().any(|f| f.name == system) {
            fields.push(orbital_core_base::ir::OirField::new(system.to_string(), ty));
        }
    }
    let hydrated = orbital_core_base::ir::OirType::Struct { additional: None, fields: fields.into() };
    fn replace(
        ty: &mut orbital_core_base::ir::OirType,
        entity: &str,
        hydrated: &orbital_core_base::ir::OirType,
    ) {
        use orbital_core_base::ir::OirType;
        match ty {
            OirType::Row { entity: row } | OirType::Reference { entity: row } if row.as_str() == entity => *ty = hydrated.clone(),
            OirType::Optional { inner } | OirType::Array { inner } => replace(inner, entity, hydrated),
            _ => {}
        }
    }
    for ty in shapes.values_mut() {
        replace(ty, entity, &hydrated);
    }
    shapes
}

/// The full declared types of the bindings one transition (or tick) of `trait_def` reads.
fn binding_types(
    trait_def: &orbital_core_base::schema::TraitDefinition,
    schema: &OrbitalSchema,
    payload: HashMap<String, orbital_core_base::ir::OirType>,
    per_event: &HashMap<String, HashMap<String, orbital_core_base::ir::OirType>>,
) -> set::BindingTypes {
    // The events the trait receives: an earlier delivery in the dispatch is one of them, so its payload is the record
    // carrying every field any of them declares, a field optional when not all declare it. A field the events
    // declare with different types has no one type and is left out.
    let mut merged: HashMap<String, (orbital_core_base::ir::OirType, usize)> = HashMap::new();
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
                    (name, if seen == deliveries { ty } else { orbital_core_base::ir::OirType::optional(ty) })
                })
                .collect(),
        )
    });
    set::BindingTypes {
        payload,
        prev_payload,
        optional_payload: Default::default(),
        locals: Default::default(),
        type_params: trait_def.type_params.iter().flatten().map(|param| param.name.clone()).collect(),
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
    trait_def: &orbital_core_base::schema::TraitDefinition,
    event: Option<&str>,
) -> HashSet<String> {
    let optional_of = |fields: &[orbital_core_base::schema::PayloadField]| -> HashSet<String> {
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
    trait_def: &orbital_core_base::schema::TraitDefinition,
    schema: &OrbitalSchema,
) -> HashMap<String, orbital_core_base::ir::OirType> {
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
    trait_def: &orbital_core_base::schema::TraitDefinition,
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
    #[allow(dead_code)] // collected + constructed by tests; not read by validators yet
    pub(crate) payload_fields: Vec<String>,
}

#[cfg(test)]
mod per_event_payload_tests {
    use super::*;

    fn listening_trait() -> orbital_core_base::schema::TraitDefinition {
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
        use orbital_core_base::ir::OirType;
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({ "name": "App", "orbitals": [] })).unwrap();
        let trait_def = listening_trait();
        let per_event = collect_per_event_payload_shapes(&trait_def, &schema);
        let types = binding_types(&trait_def, &schema, HashMap::new(), &per_event);
        let Some(OirType::Struct { fields, .. }) = types.prev_payload else {
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
        let trait_def: orbital_core_base::schema::TraitDefinition = serde_json::from_value(
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

/// `orb validate --final`: in a finished program a `cancel-call` on a `CallKey` (or a list of them) must have a keyed
/// call whose `key:` is itself `CallKey`-typed somewhere in the program. The type is how the key crosses orbitals:
/// the cast made it, the event payload carries it, the call runs under it. An atom validated alone is exempt, because
/// the composing program supplies the call; this is where that promise is checked.
pub(crate) fn validate_final_cancel_keys(schema: &OrbitalSchema) -> ValidationResult {
    let entity_definitions: HashMap<String, EntityDefinition> = schema
        .orbitals
        .iter()
        .flat_map(|o| std::iter::once(&o.entity).chain(o.auxiliary_entities.iter().flatten()))
        .filter_map(|e| e.as_inline().map(|def| (def.name.clone(), def.clone())))
        .collect();
    let mut facts = CallKeyFacts::default();
    for (trait_def, path) in super::inline_traits(schema) {
        let scopes = TraitScopes::new(schema, trait_def, &entity_definitions);
        for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
            let payload = scopes.payload_for(&transition.event);
            let types = scopes.types_for(schema, trait_def, &transition.event);
            let ctx = set::ExprReturnCtx {
                entity: &scopes.entity_field_types,
                payload: &payload,
                types: Some(&types),
                schema: Some(schema),
            };
            for (j, effect) in transition.effects.iter().flatten().enumerate() {
                let effect_path = format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
                keyed_call::collect_call_key_facts(effect, &effect_path, &ctx, &mut facts);
            }
        }
        for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
            let ctx = set::ExprReturnCtx {
                entity: &scopes.entity_field_types,
                payload: &scopes.payload_field_types,
                types: Some(&scopes.tick_types),
                schema: Some(schema),
            };
            for (j, effect) in tick.effects.iter().enumerate() {
                let effect_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                keyed_call::collect_call_key_facts(effect, &effect_path, &ctx, &mut facts);
            }
        }
    }
    let mut result = ValidationResult::new();
    if facts.call_key_calls == 0 {
        for site in &facts.typed_cancels {
            result.add_error(
                super::ValidationError::error(
                    super::ErrorCode::CancelCallKeyNeverCalled,
                    site,
                    "this `cancel-call` names a `CallKey`, but no keyed call in the finished program runs under a `CallKey`-typed `key:`, so it can never abort anything",
                )
                .with_suggestion(
                    "Key the call it should abort with the same `CallKey` (`key: <a CallKey field or payload member>`, or `key: (as <string> CallKey)`)",
                ),
            );
        }
    }
    result
}
