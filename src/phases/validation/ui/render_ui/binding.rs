//! L2.1 keystone: render-ui binding type-check.
//!
//! Independent pass run AFTER the literal-only `validate_pattern` check. Walks
//! every trait's render-ui props and resolves each binding against the
//! registry's declared prop types, plus the L2.2/L2.3 event-payload checks.

use super::node_fields::classify_prop_type;
use super::sync::patterns::get_prop_types;
use super::{ErrorCode, ValidationContext, ValidationError};
use crate::embedded::{get_pattern, EmbeddedPatternProp};
use crate::phases::validation::render_ui_binding::{
    resolve_array_element_type, resolve_binding_type, BindingContext, BindingType,
};
use crate::phases::validation::trait_path;
use orbital_core_base::schema::{
    resolve_payload_type_when, OrbitalSchema, PayloadField, SExpression, TraitDefinition,
    TraitReference,
};
use orbital_core_base::type_compat::{
    bound_shape_of_payload_field, record_prop_binding_mismatch, BoundShape, RecordBindingMismatch,
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
                            i,
                            j
                        );
                        check_render_ui_bindings_in_effect(effect, &bctx, &path, &mut diagnostics);
                        check_node_set_effects(effect, &bctx, &path, &mut diagnostics);
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

        // "object" accepts any object/entity binding, so the general compare
        // below is skipped for it. The one decidable violation is a collection
        // bound into a prop that admits no array (a single record, e.g.
        // `form-section.initialData`): it fails at runtime every time.
        if expected_types.contains(&"object") {
            if !expected_types.contains(&"array") {
                check_record_prop_binding(
                    pat,
                    prop_name,
                    prop_value,
                    expected_types,
                    bctx,
                    path,
                    diagnostics,
                );
            }
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
        if resolved == BindingType::Array && expected_types.contains(&"array") {
            check_array_element_kind(pat, prop_name, prop_value, bctx, path, diagnostics);
        }
        let resolved_str = resolved.registry_type();
        // `entity` resolves to "object" — the prop's declared types
        // accept that via the existing skip-on-object rule.

        let compatible = expected_types.iter().any(|t| prop_type_accepts(t, resolved_str));
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

/// An array bound into an array prop must carry elements of the kind the
/// prop's `items` declares (`[FieldSpec]` structs into `string[]` render object
/// cells). Compared only when both sides are resolved; anything else defers.
fn check_array_element_kind(
    pat: &str,
    prop_name: &str,
    prop_value: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let Some(item_types) = get_pattern(pat)
        .and_then(|p| p.props_schema.get(prop_name))
        .and_then(|m| m.items.as_deref())
        .map(|i| &i.types)
        .filter(|t| !t.is_empty())
    else {
        return;
    };
    const KNOWN: [&str; 4] = ["string", "number", "boolean", "object"];
    if !item_types.iter().all(|t| KNOWN.contains(&t.as_str())) {
        return;
    }
    let element = resolve_array_element_type(prop_value, bctx);
    if matches!(
        element,
        BindingType::Unknown | BindingType::TraitRef | BindingType::Array | BindingType::Null
    ) {
        return;
    }
    let element_str = element.registry_type();
    if item_types.iter().any(|t| t == element_str) {
        return;
    }
    diagnostics.push(
        ValidationError::error(
            ErrorCode::RenderUIBindingTypeMismatch,
            path,
            format!(
                "Prop '{}' on '{}' is bound to an array of '{}' elements but the registry expects an array of: {}",
                prop_name,
                pat,
                element_str,
                item_types.join(", ")
            ),
        )
        .with_suggestion(format!(
            "Bind '{}' to an array whose elements are: {}",
            prop_name,
            item_types.join(" or ")
        )),
    );
}

/// A prop the registry declares object-only takes ONE record: a collection
/// never fits it, and an `entity`-kind one renders the trait's linked entity,
/// so a row of another entity does not either. The rule is `orbital_core::
/// type_compat::record_prop_binding_mismatch`, shared with the lolo L1 check.
fn check_record_prop_binding(
    pat: &str,
    prop_name: &str,
    prop_value: &SExpression,
    expected_types: &[&str],
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    // A `@payload.X` read takes X's lowered declaration directly: the entity
    // lives in its own `entity` slot (`"object"` + `Staff`), which the
    // generic resolver folds away.
    let payload_field = match prop_value {
        SExpression::String(v) => v
            .strip_prefix("@payload.")
            .map(|rest| rest.split('.').next().unwrap_or(rest)),
        _ => None,
    };
    let bound = if let Some(field) = payload_field.filter(|f| !f.is_empty()) {
        let Some(declared) = bctx
            .event_payload
            .and_then(|p| p.iter().find(|d| d.name == field))
        else {
            return;
        };
        match bound_shape_of_payload_field(&declared.field_type, declared.entity.as_deref()) {
            Some(shape) => shape,
            None => return,
        }
    } else {
        match resolve_binding_type(prop_value, bctx) {
            BindingType::Array => BoundShape::Collection,
            BindingType::Entity(name) => BoundShape::Record(Some(name)),
            BindingType::Object => BoundShape::Record(None),
            BindingType::Unknown | BindingType::TraitRef => return,
            _ => BoundShape::Other,
        }
    };
    let entity_kind = get_pattern(pat)
        .and_then(|p| p.props_schema.get(prop_name))
        .and_then(|m| m.kind.as_deref())
        == Some("entity");
    let expected = if entity_kind {
        bctx.trait_def.linked_entity.as_deref()
    } else {
        None
    };
    let is_entity = |n: &str| bctx.entity_definitions.contains_key(n);
    let Some(mismatch) = record_prop_binding_mismatch(&bound, expected, is_entity) else {
        return;
    };
    let (message, suggestion) = match mismatch {
        RecordBindingMismatch::CollectionIntoRecord => (
            format!(
                "Prop '{}' on '{}' takes a single record ({}) but is bound to a collection",
                prop_name,
                pat,
                expected_types.join(", ")
            ),
            format!(
                "Bind '{}' to one row (a by-id fetch's result, or the selected row), not a list",
                prop_name
            ),
        ),
        RecordBindingMismatch::WrongEntity { expected, actual } => (
            format!(
                "Prop '{}' on '{}' renders a {} record but is bound to a {} row",
                prop_name, pat, expected, actual
            ),
            format!(
                "Bind '{}' to a {} row, or render the {} row from a trait linked to {}",
                prop_name, expected, actual, actual
            ),
        ),
    };
    diagnostics.push(
        ValidationError::error(ErrorCode::RenderUIBindingTypeMismatch, path, message)
            .with_suggestion(suggestion),
    );
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
    let declared_payload: Option<Vec<orbital_core_base::schema::PayloadField>> = bctx
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
                    // The component itself coerces the emitted value at
                    // runtime under some knob-conditional shape (e.g.
                    // `Input.tsx`'s `handleKeyDown` when `inputType ===
                    // 'number'`) — the registry's static declared type is
                    // just the unconditional default, not necessarily what
                    // THIS call site's resolved config actually sends.
                    // `required_field.type_when`, evaluated against this
                    // trait's own resolved config, is the declared escape
                    // hatch: a match means the component and the trait agree
                    // on the SAME resolved type, so the static mismatch below
                    // is excused. Zero knob-name knowledge here — the
                    // conditions are authored data on the component's JSDoc.
                    let type_when_covers_this_field = required_field
                        .type_when
                        .as_deref()
                        .and_then(|conds| {
                            resolve_payload_type_when(conds, bctx.trait_def.config.as_ref())
                        })
                        .is_some_and(|resolved_ty| resolved_ty == declared_field.field_type);
                    let derived = declared_payload_type_check(
                        required_field.kind.as_deref(),
                        required_field.payload_for.as_deref(),
                        &declared_field.field_type,
                    );
                    let compatible = match &derived {
                        Some((_, ok)) => *ok,
                        None => primitive_event_compat(
                            &declared_field.field_type,
                            &required_field.r#type,
                        ),
                    };
                    if !compatible && !type_when_covers_this_field {
                        diagnostics.push(
                            ValidationError::error(
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
                        .with_suggestion(match derive_typed_payload_decl(arg) {
                            Some(decl) => format!(
                                "Declare the typed payload on the trait's `emits {{ {} {{ ... }} }}`: `{} : {}`",
                                event_name, arg.name, decl,
                            ),
                            None => format!(
                                "Either rename '{}' to match a trait payload field, or add `{} : {}` to the trait's `emits {{ {} {{ ... }} }}` payload",
                                arg.name, arg.name, arg.r#type, event_name,
                            ),
                        }),
                    );
                }
                Some(declared_field) => {
                    if !primitive_event_compat(&declared_field.field_type, &arg.r#type) {
                        diagnostics.push(
                            ValidationError::error(
                                ErrorCode::RenderUIEventBindingPayloadMismatch,
                                path,
                                format!(
                                    "Callback prop '{}' on '{}' arg '{}' is type '{}' but trait declares payload field '{}' as '{}'",
                                    prop_name, pattern_type, arg.name, arg.r#type,
                                    declared_field.name, declared_field.field_type,
                                ),
                            ),
                        );
                    } else if arg.r#type == "object" {
                        // Gate B (type-integrity campaign): the registry has
                        // always carried the arg's nested shape — compare it
                        // structurally instead of accepting a shapeless
                        // `payload : object` on name+`object` parity alone.
                        check_callback_arg_schema(
                            arg,
                            declared_field,
                            prop_name,
                            pattern_type,
                            event_name,
                            path,
                            diagnostics,
                        );
                    }
                }
            }
        }
    }
}

