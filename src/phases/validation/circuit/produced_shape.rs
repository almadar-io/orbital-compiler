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

use orbital_core_base::emit_contract::{failure_payload, message_payload, success_payload, CallVariant, ProducedPayload, ShapeField};
use orbital_core_base::schema::{EntityDefinition, PayloadField, SExpression};

use crate::embedded::{get_operator, ServiceRegistryData, OperatorTypeRef, OperatorTypeRefKind};

use super::effect::entity_field_type_map;

/// The bus key an operator fires its success payload on.
///
/// Not every operator uses `success`: `ref` fires **`onChange`** in both
/// executors (`EffectExecutor.ts:919-923`, `codegen/effect/server.rs:248-253`),
/// so reading `emit.success` for it inspects a key the runtime never fires.
pub(crate) fn success_emit_key(op_name: &str) -> &'static str {
    match op_name {
        "ref" => "onChange",
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
            Some(SExpression::String(op)) if op == "batch" => return None,
            Some(SExpression::String(op))
                if op == "create" || op == "upsert" || op == "update" || op == "delete" =>
            {
                2
            }
            _ => 1,
        },
        "fetch" | "ref" | "fetch-stream" => 1,
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
            OperatorTypeRefKind::Object { fields, .. } => (
                "object".to_string(),
                Some(object_payload_fields(fields, entity)),
            ),
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
                nullable: None,
                additional_properties: None,
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
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn scalar_field(name: &str, field_type: &str) -> PayloadField {
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
            field
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn produced_to_fields(produced: ProducedPayload) -> Option<Vec<PayloadField>> {
    match produced {
        ProducedPayload::Shape(shape) => Some(shape_fields(shape)),
        ProducedPayload::Fields(names) => Some(names.iter().map(|n| scalar_field(n, "any")).collect()),
        ProducedPayload::EntityRow | ProducedPayload::Opaque => None,
    }
}

/// True when the `http` call's request object (the fifth array element, head
/// included) carries a `paginate` entry.
fn http_variant(args: &[SExpression]) -> CallVariant<'static> {
    let Some(SExpression::Object(request)) = args.get(4) else { return CallVariant::Plain };
    if let Some(SExpression::Object(auth)) = request.get("auth") {
        if matches!(auth.get("scheme"), Some(SExpression::String(scheme)) if scheme == "oauth-client") {
            if let Some(op) = auth.get("op").and_then(SExpression::as_str).and_then(orbital_core_base::oauth_client::OAuthOp::parse) {
                return CallVariant::HttpOAuth(op);
            }
        }
    }
    if !matches!(request.get("presign"), None | Some(SExpression::Null)) {
        CallVariant::HttpPresign
    } else if !matches!(request.get("paginate"), None | Some(SExpression::Null)) {
        CallVariant::HttpPaginate
    } else {
        CallVariant::Plain
    }
}

/// The `browser/page-facts` payload: `available` plus the facts the call requested; a fact left out is not
/// readable. An undecidable field list yields no shape (no check).
fn page_facts_payload(args: &[SExpression]) -> Option<Vec<PayloadField>> {
    let SExpression::Array(words) = args.get(1)? else { return None };
    let keyed = call_has_key("browser/page-facts", args);
    let requested: Vec<&str> = words.iter().map(SExpression::as_str).collect::<Option<_>>()?;
    let ProducedPayload::Shape(shape) = success_payload("browser/page-facts", CallVariant::Plain) else {
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
) -> Option<Vec<PayloadField>> {
    if orbital_core_base::os::OsOp::from_name(op_name).is_some() {
        let keyed = call_has_key(op_name, args);
        let variant = if op_name == "os/stdin-read" && keyed && stdin_has_message_route(args) {
            CallVariant::StdinStream
        } else {
            CallVariant::Plain
        };
        return with_os_key(op_name, args, produced_to_fields(success_payload(op_name, variant)));
    }
    if op_name == "http" {
        let variant = http_variant(args);
        return produced_to_fields(success_payload("http", variant));
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
            | "access-check"
            | "browser/beacon"
            | "browser/prompt"
            | "browser/embed"
            | "redis"
            | "postgres"
            | "oauth-authorize"
            | "oauth-connection-status"
            | "oauth-connection-forget"
    ) {
        return produced_to_fields(success_payload(op_name, CallVariant::Plain));
    }
    if op_name == "browser/page-facts" {
        return page_facts_payload(args);
    }
    if op_name == "random-bytes" {
        return random_bytes_payload(args);
    }
    if matches!(op_name, "call-service" | "call_service") {
        return keyed_call_service_payload(args, registry);
    }
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

fn call_has_key(op_name: &str, args: &[SExpression]) -> bool {
    if let Some(op) = orbital_core_base::os::OsOp::from_name(op_name) {
        let (_, envelope) = orbital_core_base::os::split_envelope(op, args.get(1..).unwrap_or_default());
        return envelope.is_some_and(|options| !matches!(options.get("key"), None | Some(SExpression::Null)));
    }
    match op_name {
        "http" | "redis" | "postgres" => true,
        "browser/page-facts" | "random-bytes" => matches!(
            args.get(2),
            Some(SExpression::Object(options)) if !matches!(options.get("key"), None | Some(SExpression::Null))
        ),
        // Agent-port and program ops take their emit options last at any arity (`program/dispatch`'s inbound ask
        // is two arguments, its address form four): the key is the one beside `emit`.
        op if orbital_core_base::agent_ports::is_agent_port_operator(op)
            || orbital_core_base::language::registry::is_program_host_effect(op) =>
        {
            args.iter().skip(1).rev().find_map(|arg| match arg {
                SExpression::Object(options) if options.contains_key("emit") => Some(options),
                _ => None,
            })
            .is_some_and(|options| !matches!(options.get("key"), None | Some(SExpression::Null)))
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
    if orbital_core_base::os::OsOp::from_name(op_name).is_some() && call_has_key(op_name, args) {
        fields.push(scalar_field("key", "string"));
        fields.sort_by(|a, b| a.name.cmp(&b.name));
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
    with_os_key(op_name, args, produced_to_fields(failure_payload(op_name, variant, call_has_key(op_name, args))))
}

/// The payload an operator call site puts on `emit.onMessage` (an `http` `stream`).
pub(crate) fn produced_message_payload(op_name: &str, args: &[SExpression]) -> Option<Vec<PayloadField>> {
    with_os_key(op_name, args, produced_to_fields(message_payload(op_name)))
}
