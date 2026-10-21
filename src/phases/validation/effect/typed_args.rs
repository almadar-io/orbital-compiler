//! Declared list types for the kernel's argument check (G-CROSS-270). The runtime check (D-12) binds an operator's
//! type variable from the value it is handed, so a list whose items differ in kind (`[(Adv | string)]`, `[SExpr]`)
//! is a mismatch at run time although the program declared it. This pass records, in the resolved program, the type
//! the checker inferred for every such argument: `["typed", <argument>, <declared type as JSON>]`. Only an argument
//! of a list parameter (`[T]`) whose declared type is a list of a union or of `SExpr` is recorded; every other call is
//! left as it is.

use super::set::ExprReturnCtx;
use super::TraitScopes;
use crate::type_infer::typed_slot_types;
use orbital_core_base::ir::types::{DeclaredShape, OirType};
use orbital_core_base::schema::types::PayloadField;
use orbital_core_base::language::registry::{OperatorTypeRef, OperatorTypeRefKind};
use orbital_core_base::schema::{EntityDefinition, OrbitalSchema, SExpression, TraitReference};
use std::collections::{BTreeMap, HashMap};

const SLOT: &str = "#slot:";

/// Record the declared list types of `schema`'s operator arguments in place.
pub fn record_declared_list_types(schema: &mut OrbitalSchema) {
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
        let aliases = orbital.types.clone().unwrap_or_default();
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let scopes = TraitScopes::new(&view, trait_def, &entities);
            let TraitReference::Inline(target) = &mut schema.orbitals[i].traits[j] else {
                continue;
            };
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
                    out.effects = Some(effects.iter().map(|e| declare(e, &ctx, &aliases)).collect());
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
                    out_ticks[k].effects = tick.effects.iter().map(|e| declare(e, &ctx, &aliases)).collect();
                }
            }
        }
    }
}

/// `expr` with each list argument whose declared type needs recording wrapped in `typed`, and each `json/parse` shape
/// naming a program alias recorded as that alias's type (G-ORB-420).
fn declare(expr: &SExpression, ctx: &ExprReturnCtx<'_>, aliases: &Aliases) -> SExpression {
    let mut next = 0usize;
    let tagged = tag(expr, &mut next);
    if next == 0 {
        return record_parse_shapes(expr, aliases);
    }
    let Ok(lowered) = orbital_core_eval::lower::lower_sexpr_keeping_typed(&tagged) else {
        return record_parse_shapes(expr, aliases);
    };
    let slots = typed_slot_types(&lowered, ctx);
    record_parse_shapes(&untag(&tagged, &slots, aliases), aliases)
}

type Aliases = BTreeMap<String, Vec<PayloadField>>;

/// A declared type as the kernel's run-time check reads it: the type, with the program types it names, transitively.
fn recorded_type(name: Option<String>, ty: OirType, aliases: &Aliases) -> String {
    let shape = DeclaredShape::with_named(name, ty, aliases);
    if shape.named.is_empty() && shape.name.is_none() {
        return serde_json::to_string(&shape.ty).unwrap_or_default();
    }
    serde_json::to_string(&shape).unwrap_or_default()
}

/// `expr` with every `json/parse` whose shape names a program alias given that alias's recorded type, and every
/// `typed` declaration given the program types it names, so the kernel checks the value against them.
fn record_parse_shapes(expr: &SExpression, aliases: &Aliases) -> SExpression {
    match expr {
        SExpression::Object(fields) => {
            SExpression::Object(fields.iter().map(|(k, v)| (k.clone(), record_parse_shapes(v, aliases))).collect())
        }
        SExpression::Array(items) => {
            if let Some(SExpression::String(head)) = items.first() {
                if matches!(head.as_str(), "quote" | "quasiquote") {
                    return expr.clone();
                }
            }
            let mut out: Vec<SExpression> = items.iter().map(|item| record_parse_shapes(item, aliases)).collect();
            if let [SExpression::String(head), _, SExpression::String(shape)] = out.as_slice() {
                if head == "json/parse" {
                    if let Some(declared) = DeclaredShape::of_alias(shape, aliases) {
                        out[2] = SExpression::String(recorded_type(declared.name, declared.ty, aliases));
                    }
                } else if head == "typed" {
                    // A knob's `typed` value (inline `declared_list_types`) records its type before the orbital's
                    // aliases are in reach; the types it names are added here.
                    if let Some(declared) = DeclaredShape::parse(shape).filter(|d| d.named.is_empty()) {
                        out[2] = SExpression::String(recorded_type(declared.name, declared.ty, aliases));
                    }
                }
            }
            SExpression::Array(out)
        }
        other => other.clone(),
    }
}

