//! Emit-literal payload shape validation (V2 Phase 0, P0.7).
//!
//! Walks every `(emit EVT {...})` effect in a schema and verifies the literal
//! payload object against the event's declared `PayloadField[]` on the
//! emitting trait. Complements `effect::validate_emit_against_declaration`
//! which already checks external-emit required-field presence. This pass
//! extends the coverage to:
//!
//!   1. Missing required fields (internal and external scope alike).
//!   2. Extra fields not declared on the event's payload schema.
//!   3. Primitive type mismatches between literal values and declared
//!      `field_type` (string / number / boolean / object / array / entity).
//!
//! All three classes are hard errors.

use std::collections::HashMap;

use orbital_core_base::schema::{
    EntityDefinition, EventScope, OrbitalSchema, PayloadField, SExpression, TraitReference,
};

use crate::embedded::ServiceRegistryData;
use super::effect::{
    collect_payload_field_types, entity_field_type_map, expr_return_incompatible, EntityFieldMeta,
};
use super::error::{ErrorCode, ValidationError};
use super::fired_events::trait_has_dynamic_event_prop_binding;
use super::produced_shape;
use super::{ValidationContext, ValidationResult};

/// Run emit-payload validation across every trait in every orbital.
///
/// Called from `validation/mod.rs:validate_comprehensive_full` after effect
/// validation (phase 5) and before closed-circuit (phase 12). Accumulates
/// warnings into the shared `ValidationContext`.
pub fn validate_all_emit_payloads(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // L2.7 prep: every event name fired anywhere in the schema. The walker is
    // shared with `cross_orbital::event_wiring`, whose fired-but-unheard check
    // is this one's dual — one definition of "produced", so the two can never
    // disagree about the same event.
    let all_fired_events = super::fired_events::collect_schema_fired_events(schema);

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            // Build a lookup of event -> declared payload fields for this trait.
            // Events declared without a payload (bare `emits { EVT }`) map to an
            // empty slice; used by the P0.8 operator-return check to detect
            // "operator produces X but event declares no payload" warnings.
            let emit_schemas: HashMap<&str, &[PayloadField]> = trait_def
                .emits
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|e| (e.event.as_str(), e.payload_schema.as_deref().unwrap_or(&[])))
                .collect();
            let trait_path = super::trait_path(oi, &orbital.name, ti, trait_ref);

            // Backstop inputs for the operator-call return-type inference below —
            // the SAME two maps the `(set …)` return-type check builds, so an
            // `(emit EVT { f: (op …) })` value is type-checked identically to
            // `(set @entity.f (op …))`. Both resolve `@entity`/`@payload` bindings
            // inside the expression; absent entries degrade to `Json` (permissive).
            let entity_field_types = trait_def
                .linked_entity
                .as_ref()
                .and_then(|e| ctx.entity_definitions.get(e))
                .map(entity_field_type_map)
                .unwrap_or_default();
            let payload_field_types = collect_payload_field_types(trait_def);

            for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                if let Some(effects) = &transition.effects {
                    check_repeated_emits(
                        effects,
                        &format!("{}.stateMachine.transitions[{}]", trait_path, i),
                        ctx,
                    );
                    for (j, effect) in effects.iter().enumerate() {
                        let path = format!(
                            "{}.stateMachine.transitions[{}].effects[{}]",
                            trait_path, i, j
                        );
                        if !emit_schemas.is_empty() {
                            let result = validate_emit_literal(
                                effect,
                                &path,
                                &emit_schemas,
                                &entity_field_types,
                                &payload_field_types,
                                schema,
                            );
                            ctx.merge(result);
                        }
                        // V2 Phase 0 P0.8: tie operator `effect.produces` to
                        // `emit: { success: "X" }` on the same call.
                        let p08 = validate_emit_success_return(
                            effect,
                            &path,
                            &emit_schemas,
                            &ctx.entity_definitions,
                            &ctx.service_registry,
                        );
                        ctx.merge(p08);
                    }
                }
            }

            if let Some(ticks) = &trait_def.ticks {
                for (i, tick) in ticks.iter().enumerate() {
                    for (j, effect) in tick.effects.iter().enumerate() {
                        let path = format!("{}.ticks[{}].effects[{}]", trait_path, i, j);
                        if !emit_schemas.is_empty() {
                            let result = validate_emit_literal(
                                effect,
                                &path,
                                &emit_schemas,
                                &entity_field_types,
                                &payload_field_types,
                                schema,
                            );
                            ctx.merge(result);
                        }
                        let p08 = validate_emit_success_return(
                            effect,
                            &path,
                            &emit_schemas,
                            &ctx.entity_definitions,
                            &ctx.service_registry,
                        );
                        ctx.merge(p08);
                    }
                }
            }

            // L2.7: declared-but-unfired emit detection (gap #2 defense).
            // Schema-wide scope: reports only when an event declared in
            // emits[] is never fired by ANY trait/tick across the schema.
            // Per-trait orphans (declared on trait A, fired on sibling
            // B in the same orbital) are a legitimate "namespace owner"
            // pattern (e.g. std-cms's ArticleBrowse declares all
            // Article* events for documentation; ArticleCreate /
            // ArticleEdit / ArticleDelete actually fire them) — those
            // skip the finding.
            //
            // Atom-level dynamic-event pattern: if the trait has any
            // render-ui prop with kind in {event, event-ref, event-listen,
            // callback} bound to a binding (e.g. `pageChangeEvent:
            // "@config.event"`), the actual event name fired at runtime
            // is the resolved config value, which we cannot prove
            // statically. Treat the trait's emit declarations as
            // "potentially fired" — pagination/search/filter atoms
            // expose this pattern. The component reads `@config.event`
            // and emits the consumer-supplied event name on the bus; the
            // emits[] block is the documented payload contract.
            let has_dynamic_event_binding = trait_has_dynamic_event_prop_binding(trait_def);
            // External-scope emits are intentionally undirected: the trait
            // declares the bus contract for events fired by *other* schemas
            // (a different orbital, a future external producer, an agent
            // outside the std registry, etc.). The trait's listens block
            // typically self-references them so transitions can react. The
            // declared-but-unfired check would always trip on these — that's
            // a false positive on a deliberate documentation pattern.
            let external_emits: std::collections::HashSet<&str> = trait_def
                .emits
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter(|e| matches!(e.scope, Some(EventScope::External)))
                .map(|e| e.event.as_str())
                .collect();
            if !emit_schemas.is_empty() && !has_dynamic_event_binding {
                let mut unfired: Vec<&str> = emit_schemas
                    .keys()
                    .filter(|k| {
                        !all_fired_events.contains_any_trait(k) && !external_emits.contains(*k)
                    })
                    .copied()
                    .collect();
                unfired.sort();
                for unfired_event in unfired {
                    ctx.add_error(
                        ValidationError::error(
                            ErrorCode::EmitDeclaredButUnfired,
                            &trait_path,
                            format!(
                                "Event '{}' is declared in `emits` but no trait in the schema fires it via `(emit {})` or operator emit-config",
                                unfired_event, unfired_event
                            ),
                        )
                        .with_suggestion(
                            "Either remove the unused declaration from `emits` or add an effect that fires the event somewhere in the schema",
                        ),
                    );
                }
            }
        }
    }
}