/// Lenient primitive compatibility for event-payload comparisons. Same
/// vocabulary as the L1.3/L2 emit_payload checks: `any` matches; date /
/// datetime / instant / timestamp accept ISO text (string); entity / relation accept
/// object|string; named entity types accept object.
/// The `.lolo` type a registry payload member names directly, and whether `declared`
/// matches it: `event` for an `EventKey` member (`kind: "event"`; a declared plain
/// `string` is accepted too, an event being a string at the OIR boundary,
/// `FieldType::Event => OirType::String`), `@payload(.<sibling>)` for a member paired to
/// one by `payloadFor`. `None` when it names neither and the structural types decide.
fn declared_payload_type_check(
    kind: Option<&str>,
    payload_for: Option<&str>,
    declared: &str,
) -> Option<(String, bool)> {
    let declared = declared.trim_end_matches('!');
    if kind == Some("event") {
        return Some((
            "event".to_string(),
            declared == "event" || declared == "string",
        ));
    }
    payload_for.map(|sibling| {
        let expected = format!("@payload(.{sibling})");
        let matches = declared == expected;
        (expected, matches)
    })
}

fn primitive_event_compat(declared: &str, actual: &str) -> bool {
    let d = declared.trim_end_matches('!');
    let a = actual.trim_end_matches('!');
    if d == a {
        return true;
    }
    // A UNION declares a set of alternatives, so it is compatible with any
    // shape one of its variants accepts. The variants themselves are checked
    // where the union is built (`union_payload_variants` in orbital-lolo, which
    // shares the one union predicate); here the field only has to be allowed to
    // carry each of them. Without this a trait honestly declaring
    // `value : string | number | boolean | [string] | FileValue` was reported as
    // mismatching the very prop it was generated from.
    if d == "union" || a == "union" {
        return true;
    }
    if (d == "array" || d.starts_with('[')) && (a == "array" || a.starts_with('[')) {
        return true;
    }
    // A pattern declaring `@entity` means "the bound entity's ROW". By the time
    // the trait's own payload is compared it has already been MONOMORPHIZED —
    // the sentinel was replaced with that entity's shape, which serializes as
    // `object` (+ `properties`). Comparing the pattern's pre-resolution
    // sentinel against the trait's post-resolution shape is comparing two
    // stages of the same value, so accept the resolved forms. A row is also
    // legally carried as its id string at some call sites, matching the
    // `entity`/`relation` arm below.
    if d == "@entity" || a == "@entity" {
        let other = if d == "@entity" { a } else { d };
        return other == "@entity" || other == "object" || other == "json" || other == "string";
    }
    // A quoted program / expression: the registry kind `sexpr` is the `.lolo` `SExpr` type.
    if d == "SExpr" || a == "SExpr" {
        let other = if d == "SExpr" { a } else { d };
        return other == "sexpr" || other == "SExpr";
    }
    match d {
        "any" => true,
        // `json` and `object` are the same wire shape (Gate C separately
        // deprecates declaring them; compat must not double-punish).
        "json" | "object" => a == "object" || a == "json",
        "date" | "datetime" | "instant" | "timestamp" => a == "string",
        "entity" | "relation" => a == "object" || a == "string",
        s if orbital_core_base::type_compat::short_type_name(s).chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
            a == "object" || a == "string"
        }
        _ => false,
    }
}