/// Whether the parameter at this position is a list of a type variable (`[T]`).
fn is_list_of_variable(formal: Option<&OperatorTypeRef>) -> bool {
    match formal {
        Some(OperatorTypeRef::Tagged(OperatorTypeRefKind::Array { of })) => {
            matches!(&**of, OperatorTypeRef::Tagged(OperatorTypeRefKind::Var { .. }))
        }
        _ => false,
    }
}

fn tag(expr: &SExpression, next: &mut usize) -> SExpression {
    match expr {
        SExpression::Object(fields) => {
            SExpression::Object(fields.iter().map(|(k, v)| (k.clone(), tag(v, next))).collect())
        }
        SExpression::Array(items) => {
            if let Some(SExpression::String(head)) = items.first() {
                if matches!(head.as_str(), "quote" | "quasiquote" | "typed") {
                    return expr.clone();
                }
            }
            let signature = orbital_core_eval::lower::recognized_call_head(expr)
                .filter(|op| !matches!(*op, "fn" | "lambda" | "let" | "do"))
                .and_then(orbital_core_eval::evaluator::operator_signature);
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let inner = tag(item, next);
                let declared = matches!(item, SExpression::Array(parts) if parts.first().and_then(SExpression::as_str) == Some("typed"));
                let computed = !declared
                    && (orbital_core_eval::lower::recognized_call_head(item).is_some()
                        || matches!(item, SExpression::String(s) if s.starts_with('@')));
                let wrap = i > 0 && computed && signature.is_some_and(|sig| is_list_of_variable(sig.formal(i - 1)));
                if wrap {
                    let slot = format!("{SLOT}{}", *next);
                    *next += 1;
                    out.push(SExpression::Array(vec![
                        SExpression::String("typed".to_string()),
                        inner,
                        SExpression::String(slot),
                    ]));
                } else {
                    out.push(inner);
                }
            }
            SExpression::Array(out)
        }
        other => other.clone(),
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

/// The list type to record for an argument of a list parameter declared `ty`: `ty` when it is a list whose items may
/// differ in kind, or the one list member of a union (`OrbValue = … | [OrbValue] | …`) whose items are a union or an
/// alias, so each item is matched against the union on its own (G-ORB-452).
fn declared_list(ty: &OirType) -> Option<OirType> {
    if needs_declaration(ty) {
        return Some(ty.clone());
    }
    let ty = match ty {
        OirType::Optional { inner } => inner,
        other => other,
    };
    let OirType::Union { variants } = ty else {
        return None;
    };
    let mut lists = variants.iter().filter(|v| matches!(v.field_type, OirType::Array { .. }));
    match (lists.next(), lists.next()) {
        (Some(only), None) => match &only.field_type {
            OirType::Array { inner }
                if matches!(
                    **inner,
                    OirType::Union { .. } | OirType::SExpr | OirType::TypeRef { .. } | OirType::Reference { .. }
                ) =>
            {
                Some(only.field_type.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

fn untag(expr: &SExpression, slots: &HashMap<String, OirType>, aliases: &Aliases) -> SExpression {
    match expr {
        SExpression::Object(fields) => {
            SExpression::Object(fields.iter().map(|(k, v)| (k.clone(), untag(v, slots, aliases))).collect())
        }
        SExpression::Array(items) => {
            if let [SExpression::String(head), value, SExpression::String(slot)] = items.as_slice() {
                if head == "typed" && slot.starts_with(SLOT) {
                    let value = untag(value, slots, aliases);
                    return match slots.get(slot).and_then(declared_list) {
                        Some(ty) => SExpression::Array(vec![
                            SExpression::String("typed".to_string()),
                            value,
                            SExpression::String(recorded_type(None, ty, aliases)),
                        ]),
                        None => value,
                    };
                }
            }
            SExpression::Array(items.iter().map(|item| untag(item, slots, aliases)).collect())
        }
        other => other.clone(),
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
        let TraitReference::Inline(t) = &schema.orbitals[0].traits[0] else { panic!("inline trait") };
        serde_json::to_value(&t.state_machine.transitions[0].effects.as_ref().expect("effects")[0]).unwrap()
    }

    #[test]
    fn a_list_of_a_union_handed_to_a_list_parameter_records_its_declared_type() {
        let mut schema = program(
            union_via(),
            json!(["set", "@entity.count", ["array/len", ["array/filter", "@payload.via", ["fn", "x", true]]]]),
        );
        record_declared_list_types(&mut schema);
        let effect = effect_of(&schema);
        let len_arg = &effect[2][1];
        assert_eq!(len_arg[0], "typed", "array/len's argument: {effect}");
        let filter_arg = &len_arg[1][1];
        assert_eq!(filter_arg[0], "typed", "array/filter's list: {effect}");
        assert_eq!(filter_arg[1], "@payload.via");
        let declared: OirType = serde_json::from_str(filter_arg[2].as_str().expect("type text")).expect("an IR type");
        assert!(needs_declaration(&declared), "{declared:?}");
    }

    #[test]
    fn a_recursive_union_handed_to_a_list_parameter_records_its_list_member() {
        let via = json!({ "name": "via", "type": "union", "properties": [
            { "name": "string", "type": "string" },
            { "name": "number", "type": "number" },
            { "name": "array", "type": "[OrbValue]" },
            { "name": "object", "type": "Map<string,OrbValue>" }
        ] });
        let mut schema = program(via, json!(["set", "@entity.count", ["array/len", "@payload.via"]]));
        record_declared_list_types(&mut schema);
        let effect = effect_of(&schema);
        let arg = &effect[2][1];
        assert_eq!(arg[0], "typed", "{effect}");
        let declared: OirType = serde_json::from_str(arg[2].as_str().expect("type text")).expect("an IR type");
        assert!(matches!(declared, OirType::Array { .. }), "the union's list member: {declared:?}");
    }

    #[test]
    fn control_a_union_without_a_list_member_is_left_as_written() {
        let via = json!({ "name": "via", "type": "union", "properties": [
            { "name": "string", "type": "string" },
            { "name": "number", "type": "number" }
        ] });
        let effect = json!(["set", "@entity.count", ["array/len", "@payload.via"]]);
        let mut schema = program(via, effect.clone());
        record_declared_list_types(&mut schema);
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
        let effect = json!(["set", "@entity.count", ["json/parse", "@payload.via", shape]]);
        let mut schema = with_row_types(program(via, effect));
        record_declared_list_types(&mut schema);
        effect_of(&schema)[2][2].clone()
    }

    #[test]
    fn a_json_parse_naming_a_program_alias_records_its_type_with_every_type_it_names() {
        let recorded: DeclaredShape =
            serde_json::from_str(parse_shape_of("RowFile").as_str().expect("shape text")).expect("a recorded shape");
        assert_eq!(recorded.name.as_deref(), Some("RowFile"));
        assert!(matches!(recorded.ty, OirType::Struct { .. }), "{:?}", recorded.ty);
        assert_eq!(recorded.named.keys().collect::<Vec<_>>(), ["Row", "RowCell"], "transitively, the recursive one once");
    }

    #[test]
    fn a_json_parse_naming_a_non_recursive_alias_records_its_type() {
        let recorded: DeclaredShape =
            serde_json::from_str(parse_shape_of("Point").as_str().expect("shape text")).expect("a recorded shape");
        assert!(matches!(recorded.ty, OirType::Struct { ref fields } if fields.len() == 1));
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
        record_declared_list_types(&mut schema);
        assert_eq!(effect_of(&schema), effect);
    }

    fn with_tx_types(mut schema: OrbitalSchema) -> OrbitalSchema {
        schema.orbitals[0].types = Some(
            serde_json::from_value(json!({
                "Expr": [{ "name": "", "type": "union", "properties": [
                    { "name": "string", "type": "string" },
                    { "name": "number", "type": "number" },
                    { "name": "array", "type": "[Expr]" },
                    { "name": "object", "type": "Map<string,Expr>" }
                ] }],
                "Tx": [
                    { "name": "event", "type": "string" },
                    { "name": "names", "type": "[string]" },
                    { "name": "effects", "type": "[Expr]" }
                ]
            }))
            .expect("aliases"),
        );
        schema
    }

    fn typed_around(expr: &serde_json::Value, value: &serde_json::Value) -> Option<OirType> {
        let items = expr.as_array()?;
        if items.len() == 3 && items[0] == "typed" && &items[1] == value {
            return DeclaredShape::parse(items[2].as_str()?).map(|d| d.ty);
        }
        items.iter().find_map(|item| typed_around(item, value))
    }

    fn tx_effect(effect: serde_json::Value) -> serde_json::Value {
        let txs = json!({ "name": "via", "type": "[Tx]" });
        let mut schema = with_tx_types(program(txs, effect));
        record_declared_list_types(&mut schema);
        effect_of(&schema)
    }

    #[test]
    fn a_field_of_an_alias_element_bound_by_a_lambda_records_its_declared_list() {
        let effect = tx_effect(json!(["set", "@entity.count", ["array/len", ["array/flatten",
            ["array/map", "@payload.via", ["fn", "t", ["array/filter", ["object/get", "@t", "effects", []], ["fn", "e", true]]]]]]]));
        let declared = typed_around(&effect, &json!(["object/get", "@t", "effects", []]))
            .unwrap_or_else(|| panic!("the lambda's element is a Tx: {effect}"));
        assert!(needs_declaration(&declared), "{declared:?}");
    }

    #[test]
    fn a_field_of_an_alias_element_returned_by_an_operator_records_its_declared_list() {
        let effect = tx_effect(json!(["set", "@entity.count",
            ["array/len", ["object/get", ["array/first", "@payload.via"], "effects", []]]]));
        let declared = typed_around(&effect, &json!(["object/get", ["array/first", "@payload.via"], "effects", []]))
            .unwrap_or_else(|| panic!("array/first's result is a Tx: {effect}"));
        assert!(needs_declaration(&declared), "{declared:?}");
    }

    #[test]
    fn control_a_field_of_one_kind_on_an_alias_element_is_left_as_written() {
        let effect = json!(["set", "@entity.count", ["array/len", ["array/flatten",
            ["array/map", "@payload.via", ["fn", "t", ["object/get", "@t", "names", []]]]]]]);
        assert_eq!(tx_effect(effect.clone()), effect);
    }

    #[test]
    fn control_a_list_of_entity_rows_read_through_a_lambda_is_left_as_written() {
        let rows = json!({ "name": "via", "type": "[Probe]" });
        let effect = json!(["set", "@entity.count", ["array/len",
            ["array/filter", "@payload.via", ["fn", "r", ["=", ["object/get", "@r", "count"], 1.0]]]]]);
        let mut schema = program(rows, effect.clone());
        record_declared_list_types(&mut schema);
        assert_eq!(effect_of(&schema), effect);
    }
}
