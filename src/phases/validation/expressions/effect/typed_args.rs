//! Declared shapes recorded in the resolved program: a `json/parse` / `as` shape naming a program alias gets that
//! alias's type, and a `let` binding's declared type (a local annotation, or a knob's literal bound by
//! `inline::rewrite::declared_list_types`) gets the program types it names. No operator argument is wrapped any more:
//! declared types are static and the kernel checks no argument.

use super::set::ExprReturnCtx;
use std::sync::Arc;
use super::TraitScopes;
use orbital_core_base::ir::types::{DeclaredShape, OirType};
use orbital_core_base::schema::types::PayloadField;
use orbital_core_base::schema::{
    EntityDefinition, OrbitalSchema, RewriteVisit, SExpression, TraitReference,
};
use std::collections::{BTreeMap, HashMap};

/// Record the declared list types of `schema`'s operator arguments in place.
pub fn record_declared_shapes(schema: &mut OrbitalSchema) {
    let view = schema.clone();
    let mut entities: HashMap<String, EntityDefinition> = HashMap::new();
    for orbital in &view.orbitals {
        if let Some(entity) = orbital.entity.as_inline() {
            entities.insert(entity.name.clone(), entity.clone());
        }
        for aux in orbital.auxiliary_entities.iter().flatten() {
            if let Some(entity) = aux.as_inline() {
                entities.insert(entity.name.clone(), entity.clone());
            }
        }
    }
    for (i, orbital) in view.orbitals.iter().enumerate() {
        let mut aliases = view.types.clone().unwrap_or_default();
        aliases.extend(orbital.types.clone().unwrap_or_default());
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let scopes = TraitScopes::new(&view, trait_def, &entities);
            let TraitReference::Inline(target) = &mut schema.orbitals[i].traits[j] else {
                continue;
            };
            let target = Arc::make_mut(target);
            for (k, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                let payload = scopes.payload_for(&transition.event);
                let types = scopes.types_for(&view, trait_def, &transition.event);
                let ctx = ExprReturnCtx {
                    entity: &scopes.entity_field_types,
                    payload: &payload,
                    types: Some(&types),
                    schema: Some(&view),
                };
                let out = &mut target.state_machine.transitions[k];
                if let Some(guard) = &transition.guard {
                    out.guard = Some(declare(guard, &ctx, &aliases));
                }
                if let Some(effects) = &transition.effects {
                    out.effects =
                        Some(effects.iter().map(|e| declare(e, &ctx, &aliases)).collect());
                }
            }
            if let Some(ticks) = &trait_def.ticks {
                let ctx = ExprReturnCtx {
                    entity: &scopes.entity_field_types,
                    payload: &scopes.payload_field_types,
                    types: Some(&scopes.tick_types),
                    schema: Some(&view),
                };
                let Some(out_ticks) = target.ticks.as_mut() else {
                    continue;
                };
                for (k, tick) in ticks.iter().enumerate() {
                    if let Some(guard) = &tick.guard {
                        out_ticks[k].guard = Some(declare(guard, &ctx, &aliases));
                    }
                    out_ticks[k].effects = tick
                        .effects
                        .iter()
                        .map(|e| declare(e, &ctx, &aliases))
                        .collect();
                }
            }
        }
    }
}

/// `expr` with each `json/parse` / `as` shape naming a program alias recorded as that alias's type (G-ORB-420).
fn declare(expr: &SExpression, ctx: &ExprReturnCtx<'_>, aliases: &Aliases) -> SExpression {
    let version = ctx.schema.and_then(|schema| schema.type_contract_version);
    record_parse_shapes(expr, aliases, version)
}

type Aliases = BTreeMap<String, Vec<PayloadField>>;

/// A declared type as the kernel's run-time check reads it: the type, with the program types it names, transitively.
fn recorded_type(name: Option<String>, ty: OirType, aliases: &Aliases, version: Option<u32>) -> String {
    let mut shape = DeclaredShape::with_named(name, ty, aliases);
    shape.version = version;
    if shape.version.is_none() && shape.named.is_empty() && shape.name.is_none() {
        return serde_json::to_string(&shape.ty).unwrap_or_default();
    }
    serde_json::to_string(&shape).unwrap_or_default()
}