/// Whether a prop typed `expected` in the registry takes a binding that resolved to `resolved_str`.
fn prop_type_accepts(expected: &str, resolved_str: &str) -> bool {
    expected == resolved_str
        || expected == "node"
        || expected == "string" && resolved_str == "trait"
        || (expected == "icon" && resolved_str == "string")
        || (expected == "asset" && resolved_str == "string")
        || (matches!(expected, "date" | "datetime" | "timestamp")
            && resolved_str == "string")
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

// ============================================================================
// Gate A (type-integrity campaign): node-typed entity fields
// ============================================================================

/// Is this entity field renderable content — `node`, `[node]`, or a container
/// whose ELEMENT type is renderable (`Map string node`, `Map string [node]`,
/// `Map string Map string [node]`)?
///
/// An art catalog keyed by unit kind and animation state (`kind -> state ->
/// draw items`) is renderable content that happens to be addressed by key. It
/// lowers to `Object`/`Array` carrying `items`, so recursing the `items` chain
/// recognizes it while leaving a shapeless `object` (no `items`) and a struct
/// (`properties`, no `items`) correctly outside — those stay the loophole this
/// gate exists to close.
fn is_node_field(field: &orbital_core_base::schema::FieldDefinition) -> bool {
    use orbital_core_base::schema::FieldType;
    match field.field_type {
        FieldType::Node => true,
        FieldType::Array | FieldType::Object => field.items.as_deref().is_some_and(is_node_field),
        _ => false,
    }
}

/// Peel the container levels the FIELD declares off `value`, returning the
/// renderable leaves to check. `node` yields the value itself; `[node]` yields
/// the array's elements; `Map string [node]` yields every element of every map
/// value; and so on down the `items` chain.
///
/// Driven by the declared type rather than by the value's shape, because the
/// two disagree exactly where it matters: a map's key object and a pattern
/// object are both `SExpression::Object`, and guessing picks the wrong one.
fn node_leaf_values<'a>(
    field: &orbital_core_base::schema::FieldDefinition,
    value: &'a SExpression,
) -> Vec<&'a SExpression> {
    use orbital_core_base::schema::FieldType;
    let Some(item) = field.items.as_deref() else {
        return vec![value];
    };
    let inner: Vec<&SExpression> = match (&field.field_type, value) {
        (FieldType::Array, SExpression::Array(items)) => items.iter().collect(),
        (FieldType::Object, SExpression::Object(map)) => {
            // Deterministic order — diagnostics must not depend on hash order.
            let mut keys: Vec<&orbital_core_base::value::Shared<String>> = map.keys().collect();
            keys.sort();
            keys.into_iter().filter_map(|k| map.get(k)).collect()
        }
        // The value does not match the declared container (a binding, a
        // computed expression). Hand it back whole; the caller's
        // `is_checkable_literal` gate already decided it was worth checking.
        _ => return vec![value],
    };
    inner
        .into_iter()
        .flat_map(|v| node_leaf_values(item, v))
        .collect()
}

