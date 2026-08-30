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
//! All three classes are warn-only during V2 Phase 0 rollout; they escalate
//! to hard errors in Phase 5 per `docs/Almadar_Entity_V2_Plan.md` §9.

use std::collections::HashMap;

use orbital_core::events::is_bus_excluded_event;
use orbital_core::schema::{
    EventScope, OrbitalSchema, PayloadField, SExpression, TraitDefinition, TraitReference,
};

use crate::embedded::{
    get_operator, patterns, EmbeddedOperator, OperatorTypeRef, OperatorTypeRefKind,
};

use super::effect::{
    collect_payload_field_types, entity_field_type_map, expr_return_incompatible, EntityFieldMeta,
};
use super::error::{ErrorCode, ValidationError};
use super::{ValidationContext, ValidationResult};

/// Run emit-payload validation across every trait in every orbital.
///
/// Called from `validation/mod.rs:validate_comprehensive` after effect
/// validation (phase 5) and before closed-circuit (phase 12). Accumulates
/// warnings into the shared `ValidationContext`.
pub fn validate_all_emit_payloads(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let strict = ctx.phase_f_strict;

    // L2.7 prep: collect every event name fired across the whole schema
    // (any trait, transition, or tick). Events declared in `emits[]` but
    // never fired anywhere across the schema are warned. The schema-wide
    // scope avoids false positives on the legitimate "namespace owner"
    // pattern where one trait declares events that sibling traits in the
    // same orbital actually fire.
    let mut all_fired_events: std::collections::HashSet<String> = std::collections::HashSet::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            for transition in &td.state_machine.transitions {
                if let Some(effects) = &transition.effects {
                    for effect in effects {
                        collect_fired_events_recursive(effect, &mut all_fired_events);
                    }
                }
            }
            if let Some(ticks) = &td.ticks {
                for tick in ticks {
                    for effect in &tick.effects {
                        collect_fired_events_recursive(effect, &mut all_fired_events);
                    }
                }
            }
            // Resolved-config event references. An atom's config field DEFAULT
            // may carry structured event references (e.g. an `itemActions`
            // default `[{ event: "OPEN_POLICY", … }]`). The render-ui effect
            // binds `@config.itemActions`, which the effect walk skips as a
            // runtime binding — so the fired event lives only in the config
            // value. The inline phase folds every call-site override into this
            // same `default` (override where the consumer supplied one, atom
            // default otherwise), so walking it here reflects the RESOLVED
            // config for both the standalone atom and any consumer that
            // inlines it. Same registry-driven `collect_fired_events_recursive`
            // the effect walk uses — one walker, not a parallel scan.
            if let Some(config) = &td.config {
                for field in config.values() {
                    if let Some(default) = &field.default {
                        collect_fired_events_recursive(default, &mut all_fired_events);
                    }
                }
            }
        }
    }

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
                                strict,
                                &entity_field_types,
                                &payload_field_types,
                            );
                            ctx.merge(result);
                        }
                        // V2 Phase 0 P0.8: tie operator `effect.produces` to
                        // `emit: { success: "X" }` on the same call.
                        let p08 = validate_emit_success_return(effect, &path, &emit_schemas);
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
                                strict,
                                &entity_field_types,
                                &payload_field_types,
                            );
                            ctx.merge(result);
                        }
                        let p08 = validate_emit_success_return(effect, &path, &emit_schemas);
                        ctx.merge(p08);
                    }
                }
            }

            // L2.7: declared-but-unfired emit detection (gap #2 defense).
            // Schema-wide scope: warn only when an event declared in
            // emits[] is never fired by ANY trait/tick across the schema.
            // Per-trait orphans (declared on trait A, fired on sibling
            // B in the same orbital) are a legitimate "namespace owner"
            // pattern (e.g. std-cms's ArticleBrowse declares all
            // Article* events for documentation; ArticleCreate /
            // ArticleEdit / ArticleDelete actually fire them) — those
            // skip the warning.
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
                    .filter(|k| !all_fired_events.contains(**k) && !external_emits.contains(*k))
                    .copied()
                    .collect();
                unfired.sort();
                for unfired_event in unfired {
                    ctx.add_error(
                        ValidationError::phase_f_warning(
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

/// Set of object-key names that, per the patterns registry, identify a
/// string value as a bus event-name reference. Mirrors the lolo
/// `event_prop_names()` derivation:
///
/// 1. Top-level pattern prop names with `kind` in `{event, event-ref,
///    event-listen, callback}` (e.g. `action`, `submitEvent`,
///    `tabChangeEvent`, `pageChangeEvent`, `onTabChange`, ...).
/// 2. Inner descriptor field names from `kind: "event-list"` entries'
///    `eventField` metadata (e.g. data-list.itemActions's `event`
///    inner key).
///
/// All entries are source-tagged: pattern-sync wrote each `kind` mark
/// because a component author typed the prop as `EventEmit<P>` /
/// `EventKey` / a function (callback) / an array of descriptors with an
/// event field. No hardcoded names, no heuristics.
/// True when any render-ui pattern in the trait's effects binds a
/// kind:{event, event-ref, event-listen, callback} prop to a `@`-prefixed
/// binding (config / payload / entity / state). The runtime event name
/// is dynamic; we cannot statically prove the trait's declared emits
/// are unfired. Examples: `pageChangeEvent: "@config.event"`,
/// `tabChangeEvent: "@config.event"`. Used by L2.7 to suppress
/// false-positive `EmitDeclaredButUnfired` warnings on atoms that
/// expose their event names through configuration.
fn trait_has_dynamic_event_prop_binding(td: &TraitDefinition) -> bool {
    let event_keys = ui_event_prop_names();
    for transition in &td.state_machine.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                if sexpr_has_dynamic_event_binding(effect, event_keys) {
                    return true;
                }
            }
        }
    }
    if let Some(ticks) = &td.ticks {
        for tick in ticks {
            for effect in &tick.effects {
                if sexpr_has_dynamic_event_binding(effect, event_keys) {
                    return true;
                }
            }
        }
    }
    false
}