/// Validate a single `(emit EVT literal)` call against the declared payload
/// schema for EVT on this trait's `emits` block. Non-emit effects and emits
/// without a literal payload pass silently.
///
fn validate_emit_literal(
    effect: &SExpression,
    path: &str,
    emit_schemas: &HashMap<&str, &[PayloadField]>,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    schema_types: &OrbitalSchema,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let SExpression::Array(arr) = effect else {
        return result;
    };
    let Some(SExpression::String(op)) = arr.first() else {
        return result;
    };
    if op != "emit" || arr.len() < 2 {
        return result;
    }
    let Some(SExpression::String(event_name)) = arr.get(1) else {
        return result;
    };
    let Some(schema) = emit_schemas.get(event_name.as_str()) else {
        return result; // Unknown event handled by effect::validate_emit_against_declaration.
    };

    // The literal payload object is at index 2; absent when an emit has no payload.
    let payload_obj = match arr.get(2) {
        Some(SExpression::Object(o)) => Some(o),
        _ => None,
    };

    // Missing-required-field check — blocking (the legacy `warning()` alias
    // and its `strict` toggle were removed; there was only ever one severity).
    let make_missing = ValidationError::error;
    for field in schema.iter().filter(|f| f.required.unwrap_or(false)) {
        let present = payload_obj
            .map(|obj| obj.contains_key(&field.name))
            .unwrap_or(false);
        if !present {
            result.add_error(
                make_missing(
                    ErrorCode::EmitPayloadMissingRequired,
                    path,
                    format!(
                        "Emit '{}' is missing required payload field '{}' (declared as {})",
                        event_name, field.name, field.field_type
                    ),
                )
                .with_suggestion(format!(
                    "Add {{ \"{}\": ... }} to the emit payload literal",
                    field.name
                )),
            );
        }
    }

    // Extra-field + primitive-type-mismatch checks are only meaningful when
    // the emit actually carries a literal payload object.
    if let Some(obj) = payload_obj {
        let declared: HashMap<&str, &str> = schema
            .iter()
            .map(|f| (f.name.as_str(), f.field_type.as_str()))
            .collect();
        for (key, value) in obj.iter() {
            match declared.get(key.as_str()) {
                None => {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EmitPayloadExtraField,
                            path,
                            format!(
                                "Emit '{}' payload field '{}' is not declared on the event's payload schema",
                                event_name, key
                            ),
                        )
                        .with_suggestion(format!(
                            "Either declare '{}' on the event's payload array or remove it from the emit",
                            key
                        )),
                    );
                }
                Some(expected) => {
                    // An empty list has no element type of its own; it takes the slot's.
                    let empty_list_in_list_slot = matches!(value, SExpression::Array(items) if items.is_empty())
                        && (expected.starts_with('[') || *expected == "array");
                    let list_items = match value {
                        SExpression::Array(items)
                            if orbital_core_eval::literal_type::literal_value_type(value) == Some("array") =>
                        {
                            list_element_type(expected).map(|element| (element, items))
                        }
                        _ => None,
                    };
                    if empty_list_in_list_slot {
                    } else if let Some((element, items)) = list_items {
                        if let Some(actual) =
                            list_item_mismatch(element, items, entity_field_types, payload_field_types, schema_types)
                        {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::EmitPayloadFieldTypeMismatch,
                                    path,
                                    format!(
                                        "Emit '{}' field '{}' has a list item of type '{}', declared as '{}'",
                                        event_name, key, actual, expected
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Make every list item a '{}' (the list's element type is inferred from its items)",
                                    element
                                )),
                            );
                        }
                    } else if let Some(actual) = orbital_core_eval::literal_type::literal_value_type(value) {
                        if !primitive_types_compatible(expected, actual) {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::EmitPayloadFieldTypeMismatch,
                                    path,
                                    format!(
                                        "Emit '{}' field '{}' has value of type '{}', declared as '{}'",
                                        event_name, key, actual, expected
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Align the emit literal value with the declared field_type '{}'",
                                    expected
                                )),
                            );
                        }
                    } else if let Some(inferred) = expr_return_incompatible(
                        value,
                        &orbital_core_base::type_compat::compat_kind_oir(expected),
                        entity_field_types,
                        payload_field_types,
                        Some(schema_types),
                    ) {
                        // Operator-call payload value (e.g. `(object/get (array/find …) id)`):
                        // `literal_value_type` returned `None`, so infer its result type through
                        // the expression, mirroring the `(set …)` backstop. Only a CONCRETE
                        // incompatible inferred type reaches here — an unresolvable shape or an
                        // unknowable `Json` inferred type never rejects.
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EmitPayloadFieldTypeMismatch,
                                path,
                                format!(
                                    "Emit '{}' field '{}' evaluates to '{}', declared as '{}'",
                                    event_name,
                                    key,
                                    orbital_core_base::type_compat::oir_type_kind(&inferred),
                                    expected
                                ),
                            )
                            .with_suggestion(format!(
                                "Align the emit value with the declared field_type '{}' (wrap/convert the expression)",
                                expected
                            )),
                        );
                    }
                }
            }
        }
    }

    result
}

// The shallow literal-type classifier is `orbital_core::literal_value_type` —
// the single upstream source both this L2 backstop and the lolo L1 check call,
// so they never drift. It defers operator applications to `expr_return_incompatible`
// above (which infers their result type) instead of mis-typing them as `"array"`
// (CG-EMIT-PAYLOAD-SEXPR).

