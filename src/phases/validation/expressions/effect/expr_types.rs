//! The type the check infers for every node of every guard and effect, keyed by the node's path in the program, for
//! readers that draw an expression (the inspector's circuit view labels each wire with it). The types are the
//! validator's own: the same per-transition context and inference, read rather than checked. Paths follow the IR
//! call finder (`std/spec/cli/ir-calls`): `.guard` / `.effects[m]`, then `.do[i-1]` under do, `.then`/`.else` under
//! if, `.then[i-2]` under when, `.body[i-2]` under let, `[i]` under any other list and `.key` under a map.

use super::set::ExprReturnCtx;
use super::{GuaranteedPayloads, TraitScopes};
use crate::type_infer::trace_types_under;
use orbital_core_base::ir::{OirEffect, OirExpr, OirType};
use orbital_core_base::schema::{EntityDefinition, OrbitalSchema, SExpression, TraitReference};
use orbital_core_eval::lower::{lower_sexpr, recognized_call_head};
use std::collections::{BTreeMap, HashMap};

type Trace = HashMap<*const OirExpr, OirType>;

/// Every typed node of the program's inline traits: path → the type in `.lolo` spelling.
pub fn expression_types(schema: &OrbitalSchema) -> BTreeMap<String, String> {
    let entity_definitions: HashMap<String, EntityDefinition> = schema
        .orbitals
        .iter()
        .flat_map(|o| std::iter::once(&o.entity).chain(o.auxiliary_entities.iter().flatten()))
        .filter_map(|e| e.as_inline().map(|def| (def.name.clone(), def.clone())))
        .collect();
    let mut out = BTreeMap::new();
    let guaranteed = GuaranteedPayloads::new();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let base = format!("orbitals[{oi}].traits[{ti}]");
            let scopes = TraitScopes::new(schema, trait_def, &entity_definitions, &guaranteed);
            for (k, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                let payload = scopes.payload_for(&transition.event);
                let types = scopes.types_for(schema, trait_def, &transition.event);
                let ctx = ExprReturnCtx {
                    entity: &scopes.entity_field_types,
                    payload: &payload,
                    types: Some(&types),
                    schema: Some(schema),
                };
                let at = format!("{base}.stateMachine.transitions[{k}]");
                unit(
                    transition.guard.as_ref(),
                    transition.effects.iter().flatten(),
                    &at,
                    &ctx,
                    &mut out,
                );
            }
            for (k, tick) in trait_def.ticks.iter().flatten().enumerate() {
                let ctx = ExprReturnCtx {
                    entity: &scopes.entity_field_types,
                    payload: &scopes.payload_field_types,
                    types: Some(&scopes.tick_types),
                    schema: Some(schema),
                };
                unit(
                    tick.guard.as_ref(),
                    tick.effects.iter(),
                    &format!("{base}.ticks[{k}]"),
                    &ctx,
                    &mut out,
                );
            }
        }
    }
    out
}

fn unit<'a>(
    guard: Option<&SExpression>,
    effects: impl Iterator<Item = &'a SExpression>,
    at: &str,
    ctx: &ExprReturnCtx<'_>,
    out: &mut BTreeMap<String, String>,
) {
    let condition = guard.and_then(|guard| lower_sexpr(guard).ok());
    if let (Some(guard), Some(lowered)) = (guard, condition.as_ref()) {
        let trace = trace_types_under(lowered, None, ctx);
        walk(guard, lowered, &format!("{at}.guard"), &trace, out);
    }
    for (m, effect) in effects.enumerate() {
        let Ok(lowered) = lower_sexpr(effect) else {
            continue;
        };
        let trace = trace_types_under(&lowered, condition.as_ref(), ctx);
        walk(effect, &lowered, &format!("{at}.effects[{m}]"), &trace, out);
    }
}

fn piece(head: &str, i: usize) -> String {
    match (head, i) {
        ("do", _) => format!(".do[{}]", i - 1),
        ("if", 2) => ".then".to_string(),
        ("if", 3) => ".else".to_string(),
        ("when", 2..) => format!(".then[{}]", i - 2),
        ("let", 2..) => format!(".body[{}]", i - 2),
        _ => format!("[{i}]"),
    }
}

