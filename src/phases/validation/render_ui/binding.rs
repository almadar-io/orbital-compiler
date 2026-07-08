//! L2.1 keystone: render-ui binding type-check.
//!
//! Independent pass run AFTER the literal-only `validate_pattern` check. Walks
//! every trait's render-ui props and resolves each binding against the
//! registry's declared prop types, plus the L2.2/L2.3 event-payload checks.

use super::pattern::classify_prop_type;
use super::sync::patterns::get_prop_types;
use super::{ErrorCode, ValidationContext, ValidationError};
use crate::embedded::{get_pattern, EmbeddedPatternProp};
use crate::phases::validation::trait_path;
use crate::phases::validation::render_ui_binding::{
    resolve_binding_type, BindingContext, BindingType,
};
use orbital_core::schema::{
    OrbitalSchema, PayloadField, SExpression, TraitDefinition, TraitReference,
};
use std::collections::HashMap;

/// Top-level entry: walk every trait, every transition's render-ui
/// patterns, and check that each prop binding resolves to a type
/// compatible with the registry's declared prop types.
///
/// Architecture: independent pass run AFTER the existing
/// `validate_all_render_ui` literal check. Adds the binding-resolution
/// dimension that was previously deferred to runtime / TS typecheck —
/// gap #6's primary lift in `docs/Almadar_Std_Verification.md`.
pub(super) fn validate_all_render_ui_bindings(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Snapshot the entity_definitions reference so the BindingContext
    // borrow doesn't conflict with the &mut ctx used to add errors.
    // Both reads end at the function boundary; cheap reference, no clone.
    let entity_definitions_snapshot = ctx.entity_definitions.clone();

    // Collect diagnostics into a local Vec, then drain into ctx after
    // the inner loops — keeps the borrow checker happy without
    // requiring threading a separate accumulator through.
    let mut diagnostics: Vec<ValidationError> = Vec::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        // Sibling trait names in the orbital, for `@trait.Y` validation.
        let sibling_trait_names: Vec<String> = orbital
            .traits
            .iter()
            .filter_map(|tr| match tr {
                TraitReference::Inline(td) => Some(td.name.clone()),
                _ => None,
            })
            .collect();

        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };

            // Per-trait event → payload schema lookup. Used to resolve
            // `@payload.X` bindings against the listened event's
            // declared payload. Two sources:
            //
            // 1. The trait's own `listens` block: each entry's
            //    `payload_schema` (Almadar trait IR) is the contract for
            //    `@payload.X` reads inside that listen's triggered
            //    transition.
            // 2. The trait's `state_machine.events[]` block: declared
            //    event payloads visible to the transitions that fire on
            //    those events (typed-payload v2 metadata).
            //
            // For a transition firing on event E, the resolver sees the
            // payload from whichever source declares E. If neither
            // declares it, `event_payload` stays None and `@payload.X`
            // resolutions return Unknown — the L2 rule then skips
            // rather than emit a false positive.
            let event_payloads: HashMap<String, Vec<PayloadField>> =
                collect_trait_event_payloads(trait_def);

            for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                let event_payload = event_payloads.get(&transition.event).map(|v| v.as_slice());
                let bctx = BindingContext {
                    trait_def,
                    entity_definitions: &entity_definitions_snapshot,
                    event_payload,
                    sibling_trait_names: &sibling_trait_names,
                };

                if let Some(effects) = &transition.effects {
                    for (j, effect) in effects.iter().enumerate() {
                        let path = format!(
                            "{}.stateMachine.transitions[{}].effects[{}]",
                            trait_path(oi, &orbital.name, ti, trait_ref),
                            i, j
                        );
                        check_render_ui_bindings_in_effect(effect, &bctx, &path, &mut diagnostics);
                    }
                }
            }
        }
    }

    for d in diagnostics {
        ctx.add_error(d);
    }
}