/// Enforce the payload invariant at every operator `emit` config.
///
/// **The invariant:** an event's declared payload type IS its access path.
/// Declare `Survey` → the payload is the row, read as `?name`. Declare
/// `{ row : Survey }` → read as `?row.name`. Any wrapping not in the type is a
/// lie; any field in the type that is not in the payload reads `undefined`.
///
/// **The rule:** every field an event DECLARES must exist in what the effect
/// PRODUCES. Declaring *fewer* fields than are emitted is legal and common —
/// declaring `{ data }` when the bus carries `{ data, totalCount }` just means
/// you only read `data` — so this is width subtyping (`declared ⊆ produced`),
/// not equality. Equality would reject the ~1,400 fetch sites that legitimately
/// omit `totalCount`.
///
/// This single rule replaces two hand-rolled predecessors, and covers the cases
/// each of them missed:
///   - `EmitSuccessPayloadWrapped` — persist-create/update-only, and only for a
///     wrapper naming that exact entity.
///   - `EmitSuccessPayloadReturnMismatch` — fired only when the declared payload
///     was entirely EMPTY, and keyed on `expects_entity_shape`, which matched
///     precisely the two `produces` declarations that were factually wrong
///     (`fetch`/`ref` claimed a bare entity while every executor emits
///     `{data, totalCount}`).
/// Because the rule is stated over any operator's real produced shape, it also
/// catches the mirror-image defect the old checks were blind to: an event
/// declaring a field that the operator never emits at all.
///
/// Hard error via `ValidationError::error`.
fn validate_emit_success_return(
    effect: &SExpression,
    path: &str,
    emit_schemas: &HashMap<&str, &[PayloadField]>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    registry: &ServiceRegistryData,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let SExpression::Array(arr) = effect else {
        return result;
    };
    let Some(SExpression::String(op_name)) = arr.first() else {
        return result;
    };

    // Each slot a call can fire: the bus key it fires on and the payload the
    // operator puts there. `None` = undecidable at this call site (no effect
    // metadata, an opaque `any`, an unresolvable union): never guess.
    // `ref` fires `onChange`, so reading `success` for it would inspect a key
    // the runtime never fires.
    let slots: [(&[&str], Option<Vec<PayloadField>>); 3] = [
        (
            &[produced_shape::success_emit_key(op_name.as_str())],
            produced_shape::produced_payload(op_name.as_str(), arr.as_slice(), entity_definitions, registry),
        ),
        (&["failure"], produced_shape::produced_failure_payload(op_name.as_str(), arr.as_slice())),
        (&["onMessage"], produced_shape::produced_message_payload(op_name.as_str())),
    ];
    for (emit_keys, produced) in slots {
        let Some(produced) = produced else { continue };
        let Some((emit_key, success_event)) = emit_keys
            .iter()
            .find_map(|key| emit_event_for_key(arr.as_slice(), key).map(|event| (*key, event)))
        else {
            continue;
        };
        // Unknown events are reported by `effect::validate_emit_against_declaration`.
        let Some(declared_payload) = emit_schemas.get(success_event.as_str()) else {
            continue;
        };
        check_declared_against_produced(
            op_name,
            emit_key,
            &success_event,
            declared_payload,
            &produced,
            path,
            &mut result,
        );
    }

    result
}

/// The code of a declared field the operator does not emit: `browser/page-facts` names the unrequested fact.
fn not_produced_code(op_name: &str) -> ErrorCode {
    if op_name == "browser/page-facts" {
        ErrorCode::PageFactsUnrequestedRead
    } else {
        ErrorCode::EmitPayloadNotProduced
    }
}

fn check_declared_against_produced(
    op_name: &str,
    emit_key: &str,
    success_event: &str,
    declared_payload: &[PayloadField],
    produced: &[PayloadField],
    path: &str,
    result: &mut ValidationResult,
) {
    for declared in declared_payload.iter() {
        match produced.iter().find(|p| p.name == declared.name) {
            None => {
                result.add_error(
                    ValidationError::error(
                        not_produced_code(op_name),
                        path,
                        format!(
                            "Event '{}' declares payload field '{}', but '{}' emits {} on '{}' — '{}' is not one of them, so '?{}' reads undefined at runtime",
                            success_event,
                            declared.name,
                            op_name,
                            describe_fields(produced),
                            emit_key,
                            declared.name,
                            declared.name
                        ),
                    )
                    .with_suggestion(format!(
                        "Declare '{}' with the shape '{}' actually emits ({}), or drop the field",
                        success_event,
                        op_name,
                        describe_fields(produced)
                    )),
                );
            }
            Some(produced_field) => {
                // Only a CONCRETE disagreement is reported — see
                // `produced_type_accepts` for why most pairs cannot be one.
                if !produced_type_accepts(&produced_field.field_type, &declared.field_type) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EmitPayloadNotProduced,
                            path,
                            format!(
                                "Event '{}' declares payload field '{}' as '{}', but '{}' emits it as '{}'",
                                success_event,
                                declared.name,
                                declared.field_type,
                                op_name,
                                produced_field.field_type
                            ),
                        )
                        .with_suggestion(format!(
                            "Align '{}.{}' with the emitted type '{}'",
                            success_event, declared.name, produced_field.field_type
                        )),
                    );
                }
            }
        }
    }
}