/// Is this value a checkable literal (pattern object, or a literal array whose
/// first element is not an operator head)?
fn is_checkable_literal(value: &SExpression) -> bool {
    match value {
        SExpression::Object(_) => true,
        SExpression::Array(items) => !matches!(items.first(), Some(SExpression::String(_))),
        _ => false,
    }
}

/// Walk one transition effect for `(set @entity.<field> <value>)` calls at any
/// depth (through `let` / `do` / `if`), carrying the innermost `let` bindings
/// so classify-style `(let ((view [ … ])) … (set @entity.view @view))` resolves
/// to its literal and gets the FULL render-ui check family — closing the §19
/// blind spot (`C-RUI-CALLBACK-GATE-JSON-BLIND`).
pub(super) fn check_node_set_effects(
    effect: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let mut env: Vec<(String, SExpression)> = Vec::new();
    walk_for_node_sets(effect, bctx, path, &mut env, diagnostics);
}

fn walk_for_node_sets(
    expr: &SExpression,
    bctx: &BindingContext,
    path: &str,
    env: &mut Vec<(String, SExpression)>,
    diagnostics: &mut Vec<ValidationError>,
) {
    let SExpression::Array(items) = expr else {
        return;
    };
    let Some(SExpression::String(head)) = items.first() else {
        return;
    };
    match head.as_str() {
        "let" => {
            let depth = env.len();
            if let Some(SExpression::Array(bindings)) = items.get(1) {
                for b in bindings {
                    if let SExpression::Array(pair) = b {
                        if let (Some(SExpression::String(name)), Some(value)) =
                            (pair.first(), pair.get(1))
                        {
                            env.push((name.clone().into_inner(), value.clone()));
                        }
                    }
                }
            }
            for body in items.iter().skip(2) {
                walk_for_node_sets(body, bctx, path, env, diagnostics);
            }
            env.truncate(depth);
        }
        "do" | "if" => {
            for body in items.iter().skip(1) {
                walk_for_node_sets(body, bctx, path, env, diagnostics);
            }
        }
        "set" => {
            let Some(field_name) = items.get(1).and_then(|t| match t {
                SExpression::String(s) => s.strip_prefix("@entity."),
                _ => None,
            }) else {
                return;
            };
            let Some(value) = items.get(2) else { return };
            // Resolve one level of let-binding indirection.
            let resolved = match value {
                SExpression::String(s) if s.starts_with('@') && !s.starts_with("@entity.") => {
                    let name = s.trim_start_matches('@');
                    env.iter()
                        .rev()
                        .find(|(n, _)| n == name)
                        .map(|(_, v)| v)
                        .unwrap_or(value)
                }
                _ => value,
            };
            let field_def = bctx
                .trait_def
                .linked_entity
                .as_deref()
                .and_then(|e| bctx.entity_definitions.get(e))
                .and_then(|ed| {
                    ed.fields
                        .iter()
                        .find(|f| f.name.as_deref() == Some(field_name))
                });
            let node_typed = field_def.map(is_node_field).unwrap_or(false);

            let set_path = format!("{path}.set(@entity.{field_name})");
            if node_typed {
                // A computed value (not `is_checkable_literal`) has no
                // statically-checkable consequence: nothing here can prove it
                // wrong, so nothing is constructed for it. Keep checking the
                // literal case — a node field legally holds one pattern
                // object, a literal array of them, or — for `Map string node`
                // and friends — a keyed container of either.
                if is_checkable_literal(resolved) {
                    // Peel the declared container levels off the VALUE before
                    // checking, so neither an array root nor a map's key
                    // object is misread as a pattern.
                    let mut elements: Vec<&SExpression> = vec![resolved];
                    if let Some(def) = field_def {
                        elements = node_leaf_values(def, resolved);
                    }
                    for element in elements {
                        if matches!(element, SExpression::String(_) | SExpression::Null) {
                            // String leaves / null slots are legal node values.
                            continue;
                        }
                        walk_pattern_props(element, bctx, &set_path, diagnostics);
                    }
                }
            }
        }
        _ => {
            // Scan remaining call arguments — sets can hide under other
            // effect combinators.
            for body in items.iter().skip(1) {
                walk_for_node_sets(body, bctx, path, env, diagnostics);
            }
        }
    }
}