/// Build the per-event payload-schema map for a trait.
///
/// Pulls from `state_machine.events[]` — typed-payload v2 metadata that
/// declares payloads for the events the trait fires on its transitions.
/// Listen entries don't carry a separate `payload_schema` field on the
/// IR type (they have `payload_mapping` for cross-event field rewires
/// only); the canonical payload contract lives on the matching events
/// declaration.
fn collect_trait_event_payloads(trait_def: &TraitDefinition) -> HashMap<String, Vec<PayloadField>> {
    let mut map: HashMap<String, Vec<PayloadField>> = HashMap::new();
    if let Some(events) = &trait_def.state_machine.events {
        for evt in events {
            if let Some(payload) = &evt.payload_schema {
                map.insert(evt.key.clone(), payload.clone());
            }
        }
    }
    map
}

/// Recursively descend into an effect SExpression, looking for
/// `["render-ui", "<slot>", <pattern>]` calls. When found, walk the
/// pattern tree and check each prop binding.
fn check_render_ui_bindings_in_effect(
    effect: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    if let SExpression::Array(arr) = effect {
        if let Some(SExpression::String(op)) = arr.first() {
            if op == "render-ui" {
                if let Some(pattern) = arr.get(2) {
                    walk_pattern_props(pattern, bctx, path, diagnostics);
                }
            }
        }
        for item in arr {
            check_render_ui_bindings_in_effect(item, bctx, path, diagnostics);
        }
    } else if let SExpression::Object(obj) = effect {
        for value in obj.values() {
            check_render_ui_bindings_in_effect(value, bctx, path, diagnostics);
        }
    }
}

/// Walk a render-ui pattern's prop tree. For every prop value that's a
/// binding (`@`-prefixed string) or an SExpression operator call, ask
/// F3's resolver for the type and compare against the prop's declared
/// types in the registry.
///
/// Recurses into nested patterns (children arrays of stack/dashboard-
/// layout, etc.) so deep render-ui trees are covered without needing
/// per-pattern dispatch.
fn walk_pattern_props(
    pattern: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let SExpression::Object(obj) = pattern else {
        return;
    };
    let pattern_type = match obj.get("type") {
        Some(SExpression::String(t)) => Some(t.as_str()),
        _ => None,
    };

    for (prop_name, prop_value) in obj.iter() {
        if prop_name == "type" {
            continue;
        }

        // Recurse first so nested patterns get checked regardless of
        // whether the parent prop has a registry type.
        recurse_into_value(prop_value, bctx, path, diagnostics);

        // Only check props with declared types in the registry.
        let Some(pat) = pattern_type else { continue };

        // L2.2 first: when the prop is a `kind: "event-ref"` /
        // `kind: "callback"` event-name reference and the value is a
        // literal string event name, compare the registry's documented
        // bus payload (emitPayloadSchema or callbackArgs) against the
        // trait's declared `emits { EVENT { ... } }` payload. Done
        // BEFORE the L2.1 type-check because event-ref props are
        // typed as `string` in the registry, which the literal-pass
        // already accepts — but the structural payload-shape compare
        // is the deeper contract.
        if let SExpression::String(event_name) = prop_value {
            if !event_name.starts_with('@') {
                check_event_binding_payload(pat, prop_name, event_name, bctx, path, diagnostics);
                // L2.3: when the literal matches a known event-bearing
                // prop in the registry but the event isn't declared on
                // the trait, the literal is stale — likely a leftover
                // from an events-override rename that the compiler's
                // C1 inline rewrite missed. Gap #30.
                check_event_literal_stale(pat, prop_name, event_name, bctx, path, diagnostics);
            }
        }

        let Some(expected_types) = get_prop_types(pat, prop_name) else {
            continue;
        };

        // Skip the literal-only check that's already performed by the
        // existing `validate_pattern` pass.
        let actual_kind = classify_prop_type(prop_value);
        if actual_kind != "binding" && !matches!(prop_value, SExpression::Array(_)) {
            continue;
        }

        // Skip when expected is "object" — too broad; the existing rule
        // already documents this as deferred.
        if expected_types.contains(&"object") {
            continue;
        }
        // Skip event-name bindings (kind: event-ref / callback / etc.) —
        // L2.2 covers those via emit-payload schema compare. Here we
        // only care about value-typed bindings.
        if expected_types.contains(&"function") {
            continue;
        }

        // Literal array short-circuit: when the expected type is `array`
        // and the value is a literal `SExpression::Array(...)`, accept
        // it as an array regardless of what `resolve_binding_type` would
        // return. Without this, an array of bare-string field names
        // like `["path", "method", "backend"]` gets parsed as the
        // `path` canonical operator (returnType "string") and trips a
        // false positive. Field names commonly collide with operator
        // names; the surrounding prop's declared type is the
        // disambiguator.
        if expected_types.contains(&"array") && matches!(prop_value, SExpression::Array(_)) {
            continue;
        }

        let resolved = resolve_binding_type(prop_value, bctx);
        if matches!(resolved, BindingType::Unknown | BindingType::TraitRef) {
            // Resolver couldn't determine the type (cross-orbital
            // payload, missing alias, dynamic field name, or sentinel
            // trait ref). Skip — emitting a false-positive here is
            // costly when ~half of bindings still resolve to Unknown
            // pending fuller IR coverage.
            continue;
        }
        let resolved_str = resolved.registry_type();
        // `entity` resolves to "object" — the prop's declared types
        // accept that via the existing skip-on-object rule.

        let compatible = expected_types.iter().any(|t| {
            *t == resolved_str
                || *t == "node"   // any rendered value is a ReactNode
                || *t == "string" && resolved_str == "trait"  // @trait.X used as label fallback
                || (*t == "icon" && resolved_str == "string")
        });
        if !compatible {
            diagnostics.push(
                ValidationError::error(
                    ErrorCode::RenderUIBindingTypeMismatch,
                    path,
                    format!(
                        "Prop '{}' on '{}' is bound to a value of resolved type '{}' but the registry expects: {}",
                        prop_name,
                        pat,
                        resolved_str,
                        expected_types.join(", ")
                    ),
                )
                .with_suggestion(format!(
                    "Bind '{}' to a value whose resolved type matches: {}",
                    prop_name,
                    expected_types.join(" or ")
                )),
            );
        }
    }
}