/// Reads `source` and the expression it lowered to side by side, naming each lowered node by its source path. A
/// shape the lowering rewrote beyond these forms is left unnamed below that point.
fn walk(
    source: &SExpression,
    lowered: &OirExpr,
    path: &str,
    trace: &Trace,
    out: &mut BTreeMap<String, String>,
) {
    if let Some(ty) = trace.get(&(lowered as *const OirExpr)) {
        out.insert(
            path.to_string(),
            orbital_core_base::signature::describe_type(ty),
        );
    }
    match source {
        SExpression::Array(items) => match recognized_call_head(source) {
            Some(head) => call(head, items, lowered, path, trace, out),
            None => {
                if let OirExpr::Array {
                    items: lowered_items,
                } = lowered
                {
                    for (i, (s, l)) in items.iter().zip(lowered_items).enumerate() {
                        walk(s, l, &format!("{path}[{i}]"), trace, out);
                    }
                }
            }
        },
        SExpression::Object(fields) => {
            if let OirExpr::Object {
                fields: lowered_fields,
            } = lowered
            {
                for (key, s) in fields.iter() {
                    let key = key.to_string();
                    if let Some(l) = lowered_fields.get(&key) {
                        walk(s, l, &format!("{path}.{key}"), trace, out);
                    }
                }
            }
        }
        _ => {}
    }
}