/// Can a field the operator emits as `produced` legitimately be declared as
/// `declared`?
///
/// The two sides speak different dialects and the produced side is strictly
/// COARSER, because it is derived from `EntityDefinition` field tags
/// (`field_type_str` → the lowercase serde tag: `relation`, `array`, `money`,
/// `object`). A payload declaration is richer and names the thing precisely:
/// the relation's target entity (`Member`), the array's element type
/// (`[string]`), the semantic scalar (`number` for a money column). Comparing
/// those two vocabularies with the literal-value table produced ~93 findings
/// across the corpus that were purely dialect noise — `skills: [string]` vs
/// `array`, `parentId: StripTag` vs `relation`, `subtotal: number` vs `money`.
/// Reporting those would have been worse than useless: it pressures authors to
/// downgrade a precise declaration to the validator's internal tag, discarding
/// the element type and the relation target.
///
/// So a coarse produced tag ACCEPTS any refinement of itself, and a mismatch is
/// only reported when both sides are concrete and genuinely disagree (declared
/// `string` where a `number` is emitted). The access-path half of the rule —
/// the field existing at all — is unaffected and remains exact; that is the
/// half that catches the actual defect class.
fn produced_type_accepts(produced: &str, declared: &str) -> bool {
    if produced == declared {
        return true;
    }
    match produced {
        // Unresolvable produced side, or a genuinely open shape.
        "any" | "json" | "object" | "entity" => true,
        orbital_core_base::emit_contract::REDIS_REPLY_TYPE => orbital_core_base::emit_contract::redis_reply_accepts(declared),
        // A relation column stores an id; a payload may name the target entity
        // or type it as the id string.
        "relation" => true,
        // The element type lives only on the declared side.
        "array" => declared.starts_with('[') || declared == "array",
        "money" => matches!(declared, "number" | "money"),
        "date" | "datetime" | "timestamp" => matches!(
            declared,
            "string" | "number" | "date" | "datetime" | "timestamp"
        ),
        // Semantic string domains.
        "email" | "url" | "phone" | "uuid" | "image" | "file" => {
            matches!(declared, "string" | "object") || declared == produced
        }
        // Both concrete: defer to the shared literal-compat table, which is
        // itself lenient about `any` and the date family.
        _ => primitive_types_compatible(declared, produced),
    }
}

/// Render a produced shape as a readable field list for diagnostics.
fn describe_fields(fields: &[PayloadField]) -> String {
    if fields.is_empty() {
        return "no payload".to_string();
    }
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    format!("{{ {} }}", names.join(", "))
}

/// Read `emit.<key>`'s event name off whichever trailing argument carries the
/// options object.
///
/// Scans from the back for the first object with an `emit` block rather than
/// indexing a per-operator slot. The previous slot table was wrong in three
/// ways at once: it read `set` at `args[3]` (`set` has `maxArity: 2`, so the
/// slot never exists), matched `"call_service"` (not an operator name — the
/// real one is `call-service`), and looked up `success` for `ref` (which fires
/// `on_change`). Scanning is also robust to persist's 3- vs 4-element forms.
fn emit_event_for_key(args: &[SExpression], key: &str) -> Option<String> {
    args.iter().rev().find_map(|value| {
        let SExpression::Object(obj) = value else {
            return None;
        };
        let SExpression::Object(emit_obj) = obj.get("emit")? else {
            return None;
        };
        match emit_obj.get(key)? {
            SExpression::String(s) => Some(s.clone()),
            _ => None,
        }
    })
}

/// Lenient primitive compatibility — delegated to orbital-core's canonical
/// `set_value_type_compatible` (the `sub`/`Coercion` relation), which is a
/// strict superset of this check's historical arms (equal kinds, raw
/// `Map<...>` tags vs object literals, `any`, the date family vs
/// string/number, entity/relation vs object/string). The fork was deduped in
/// Phase 4.1; the differential harness pins zero baseline movement.
/// An `emit` that repeats an earlier emit of the same transition with the same event and payload, with nothing
/// between them that writes state: the dispatch loop skips a delivery it already processed (EXEC-QUEUE-4), so the
/// second is silently dropped. A repeat across arms or rounds depends on run-time state and is left to `orb verify`.
fn check_repeated_emits(effects: &[SExpression], path: &str, ctx: &mut ValidationContext) {
    let mut seen: Vec<&SExpression> = Vec::new();
    for (index, effect) in effects.iter().enumerate() {
        let SExpression::Array(items) = effect else { continue };
        match items.first() {
            Some(SExpression::String(head)) if head == "emit" => {
                if seen.contains(&effect) {
                    let mut warning = ValidationError::error(
                        ErrorCode::EmitRepeatsIdentically,
                        format!("{path}.effects[{index}]"),
                        "this `emit` repeats an earlier one of the same transition identically: the dispatch loop delivers an identical emit once, so this one is dropped",
                    )
                    .with_suggestion("Remove the repeat, or give the two emits different payloads if both are meant to be heard");
                    warning.severity = super::Severity::Warning;
                    ctx.result.add_error(warning);
                } else {
                    seen.push(effect);
                }
            }
            Some(SExpression::String(head)) if matches!(head.as_str(), "render-ui" | "log" | "navigate" | "notify") => {}
            _ => seen.clear(),
        }
    }
}

/// The element type of a `[T]` slot (`[string]` -> `string`); `None` for a bare `array`, a tuple (`[a, b]`) or
/// anything that is not a one-element list type.
fn list_element_type(declared: &str) -> Option<&str> {
    let inner = declared.strip_prefix('[')?.strip_suffix(']')?.trim();
    (!inner.is_empty() && !inner.contains(',')).then_some(inner)
}

/// The kind of the first item of a list literal that does not fit `element`: a literal by its kind, a nested list
/// literal against a nested list slot, anything else (a binding, an operator call) by its inferred type.
fn list_item_mismatch(
    element: &str,
    items: &[SExpression],
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    schema_types: &OrbitalSchema,
) -> Option<String> {
    for item in items {
        match orbital_core_eval::literal_type::literal_value_type(item) {
            Some("array") => {
                if let (SExpression::Array(inner), Some(inner_element)) = (item, list_element_type(element)) {
                    if let Some(bad) = list_item_mismatch(inner_element, inner, entity_field_types, payload_field_types, schema_types) {
                        return Some(bad);
                    }
                } else if !primitive_types_compatible(element, "array") {
                    return Some("array".to_string());
                }
            }
            Some(actual) => {
                if !primitive_types_compatible(element, actual) {
                    return Some(actual.to_string());
                }
            }
            None => {
                if let Some(inferred) = expr_return_incompatible(
                    item,
                    &orbital_core_base::type_compat::compat_kind_oir(element),
                    entity_field_types,
                    payload_field_types,
                    Some(schema_types),
                ) {
                    return Some(orbital_core_base::type_compat::oir_type_kind(&inferred));
                }
            }
        }
    }
    None
}