// ============================================================================
// Gate B (type-integrity campaign): structural callback-payload matching
// ============================================================================

/// Render a registry callback-arg schema as a `.lolo` payload type
/// declaration, e.g. `{ id : string, type : string, index : number! }` —
/// used in fix hints so the author never has to reach for `object`.
fn derive_typed_payload_decl(arg: &crate::embedded::EmbeddedCallbackArg) -> Option<String> {
    let schema = arg.schema.as_ref()?;
    let props = schema.properties.as_ref()?;
    if props.is_empty() {
        return None;
    }
    let required: Vec<String> = schema.required.as_names().to_vec();
    let mut parts: Vec<String> = props
        .iter()
        .map(|(name, p)| {
            let ty = p.types.first().map(String::as_str).unwrap_or("string");
            let bang = if required.iter().any(|r| r == name) {
                "!"
            } else {
                ""
            };
            format!("{name} : {ty}{bang}")
        })
        .collect();
    parts.sort();
    Some(format!("{{ {} }}", parts.join(", ")))
}

/// Structurally compare a callback arg's registry schema against the trait's
/// declared payload field. Fires the payload-mismatch code (blocking) when the
/// declared side is a shapeless `object` or is missing/mistyping a nested
/// field the component will actually send.
fn check_callback_arg_schema(
    arg: &crate::embedded::EmbeddedCallbackArg,
    declared_field: &PayloadField,
    prop_name: &str,
    pattern_type: &str,
    event_name: &str,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let Some(schema) = arg.schema.as_ref() else {
        return;
    };
    let Some(schema_props) = schema.properties.as_ref() else {
        return;
    };
    if schema_props.is_empty() {
        return;
    }

    let Some(declared_props) = declared_field.properties.as_ref() else {
        let hint = derive_typed_payload_decl(arg).unwrap_or_else(|| "{ ... }".to_string());
        diagnostics.push(
            ValidationError::error(
                ErrorCode::RenderUIEventBindingPayloadMismatch,
                path,
                format!(
                    "Callback prop '{}' on '{}' dispatches '{}' with a structured '{}' arg, but the trait declares `{} : object` with no shape — a shapeless payload escapes every downstream check",
                    prop_name, pattern_type, event_name, arg.name, declared_field.name,
                ),
            )
            .with_suggestion(format!(
                "Declare the shape the component actually sends: `{} : {}`",
                declared_field.name, hint,
            )),
        );
        return;
    };

    let required: Vec<String> = schema.required.as_names().to_vec();
    for (name, prop) in schema_props {
        let declared_nested = declared_props.iter().find(|p| &p.name == name);
        match declared_nested {
            None if required.iter().any(|r| r == name) => {
                diagnostics.push(
                    ValidationError::error(
                        ErrorCode::RenderUIEventBindingPayloadMismatch,
                        path,
                        format!(
                            "Callback prop '{}' on '{}' sends required nested field '{}.{}' for '{}', but the trait's declared payload shape has no '{}' field",
                            prop_name, pattern_type, arg.name, name, event_name, name,
                        ),
                    )
                    .with_suggestion(format!(
                        "Add `{} : {}` to the declared `{}` shape",
                        name,
                        prop.types.first().map(String::as_str).unwrap_or("string"),
                        declared_field.name,
                    )),
                );
            }
            None => {}
            Some(nested) => {
                // A struct member typed the `EventKey` semantic alias
                // (`FloatingToolbarItem.action`, `SwipeAction.event`) carries
                // `kind: "event"` in the registry (mirrors a top-level prop's
                // `kind`, but for a member with no parent `ExtractedProp` to
                // surface it on) — the trait honestly declares it `event`,
                // never the structural `string` `prop.types` alone would say.
                // Likewise `payloadFor` on an `eventPayloadBrand` member
                // means the trait declares the dependent `@payload(.<sibling>)`
                // type, not the registry's structural `object`. Both bypass
                // `prop.types` entirely — there is no primitive kind here to
                // union-match.
                let derived = declared_payload_type_check(
                    prop.kind.as_deref(),
                    prop.payload_for.as_deref(),
                    &nested.field_type,
                );
                let (expected, compatible): (String, bool) = if let Some(derived) = derived {
                    derived
                } else {
                    // A prop with several `types` is a UNION, and the declared
                    // field only has to match ONE of them — taking `.first()`
                    // alone made the check depend on member ORDER, so the same
                    // union passed or failed by however the extractor happened
                    // to sort it.
                    let expected = prop.types.first().map(String::as_str).unwrap_or("string");
                    let compatible = if prop.types.len() > 1 {
                        prop.types
                            .iter()
                            .any(|t| primitive_event_compat(&nested.field_type, t))
                    } else {
                        primitive_event_compat(&nested.field_type, expected)
                    };
                    (expected.to_string(), compatible)
                };
                if !compatible {
                    diagnostics.push(
                        ValidationError::error(
                            ErrorCode::RenderUIEventBindingPayloadMismatch,
                            path,
                            format!(
                                "Callback prop '{}' on '{}' sends '{}.{}' as '{}' but the trait declares it as '{}'",
                                prop_name, pattern_type, arg.name, name, expected, nested.field_type,
                            ),
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod array_into_record_tests {
    //! A collection (`[object]` payload field) bound into a prop the registry
    //! declares object-only (`form-section.initialData`, cardinality record)
    //! validated 0/0 and failed at runtime with "expected object, got array"
    //! (2026-09-30: ui-form-section / ui-form / ui-detail-panel).
    use super::*;
    use orbital_core_base::schema::{EntityDefinition, PayloadField, TraitDefinition};

    fn bare_trait() -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "FormSectionRender",
            "stateMachine": { "states": [], "transitions": [] }
        }))
        .expect("minimal trait")
    }

    /// `data : Staff` lowers to `"object"` + entity `Staff`; `data : [Staff]`
    /// to `"[object]"` + entity `Staff` — the fixture takes the source spelling.
    fn payload(source_type: &str) -> Vec<PayloadField> {
        let (field_type, entity) = match source_type {
            t if t.starts_with('[') && t[1..].starts_with(|c: char| c.is_ascii_uppercase()) => (
                "[object]".to_string(),
                Some(t.trim_matches(|c| c == '[' || c == ']').to_string()),
            ),
            t if t.starts_with(|c: char| c.is_ascii_uppercase()) => {
                ("object".to_string(), Some(t.to_string()))
            }
            t => (t.to_string(), None),
        };
        vec![PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "data".to_string(),
            field_type,
            required: Some(true),
            description: None,
            properties: None,
            type_when: None,
            entity,
            projected_from: None,
            items: None,
            relation: None,
        }]
    }

    fn pattern(ty: &str, prop: &str, value: &str) -> SExpression {
        SExpression::object(
            [
                ("type".into(), SExpression::String(ty.to_string().into())),
                (prop.into(), SExpression::String(value.to_string().into())),
            ]
            .into_iter()
            .collect(),
        )
    }

    fn mismatches(ty: &str, prop: &str, value: &str, data_type: &str) -> Vec<ValidationError> {
        let t = bare_trait();
        let entities = HashMap::new();
        let fields = payload(data_type);
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: Some(&fields),
            sibling_trait_names: &[],
        };
        let mut out = Vec::new();
        walk_pattern_props(&pattern(ty, prop, value), &ctx, "test", &mut out);
        out.into_iter()
            .filter(|e| e.code == ErrorCode::RenderUIBindingTypeMismatch)
            .collect()
    }

    /// A registry payload member typed `EventKey` (`kind: "event"`) or paired to one by
    /// `payloadFor` names its `.lolo` type directly; the structural `types` do not decide it.
    #[test]
    fn event_kind_member_expects_event_and_accepts_string() {
        assert_eq!(
            declared_payload_type_check(Some("event"), None, "event!"),
            Some(("event".to_string(), true))
        );
        assert_eq!(
            declared_payload_type_check(Some("event"), None, "string"),
            Some(("event".to_string(), true))
        );
    }

    #[test]
    fn control_event_kind_member_rejects_an_unrelated_type() {
        assert_eq!(
            declared_payload_type_check(Some("event"), None, "number"),
            Some(("event".to_string(), false))
        );
    }

    #[test]
    fn payload_for_member_expects_the_dependent_type() {
        assert_eq!(
            declared_payload_type_check(None, Some("event"), "@payload(.event)!"),
            Some(("@payload(.event)".to_string(), true))
        );
        assert_eq!(
            declared_payload_type_check(None, Some("event"), "object"),
            Some(("@payload(.event)".to_string(), false))
        );
    }

    #[test]
    fn edge_plain_member_is_left_to_the_structural_check() {
        assert_eq!(declared_payload_type_check(None, None, "string"), None);
    }

    /// An asset prop is a string brand (a URL) in the literal check; a string-typed binding
    /// (an `if` choosing between two URLs, an `image`/`url` field) is accepted the same way.
    #[test]
    fn string_binding_into_asset_prop_is_fine() {
        assert!(mismatches("header", "logoSrc", "@payload.data", "string").is_empty());
    }

    #[test]
    fn control_number_binding_into_asset_prop_is_an_error() {
        assert_eq!(
            mismatches("header", "logoSrc", "@payload.data", "number").len(),
            1
        );
    }

    #[test]
    fn collection_into_record_prop_is_an_error() {
        let errs = mismatches("form-section", "initialData", "@payload.data", "[object]");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].message.contains("initialData"));
    }

    #[test]
    fn collection_into_record_entity_prop_is_an_error() {
        let errs = mismatches("detail-panel", "entity", "@payload.data", "[object]");
        assert_eq!(errs.len(), 1, "{errs:?}");
    }

    #[test]
    fn control_record_into_record_prop_is_fine() {
        assert!(mismatches("form-section", "initialData", "@payload.data", "object").is_empty());
    }

    #[test]
    fn control_collection_into_object_or_array_prop_is_fine() {
        assert!(mismatches("data-grid", "entity", "@payload.data", "[object]").is_empty());
    }

    #[test]
    fn edge_unknown_payload_field_is_not_guessed() {
        assert!(mismatches(
            "form-section",
            "initialData",
            "@payload.missing",
            "[object]"
        )
        .is_empty());
    }

    fn entity(name: &str) -> EntityDefinition {
        serde_json::from_value(
            serde_json::json!({ "name": name, "fields": [{ "name": "id", "type": "string" }] }),
        )
        .expect("minimal entity")
    }

    fn entity_mismatches(prop: &str, data_type: &str, linked: &str) -> Vec<ValidationError> {
        let t: TraitDefinition = serde_json::from_value(serde_json::json!({
            "name": "BookingDetail",
            "linkedEntity": linked,
            "stateMachine": { "states": [], "transitions": [] }
        }))
        .expect("trait");
        let entities: HashMap<String, EntityDefinition> = ["Booking", "Staff"]
            .into_iter()
            .map(|n| (n.to_string(), entity(n)))
            .collect();
        let fields = payload(data_type);
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: Some(&fields),
            sibling_trait_names: &[],
        };
        let mut out = Vec::new();
        walk_pattern_props(
            &pattern("detail-panel", prop, "@payload.data"),
            &ctx,
            "test",
            &mut out,
        );
        out.into_iter()
            .filter(|e| e.code == ErrorCode::RenderUIBindingTypeMismatch)
            .collect()
    }

    #[test]
    fn a_row_of_another_entity_into_a_record_entity_prop_is_an_error() {
        let errs = entity_mismatches("entity", "Staff", "Booking");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].message.contains("Staff") && errs[0].message.contains("Booking"));
    }

    #[test]
    fn control_a_row_of_the_linked_entity_fits() {
        assert!(entity_mismatches("entity", "Booking", "Booking").is_empty());
    }

    #[test]
    fn edge_a_rebindable_trait_is_not_compared() {
        assert!(entity_mismatches("entity", "Staff", "@rebindable").is_empty());
    }
}

