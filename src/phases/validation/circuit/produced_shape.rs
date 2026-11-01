//! The operator side of the payload invariant.
//!
//! **The invariant:** an event's declared payload type IS its access path. An
//! event declaring `Survey` is read as `?name`; one declaring `{ row : Survey }`
//! is read as `?row.name`. Any wrapping that is not in the type is a lie, and
//! any field in the type that is not in the payload reads `undefined`.
//!
//! Enforcing that needs both sides in one vocabulary. The declared side is
//! already `Vec<PayloadField>`. This module supplies the produced side by
//! lowering an operator's registry `effect.produces` — an `OperatorTypeRef`,
//! a type language that until now had **no** conversion into the compiler's own
//! types anywhere in the tree — into the same `Vec<PayloadField>` shape.
//!
//! Deliberately conservative: anything this module cannot decide returns `None`,
//! which means *no check*. It never guesses a shape.

use std::collections::HashMap;

use orbital_core_base::emit_contract::{
    failure_payload, message_payload, success_payload, CallVariant, ProducedPayload, ShapeField,
};
use orbital_core_base::ir::OirType;
use orbital_core_base::schema::{EntityDefinition, PayloadField, SExpression};

use crate::embedded::{get_operator, OperatorTypeRef, OperatorTypeRefKind, ServiceRegistryData};

use super::effect::entity_field_type_map;

/// Extract the entity name a call site names, using each operator's own
/// argument layout. `persist` mirrors `effect::persist::validate_persist`'s
/// entity-index detection (4-element `["persist", action, Entity, data]` vs the
/// legacy 3-element `["persist", Entity, data]`) rather than assuming a slot —
/// the legacy form has no action keyword, so a fixed index would read the wrong
/// argument.
fn call_site_entity(op_name: &str, args: &[SExpression]) -> Option<String> {
    let idx = match op_name {
        "persist" => match args.get(1) {
            Some(SExpression::String(op)) if op == "batch" => return None,
            Some(SExpression::String(op))
                if op == "create" || op == "upsert" || op == "update" || op == "delete" =>
            {
                2
            }
            _ => 1,
        },
        "fetch" | "fetch-stream" => 1,
        _ => return None,
    };
    match args.get(idx) {
        Some(SExpression::String(name)) => Some(name.clone().into_inner()),
        _ => None,
    }
}

/// The persist action at a call site, or `None` for the legacy 3-element form
/// (which carries no action keyword and must not be guessed at).
fn persist_action(args: &[SExpression]) -> Option<&str> {
    match args.get(1) {
        Some(SExpression::String(op))
            if op == "create"
                || op == "upsert"
                || op == "update"
                || op == "delete"
                || op == "batch" =>
        {
            Some(op.as_str())
        }
        _ => None,
    }
}