fn call(
    head: &str,
    items: &[SExpression],
    lowered: &OirExpr,
    path: &str,
    trace: &Trace,
    out: &mut BTreeMap<String, String>,
) {
    let args = &items[1..];
    let arg = |i: usize, l: &OirExpr, out: &mut BTreeMap<String, String>| {
        walk(
            &items[i],
            l,
            &format!("{path}{}", piece(head, i)),
            trace,
            out,
        )
    };
    match lowered {
        // A variadic operator folds left: `(and a b c)` is `And(And(a, b), c)`.
        OirExpr::BinaryOp { .. } => {
            let mut operands = Vec::with_capacity(args.len());
            let mut node = lowered;
            for _ in 1..args.len() {
                let OirExpr::BinaryOp { left, right, .. } = node else {
                    return;
                };
                operands.push(right.as_ref());
                node = left;
            }
            operands.push(node);
            for (i, l) in operands.into_iter().rev().enumerate() {
                arg(i + 1, l, out);
            }
        }
        OirExpr::UnaryOp { operand, .. } if !args.is_empty() => arg(1, operand, out),
        OirExpr::Conditional {
            condition,
            then_branch,
            else_branch,
        } if args.len() >= 3 => {
            arg(1, condition, out);
            arg(2, then_branch, out);
            arg(3, else_branch, out);
        }
        OirExpr::Lambda { body, .. } if head == "fn" => arg(items.len() - 1, body, out),
        OirExpr::Call {
            function,
            args: lowered_args,
        } if function == "let" => {
            if let (Some(SExpression::Array(pairs)), Some(OirExpr::Array { items: bindings })) =
                (items.get(1), lowered_args.first())
            {
                for (k, (pair, binding)) in pairs.iter().zip(bindings).enumerate() {
                    if let (SExpression::Array(kv), OirExpr::Array { items: lowered_kv }) =
                        (pair, binding)
                    {
                        if let (Some(value), Some(lowered_value)) = (kv.get(1), lowered_kv.get(1)) {
                            walk(
                                value,
                                lowered_value,
                                &format!("{path}[1][{k}][1]"),
                                trace,
                                out,
                            );
                        }
                    }
                }
            }
            for (i, l) in lowered_args.iter().enumerate().skip(1) {
                arg(i + 1, l, out);
            }
        }
        OirExpr::Call {
            args: lowered_args, ..
        } if lowered_args.len() == args.len() => {
            for (i, l) in lowered_args.iter().enumerate() {
                match l {
                    // A raw callback body lowered to the lambda the operator binds its names in.
                    OirExpr::Lambda { body, .. }
                        if recognized_call_head(&args[i]) != Some("fn") =>
                    {
                        arg(i + 1, body, out)
                    }
                    _ => arg(i + 1, l, out),
                }
            }
        }
        OirExpr::Effect { effect } => match effect.as_ref() {
            OirEffect::Set { value, .. } if args.len() >= 2 => arg(2, value, out),
            OirEffect::Emit {
                payload: Some(payload),
                ..
            } if args.len() >= 2 => arg(2, payload, out),
            OirEffect::Fetch { id, filter, order_by, offset, limit, nearest, .. } => {
                if let Some(SExpression::Object(options)) = items.get(2) {
                    let lowered_options = [("id", id), ("filter", filter), ("orderBy", order_by), ("offset", offset), ("limit", limit), ("nearest", nearest)];
                    for (key, source) in options.iter() {
                        let key = key.to_string();
                        if let Some((_, Some(l))) = lowered_options.iter().find(|(name, _)| *name == key) {
                            walk(source, l, &format!("{path}[2].{key}"), trace, out);
                        }
                    }
                }
            }
            OirEffect::Persist { data, .. } if args.len() >= 3 => arg(3, data, out),
            OirEffect::Custom {
                args: lowered_args, ..
            } => {
                for i in 0..args.len() {
                    if let Some(l) = lowered_args.get(&format!("arg{i}")) {
                        arg(i + 1, l, out);
                    }
                }
            }
            _ => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(guard: serde_json::Value, effects: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "app",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Row", "persistence": "runtime", "fields": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "ok", "type": "boolean", "required": true },
                    { "name": "count", "type": "number", "required": true },
                    { "name": "tags", "type": "array", "items": { "name": "tag", "type": "string" }, "required": true }
                ] },
                "traits": [{
                    "name": "T",
                    "linkedEntity": "Row",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "GO", "name": "Go" }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "guard": guard, "effects": effects }]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("program")
    }

    const AT: &str = "orbitals[0].traits[0].stateMachine.transitions[0]";

    #[test]
    fn expression_types_label_each_node_by_its_source_path() {
        let schema = program(
            serde_json::json!([
                "and",
                ["=", "@entity.count", 1],
                ["not", "@entity.ok"],
                "@entity.ok"
            ]),
            serde_json::json!([[
                "set",
                "@entity.count",
                [
                    "array/len",
                    [
                        "array/filter",
                        "@entity.tags",
                        ["fn", "t", ["!=", "@t", ""]]
                    ]
                ]
            ]]),
        );
        let types = expression_types(&schema);
        let at = |rest: &str| types.get(&format!("{AT}{rest}")).map(String::as_str);
        assert_eq!(at(".guard"), Some("boolean"));
        assert_eq!(at(".guard[1]"), Some("boolean"));
        assert_eq!(at(".guard[1][1]"), Some("number"));
        assert_eq!(at(".guard[2][1]"), Some("boolean"));
        assert_eq!(at(".guard[3]"), Some("boolean"));
        assert_eq!(at(".effects[0][2]"), Some("int"));
        assert_eq!(at(".effects[0][2][1]"), Some("[string]"));
        assert_eq!(at(".effects[0][2][1][1]"), Some("[string]"));
        assert_eq!(at(".effects[0][2][1][2][2]"), Some("boolean"));
        assert_eq!(at(".effects[0][2][1][2][2][1]"), Some("string"));
    }

    #[test]
    fn expression_types_follow_let_and_if_paths() {
        let schema = program(
            serde_json::Value::Null,
            serde_json::json!([[
                "set",
                "@entity.count",
                [
                    "let",
                    [["n", ["array/len", "@entity.tags"]]],
                    ["if", "@entity.ok", "@n", 0]
                ]
            ]]),
        );
        let types = expression_types(&schema);
        let at = |rest: &str| types.get(&format!("{AT}{rest}")).map(String::as_str);
        assert_eq!(at(".effects[0][2][1][0][1]"), Some("int"));
        assert_eq!(at(".effects[0][2].body[0]"), Some("int"));
        assert_eq!(at(".effects[0][2].body[0][1]"), Some("boolean"));
        assert_eq!(at(".effects[0][2].body[0].then"), Some("int"));
        assert!(types.keys().all(|path| !path.contains(".guard")));
    }

    #[test]
    fn expression_types_name_the_nodes_of_a_fetch_filter_and_a_persist_payload() {
        let schema = program(
            serde_json::Value::Null,
            serde_json::json!([
                ["fetch", "Row", { "filter": ["and", "@entity.ok", [">", "@entity.count", 1]], "offset": ["*", 2, 5] }],
                ["persist", "create", "Row", { "count": ["+", 1, 2] }]
            ]),
        );
        let types = expression_types(&schema);
        let at = |rest: &str| types.get(&format!("{AT}{rest}")).map(String::as_str);
        assert_eq!(at(".effects[0][2].filter"), Some("boolean"));
        assert_eq!(at(".effects[0][2].filter[2]"), Some("boolean"));
        assert_eq!(at(".effects[0][2].filter[2][1]"), Some("number"));
        assert_eq!(at(".effects[0][2].offset"), Some("int"));
        assert_eq!(at(".effects[1][3].count"), Some("int"));
        assert_eq!(at(".effects[0][1]"), None, "control: the entity name is not an expression");
    }

    #[test]
    fn a_null_operand_takes_the_type_its_position_requires() {
        let schema = program(
            serde_json::json!(["and", ["=", "@entity.count", null], ["=", null, null]]),
            serde_json::json!([["set", "@entity.ok", ["str/includes", "abc", null]]]),
        );
        let types = expression_types(&schema);
        let at = |rest: &str| types.get(&format!("{AT}{rest}")).map(String::as_str);
        assert_eq!(at(".guard[1][2]"), Some("number?"));
        assert_eq!(at(".effects[0][2][2]"), Some("string?"));
        assert_eq!(at(".guard[2][1]"), None, "control: nothing determines the type of two nulls");
        assert_eq!(at(".guard[2][2]"), None);
        assert_eq!(at(".guard[1][1]"), Some("number"), "control: a typed operand keeps its own type");
    }
}