#[cfg(test)]
mod array_element_kind_tests {
    //! A `[FieldSpec]` config knob bound into `timeline.fields` (`string[]`)
    //! validated 0/0 and rendered object cells (G-ORB-029).
    use super::*;
    use orbital_core_base::schema::{EntityDefinition, TraitDefinition};

    fn trait_with_config(config: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Render",
            "config": config,
            "stateMachine": { "states": [], "transitions": [] }
        }))
        .expect("trait")
    }

    fn mismatches(t: &TraitDefinition, ty: &str, prop: &str, value: &str) -> Vec<ValidationError> {
        let entities: HashMap<String, EntityDefinition> = HashMap::new();
        let ctx = BindingContext {
            trait_def: t,
            entity_definitions: &entities,
            event_payload: None,
            sibling_trait_names: &[],
        };
        let pat = SExpression::object(
            [
                ("type".into(), SExpression::String(ty.to_string().into())),
                (prop.into(), SExpression::String(value.to_string().into())),
            ]
            .into_iter()
            .collect(),
        );
        let mut out = Vec::new();
        walk_pattern_props(&pat, &ctx, "test", &mut out);
        out.into_iter()
            .filter(|e| e.code == ErrorCode::RenderUIBindingTypeMismatch)
            .collect()
    }

    fn struct_knob() -> serde_json::Value {
        serde_json::json!({ "fields": {
            "type": "[FieldSpec]",
            "items": { "type": "object", "properties": { "label": { "type": "string" } } }
        }})
    }

    #[test]
    fn struct_array_knob_into_string_array_prop_is_an_error() {
        let t = trait_with_config(struct_knob());
        let errs = mismatches(&t, "timeline", "fields", "@config.fields");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].message.contains("object"), "{errs:?}");
    }

    #[test]
    fn control_string_array_knob_into_string_array_prop_is_fine() {
        let t = trait_with_config(serde_json::json!({ "fields": { "type": "[string]" } }));
        assert!(mismatches(&t, "timeline", "fields", "@config.fields").is_empty());
    }

    #[test]
    fn control_struct_array_knob_into_object_array_prop_is_fine() {
        let t = trait_with_config(struct_knob());
        assert!(mismatches(&t, "table-view", "columns", "@config.fields").is_empty());
    }

    #[test]
    fn control_string_array_knob_into_object_array_prop_is_an_error() {
        let t = trait_with_config(serde_json::json!({ "fields": { "type": "[string]" } }));
        assert_eq!(
            mismatches(&t, "table-view", "columns", "@config.fields").len(),
            1
        );
    }

    #[test]
    fn edge_unresolved_element_kind_defers() {
        let t = trait_with_config(serde_json::json!({ "fields": { "type": "[FieldSpec]" } }));
        assert!(mismatches(&t, "timeline", "fields", "@config.fields").is_empty());
        assert!(mismatches(&t, "timeline", "fields", "@config.missing").is_empty());
    }
}