/// L2.2: when a render-ui prop with `kind: "event-ref"` (declarative
/// bus emit) or `kind: "callback"` (React callback wrapped by codegen)
/// is bound to an event-name string, compare the registry's documented
/// bus payload shape against the trait's declared
/// `emits { EVENT { ... } }` payload. Mismatch → diagnostic with
/// pattern + trait + event side-by-side.
///
/// Two flavours:
///
/// - **`kind: "event-ref"`** (e.g. `tabChangeEvent: "TAB_CHANGED"`):
///   the molecule emits `eventBus.emit('UI:{prop}', emitPayloadSchema)`.
///   The trait's declared payload for `TAB_CHANGED` must include every
///   required field in `emitPayloadSchema`.
///
/// - **`kind: "callback"`** (e.g. `onTabChange: "TAB_CHANGED"`):
///   React invokes the callback with positional args; codegen
///   synthesises `dispatch('TAB_CHANGED', { name1: arg1, name2: arg2 })`
///   from the registry's `callbackArgs` (per C2 plan). The trait's
///   declared payload must contain a field for every callbackArg by
///   name and compatible type.
///
/// Bindings (`@payload.X` / etc.) skip — the prop value isn't a static
/// event name. Empty `emits[]` or no payload contract on the trait
/// also skip (no constraint to compare against).
fn check_event_binding_payload(
    pattern_type: &str,
    prop_name: &str,
    event_name: &str,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let Some(pattern) = get_pattern(pattern_type) else {
        return;
    };
    let Some(prop_meta): Option<&EmbeddedPatternProp> = pattern.props_schema.get(prop_name) else {
        return;
    };

    // Only event-ref / callback props carry the metadata we need. The
    // legacy `event` kind is treated as `event-ref` for back-compat.
    let kind = prop_meta.kind.as_deref().unwrap_or("");
    let is_event_ref = matches!(kind, "event" | "event-ref" | "event-listen");
    let is_callback = kind == "callback";
    if !is_event_ref && !is_callback {
        return;
    }

    // Find the trait's declared payload for this event. Walk emits[]
    // first (the canonical declaration site for produced events); the
    // event must be declared somewhere on the trait or its parent for
    // the comparison to make sense.
    let declared_payload: Option<Vec<orbital_core::schema::PayloadField>> = bctx
        .trait_def
        .emits
        .as_deref()
        .and_then(|emits| emits.iter().find(|e| e.event == event_name))
        .and_then(|e| e.payload_schema.clone())
        .or_else(|| {
            bctx.trait_def
                .state_machine
                .events
                .as_ref()
                .and_then(|evts| evts.iter().find(|e| e.key == event_name))
                .and_then(|e| e.payload_schema.clone())
        });
    let Some(declared) = declared_payload else {
        // Event isn't declared on this trait. The L1.1 reachability
        // check already catches typos at parse; here we skip rather
        // than re-emit. A fully-undeclared event surfacing only at L2
        // is rare in practice.
        return;
    };

    if is_event_ref {
        // Compare emitPayloadSchema (registry) vs declared payload.
        let Some(emit_schema) = &prop_meta.emit_payload_schema else {
            return; // legacy `event` kind without emitPayloadSchema
        };
        for required_field in emit_schema.iter().filter(|f| f.required) {
            let trait_field = declared.iter().find(|p| p.name == required_field.name);
            match trait_field {
                None => {
                    diagnostics.push(
                        ValidationError::error(
                            ErrorCode::RenderUIEventBindingPayloadMismatch,
                            path,
                            format!(
                                "Prop '{}' on '{}' emits 'UI:{}' with payload '{}: {}' but trait's `emits {{ {} {{ ... }} }}` doesn't declare '{}'",
                                prop_name, pattern_type, event_name,
                                required_field.name, required_field.r#type,
                                event_name, required_field.name,
                            ),
                        )
                        .with_suggestion(format!(
                            "Add `{} : {}` to the trait's `emits {{ {} {{ ... }} }}` payload",
                            required_field.name, required_field.r#type, event_name
                        )),
                    );
                }
                Some(declared_field) => {
                    if !primitive_event_compat(&declared_field.field_type, &required_field.r#type) {
                        diagnostics.push(
                            ValidationError::warning(
                                ErrorCode::RenderUIEventBindingPayloadMismatch,
                                path,
                                format!(
                                    "Prop '{}' on '{}' emits 'UI:{}' with '{}: {}' but trait declares '{} : {}'",
                                    prop_name, pattern_type, event_name,
                                    required_field.name, required_field.r#type,
                                    declared_field.name, declared_field.field_type,
                                ),
                            )
                            .with_suggestion(format!(
                                "Reconcile the type — the component emits '{}' as {} but the trait declares it as {}",
                                required_field.name, required_field.r#type, declared_field.field_type,
                            )),
                        );
                    }
                }
            }
        }
    } else if is_callback {
        // Compare callbackArgs (registry) vs declared payload by name.
        let Some(args) = &prop_meta.callback_args else {
            return;
        };
        for arg in args.iter() {
            let trait_field = declared.iter().find(|p| p.name == arg.name);
            match trait_field {
                None => {
                    diagnostics.push(
                        ValidationError::error(
                            ErrorCode::RenderUIEventBindingPayloadMismatch,
                            path,
                            format!(
                                "Callback prop '{}' on '{}' will dispatch '{}' with positional arg '{}: {}' (codegen wraps to {{ {} }}) but trait's payload for '{}' has no '{}' field",
                                prop_name, pattern_type, event_name,
                                arg.name, arg.r#type, arg.name,
                                event_name, arg.name,
                            ),
                        )
                        .with_suggestion(format!(
                            "Either rename '{}' to match a trait payload field, or add `{} : {}` to the trait's `emits {{ {} {{ ... }} }}` payload",
                            arg.name, arg.name, arg.r#type, event_name,
                        )),
                    );
                }
                Some(declared_field) => {
                    if !primitive_event_compat(&declared_field.field_type, &arg.r#type) {
                        diagnostics.push(
                            ValidationError::warning(
                                ErrorCode::RenderUIEventBindingPayloadMismatch,
                                path,
                                format!(
                                    "Callback prop '{}' on '{}' arg '{}' is type '{}' but trait declares payload field '{}' as '{}'",
                                    prop_name, pattern_type, arg.name, arg.r#type,
                                    declared_field.name, declared_field.field_type,
                                ),
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// L2.3: when a render-ui pattern carries a string literal at an
/// event-bearing prop key, but that string isn't declared anywhere in
/// the trait's event set (`emits[]` + `state_machine.events[]` +
/// `listens[]`), the literal is stale. Most common cause: a trait-ref
/// used `events: { OLD: NEW }` to rename an imported atom's events, but
/// the compiler's inline phase didn't rewrite the literal inside the
/// atom's render-ui pattern (gap #30 / C1 bug).
///
/// Skips when the prop isn't an event-bearing kind (event / event-ref /
/// event-listen / callback / event-list-eventField). Skips when the
/// trait's event set is empty (no constraint to compare).
///
/// Also surfaces non-events-override stale literals: an author rename
/// from `OPEN` → `START` on a trait but missing the rewrite at one
/// render-ui call site lands the same diagnostic.
fn check_event_literal_stale(
    pattern_type: &str,
    prop_name: &str,
    event_name: &str,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let Some(pattern) = get_pattern(pattern_type) else {
        return;
    };
    let Some(prop_meta) = pattern.props_schema.get(prop_name) else {
        return;
    };
    let kind = prop_meta.kind.as_deref().unwrap_or("");
    let is_event_bearing = matches!(kind, "event" | "event-ref" | "event-listen" | "callback");
    if !is_event_bearing {
        return;
    }

    // Build the trait's full event set: every event the trait declares
    // as emit, listens for, or transitions on. A literal that matches
    // none of these is stale.
    let mut declared_events: std::collections::HashSet<&str> = std::collections::HashSet::new();
    if let Some(emits) = &bctx.trait_def.emits {
        for emit in emits {
            declared_events.insert(emit.event.as_str());
        }
    }
    if let Some(listens) = &bctx.trait_def.listens {
        for listen in listens {
            declared_events.insert(listen.event.as_str());
            if let Some(trigger) = &listen.triggers {
                declared_events.insert(trigger.as_str());
            }
        }
    }
    if let Some(events) = &bctx.trait_def.state_machine.events {
        for evt in events {
            declared_events.insert(evt.key.as_str());
        }
    }
    for tr in &bctx.trait_def.state_machine.transitions {
        declared_events.insert(tr.event.as_str());
    }

    if declared_events.is_empty() {
        // Vacuous compare — skip.
        return;
    }
    if !declared_events.contains(event_name) {
        diagnostics.push(
            ValidationError::error(
                ErrorCode::RenderUIEventLiteralStale,
                path,
                format!(
                    "Prop '{}' on '{}' references event '{}' but the trait's event set has no such key — literal is stale (likely missed by an `events: {{ OLD: NEW }}` override rewrite, or an authoring rename that didn't reach this call site)",
                    prop_name, pattern_type, event_name,
                ),
            )
            .with_suggestion(format!(
                "Either add '{}' to the trait's `emits` / `listens` / transition events, or rewrite the literal to match an existing event key",
                event_name
            )),
        );
    }
}

/// Lenient primitive compatibility for event-payload comparisons. Same
/// vocabulary as the L1.3/L2 emit_payload checks: `any` matches; date /
/// datetime / timestamp accept string|number; entity / relation accept
/// object|string; named entity types accept object.
fn primitive_event_compat(declared: &str, actual: &str) -> bool {
    let d = declared.trim_end_matches('!');
    let a = actual.trim_end_matches('!');
    if d == a {
        return true;
    }
    if (d == "array" || d.starts_with('[')) && (a == "array" || a.starts_with('[')) {
        return true;
    }
    match d {
        "any" => true,
        "date" | "datetime" | "timestamp" => a == "string" || a == "number",
        "entity" | "relation" => a == "object" || a == "string",
        s if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
            a == "object" || a == "string"
        }
        _ => false,
    }
}

/// Recurse into prop values that may carry nested render-ui patterns
/// (children arrays, content slots, items, etc.).
fn recurse_into_value(
    value: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    match value {
        SExpression::Object(_) => walk_pattern_props(value, bctx, path, diagnostics),
        SExpression::Array(arr) => {
            for item in arr {
                recurse_into_value(item, bctx, path, diagnostics);
            }
        }
        _ => {}
    }
}