fn sexpr_has_dynamic_event_binding(
    expr: &SExpression,
    event_keys: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        SExpression::Object(obj) => {
            for (key, value) in obj.iter() {
                if event_keys.contains(key) {
                    if let SExpression::String(s) = value {
                        if s.starts_with('@') {
                            return true;
                        }
                    }
                }
                if sexpr_has_dynamic_event_binding(value, event_keys) {
                    return true;
                }
            }
            false
        }
        SExpression::Array(arr) => arr
            .iter()
            .any(|item| sexpr_has_dynamic_event_binding(item, event_keys)),
        _ => false,
    }
}

fn ui_event_prop_names() -> &'static std::collections::HashSet<String> {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static EVENT_PROP_NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    EVENT_PROP_NAMES.get_or_init(|| {
        let mut names: HashSet<String> = HashSet::new();
        for pattern in patterns().values() {
            for (prop_name, prop) in &pattern.props_schema {
                let Some(kind) = prop.kind.as_deref() else {
                    continue;
                };
                match kind {
                    "event" | "event-ref" | "event-listen" | "callback" => {
                        names.insert(prop_name.clone());
                    }
                    "event-list" => {
                        if let Some(field) = prop.event_field.as_deref() {
                            names.insert(field.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        names
    })
}

/// Recursively walk an SExpression collecting every event-name string
/// fired by:
///
/// 1. `["emit", "EVENT_NAME", ...]` standalone effect.
/// 2. `[op, ..., {emit: {success: "X", failure: "Y"}}]` operator emit-
///    config (fetch / persist / call-service / ref / deref / agent-
///    invoke — generic scan, every operator with an `emit` config-key
///    is covered).
/// 3. UI render-ui pattern props that carry event names — the registry's
///    `kind: "event"|"event-ref"|"event-listen"|"callback"` props (e.g.
///    `action: "SAVE"`, `tabChangeEvent: "TAB_CHANGED"`,
///    `submitEvent: "FORM_SAVED"`, `onTabChange: "TAB_CHANGED"`) — and
///    `kind: "event-list"` inner-descriptor fields (e.g.
///    `itemActions: [{ event: "VIEW", ... }]`).
///
/// Source-tagged: every UI prop name comes from a registry `kind` mark
/// pattern-sync wrote because a component author typed the prop
/// accordingly. The set extends automatically as new event-bearing
/// props ship in @almadar/ui.
fn collect_fired_events_recursive(
    expr: &SExpression,
    fired: &mut std::collections::HashSet<String>,
) {
    match expr {
        SExpression::Array(arr) => {
            // `["emit", "EVENT_NAME", ...]`
            if let (Some(SExpression::String(op)), Some(SExpression::String(event))) =
                (arr.first(), arr.get(1))
            {
                if op == "emit" {
                    fired.insert(event.clone());
                }
            }
            // `[op, ..., {emit: {success: "X", failure: "Y", on_message:
            // "Z", ...}}]` — scan every string value under any `emit`
            // sub-key. Done generically so every operator + every
            // fire-key (success / failure / on_message / on_change /
            // future additions) is covered without per-key maintenance.
            for item in arr {
                if let SExpression::Object(obj) = item {
                    if let Some(SExpression::Object(emit_cfg)) = obj.get("emit") {
                        for (_k, v) in emit_cfg.iter() {
                            if let SExpression::String(s) = v {
                                fired.insert(s.clone());
                            }
                        }
                    }
                }
            }
            for item in arr {
                collect_fired_events_recursive(item, fired);
            }
        }
        SExpression::Object(obj) => {
            // UI pattern props that carry event-name strings. The
            // registry-driven event-prop set covers all four kinds of
            // declarative event reference (event / event-ref /
            // event-listen / callback) plus event-list inner-field
            // names — pattern-sync writes the `kind` mark; we read it.
            let event_keys = ui_event_prop_names();
            for (key, value) in obj.iter() {
                if event_keys.contains(key) {
                    // A conditional value contributes every branch's leaf —
                    // only one renders at a time, but each is a live emitter.
                    for leaf in value.conditional_leaf_values() {
                        if let SExpression::String(s) = leaf {
                            // Skip bindings (`@entity.X`, `@payload.Y`) —
                            // those resolve at runtime to event-name
                            // strings, but at static analysis we can't know
                            // the value. Treating `@`-prefixed strings as
                            // fired would suppress legitimate orphan
                            // detection.
                            // …and skip bus-excluded names. A `Button
                            // action={INIT}` is a DEAD control (the bus never
                            // delivers INIT/LOAD/$MOUNT), so counting it as an
                            // emitter let the dead button suppress the very
                            // warning that would have found it.
                            if !s.starts_with('@') && !is_bus_excluded_event(s) {
                                fired.insert(s.clone());
                            }
                        }
                    }
                }
                collect_fired_events_recursive(value, fired);
            }
        }
        _ => {}
    }
}

/// Validate a single `(emit EVT literal)` call against the declared payload
/// schema for EVT on this trait's `emits` block. Non-emit effects and emits
/// without a literal payload pass silently.
///
/// `strict` controls Phase F.1 escalation of the
/// `EmitPayloadMissingRequired` class from warning to hard error. Extra and
/// type-mismatch fields stay at warn severity regardless.
fn validate_emit_literal(
    effect: &SExpression,
    path: &str,
    emit_schemas: &HashMap<&str, &[PayloadField]>,
    strict: bool,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
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

    // Missing-required-field check. Phase F.1: when `strict` is set, escalate
    // this class to error. The other two emit-payload classes stay at warn.
    let make_missing = if strict {
        ValidationError::error
    } else {
        ValidationError::warning
    };
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
                        ValidationError::warning(
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
                    if let Some(actual) = orbital_core::literal_value_type(value) {
                        if !primitive_types_compatible(expected, actual) {
                            result.add_error(
                                ValidationError::warning(
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
                        expected,
                        entity_field_types,
                        payload_field_types,
                    ) {
                        // Operator-call payload value (e.g. `(object/get (array/find …) id)`):
                        // `literal_value_type` returned `None`, so infer its result type through
                        // the expression, mirroring the `(set …)` backstop. Only a CONCRETE
                        // incompatible inferred type reaches here — an unresolvable shape or an
                        // unknowable `Json` inferred type never rejects.
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::EmitPayloadFieldTypeMismatch,
                                path,
                                format!(
                                    "Emit '{}' field '{}' evaluates to '{}', declared as '{}'",
                                    event_name,
                                    key,
                                    orbital_core::oir_type_kind(&inferred),
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

/// V2 Phase 0 P0.8: for effect calls of the five data operators that carry
/// an `emit: { success: "X" }` config, check that event X is declared on the
/// trait and its declared payload is compatible with the operator's
/// `effect.produces` at this call site.
///
/// Scope for this pass is deliberately narrow: we narrow fetch/ref/persist's
/// union `produces` at the call site (options.id presence for fetch/ref;
/// action literal for persist) and warn when the narrowed branch expects
/// an entity-shaped payload but the declared event carries no payload. Full
/// field-by-field type equality between the narrowed OperatorTypeRef and the
/// declared PayloadField[] is deferred to a follow-up session so the
/// narrowing logic stays reviewable.
fn validate_emit_success_return(
    effect: &SExpression,
    path: &str,
    emit_schemas: &HashMap<&str, &[PayloadField]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let SExpression::Array(arr) = effect else {
        return result;
    };
    let Some(SExpression::String(op_name)) = arr.first() else {
        return result;
    };
    let Some(op_meta) = get_operator(op_name) else {
        return result;
    };
    let Some(effect_meta) = op_meta.effect.as_ref() else {
        return result;
    };
    let Some(produces) = effect_meta.produces.as_ref() else {
        return result;
    };

    // Extract the emit.success event name from whichever arg slot carries the
    // options object for this operator.
    let Some(success_event) = success_event_from_effect(op_name.as_str(), arr.as_slice()) else {
        return result;
    };

    // Every declared event on the trait already passed the "is known" check
    // in effect.rs; only proceed if this one is known. Unknown events are
    // reported elsewhere.
    let Some(declared_payload) = emit_schemas.get(success_event.as_str()) else {
        return result;
    };

    // Narrow the produces union based on the specific call site.
    let narrowed = narrow_produces(op_meta, arr.as_slice(), produces);

    // Minimal check: when the narrowed type is an entity (scalar) or an array
    // of entity, we expect the declared payload to carry at least one field
    // (it will have been flattened by the lowerer from the `:: TypeName`
    // annotation). Empty payloads on events named in `emit.success` for these
    // operators are almost certainly authoring bugs.
    if expects_entity_shape(narrowed) && declared_payload.is_empty() {
        result.add_error(
            ValidationError::warning(
                ErrorCode::EmitSuccessPayloadReturnMismatch,
                path,
                format!(
                    "Operator '{}' produces an entity-shaped value for 'emit.success = {}' but the declared event has an empty payload",
                    op_name, success_event
                ),
            )
            .with_suggestion(format!(
                "Declare a payload on event '{}' using `:: <EntityType>` (Form A) or a field list so consumers can read typed @payload values",
                success_event
            )),
        );
    }

    result
}

/// Extract the `emit.success` event name from an effect call's arguments,
/// checking the options-object slot that this specific operator places its
/// options at. Returns `None` when the call doesn't have an emit config.
fn success_event_from_effect(op_name: &str, args: &[SExpression]) -> Option<String> {
    // args[0] is the operator name itself; the options-object slot differs
    // per operator shape in modules/core.ts.
    let options_slot = match op_name {
        // (fetch Entity options?) -> args[2]
        "fetch" | "ref" => 2,
        // (persist action Entity? data?) — options may come as a trailing
        // object on args[3] when the `emit` key is present (per
        // EffectExecutor.ts:493 heuristic).
        "persist" => {
            // Walk from the back for the first object with `emit` key.
            return args.iter().rev().find_map(extract_emit_success);
        }
        // (call-service "svc" "action" params options?) -> args[4]
        "call-service" | "call_service" => 4,
        // (set binding value options?) -> args[3]
        "set" => 3,
        _ => return None,
    };
    args.get(options_slot).and_then(extract_emit_success)
}

fn extract_emit_success(value: &SExpression) -> Option<String> {
    let SExpression::Object(obj) = value else {
        return None;
    };
    let emit = obj.get("emit")?;
    let SExpression::Object(emit_obj) = emit else {
        return None;
    };
    match emit_obj.get("success")? {
        SExpression::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Narrow a union `produces` to the branch matching the call site. Non-union
/// produces return unchanged. Unknown operators or unrecognised argument
/// shapes fall through to the full union (lenient on mismatch).
fn narrow_produces<'a>(
    op_meta: &'a EmbeddedOperator,
    args: &'a [SExpression],
    produces: &'a OperatorTypeRef,
) -> &'a OperatorTypeRef {
    let OperatorTypeRef::Tagged(OperatorTypeRefKind::Union { of }) = produces else {
        return produces;
    };
    let kind = op_meta
        .effect
        .as_ref()
        .map(|e| e.kind.as_str())
        .unwrap_or("");
    let picked: Option<usize> = match kind {
        "fetch" | "ref" => {
            // options at args[2]; has `id` -> scalar (branch 0), else array (branch 1).
            let has_id = args
                .get(2)
                .and_then(|opt| {
                    let SExpression::Object(o) = opt else {
                        return None;
                    };
                    Some(o.contains_key("id"))
                })
                .unwrap_or(false);
            Some(if has_id { 0 } else { 1 })
        }
        "persist" => {
            // action at args[1]; union order: [create/update scalar, delete object, batch summary].
            let action = args
                .get(1)
                .and_then(|a| match a {
                    SExpression::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            Some(match action {
                "create" | "update" => 0,
                "delete" | "clear" => 1,
                "batch" => 2,
                _ => return produces, // unknown action, keep union
            })
        }
        _ => None,
    };
    match picked {
        Some(idx) if idx < of.len() => &of[idx],
        _ => produces,
    }
}

fn expects_entity_shape(type_ref: &OperatorTypeRef) -> bool {
    matches!(
        type_ref,
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Entity { .. })
            | OperatorTypeRef::Tagged(OperatorTypeRefKind::Array { .. })
    )
}

/// Lenient primitive compatibility check. `any` matches anything; `date` /
/// `datetime` / `timestamp` are acceptable as strings at literal time.
fn primitive_types_compatible(declared: &str, actual: &str) -> bool {
    if declared == actual {
        return true;
    }
    match declared {
        "any" => true,
        "date" | "datetime" | "timestamp" => actual == "string" || actual == "number",
        // Entity / relation fields accept object literals at emit time.
        "entity" | "relation" => actual == "object" || actual == "string",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_unfired(ctx: &ValidationContext, event: &str) -> bool {
        // EmitDeclaredButUnfired is a warning, so it lands in `warnings`;
        // scan `errors` too in case Phase F escalation ever moves it.
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

    /// `collect_fired_events_recursive` recurses generically through every
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
}