#[cfg(test)]
mod sexpr_payload_compat_tests {
    //! A callback arg the registry types `sexpr` (a quoted program, e.g.
    //! `flow-canvas.onSchemaChange(schema)`) is the `.lolo` `SExpr` type.
    use super::primitive_event_compat;

    use super::prop_type_accepts;

    #[test]
    fn a_date_prop_takes_iso_text_and_never_a_number() {
        for prop in ["date", "datetime", "timestamp"] {
            assert!(prop_type_accepts(prop, "string"), "{prop} takes ISO text");
            assert!(!prop_type_accepts(prop, "number"), "{prop} refuses a number");
            assert!(!prop_type_accepts(prop, "boolean"), "{prop} refuses a boolean");
        }
        assert!(prop_type_accepts("number", "number"), "control: a number prop takes a number");
    }

    #[test]
    fn a_point_in_time_never_stands_for_a_number() {
        for point in ["date", "datetime", "instant", "timestamp"] {
            assert!(!primitive_event_compat("number", point), "number refuses {point}");
            assert!(primitive_event_compat(point, "string"), "{point} takes ISO text");
            assert!(!primitive_event_compat(point, "number"), "{point} refuses a number");
        }
        assert!(primitive_event_compat("number", "number"), "control");
        assert!(!primitive_event_compat("string", "number"), "control");
    }

    #[test]
    fn declared_sexpr_accepts_registry_sexpr() {
        assert!(primitive_event_compat("SExpr", "sexpr"));
        assert!(primitive_event_compat("SExpr!", "sexpr"));
    }

    #[test]
    fn control_sexpr_is_not_a_struct_and_struct_is_not_sexpr() {
        assert!(!primitive_event_compat("SExpr", "number"));
        assert!(!primitive_event_compat("Order", "sexpr"));
        assert!(!primitive_event_compat("string", "sexpr"));
    }
}
