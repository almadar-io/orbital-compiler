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

use orbital_core::schema::{EntityDefinition, PayloadField, SExpression};

use crate::embedded::{get_operator, OperatorTypeRef, OperatorTypeRefKind};

use super::effect::entity_field_type_map;

/// The bus key an operator fires its success payload on.
///
/// Not every operator uses `success`: `ref` fires **`on_change`** in both
/// executors (`EffectExecutor.ts:919-923`, `codegen/effect/server.rs:248-253`),
/// so reading `emit.success` for it inspects a key the runtime never fires.
pub(crate) fn success_emit_key(op_name: &str) -> &'static str {
    match op_name {
        "ref" => "on_change",
        _ => "success",
    }
}

/// Extract the entity name a call site names, using each operator's own
/// argument layout. `persist` mirrors `effect::persist::validate_persist`'s
/// entity-index detection (4-element `["persist", action, Entity, data]` vs the
/// legacy 3-element `["persist", Entity, data]`) rather than assuming a slot —
/// the legacy form has no action keyword, so a fixed index would read the wrong
/// argument.
fn call_site_entity(op_name: &str, args: &[SExpression]) -> Option<String> {
    let idx = match op_name {
        "persist" => match args.get(1) {
            Some(SExpression::String(op))
                if op == "create" || op == "update" || op == "delete" =>
            {
                2
            }
            _ => 1,
        },
        "fetch" | "ref" | "fetch-stream" => 1,
        _ => return None,
    };
    match args.get(idx) {
        Some(SExpression::String(name)) => Some(name.clone()),
        _ => None,
    }
}

/// The persist action at a call site, or `None` for the legacy 3-element form
/// (which carries no action keyword and must not be guessed at).
fn persist_action(args: &[SExpression]) -> Option<&str> {
    match args.get(1) {
        Some(SExpression::String(op)) if op == "create" || op == "update" || op == "delete" => {
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
            "create" | "update" => 0,
            "delete" => 1,
            _ => return None,
        };
        return of.get(idx);
    }
    None
}

/// Lower one `OperatorTypeRef` into the `(field_type, properties)` pair a
/// `PayloadField` carries. `entity` is the call site's resolved entity, used
/// wherever the registry says `{kind:entity}`.
fn field_shape(
    type_ref: &OperatorTypeRef,
    entity: Option<&EntityDefinition>,
) -> (String, Option<Vec<PayloadField>>) {
    match type_ref {
        OperatorTypeRef::Primitive(p) => (p.clone(), None),
        OperatorTypeRef::Tagged(kind) => match kind {
            OperatorTypeRefKind::Entity { .. } => match entity {
                Some(e) => ("object".to_string(), Some(entity_payload_fields(e))),
                // The operator produces a row but we could not resolve which
                // entity — permissive rather than wrong.
                None => ("any".to_string(), None),
            },
            OperatorTypeRefKind::Array { of } => {
                let (inner, props) = field_shape(of, entity);
                (format!("[{inner}]"), props)
            }
            OperatorTypeRefKind::Object { fields, .. } => {
                ("object".to_string(), Some(object_payload_fields(fields, entity)))
            }
            // A union / binding / sexpr / slot in a field position is not
            // something this check can adjudicate.
            _ => ("any".to_string(), None),
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
            let (field_type, properties) = field_shape(ty, entity);
            PayloadField {
                name: name.clone(),
                field_type,
                required: None,
                description: None,
                properties,
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
/// alias, so both sides of the comparison speak one alphabet.
fn entity_payload_fields(entity: &EntityDefinition) -> Vec<PayloadField> {
    let mut out: Vec<PayloadField> = entity_field_type_map(entity)
        .into_iter()
        .map(|(name, meta)| PayloadField {
            name,
            field_type: meta.field_type,
            required: None,
            description: None,
            properties: None,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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
) -> Option<Vec<PayloadField>> {
    let produces = get_operator(op_name)?.effect.as_ref()?.produces.as_ref()?;
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
            Some(object_payload_fields(fields, entity))
        }
        // `any` (call-service, set) and anything else carries no shape to check.
        _ => None,
    }
}