/// `expr` with every `json/parse` whose shape names a program alias given that alias's recorded type, and every
/// `let` binding's declared type given the program types it names.
fn record_parse_shapes(expr: &SExpression, aliases: &Aliases, version: Option<u32>) -> SExpression {
    let rewritten: Result<SExpression, std::convert::Infallible> = expr.try_rewrite(
        |node| {
            Ok(if matches!(node.operator(), Some("quote" | "quasiquote")) {
                RewriteVisit::Keep
            } else {
                RewriteVisit::Descend
            })
        },
        |node| {
            let mut out = match node {
                SExpression::Array(out) => out,
                other => return Ok(other),
            };
            if out.first().and_then(SExpression::as_str) == Some("let") {
                if let Some(SExpression::Array(bindings)) = out.get_mut(1) {
                    for binding in bindings {
                        // A knob's declaration records its type before the orbital's aliases are in reach; the
                        // types it names are added here.
                        if let SExpression::Array(parts) = binding {
                            if let [_, _, SExpression::String(shape)] = parts.as_mut_slice() {
                                if let Some(mut declared) = DeclaredShape::parse(shape) {
                                    declared.version = version.or(declared.version);
                                    *shape = if declared.named.is_empty() {
                                        recorded_type(declared.name, declared.ty, aliases, declared.version).into()
                                    } else {
                                        serde_json::to_string(&declared).unwrap_or_default().into()
                                    };
                                }
                            }
                        }
                    }
                }
            }
            if let [SExpression::String(head), _, SExpression::String(shape)] = out.as_slice() {
                if head == "json/parse" || head == "as" {
                    if let Some(declared) = DeclaredShape::of_alias(shape, aliases) {
                        out[2] =
                            SExpression::string(recorded_type(declared.name, declared.ty, aliases, version.or(declared.version)));
                    }
                }
            }
            Ok(SExpression::Array(out))
        },
    );
    match rewritten {
        Ok(out) => out,
        Err(never) => match never {},
    }
}