fn primitive_types_compatible(declared: &str, actual: &str) -> bool {
    orbital_core_base::type_compat::set_value_type_compatible(declared, actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// True when the payload-invariant check flagged `event`. Scans both
    /// buckets even though the check is a hard error (lands in `errors`)
    /// so the helper stays correct regardless of severity.
    fn has_wrapped(ctx: &ValidationContext, event: &str) -> bool {
        ctx.result
            .errors
            .iter()
            .chain(ctx.result.warnings.iter())
            .any(|e| e.code == ErrorCode::EmitPayloadNotProduced && e.message.contains(event))
    }

    /// The fixture every payload-invariant test uses. The entity is INLINE (not
    /// a string reference) because the check resolves the produced shape from
    /// `ctx.entity_definitions` — without a real entity it has no shape to
    /// compare against and, by design, declines to check rather than guess.
    fn persist_trait_json(emits: &str, effect: &str) -> String {
        format!(
            r#"{{
                "name": "wrap-app",
                "orbitals": [{{
                    "name": "ThreadOrbital",
                    "entity": {{
                        "name": "ThreadPost",
                        "fields": [
                            {{ "name": "id", "type": "string" }},
                            {{ "name": "content", "type": "string" }}
                        ]
                    }},
                    "traits": [{{
                        "name": "ThreadPostCreate",
                        "emits": [{emits}],
                        "stateMachine": {{
                            "states": [{{ "name": "browsing", "isInitial": true }}],
                            "transitions": [{{
                                "from": "browsing", "to": "browsing", "event": "SUBMIT",
                                "effects": [{effect}]
                            }}]
                        }}
                    }}]
                }}]
            }}"#
        )
    }

    /// Unit tests call `validate_all_emit_payloads` directly, bypassing
    /// `validate_comprehensive_full` and therefore Phase 1 (`collect_names`),
    /// which is what normally populates `entity_definitions`. Seed it the same
    /// way so the produced shape is resolvable.
    fn ctx_with_seeded_entities(schema: &OrbitalSchema) -> ValidationContext {
        let mut ctx = ValidationContext::new();
        for orbital in &schema.orbitals {
            if let Some(entity) = orbital.entity.as_inline() {
                ctx.entity_definitions
                    .insert(entity.name.clone(), entity.clone());
            }
        }
        ctx
    }

    fn check(emits: &str, effect: &str) -> ValidationContext {
        let json = persist_trait_json(emits, effect);
        let schema: OrbitalSchema = serde_json::from_str(&json).expect("schema parses");
        let mut ctx = ctx_with_seeded_entities(&schema);
        validate_all_emit_payloads(&schema, &mut ctx);
        ctx
    }

    const CREATE: &str = r#"["persist", "create", "ThreadPost", { "content": "@entity.content" }, { "emit": { "success": "ThreadPostCreated" } }]"#;

    /// POSITIVE CONTROL — the exact std-thread `ThreadPostCreated` defect
    /// (round 5, found live): `persist create` emits the row FLAT, so an event
    /// declaring `{ row : ThreadPost }` has a field `row` that is never emitted.
    /// Every `?row.<field>` read is undefined at runtime.
    ///
    /// This is the alias-only lowering: one field, `properties: None`,
    /// `field_type` == the entity's own name.
    #[test]
    fn persist_create_wrapped_row_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostCreated", "payloadSchema": [{ "name": "row", "type": "ThreadPost" }] }"#,
            CREATE,
        );
        assert!(
            has_wrapped(&ctx, "ThreadPostCreated"),
            "a persist create whose event declares a `row` wrapper must warn — persist emits the row flat"
        );
    }

    /// Same defect via `persist update`.
    #[test]
    fn persist_update_wrapped_row_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostUpdated", "payloadSchema": [{ "name": "row", "type": "ThreadPost" }] }"#,
            r#"["persist", "update", "ThreadPost", { "content": "@entity.content" }, { "emit": { "success": "ThreadPostUpdated" } }]"#,
        );
        assert!(
            has_wrapped(&ctx, "ThreadPostUpdated"),
            "a persist update whose event declares a `row` wrapper must warn"
        );
    }

    /// POSITIVE CONTROL — the same wrap arriving through the OTHER lowering
    /// path. When the event also carries an inline `emits {}` restatement (the
    /// corpus-mandated form whenever a matching alias exists), `lower_emit`
    /// EXPANDS the entity to `field_type: "object"` + `properties`, rather than
    /// leaving a bare entity name. The first cut of this check only recognised
    /// the alias-only shape and was therefore silently inert on essentially
    /// every real corpus file. The general rule sees both, because it compares
    /// field NAMES against what is produced rather than pattern-matching a shape.
    #[test]
    fn persist_create_inline_restatement_wrapped_row_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostCreated", "payloadSchema": [{ "name": "row", "type": "object", "properties": [{ "name": "id", "type": "string" }, { "name": "content", "type": "string" }] }] }"#,
            CREATE,
        );
        assert!(
            has_wrapped(&ctx, "ThreadPostCreated"),
            "an inline-restated row wrapper must warn just like the alias-only form"
        );
    }

    const CREATE_FAILING: &str = r#"["persist", "create", "ThreadPost", { "content": "@entity.content" }, { "emit": { "failure": "ThreadPostFailed" } }]"#;

    /// A persist failure carries `{ error, code }` (EXEC-STEP-8): an event declaring a field the kernel never
    /// emits (the JS-only `entityType`) is reported, so a `?field` read cannot be undefined at runtime.
    #[test]
    fn persist_failure_declaring_a_field_it_never_emits_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostFailed", "payloadSchema": [{ "name": "error", "type": "string" }, { "name": "entityType", "type": "string" }] }"#,
            CREATE_FAILING,
        );
        assert!(has_wrapped(&ctx, "ThreadPostFailed"));
    }

    /// NEGATIVE CONTROL — `error` and `code` are what a persist failure emits.
    #[test]
    fn persist_failure_declaring_error_and_code_does_not_warn() {
        let ctx = check(
            r#"{ "event": "ThreadPostFailed", "payloadSchema": [{ "name": "error", "type": "string" }, { "name": "code", "type": "string" }] }"#,
            CREATE_FAILING,
        );
        assert!(!has_wrapped(&ctx, "ThreadPostFailed"));
    }

    const EXEC_EMIT: &str = r#"{ "event": "EXEC", "payloadSchema": [{ "name": "argv", "type": "[string]" }] }"#;

    fn list_slot_mismatch(argv: &str) -> bool {
        let effect = format!(r#"["emit", "EXEC", {{ "argv": {argv} }}]"#);
        check(EXEC_EMIT, &effect)
            .result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EmitPayloadFieldTypeMismatch)
    }

    /// A list literal's element type comes from its items: strings, and an `@`-reference whose type is known, fill
    /// a `[string]` slot without wrapping the list in `array/concat`.
    #[test]
    fn a_list_literal_of_matching_items_fills_a_typed_list_slot() {
        assert!(!list_slot_mismatch(r#"["stop", "x"]"#));
        assert!(!list_slot_mismatch(r#"["stop", "@entity.content"]"#));
        assert!(!list_slot_mismatch("[]"));
    }

    /// NEGATIVE CONTROL — an item of the wrong type is still refused, literal or `@`-referenced.
    #[test]
    fn a_list_literal_with_an_item_of_the_wrong_type_is_refused_in_a_typed_list_slot() {
        assert!(list_slot_mismatch(r#"["stop", 5]"#));
        assert!(list_slot_mismatch(r#"[true]"#));
    }

    fn list_alias_mismatch(shape: &str) -> bool {
        let json = persist_trait_json(
            r#"{ "event": "LISTED", "payloadSchema": [{ "name": "items", "type": "[object]" }] }"#,
            &format!(r#"["emit", "LISTED", {{ "items": ["json/parse", "[]", "{shape}"] }}]"#),
        )
        .replacen(
            r#""name": "ThreadOrbital","#,
            r#""name": "ThreadOrbital", "types": { "Point": [{ "name": "x", "type": "number" }], "Points": [{ "name": "", "type": "[Point]" }] },"#,
            1,
        );
        let schema: OrbitalSchema = serde_json::from_str(&json).expect("schema parses");
        let mut ctx = ctx_with_seeded_entities(&schema);
        validate_all_emit_payloads(&schema, &mut ctx);
        ctx.result.errors.iter().any(|e| e.code == ErrorCode::EmitPayloadFieldTypeMismatch)
    }

    /// `json/parse` over a list alias is the list the alias names, so it fills a list slot.
    #[test]
    fn json_parse_over_a_list_alias_fills_a_list_slot() {
        assert!(!list_alias_mismatch("Points"));
    }

    /// NEGATIVE CONTROL — a struct alias is a record, not a list.
    #[test]
    fn json_parse_over_a_struct_alias_does_not_fill_a_list_slot() {
        assert!(list_alias_mismatch("Point"));
    }

    const EMBED_TYPES: &str = r#""types": {
        "EmbedDatum": [{ "name": "index", "type": "number" }, { "name": "embedding", "type": "[number]" }],
        "EmbedReply": [{ "name": "data", "type": "[EmbedDatum]" }]
    },"#;

    fn embeddings_mismatch(value: &str) -> bool {
        let json = persist_trait_json(
            r#"{ "event": "EMBEDDED", "payloadSchema": [{ "name": "embeddings", "type": "[[number]]" }] }"#,
            &format!(r#"["emit", "EMBEDDED", {{ "embeddings": {value} }}]"#),
        )
        .replacen(r#""name": "ThreadOrbital","#, &format!(r#""name": "ThreadOrbital", {EMBED_TYPES}"#), 1);
        let schema: OrbitalSchema = serde_json::from_str(&json).expect("schema parses");
        let mut ctx = ctx_with_seeded_entities(&schema);
        validate_all_emit_payloads(&schema, &mut ctx);
        ctx.result.errors.iter().any(|e| e.code == ErrorCode::EmitPayloadFieldTypeMismatch)
    }

    fn embeddings_expr(inner: &str) -> String {
        format!(
            r#"["let", [["data", ["object/get", ["json/parse", "@payload.body", "EmbedReply"], "data", []]]],
                ["let", [["n", ["array/len", "@data"]]],
                  ["array/map", ["array/sort", ["array/map", "@data", ["fn", "x", ["+", ["*", ["object/get", "@x", "index", 0], "@n"], "@index"]]]],
                    ["fn", "k", {inner}]]]]"#
        )
    }

    /// The element type survives `array/map` over `array/sort`, `array/nth` and `object/get` with a default.
    #[test]
    fn a_mapped_list_of_embeddings_keeps_its_element_type() {
        let embedding = r#"["object/get", ["array/nth", "@data", ["math/mod", "@k", "@n"]], "embedding", []]"#;
        assert!(!embeddings_mismatch(&embeddings_expr(embedding)));
    }

    /// NEGATIVE CONTROL: a genuinely wrong element type is still refused.
    #[test]
    fn a_mapped_list_of_numbers_is_not_a_list_of_embeddings() {
        let index = r#"["object/get", ["array/nth", "@data", ["math/mod", "@k", "@n"]], "index", 0]"#;
        assert!(embeddings_mismatch(&embeddings_expr(index)));
    }

    fn repeats(effects: &str) -> bool {
        let ctx = check(r#"{ "event": "PING", "payloadSchema": [] }"#, effects);
        ctx.result
            .warnings
            .iter()
            .chain(ctx.result.errors.iter())
            .any(|e| e.code == ErrorCode::EmitRepeatsIdentically)
    }

    /// The dispatch loop collapses a second identical plain emit (EXEC-QUEUE-4); that is never silent.
    #[test]
    fn an_emit_repeated_identically_in_one_transition_is_flagged() {
        assert!(repeats(r#"["emit", "PING", {}], ["emit", "PING", {}]"#));
        assert!(repeats(r#"["emit", "PING", { "a": 1 }], ["render-ui", "main", { "type": "typography" }], ["emit", "PING", { "a": 1 }]"#));
    }

    /// NEGATIVE CONTROLS — a different payload, a different event, or a write between the two is not a repeat.
    #[test]
    fn emits_that_differ_or_straddle_a_write_are_not_flagged() {
        assert!(!repeats(r#"["emit", "PING", { "a": 1 }], ["emit", "PING", { "a": 2 }]"#));
        assert!(!repeats(r#"["emit", "PING", {}], ["emit", "PONG", {}]"#));
        assert!(!repeats(r#"["emit", "PING", {}], ["set", "@entity.id", "x"], ["emit", "PING", {}]"#));
        assert!(!repeats(r#"["emit", "PING", {}]"#));
    }

    const FETCH_FAILING: &str = r#"["fetch", "ThreadPost", { "emit": { "success": "ThreadPostsLoaded", "failure": "ThreadPostsFailed" } }]"#;

    /// A fetch failure fires its event with `{ error }` (`fetch_completion_emit`): a declared `message` is
    /// never delivered, so every `?message` read is undefined.
    #[test]
    fn fetch_failure_declaring_a_field_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostsFailed", "payloadSchema": [{ "name": "message", "type": "string" }] }"#,
            FETCH_FAILING,
        );
        assert!(has_wrapped(&ctx, "ThreadPostsFailed"));
    }

    /// NEGATIVE CONTROL — `error` is what the kernel sends.
    #[test]
    fn fetch_failure_declaring_error_does_not_warn() {
        let ctx = check(r#"{ "event": "ThreadPostsFailed", "payloadSchema": [{ "name": "error", "type": "string" }] }"#, FETCH_FAILING);
        assert!(!has_wrapped(&ctx, "ThreadPostsFailed"));
    }

    /// NEGATIVE CONTROL — the FIXED shape. A bare `type X = Event ThreadPost`
    /// lowers to the entity's own fields flat at the payload root, which is
    /// exactly what persist emits. Nothing to report.
    #[test]
    fn persist_create_bare_entity_does_not_warn() {
        let ctx = check(
            r#"{ "event": "ThreadPostCreated", "payloadSchema": [{ "name": "id", "type": "string" }, { "name": "content", "type": "string" }] }"#,
            CREATE,
        );
        assert!(
            !has_wrapped(&ctx, "ThreadPostCreated"),
            "a bare flattened-entity payload matches what persist emits"
        );
    }

    /// NEGATIVE CONTROL — **omission is legal**. The rule is width subtyping
    /// (`declared ⊆ produced`): declaring only the fields you actually read is
    /// fine. Equality would reject this, and with it the ~1,400 corpus fetch
    /// sites that legitimately omit `totalCount`.
    #[test]
    fn declaring_a_subset_of_the_produced_fields_is_legal() {
        let ctx = check(
            r#"{ "event": "ThreadPostCreated", "payloadSchema": [{ "name": "id", "type": "string" }] }"#,
            CREATE,
        );
        assert!(
            !has_wrapped(&ctx, "ThreadPostCreated"),
            "declaring fewer fields than are emitted must be allowed"
        );
    }

    /// POSITIVE CONTROL for the case the OLD narrow check was blind to — an
    /// event declaring a field the operator simply never emits. No wrapper
    /// involved, no entity-name coincidence; `summary` is just not part of a
    /// persisted ThreadPost, so `?summary` reads undefined. The predecessor
    /// check could not see this at all: it only pattern-matched a wrapper whose
    /// type named the persisted entity.
    #[test]
    fn declaring_a_field_the_operator_never_emits_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostCreated", "payloadSchema": [{ "name": "summary", "type": "object", "properties": [{ "name": "id", "type": "string" }, { "name": "unrelatedField", "type": "string" }] }] }"#,
            CREATE,
        );
        assert!(
            has_wrapped(&ctx, "ThreadPostCreated"),
            "a declared field that is never emitted must warn regardless of its shape"
        );
    }

    /// POSITIVE CONTROL — a wrapper naming a DIFFERENT entity is still a
    /// wrapper: `row` is not a field of a persisted ThreadPost either way.
    /// Under the old check this passed silently, because it required the
    /// wrapper's type to equal the persisted entity's name.
    #[test]
    fn wrapper_naming_a_different_entity_still_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostCreated", "payloadSchema": [{ "name": "row", "type": "SomeOtherEntity" }] }"#,
            CREATE,
        );
        assert!(
            has_wrapped(&ctx, "ThreadPostCreated"),
            "the field name is what matters — `row` is not emitted whatever it is typed as"
        );
    }

    /// NEGATIVE CONTROL — `persist delete` emits `{ id, deleted }`, a genuinely
    /// different (and correctly declared) shape. An event declaring `id` is
    /// consistent with it, so the check must stay silent: the rule is driven by
    /// each operation's real contract, not by a blanket "persist means flat row".
    #[test]
    fn persist_delete_declaring_id_does_not_warn() {
        let ctx = check(
            r#"{ "event": "ThreadPostDeleted", "payloadSchema": [{ "name": "id", "type": "string" }] }"#,
            r#"["persist", "delete", "ThreadPost", {}, { "emit": { "success": "ThreadPostDeleted" } }]"#,
        );
        assert!(
            !has_wrapped(&ctx, "ThreadPostDeleted"),
            "persist delete really does emit an `id`, so declaring it is correct"
        );
    }

    /// POSITIVE CONTROL — the delete contract is enforced too: `row` is not
    /// part of `{ id, deleted }`.
    #[test]
    fn persist_delete_wrapped_row_warns() {
        let ctx = check(
            r#"{ "event": "ThreadPostDeleted", "payloadSchema": [{ "name": "row", "type": "ThreadPost" }] }"#,
            r#"["persist", "delete", "ThreadPost", {}, { "emit": { "success": "ThreadPostDeleted" } }]"#,
        );
        assert!(
            has_wrapped(&ctx, "ThreadPostDeleted"),
            "persist delete emits {{id, deleted}} — a `row` field is not among them"
        );
    }

    /// NEGATIVE CONTROL — the legacy 3-element persist form
    /// (`["persist", "Entity", data]`) carries no action keyword. The check must
    /// decline rather than assume "create": guessing here would mis-narrow the
    /// produces union and report against the wrong contract.
    #[test]
    fn legacy_three_arg_persist_is_not_guessed_at() {
        let ctx = check(
            r#"{ "event": "ThreadPostCreated", "payloadSchema": [{ "name": "row", "type": "ThreadPost" }] }"#,
            r#"["persist", "ThreadPost", { "content": "@entity.content" }, { "emit": { "success": "ThreadPostCreated" } }]"#,
        );
        assert!(
            !has_wrapped(&ctx, "ThreadPostCreated"),
            "an undecidable call site must produce no finding, not a guessed one"
        );
    }
    fn has_unfired(ctx: &ValidationContext, event: &str) -> bool {
        // EmitDeclaredButUnfired is a hard error; scans both buckets so
        // the helper stays correct regardless of severity.
        ctx.result
            .errors
            .iter()
            .chain(ctx.result.warnings.iter())
            .any(|e| e.code == ErrorCode::EmitDeclaredButUnfired && e.message.contains(event))
    }

    /// std-gallery-shaped regression: an atom declares an event whose ONLY
    /// firing lives in a config field DEFAULT (an `itemActions`-style item
    /// list carrying `{ event: "OPEN_POLICY" }`). No transition fires it and
    /// no call-site override exists. Before the resolved-config walk this
    /// tripped a false-positive `ORB_EMIT_DECLARED_BUT_UNFIRED`.
    #[test]
    fn config_default_firing_suppresses_declared_but_unfired() {
        let json = r#"{
            "name": "gallery-app",
            "orbitals": [{
                "name": "GalleryOrbital",
                "entity": "GalleryItem",
                "traits": [{
                    "name": "GalleryItemGallery",
                    "emits": [{ "event": "OPEN_POLICY", "payloadSchema": [] }],
                    "stateMachine": {
                        "states": [{ "name": "browsing", "isInitial": true }],
                        "transitions": []
                    },
                    "config": {
                        "itemActions": {
                            "type": "unknown",
                            "default": [{ "event": "OPEN_POLICY", "label": "Open" }]
                        }
                    }
                }]
            }]
        }"#;
        let schema: OrbitalSchema = serde_json::from_str(json).expect("schema parses");
        let mut ctx = ValidationContext::new();
        validate_all_emit_payloads(&schema, &mut ctx);
        assert!(
            !has_unfired(&ctx, "OPEN_POLICY"),
            "config-default fired event must not warn as declared-but-unfired"
        );
    }

    /// Negative control: an event fired NOWHERE — not in effects, ticks, or a
    /// config default — must still warn. Proves the resolved-config walk did
    /// not blanket-suppress the rule.
    #[test]
    fn truly_unfired_event_still_warns() {
        let json = r#"{
            "name": "orphan-app",
            "orbitals": [{
                "name": "OrphanOrbital",
                "entity": "Thing",
                "traits": [{
                    "name": "ThingTrait",
                    "emits": [{ "event": "NEVER_FIRED", "payloadSchema": [] }],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": []
                    },
                    "config": {
                        "label": { "type": "string", "default": "Hello" }
                    }
                }]
            }]
        }"#;
        let schema: OrbitalSchema = serde_json::from_str(json).expect("schema parses");
        let mut ctx = ValidationContext::new();
        validate_all_emit_payloads(&schema, &mut ctx);
        assert!(
            has_unfired(&ctx, "NEVER_FIRED"),
            "an event fired nowhere must still warn as declared-but-unfired"
        );
    }

    /// POSITIVE CONTROL for the bus-exclusion fix. The only thing naming INIT
    /// is a `Button action={INIT}` — a control the bus never delivers, so it
    /// is DEAD. Counting it as an emitter let the dead button suppress the
    /// very warning that would have found it.
    #[test]
    fn a_dead_lifecycle_button_does_not_count_as_an_emitter() {
        let json = r#"{
            "name": "dead-button-app",
            "orbitals": [{
                "name": "ProbeOrbital",
                "entity": "Probe",
                "traits": [{
                    "name": "ProbeTrait",
                    "emits": [{ "event": "INIT", "payloadSchema": [] }],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{
                            "from": "idle", "to": "idle", "event": "INIT",
                            "effects": [["render-ui", "main", {
                                "type": "button", "action": "INIT", "label": "Retry"
                            }]]
                        }]
                    }
                }]
            }]
        }"#;
        let schema: OrbitalSchema = serde_json::from_str(json).expect("schema parses");
        let mut ctx = ValidationContext::new();
        validate_all_emit_payloads(&schema, &mut ctx);
        assert!(
            has_unfired(&ctx, "INIT"),
            "a bus-excluded affordance must not satisfy the declared-but-unfired check"
        );
    }

    /// NEGATIVE CONTROL: a NON-lifecycle affordance still counts as firing.
    #[test]
    fn a_live_button_still_counts_as_an_emitter() {
        let json = r#"{
            "name": "live-button-app",
            "orbitals": [{
                "name": "ProbeOrbital",
                "entity": "Probe",
                "traits": [{
                    "name": "ProbeTrait",
                    "emits": [{ "event": "RETRY", "payloadSchema": [] }],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{
                            "from": "idle", "to": "idle", "event": "INIT",
                            "effects": [["render-ui", "main", {
                                "type": "button", "action": "RETRY", "label": "Retry"
                            }]]
                        }]
                    }
                }]
            }]
        }"#;
        let schema: OrbitalSchema = serde_json::from_str(json).expect("schema parses");
        let mut ctx = ValidationContext::new();
        validate_all_emit_payloads(&schema, &mut ctx);
        assert!(
            !has_unfired(&ctx, "RETRY"),
            "a deliverable affordance must still satisfy the check"
        );
    }

    /// `collect_fired_with_knobs` recurses generically through every
    /// array/object node with no `type:`-based gating, so a conditional
    /// (`if`-wrapped) `itemActions` config default is walked into both
    /// branches automatically — proven here rather than assumed: both
    /// branch events must be credited as fired.
    #[test]
    fn conditional_item_actions_config_default_credits_both_branches() {
        let json = r#"{
            "name": "gallery-app",
            "orbitals": [{
                "name": "GalleryOrbital",
                "entity": "GalleryItem",
                "traits": [{
                    "name": "GalleryItemGallery",
                    "emits": [
                        { "event": "EDIT_ITEM", "payloadSchema": [] },
                        { "event": "VIEW_ITEM", "payloadSchema": [] }
                    ],
                    "stateMachine": {
                        "states": [{ "name": "browsing", "isInitial": true }],
                        "transitions": []
                    },
                    "config": {
                        "itemActions": {
                            "type": "unknown",
                            "default": ["if", ["=", "@user.role", "reader"],
                                [{ "event": "EDIT_ITEM", "label": "Edit" }],
                                [{ "event": "VIEW_ITEM", "label": "View" }]
                            ]
                        }
                    }
                }]
            }]
        }"#;
        let schema: OrbitalSchema = serde_json::from_str(json).expect("schema parses");
        let mut ctx = ValidationContext::new();
        validate_all_emit_payloads(&schema, &mut ctx);
        assert!(
            !has_unfired(&ctx, "EDIT_ITEM"),
            "then-branch event must not warn as declared-but-unfired"
        );
        assert!(
            !has_unfired(&ctx, "VIEW_ITEM"),
            "else-branch event must not warn as declared-but-unfired"
        );
    }

    #[test]
    fn a_record_literal_fills_a_program_type_slot_and_a_scalar_does_not() {
        assert!(primitive_types_compatible("Viewer", "object"));
        assert!(!primitive_types_compatible("Viewer", "number"));
        assert!(!primitive_types_compatible("Viewer", "string"));
    }
}