/// Narrow a union `produces` to the branch this call site actually takes.
/// Non-unions pass through. An undecidable call site yields `None` — no check.
fn narrow<'a>(
    op_name: &str,
    args: &[SExpression],
    produces: &'a OperatorTypeRef,
) -> Option<&'a OperatorTypeRef> {
    let OperatorTypeRef::Tagged(OperatorTypeRefKind::Union { of }) = produces else {
        return Some(produces);
    };
    // persist's union is ordered [create/update row, delete result, batch summary].
    if op_name == "persist" {
        let idx = match persist_action(args)? {
            "create" | "upsert" | "update" => 0,
            "delete" => 1,
            "batch" => 2,
            _ => return None,
        };
        return of.get(idx);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn union_of_three() -> OperatorTypeRef {
        let table: OperatorTypeRef = serde_json::from_str(
            r#"{"kind":"union","of":[{"kind":"object","fields":{"row":"string"},"open":false},{"kind":"object","fields":{"deleted":"boolean"},"open":false},{"kind":"object","fields":{"count":"number"},"open":false}]}"#,
        )
        .expect("a union of three shapes");
        table
    }

    fn call(action: &str) -> Vec<SExpression> {
        let json: serde_json::Value = serde_json::from_str(&format!(r#"["persist","{action}","Row",{{}}]"#)).expect("a call");
        match serde_json::from_value::<SExpression>(json).expect("an s-expression") {
            SExpression::Array(items) => items.into_iter().collect(),
            other => panic!("a call is an array, got {other:?}"),
        }
    }

    fn names(narrowed: Option<&OperatorTypeRef>) -> Vec<String> {
        match narrowed {
            Some(OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. })) => {
                fields.keys().cloned().collect()
            }
            other => panic!("expected one object branch, got {other:?}"),
        }
    }

    #[test]
    fn a_persist_action_narrows_to_the_branch_it_selects() {
        let union = union_of_three();
        assert_eq!(names(narrow("persist", &call("create"), &union)), ["row"]);
        assert_eq!(names(narrow("persist", &call("update"), &union)), ["row"]);
        assert_eq!(names(narrow("persist", &call("delete"), &union)), ["deleted"]);
        assert_eq!(names(narrow("persist", &call("batch"), &union)), ["count"]);
    }

    #[test]
    fn control_an_unknown_action_or_another_operator_is_not_narrowed() {
        let union = union_of_three();
        assert!(narrow("persist", &call("frobnicate"), &union).is_none());
        assert!(narrow("fetch", &call("create"), &union).is_none());
    }

    fn stepping() -> orbital_core_base::language::registry::EmbeddedOperator {
        serde_json::from_value(serde_json::json!({
            "category": "std-program", "minArity": 2, "maxArity": 3,
            "returnType": { "kind": "array", "of": { "kind": "object", "fields": { "trait": "string" }, "open": true } },
            "params": [
                { "name": "program", "type": "sexpr" },
                { "name": "requests", "type": "array" },
                { "name": "options", "type": { "kind": "object", "fields": { "report": { "kind": "literal", "value": "raw" } }, "open": true } }
            ],
            "resultByOption": { "option": "report", "results": {
                "raw": { "kind": "object", "fields": { "success": "boolean", "steps": { "kind": "array", "of": "string" } }, "open": false }
            } }
        }))
        .expect("an operator with an option-selected result")
    }

    fn step_call(options: Option<&str>) -> Vec<SExpression> {
        let tail = options.map(|o| format!(",{o}")).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&format!(r#"["program/step","@p",[]{tail}]"#)).expect("a call");
        match serde_json::from_value::<SExpression>(json).expect("an s-expression") {
            SExpression::Array(items) => items.into_iter().collect(),
            other => panic!("a call is an array, got {other:?}"),
        }
    }

    fn selected_is_raw(op: &orbital_core_base::language::registry::EmbeddedOperator, call: &[SExpression]) -> Option<bool> {
        match op.result_for_call(call)? {
            OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. }) => Some(fields.contains_key("success")),
            OperatorTypeRef::Tagged(OperatorTypeRefKind::Array { .. }) => Some(false),
            other => panic!("unexpected result {other:?}"),
        }
    }

    #[test]
    fn a_literal_option_value_selects_the_result_the_table_declares_for_it() {
        let op = stepping();
        assert_eq!(selected_is_raw(&op, &step_call(Some(r#"{"report":"raw"}"#))), Some(true));
        assert_eq!(selected_is_raw(&op, &step_call(Some(r#"{"report":"raw","emit":{"success":"OK"}}"#))), Some(true));
    }

    #[test]
    fn an_absent_option_answers_the_declared_return_type() {
        let op = stepping();
        for call in [step_call(None), step_call(Some("{}")), step_call(Some(r#"{"report":null}"#)), step_call(Some(r#"{"render":true}"#))] {
            assert_eq!(selected_is_raw(&op, &call), Some(false), "{call:?}");
        }
    }

    #[test]
    fn a_computed_or_undeclared_option_value_claims_no_result() {
        let op = stepping();
        assert_eq!(selected_is_raw(&op, &step_call(Some(r#"{"report":"@payload.mode"}"#))), None, "a binding");
        assert_eq!(selected_is_raw(&op, &step_call(Some(r#"{"report":true}"#))), None, "not a word");
        assert_eq!(selected_is_raw(&op, &step_call(Some(r#"{"report":"loose"}"#))), None, "a value the table declares no result for");
    }

    #[test]
    fn the_selected_result_is_the_shape_the_event_is_judged_against() {
        let op = stepping();
        let members = |call: &[SExpression]| -> Vec<String> {
            let payload = program_result_payload("program/step", call, op.result_for_call(call).unwrap(), &|_| None);
            let result = payload.iter().find(|f| f.name == "result").expect("result");
            result.properties.iter().flatten().map(|f| f.name.clone()).collect()
        };
        assert_eq!(members(&step_call(Some(r#"{"report":"raw"}"#))), ["steps", "success"]);
        assert_eq!(members(&step_call(None)), ["trait"], "control: the played steps");
    }

    #[test]
    fn an_operator_declaring_no_selector_answers_its_return_type_whatever_the_options_say() {
        let mut op = stepping();
        op.result_by_option = None;
        assert_eq!(selected_is_raw(&op, &step_call(Some(r#"{"report":"raw"}"#))), Some(false));
    }

    #[test]
    fn a_union_of_another_operator_is_still_undecidable() {
        assert!(narrow("fetch", &call("create"), &union_of_three()).is_none());
    }
}

/// Lower one `OperatorTypeRef` into the `(field_type, properties)` pair a
/// `PayloadField` carries. `entity` is the call site's resolved entity, used
/// wherever the registry says `{kind:entity}`.
fn field_shape(
    type_ref: &OperatorTypeRef,
    entity: Option<&EntityDefinition>,
) -> (String, Option<Vec<PayloadField>>, bool) {
    match type_ref {
        OperatorTypeRef::Primitive(p) => (p.clone(), None, false),
        OperatorTypeRef::Tagged(kind) => match kind {
            OperatorTypeRefKind::Entity { .. } => match entity {
                Some(e) => ("object".to_string(), Some(entity_payload_fields(e)), false),
                // The operator produces a row but we could not resolve which
                // entity — permissive rather than wrong.
                None => ("any".to_string(), None, false),
            },
            OperatorTypeRefKind::Array { of } => {
                let (inner, props, open) = field_shape(of, entity);
                (format!("[{inner}]"), props, open)
            }
            OperatorTypeRefKind::Object { fields, open, .. } => (
                "object".to_string(),
                Some(object_payload_fields(fields, entity)),
                open.unwrap_or(false),
            ),
            // A union / binding / sexpr / slot in a field position is not
            // something this check can adjudicate.
            _ => ("any".to_string(), None, false),
        },
    }
}

fn object_payload_fields(
    fields: &HashMap<String, OperatorTypeRef>,
    entity: Option<&EntityDefinition>,
) -> Vec<PayloadField> {
    let mut out: Vec<PayloadField> = fields
        .iter()
        .map(|(name, ty)| {
            let (field_type, properties, open) = field_shape(ty, entity);
            PayloadField {
                nullable: None,
                additional_properties: open.then(|| Box::new(scalar_field("", "any"))),
                values: None,
                name: name.clone(),
                field_type,
                required: None,
                description: None,
                properties,
                type_when: None,
                entity: None,
                projected_from: None,
                items: None,
                relation: None,
            }
        })
        .collect();
    // `Object.fields` is a HashMap, so parse order is lost; sort for stable
    // diagnostics.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// An entity's own fields, flat — the same conversion `orbital-lolo`'s
/// `resolve_payload_type` performs for a bare `type X = Event EntityName`
/// alias, so both sides of the comparison speak one alphabet. The produced
/// side of `produced_type_accepts` speaks the COARSE `field_type_str` tag
/// dialect (`"array"`, `"relation"`) — `EntityFieldMeta::field_type` already
/// is that flat tag.
fn entity_payload_fields(entity: &EntityDefinition) -> Vec<PayloadField> {
    let mut out: Vec<PayloadField> = entity_field_type_map(entity)
        .into_iter()
        .map(|(name, meta)| PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name,
            field_type: meta.field_type,
            required: None,
            description: None,
            properties: None,
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
            relation: None,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub(super) fn scalar_field(name: &str, field_type: &str) -> PayloadField {
    PayloadField {
        nullable: None,
        additional_properties: None,
        values: None,
        name: name.to_string(),
        field_type: field_type.to_string(),
        required: None,
        description: None,
        properties: None,
        type_when: None,
        entity: None,
        projected_from: None,
        items: None,
        relation: None,
    }
}

/// A KEYED `call-service` success payload: the registry result record with
/// `key: string` merged beside its fields (D-A82). Unkeyed calls and results
/// that are not a known record carry no shape to check.
fn keyed_call_service_payload(
    args: &[SExpression],
    registry: &ServiceRegistryData,
) -> Option<Vec<PayloadField>> {
    let (SExpression::String(service), SExpression::String(action)) = (args.get(1)?, args.get(2)?)
    else {
        return None;
    };
    if !call_has_key("call-service", args) {
        return None;
    }
    let mut fields: Vec<PayloadField> = registry
        .response_shape(service, action)?
        .iter()
        .map(|f| scalar_field(&f.name, &f.field_type))
        .collect();
    fields.push(scalar_field("key", "string"));
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    Some(fields)
}

/// A core-owned typed shape as the `PayloadField`s a declared payload is compared against.
fn shape_fields(shape: &[ShapeField]) -> Vec<PayloadField> {
    let mut out: Vec<PayloadField> = shape
        .iter()
        .map(|f| {
            let mut field = scalar_field(f.name, f.field_type);
            if !f.fields.is_empty() {
                field.properties = Some(shape_fields(f.fields));
            }
            if f.open {
                field.additional_properties = Some(Box::new(scalar_field("", "any")));
            }
            field
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn produced_to_fields(produced: ProducedPayload) -> Option<Vec<PayloadField>> {
    match produced {
        ProducedPayload::Shape(shape) => Some(shape_fields(shape)),
        ProducedPayload::Fields(names) => {
            Some(names.iter().map(|n| scalar_field(n, "any")).collect())
        }
        ProducedPayload::EntityRow | ProducedPayload::Opaque => None,
    }
}

/// The `browser/page-facts` payload: `available` plus the facts the call requested; a fact left out is not
/// readable. An undecidable field list yields no shape (no check).
fn page_facts_payload(args: &[SExpression]) -> Option<Vec<PayloadField>> {
    let SExpression::Array(words) = args.get(1)? else {
        return None;
    };
    let keyed = call_has_key("browser/page-facts", args);
    let requested: Vec<&str> = words
        .iter()
        .map(SExpression::as_str)
        .collect::<Option<_>>()?;
    let ProducedPayload::Shape(shape) = success_payload("browser/page-facts", CallVariant::Plain)
    else {
        return None;
    };
    let kept: Vec<ShapeField> = shape
        .iter()
        .filter(|f| f.name == "available" || requested.contains(&f.name))
        .copied()
        .collect();
    let mut fields = shape_fields(&kept);
    if keyed {
        fields.push(scalar_field("key", "string"));
        fields.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Some(fields)
}

/// The `random-bytes` payload: `value`, plus the call's `key` when it is keyed.
fn random_bytes_payload(args: &[SExpression]) -> Option<Vec<PayloadField>> {
    let ProducedPayload::Shape(shape) = success_payload("random-bytes", CallVariant::Plain) else {
        return None;
    };
    let mut fields = shape_fields(shape);
    if call_has_key("random-bytes", args) {
        fields.push(scalar_field("key", "string"));
        fields.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Some(fields)
}

/// The payload an operator call site actually puts on the bus, expressed as
/// `PayloadField`s so it can be compared directly against an event's declared
/// payload.
///
/// Returns `None` — meaning **do not check** — when the operator has no effect
/// metadata, declares no `produces`, produces an undecidable union at this call
/// site, or produces an opaque `any` (`call-service`, `set`).
pub(crate) fn produced_payload(
    op_name: &str,
    args: &[SExpression],
    entity_definitions: &HashMap<String, EntityDefinition>,
    registry: &ServiceRegistryData,
    named_type: &dyn Fn(&str) -> Option<OirType>,
) -> Option<Vec<PayloadField>> {
    if orbital_core_base::os::OsOp::from_name(op_name).is_some() {
        let keyed = call_has_key(op_name, args);
        let variant = if op_name == "os/stdin-read" && keyed && stdin_has_message_route(args) {
            CallVariant::StdinStream
        } else {
            CallVariant::Plain
        };
        return with_os_key(
            op_name,
            args,
            produced_to_fields(success_payload(op_name, variant)),
        );
    }
    if orbital_core_base::tab::TabOp::is_tab_effect(op_name) {
        return produced_to_fields(success_payload(op_name, CallVariant::Plain));
    }
    if orbital_core_base::credential_admin::CredentialAdminOp::from_name(op_name).is_some() {
        return produced_to_fields(success_payload(op_name, CallVariant::Plain));
    }
    if matches!(
        op_name,
        "credential-status"
            | "credential-forget"
            | "credential-connect"
            | "access-check"
            | "browser/prompt"
            | "browser/embed"
    ) {
        return produced_to_fields(success_payload(op_name, CallVariant::Plain));
    }
    if op_name == "browser/page-facts" {
        return page_facts_payload(args);
    }
    if op_name == "random-bytes" {
        return random_bytes_payload(args);
    }
    if op_name == "call-service" {
        return keyed_call_service_payload(args, registry);
    }
    let operator = get_operator(op_name)?;
    if operator.effect.as_ref().and_then(|e| e.produces.as_ref()).is_none()
        && orbital_core_base::ir::is_substrate_operator(op_name)
    {
        return Some(program_result_payload(op_name, args, operator.result_for_call(args)?, named_type));
    }
    let produces = operator.effect.as_ref()?.produces.as_ref()?;
    let narrowed = narrow(op_name, args, produces)?;

    let entity = call_site_entity(op_name, args).and_then(|n| entity_definitions.get(&n));

    match narrowed {
        // A bare entity row IS the whole payload — its fields sit at the root.
        // This is the persist create/update contract, and the reason a
        // `{ row : Entity }` declaration is a lie: `row` is not a field of it.
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Entity { .. }) => {
            entity.map(entity_payload_fields)
        }
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. }) => {
            let mut payload = object_payload_fields(fields, entity);
            if op_name == "fetch" && call_has_key(op_name, args) {
                payload.push(scalar_field("key", "string"));
                payload.sort_by(|a, b| a.name.cmp(&b.name));
            }
            Some(payload)
        }
        // `any` (call-service, set) and anything else carries no shape to check.
        _ => None,
    }
}

/// A program-host or substrate op's success payload is `{ result }` (plus `text`, the result as JSON text, for an
/// agent-port op, and the call's `key` when it has one), as the kernel's `completion_result` builds it; `result` is the
/// operator's declared return type. A return type naming a type the table does not spell out is judged by the shape the
/// program or the language declares for that name (`named_type`); a name neither declares is undecidable: `any`.
fn program_result_payload(
    op_name: &str,
    args: &[SExpression],
    return_type: &OperatorTypeRef,
    named_type: &dyn Fn(&str) -> Option<OirType>,
) -> Vec<PayloadField> {
    let (field_type, properties, open) = match with_declared_names(return_type, named_type) {
        Some(spelled) if !names_unspelled_type(&spelled) => field_shape(&spelled, None),
        _ => ("any".to_string(), None, false),
    };
    let mut fields = vec![PayloadField {
        nullable: None,
        additional_properties: open.then(|| Box::new(scalar_field("", "any"))),
        values: None,
        name: "result".to_string(),
        field_type,
        required: None,
        description: None,
        properties,
        type_when: None,
        entity: None,
        projected_from: None,
        items: None,
        relation: None,
    }];
    if orbital_core_base::agent_ports::is_agent_port_operator(op_name) {
        fields.push(scalar_field("text", "string"));
    }
    if call_has_key(op_name, args) {
        fields.push(scalar_field("key", "string"));
    }
    fields
}

/// `type_ref` with each bare name the table does not spell out replaced by the shape `named_type` declares for it;
/// `None` when a name has no declared shape. A part of a declared shape this check cannot read is `any`.
fn with_declared_names(
    type_ref: &OperatorTypeRef,
    named_type: &dyn Fn(&str) -> Option<OirType>,
) -> Option<OperatorTypeRef> {
    match type_ref {
        OperatorTypeRef::Primitive(name) if matches!(name.as_str(), "string" | "number" | "boolean" | "any") => {
            Some(type_ref.clone())
        }
        OperatorTypeRef::Primitive(name) => declared_shape(&named_type(name)?),
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Array { of }) => Some(OperatorTypeRef::Tagged(
            OperatorTypeRefKind::Array { of: Box::new(with_declared_names(of, named_type)?) },
        )),
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, open, optional }) => {
            let fields = fields
                .iter()
                .map(|(name, ty)| Some((name.clone(), with_declared_names(ty, named_type)?)))
                .collect::<Option<HashMap<_, _>>>()?;
            Some(OperatorTypeRef::Tagged(OperatorTypeRefKind::Object {
                fields,
                open: *open,
                optional: optional.clone(),
            }))
        }
        OperatorTypeRef::Tagged(_) => Some(type_ref.clone()),
    }
}

/// A declared record, list or scalar as the table's own type language; a part that is none of them is `any`, which the
/// check never judges.
fn declared_shape(ty: &OirType) -> Option<OperatorTypeRef> {
    let any = || OperatorTypeRef::Primitive("any".to_string());
    Some(match ty {
        OirType::String => OperatorTypeRef::Primitive("string".to_string()),
        OirType::Number | OirType::Integer => OperatorTypeRef::Primitive("number".to_string()),
        OirType::Boolean => OperatorTypeRef::Primitive("boolean".to_string()),
        OirType::Optional { inner } => declared_shape(inner).unwrap_or_else(any),
        OirType::Array { inner } => OperatorTypeRef::Tagged(OperatorTypeRefKind::Array {
            of: Box::new(declared_shape(inner).unwrap_or_else(any)),
        }),
        OirType::Struct { fields, additional } => OperatorTypeRef::Tagged(OperatorTypeRefKind::Object {
            fields: fields
                .iter()
                .map(|f| (f.name.clone(), declared_shape(&f.field_type).unwrap_or_else(any)))
                .collect(),
            open: Some(additional.is_some()),
            optional: Vec::new(),
        }),
        _ => return None,
    })
}

fn names_unspelled_type(type_ref: &OperatorTypeRef) -> bool {
    match type_ref {
        OperatorTypeRef::Primitive(name) => !matches!(name.as_str(), "string" | "number" | "boolean" | "any"),
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Array { of }) => names_unspelled_type(of),
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. }) => fields.values().any(names_unspelled_type),
        OperatorTypeRef::Tagged(_) => false,
    }
}

fn call_has_key(op_name: &str, args: &[SExpression]) -> bool {
    if let Some(op) = orbital_core_base::os::OsOp::from_name(op_name) {
        let (_, envelope) =
            orbital_core_base::os::split_envelope(op, args.get(1..).unwrap_or_default());
        return envelope
            .is_some_and(|options| !matches!(options.get("key"), None | Some(SExpression::Null)));
    }
    match op_name {
        "browser/page-facts" | "random-bytes" | "fetch" => matches!(
            args.get(2),
            Some(SExpression::Object(options)) if !matches!(options.get("key"), None | Some(SExpression::Null))
        ),
        // Agent-port and program ops take their emit options last at any arity (`program/dispatch`'s inbound ask
        // is two arguments, its address form four): the key is the one beside `emit`.
        op if orbital_core_base::agent_ports::is_agent_port_operator(op)
            || orbital_core_base::language::registry::is_program_host_effect(op) =>
        {
            args.iter()
                .skip(1)
                .rev()
                .find_map(|arg| match arg {
                    SExpression::Object(options) if options.contains_key("emit") => {
                        Some(&**options)
                    }
                    _ => None,
                })
                .is_some_and(|options| {
                    !matches!(options.get("key"), None | Some(SExpression::Null))
                })
        }
        _ => matches!(
            args.last(),
            Some(SExpression::Object(options))
                if args.len() > 4 && !matches!(options.get("key"), None | Some(SExpression::Null))
        ),
    }
}

fn stdin_has_message_route(args: &[SExpression]) -> bool {
    let (_, envelope) = orbital_core_base::os::split_envelope(
        orbital_core_base::os::OsOp::StdinRead,
        args.get(1..).unwrap_or_default(),
    );
    let Some(SExpression::Object(routes)) = envelope.and_then(|options| options.get("emit")) else {
        return false;
    };
    !matches!(routes.get("onMessage"), None | Some(SExpression::Null))
}

fn with_os_key(
    op_name: &str,
    args: &[SExpression],
    fields: Option<Vec<PayloadField>>,
) -> Option<Vec<PayloadField>> {
    let mut fields = fields?;
    if orbital_core_base::os::OsOp::from_name(op_name).is_some() {
        fields.retain(|field| field.name != "key");
        if call_has_key(op_name, args) {
            fields.push(scalar_field("key", "string"));
            fields.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }
    Some(fields)
}

/// The payload an operator call site puts on `emit.failure`, or `None` (do not
/// check) when its failure shape is not decidable here.
pub(crate) fn produced_failure_payload(
    op_name: &str,
    args: &[SExpression],
) -> Option<Vec<PayloadField>> {
    let variant = match (op_name, persist_action(args)) {
        ("persist", Some(action)) => CallVariant::PersistAction(action),
        _ => CallVariant::Plain,
    };
    with_os_key(
        op_name,
        args,
        produced_to_fields(failure_payload(
            op_name,
            variant,
            call_has_key(op_name, args),
        )),
    )
}

/// The payload an operator call site puts on `emit.onMessage` (an `http` `stream`).
pub(crate) fn produced_message_payload(
    op_name: &str,
    args: &[SExpression],
) -> Option<Vec<PayloadField>> {
    with_os_key(op_name, args, produced_to_fields(message_payload(op_name)))
}