/// Whether a declared type is a list whose items may differ in kind: a list of a union or of `SExpr`.
pub(crate) fn needs_declaration(ty: &OirType) -> bool {
    let peel = |t: &OirType| match t {
        OirType::Optional { inner } => (**inner).clone(),
        other => other.clone(),
    };
    match peel(ty) {
        OirType::Array { inner } => matches!(peel(&inner), OirType::Union { .. } | OirType::SExpr),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program(via: serde_json::Value, effect: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(json!({
            "name": "probe",
            "orbitals": [{
                "name": "ProbeOrbital",
                "entity": { "name": "Probe", "persistence": "runtime", "fields": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "count", "type": "number" }
                ] },
                "traits": [{
                    "name": "Prober",
                    "linkedEntity": "Probe",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "SCAN", "name": "Scan", "payloadSchema": [via] }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "SCAN", "effects": [effect] }]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("probe program")
    }

    fn union_via() -> serde_json::Value {
        json!({ "name": "via", "type": "array", "items": { "name": "item", "type": "union", "properties": [
            { "name": "Adv", "type": "object", "properties": [{ "name": "id", "type": "number" }] },
            { "name": "string", "type": "string" }
        ] } })
    }

    fn effect_of(schema: &OrbitalSchema) -> serde_json::Value {
        let TraitReference::Inline(t) = &schema.orbitals[0].traits[0] else {
            panic!("inline trait")
        };
        serde_json::to_value(
            &t.state_machine.transitions[0]
                .effects
                .as_ref()
                .expect("effects")[0],
        )
        .unwrap()
    }

    #[test]
    fn checked_local_initializers_keep_nested_declared_union_arguments_at_runtime() {
        use orbital_core_base::ir::OirField;
        use orbital_core_eval::evaluator::{context::BindingContextBuilder, Evaluator};
        let ty = OirType::array(OirType::Union {
            variants: vec![
                OirField::new(
                    "Adv",
                    OirType::Struct {
                        additional: None,
                        fields: vec![OirField::new("id", OirType::Number)].into(),
                    },
                ),
                OirField::new("string", OirType::String),
            ].into(),
        });
        let mut via = union_via();
        via["items"]["properties"][0]["properties"][0]["required"] = json!(true);
        let mut schema = program(
            via,
            json!([
                "set",
                "@entity.count",
                [
                    "let",
                    [[
                        "values",
                        ["array/filter", "@payload.via", ["fn", "x", true]],
                        serde_json::to_string(&ty).unwrap()
                    ]],
                    ["array/len", "@values"]
                ]
            ]),
        );
        record_declared_shapes(&mut schema);
        let effect = effect_of(&schema);
        let expression = serde_json::from_value(effect[2].clone()).unwrap();
        let payload = serde_json::from_value(json!({"via": [{"id": 1}, "text"]})).unwrap();
        let context = BindingContextBuilder::new().payload(payload).build();
        let actual = Evaluator::new()
            .evaluate(&expression, &context)
            .unwrap_or_else(|error| panic!("declared union argument inside a checked initializer: {error}; effect={effect}"));
        assert_eq!(actual, orbital_core_base::value::Value::Number(2.0));
    }

    #[test]
    fn control_a_union_without_a_list_member_is_left_as_written() {
        let via = json!({ "name": "via", "type": "union", "properties": [
            { "name": "string", "type": "string" },
            { "name": "number", "type": "number" }
        ] });
        let effect = json!(["set", "@entity.count", ["array/len", "@payload.via"]]);
        let mut schema = program(via, effect.clone());
        record_declared_shapes(&mut schema);
        assert_eq!(effect_of(&schema), effect);
    }

    fn with_row_types(mut schema: OrbitalSchema) -> OrbitalSchema {
        schema.orbitals[0].types = Some(
            serde_json::from_value(json!({
                "RowFile": [
                    { "name": "rows", "type": "[Row]", "required": true },
                    { "name": "changes", "type": "[string]", "required": true }
                ],
                "Row": [{ "name": "", "type": "Map<string,RowCell>" }],
                "RowCell": [{ "name": "", "type": "union", "properties": [
                    { "name": "string", "type": "string" },
                    { "name": "array", "type": "[RowCell]" }
                ] }],
                "Point": [{ "name": "x", "type": "number", "required": true }]
            }))
            .expect("aliases"),
        );
        schema
    }

    fn parse_shape_of(shape: &str) -> serde_json::Value {
        let via = json!({ "name": "via", "type": "string" });
        let effect = json!([
            "set",
            "@entity.count",
            ["json/parse", "@payload.via", shape]
        ]);
        let mut schema = with_row_types(program(via, effect));
        record_declared_shapes(&mut schema);
        effect_of(&schema)[2][2].clone()
    }

    #[test]
    fn native_contract_root_version_and_aliases_reach_json_parse_descriptors() {
        let effect = json!(["set", "@entity.count", ["json/parse", "@payload.via", "Point"]]);
        let mut schema = with_row_types(program(json!({"name":"via","type":"string"}), effect));
        schema.types = schema.orbitals[0].types.take();
        schema.type_contract_version = Some(1);
        record_declared_shapes(&mut schema);
        let value = effect_of(&schema);
        let recorded = DeclaredShape::parse(value[2][2].as_str().expect("recorded descriptor"))
            .expect("root alias must resolve through the canonical schema owner");
        assert_eq!(recorded.version, Some(1));
        assert_eq!(recorded.name.as_deref(), Some("Point"));
        assert!(matches!(recorded.ty, OirType::Struct { ref fields, .. } if fields.iter().any(|field| field.name == "x")));
    }

    #[test]
    fn native_contract_legacy_root_does_not_infer_version_from_field_metadata() {
        let effect = json!(["set", "@entity.count", ["json/parse", "@payload.via", "Point"]]);
        let mut schema = with_row_types(program(json!({"name":"via","type":"string"}), effect));
        schema.orbitals[0].types.as_mut().unwrap().get_mut("Point").unwrap()[0].nullable = Some(false);
        assert_eq!(schema.type_contract_version, None);
        record_declared_shapes(&mut schema);
        let value = effect_of(&schema);
        let recorded = DeclaredShape::parse(value[2][2].as_str().expect("recorded descriptor")).unwrap();
        assert_eq!(recorded.version, None, "field facts cannot invent root provenance");
    }

    #[test]
    fn a_json_parse_naming_a_program_alias_records_its_type_with_every_type_it_names() {
        let recorded: DeclaredShape =
            serde_json::from_str(parse_shape_of("RowFile").as_str().expect("shape text"))
                .expect("a recorded shape");
        assert_eq!(recorded.name.as_deref(), Some("RowFile"));
        assert!(
            matches!(recorded.ty, OirType::Struct { .. }),
            "{:?}",
            recorded.ty
        );
        assert_eq!(
            recorded.named.keys().collect::<Vec<_>>(),
            ["Row", "RowCell"],
            "transitively, the recursive one once"
        );
    }

    #[test]
    fn a_cast_naming_a_program_alias_records_its_type_and_a_nominal_cast_is_left_as_written() {
        let cast = |shape: &str| {
            let effect = json!(["set", "@entity.count", ["as", "@payload.via", shape]]);
            let mut schema = with_row_types(program(json!({ "name": "via", "type": "string" }), effect));
            record_declared_shapes(&mut schema);
            effect_of(&schema)[2][2].clone()
        };
        let recorded = DeclaredShape::parse(cast("Point").as_str().expect("recorded descriptor")).expect("an alias records its type");
        assert_eq!(recorded.name.as_deref(), Some("Point"));
        assert_eq!(cast("CallKey"), json!("CallKey"), "control: a nominal type is not an alias");
    }

    #[test]
    fn a_json_parse_naming_a_non_recursive_alias_records_its_type() {
        let recorded: DeclaredShape =
            serde_json::from_str(parse_shape_of("Point").as_str().expect("shape text"))
                .expect("a recorded shape");
        assert!(matches!(recorded.ty, OirType::Struct { ref fields, .. } if fields.len() == 1));
        assert!(recorded.named.is_empty());
    }

    #[test]
    fn control_a_json_parse_naming_no_program_alias_is_left_as_written() {
        assert_eq!(parse_shape_of("number"), json!("number"));
        assert_eq!(parse_shape_of("Elsewhere"), json!("Elsewhere"));
    }

    #[test]
    fn control_a_list_of_one_kind_is_left_as_written() {
        let names = json!({ "name": "via", "type": "[string]" });
        let effect = json!(["set", "@entity.count", ["array/len", "@payload.via"]]);
        let mut schema = program(names, effect.clone());
        record_declared_shapes(&mut schema);
        assert_eq!(effect_of(&schema), effect);
    }
}
