//! SExpression / payload identifier rewriting and config-binding substitution.

use crate::embedded::get_pattern;
use crate::phases::validation::field_type_str;
use crate::phases::validation::sync::patterns::{PATTERN_EVENT_LIST_PROPS, PATTERN_EVENT_PROPS};
use orbital_core::schema::{
    ConfigProvenanceRecord, EntityDefinition, EntityRef, FieldDefinition, FieldType, ListenSource,
    Orbital, PayloadField, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Fold an `events:` rename into the `event`-TYPED MEMBERS of a config knob
/// value (SCAN-DETAIL-PANEL-ACTIONS-UNTAGGED-1).
///
/// The scalar twin of this rule already lives in `inline/trait.rs`: *a knob
/// declared `: event` holds an event-name REFERENCE, so an `events:` rename
/// must rewrite its settled value.* A member of a struct-array knob
/// (`actions : [RecordAction]` where `RecordAction.event` is `event`-typed) is
/// the same reference, and was never folded — the effect walk cannot reach it
/// because the render-ui node only forwards `actions: "@config.actions"` as a
/// STRING, and the literal array lives on the knob.
///
/// Type-directed, never name-directed: the member may be called anything, and
/// a member named `event` that is not declared `event`-typed is left alone.
/// Descends the same `items.properties` schema `validation/config_type.rs`
/// walks for `ORB_T_INVALID_EVENT_DEFAULT`, so both agree on what an
/// event-typed member is.
pub(crate) fn fold_event_typed_members(
    value: &mut SExpression,
    items: Option<&orbital_core::schema::FieldDefinition>,
    event_subs: &HashMap<String, String>,
) -> bool {
    let Some(schema) = items else {
        return false;
    };
    match value {
        SExpression::Array(entries) => {
            let mut touched = false;
            for entry in entries.iter_mut() {
                touched |= fold_event_typed_members(entry, Some(schema), event_subs);
            }
            touched
        }
        SExpression::Object(map) => {
            let Some(properties) = schema.properties.as_ref() else {
                return false;
            };
            let mut touched = false;
            for (member, decl) in properties.iter() {
                if decl.field_type == orbital_core::schema::FieldType::Event {
                    if let Some(SExpression::String(current)) = map.get_mut(member.as_str()) {
                        // `@`-bindings resolve elsewhere; "" is the
                        // default-off outlet.
                        if !current.is_empty() && !current.starts_with('@') {
                            if let Some(to) = event_subs.get(current.as_str()) {
                                *current = to.clone();
                                touched = true;
                            }
                        }
                    }
                    continue;
                }
                // A descriptor can hold descriptors.
                if decl.items.is_some() {
                    if let Some(nested) = map.get_mut(member.as_str()) {
                        touched |= fold_event_typed_members(nested, decl.items.as_deref(), event_subs);
                    }
                }
            }
            touched
        }
        _ => false,
    }
}

/// Phase F.3: Generic identifier-substitution walker for SExpressions.
///
/// Treats every imported orbital fragment as a function whose hardcoded
/// identifiers (entity names and event keys) are implicit parameters. The
/// caller-supplied substitution maps drive the rewrite.
///
/// Patterns recognized:
///
/// 1. **Entity ref list `["ref", "EntityName"]`** — when the array's first
///    element is the literal `"ref"` and the second is a string in
///    `entity_subs`, the second element is replaced with the substitution
///    target.
///
/// 2. **Entity binding root `@EntityName`** — when an `SExpression::String`
///    begins with `"@<from>"` AND the next character is `.` or end-of-string,
///    the binding root is rewritten to `@<to>` plus the trailing path. The
///    suffix-boundary check prevents matching `@EntityNameSomethingElse`.
///
/// 3. **Emit list `["emit", "EVENT_NAME", ...]`** — when the array's first
///    element is `"emit"` and the second is a string in `event_subs`, the
///    event name is rewritten. Matches both the bare 2-element form and
///    the 3-element form `["emit", "EVENT", payload]` where a payload
///    object trails. The walk falls through into the payload tail so any
///    nested `["ref", X]` inside the payload still gets entity-rewritten.
///
/// 4. **Fetch effect `["fetch", "EntityName", ...]`** (GAP-COMP-11) — when
///    the array's first element is `"fetch"` and position 1 is a string in
///    `entity_subs`, the entity-name slot is rewritten. Walk continues into
///    trailing elements (criteria, etc.) so nested refs are still visited.
///
/// 5. **Persist effect `["persist", action, "EntityName", ...]`** (GAP-COMP-11)
///    — for single-record actions (`create` | `update` | `delete`), position
///    2 is a bare entity name and is rewritten via `entity_subs`. For the
///    `batch` action, position 2 is a rows array (not a bare entity name),
///    so the rewrite is skipped in that case — the generic recursion still
///    walks the rows so any nested refs inside them are handled.
///
/// All other variants (`Null`, `Bool`, `Number`, unrelated `String`,
/// `Array`, `Object`) are walked recursively so nested references inside
/// `render-ui` props or compound effects are also rewritten.
///
/// Both maps may be empty; an empty map produces an identity walk (still
/// recurses, but no rewrites fire).
pub(crate) fn rewrite_identifiers(
    expr: &mut SExpression,
    entity_subs: &HashMap<String, String>,
    event_subs: &HashMap<String, String>,
) {
    // Hoist the `@<from>` prefix allocation out of the per-node recursion:
    // build each `@`-prefixed key once here instead of re-formatting it for
    // every string node visited during the walk.
    let entity_at_prefixes: Vec<(String, &str)> = entity_subs
        .iter()
        .map(|(from, to)| (format!("@{}", from), to.as_str()))
        .collect();
    rewrite_identifiers_inner(expr, entity_subs, event_subs, &entity_at_prefixes);
}

fn rewrite_identifiers_inner(
    expr: &mut SExpression,
    entity_subs: &HashMap<String, String>,
    event_subs: &HashMap<String, String>,
    entity_at_prefixes: &[(String, &str)],
) {
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            // Entity binding root rewrite: `@From` or `@From.path` → `@To` or
            // `@To.path`. Boundary-checked so `@FromOther` is left alone.
            for (prefix_with_at, to) in entity_at_prefixes {
                if let Some(rest) = s.strip_prefix(prefix_with_at.as_str()) {
                    if rest.is_empty() || rest.starts_with('.') {
                        *s = format!("@{}{}", to, rest);
                        return;
                    }
                }
            }
        }
        SExpression::Array(items) => {
            // Detect `["ref", "From"]` (entity rewrite). Strict 2-element
            // form so we don't accidentally rewrite unrelated 2-arg calls.
            if items.len() == 2 {
                if let (SExpression::String(op), SExpression::String(arg)) = (&items[0], &items[1])
                {
                    if op == "ref" {
                        if let Some(to) = entity_subs.get(arg) {
                            items[1] = SExpression::String(to.clone());
                            return;
                        }
                    }
                }
            }
            // Detect `["emit", "EVENT", ...]` and rewrite the event name in
            // position 1. The form is canonically 2 or 3 elements (`emit
            // EVENT` or `emit EVENT payload`); both must be rewritten when
            // the event is in `event_subs`. Falling through to the
            // recursive walk below ensures any nested `["ref", X]` inside
            // a payload tail still gets entity-rewritten.
            if items.len() >= 2 {
                if let (SExpression::String(op), SExpression::String(arg)) = (&items[0], &items[1])
                {
                    if op == "emit" {
                        if let Some(to) = event_subs.get(arg) {
                            items[1] = SExpression::String(to.clone());
                            // Don't return — let the recursive walk visit
                            // any payload tail so nested entity refs in
                            // `(emit EVENT { ref: [...] })` still rewrite.
                        }
                    }
                }
            }
            // GAP-COMP-11: Detect entity-name literals in `fetch` / `persist`
            // effect shapes and rewrite them via `entity_subs`. These shapes
            // embed the entity name as a bare string, not wrapped in `["ref",
            // X]`, so the generic ref-arm above does not reach them.
            //
            // Shapes:
            //   ["fetch", "EntityName", ...]           — position 1 is entity
            //   ["persist", "create", "EntityName", ...] — position 2 is entity
            //   ["persist", "update", "EntityName", ...] — position 2 is entity
            //   ["persist", "delete", "EntityName", ...] — position 2 is entity
            //   ["persist", "batch", rows_array, ...]  — position 2 is NOT an
            //                                             entity name (it's a
            //                                             rows array); skip.
            //
            // After rewriting the entity-slot literal, we fall through to the
            // generic recursive walk below so nested SExpressions in the
            // effect's tail (payloads, criteria, render-ui children, etc.)
            // are still visited.
            if let Some(SExpression::String(op)) = items.first() {
                match op.as_str() {
                    "fetch" => {
                        if let Some(SExpression::String(name)) = items.get_mut(1) {
                            if let Some(to) = entity_subs.get(name) {
                                *name = to.clone();
                            }
                        }
                    }
                    "persist" => {
                        // Only single-record actions carry a bare entity name
                        // at index 2. For `batch`, index 2 is a rows array —
                        // leave it for the generic recursive walk so nested
                        // literals inside the rows (unlikely but possible)
                        // still get visited.
                        let is_single_record = matches!(
                            items.get(1),
                            Some(SExpression::String(action))
                                if matches!(action.as_str(), "create" | "update" | "delete")
                        );
                        if is_single_record {
                            if let Some(SExpression::String(name)) = items.get_mut(2) {
                                if let Some(to) = entity_subs.get(name) {
                                    *name = to.clone();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            for item in items.iter_mut() {
                rewrite_identifiers_inner(item, entity_subs, event_subs, entity_at_prefixes);
            }
        }
        SExpression::Object(map) => {
            // Keys whose string value is an entity name that must be
            // rewritten when the enclosing trait is rebound to a new entity
            // via `linkedEntity:`. These correspond to render-ui pattern
            // props that address the entity store: `data-grid`/`data-list`
            // use `entity: "X"`; some patterns use `source` or
            // `entityType`; trait-ref objects use `linkedEntity`.
            // Without this rewrite, an imported atom's render-ui keeps
            // reading from the atom-era store key, so molecules built via
            // `@trait.X` + `linkedEntity:` show `No items` even when the
            // new entity's store is populated.
            const ENTITY_VALUED_PROPS: &[&str] =
                &["entity", "source", "entityType", "linkedEntity"];

            // Detect the pattern type at this render-ui node. When an object
            // has `type: "<pattern>"` and the pattern is in the registry,
            // rewrite event-valued props per `PATTERN_EVENT_PROPS` (direct
            // EventKey string) and `PATTERN_EVENT_LIST_PROPS` (array of
            // descriptor objects, each with a `<field>` holding an event
            // key). When no pattern type is present or the type is unknown,
            // the inline phase falls back to recursive walking without any
            // event-key rewriting — absence-of-schema means we can't prove
            // intent, so we don't guess.
            let pattern_type = match map.get("type") {
                Some(SExpression::String(ty)) => Some(ty.clone()),
                _ => None,
            };
            let event_props_for_pattern: Option<&HashSet<&'static str>> = pattern_type
                .as_deref()
                .and_then(|t| PATTERN_EVENT_PROPS.get(t));
            let event_list_for_pattern: Option<&HashMap<&'static str, &'static str>> = pattern_type
                .as_deref()
                .and_then(|t| PATTERN_EVENT_LIST_PROPS.get(t));

            // C1 plan: dynamic registry lookup as the canonical source
            // for "is this prop an event-name reference?" The static
            // PATTERN_EVENT_PROPS table covers `kind: "event"` (legacy
            // EventKey alias) but misses F1's new kinds (`event-ref`
            // for `EventEmit<P>`, `callback` for function-typed event
            // bindings). Consulting `get_pattern(...).props_schema`
            // directly picks up every kind pattern-sync writes, so
            // events-override rewrites stay in sync with whatever the
            // registry says is event-bearing today and tomorrow.
            let pattern_meta = pattern_type.as_deref().and_then(get_pattern);

            for (k, v) in map.iter_mut() {
                // 0) Effect emit-options map. fetch/persist/call-service/ref/
                //    os-watch/set effects carry `emit: {success|failure|
                //    on_message|on_change: "EVENT"}` (the slot-key universe of
                //    every `validate_emit_config` call site) and the string
                //    form `emit: "EVENT"`. A call-site `events {OLD: NEW}`
                //    rename that skips these leaves the atom's fetch success
                //    emit firing the pre-rename name →
                //    ORB_EFF_EMIT_UNDECLARED_EVENT per fetch site
                //    (C-EVENTS-RENAME-MISSES-FETCH-EMIT-MAP).
                if k == "emit" {
                    match v {
                        SExpression::String(ev) => {
                            if let Some(to) = event_subs.get(ev) {
                                *ev = to.clone();
                            }
                        }
                        SExpression::Object(emit_map) => {
                            for (slot, slot_v) in emit_map.iter_mut() {
                                if matches!(
                                    slot.as_str(),
                                    "success" | "failure" | "on_message" | "on_change"
                                ) {
                                    if let SExpression::String(ev) = slot_v {
                                        if let Some(to) = event_subs.get(ev) {
                                            *ev = to.clone();
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // 1) Direct event-valued prop (kind: "event" / "event-ref" /
                //    "event-listen" / "callback" in registry). Rewrite the
                //    string value via event_subs unless it's a binding
                //    placeholder (leading `@`), in which case the runtime
                //    resolves it at render time and we must not substitute
                //    here.
                let is_event_prop = match pattern_meta {
                    Some(p) => p
                        .props_schema
                        .get(k.as_str())
                        .and_then(|prop| prop.kind.as_deref())
                        .map(|kind| {
                            matches!(kind, "event" | "event-ref" | "event-listen" | "callback")
                        })
                        .unwrap_or(false),
                    None => false,
                };
                if is_event_prop
                    || event_props_for_pattern.is_some_and(|set| set.contains(k.as_str()))
                {
                    if let SExpression::String(s) = v {
                        if !s.starts_with('@') {
                            if let Some(to) = event_subs.get(s) {
                                *s = to.clone();
                                continue;
                            }
                        }
                    }
                }
                // 2) Event-list prop (kind: "event-list"). The value is an
                //    array; each item is a descriptor object with a field
                //    (default "event", override via registry.eventField)
                //    holding a declared event key. Rewrite per-item.
                if let Some(map_field) = event_list_for_pattern {
                    if let Some(field) = map_field.get(k.as_str()) {
                        // A conditional (`if`-wrapped) event-list value must
                        // have BOTH branches' descriptors renamed — the
                        // runtime picks a branch at render time, so a rename
                        // reaching only one branch would leave the other's
                        // descriptor pointing at the stale event name.
                        if matches!(v, SExpression::Array(_)) {
                            for leaf in v.conditional_leaf_values_mut() {
                                if let SExpression::Array(items) = leaf {
                                    for item in items.iter_mut() {
                                        if let SExpression::Object(entry) = item {
                                            if let Some(SExpression::String(s)) =
                                                entry.get_mut(*field)
                                            {
                                                if !s.starts_with('@') {
                                                    if let Some(to) = event_subs.get(s) {
                                                        *s = to.clone();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }
                if ENTITY_VALUED_PROPS.contains(&k.as_str()) {
                    if let SExpression::String(s) = v {
                        if let Some(to) = entity_subs.get(s) {
                            *s = to.clone();
                            continue;
                        }
                    }
                }
                rewrite_identifiers_inner(v, entity_subs, event_subs, entity_at_prefixes);
            }
        }
    }
}

/// Rewrite `@entity.<old>` reads to `@entity.<new>` per the call-site `fields`
/// remap map. Mirrors the entity-root rewrite in `rewrite_identifiers` but
/// targets the FIELD segment (the part after `@entity.`) rather than the entity
/// root. Boundary-checked on the field name so `@entity.titleSuffix` is left
/// alone when remapping `title`; nested paths (`@entity.title.x`) and array
/// indices (`@entity.title[0]`) are preserved. Empty map = identity walk.
pub(crate) fn rewrite_entity_field_paths(
    expr: &mut SExpression,
    field_subs: &HashMap<String, String>,
) {
    if field_subs.is_empty() {
        return;
    }
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@entity.") {
                // Split off the first segment (field name), keeping any nested
                // path / index suffix intact.
                let split_at = rest.find(['.', '[']).unwrap_or(rest.len());
                let (field, suffix) = rest.split_at(split_at);
                if let Some(to) = field_subs.get(field) {
                    *s = format!("@entity.{}{}", to, suffix);
                }
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_entity_field_paths(item, field_subs);
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                rewrite_entity_field_paths(v, field_subs);
            }
        }
    }
}

/// V4 leverage-ids reader: given a resolved inline trait and a rebind target
/// entity name, return the extra `stale-token-name → new_entity` entity
/// substitutions implied by the trait's `entity_ref_ids` side-map.
///
/// When the trait's bound entity declaration was RENAMED (its stable id is
/// unchanged but the `@entity`/`["ref", X]`/`fetch`/`persist` tokens in the
/// trait body kept the OLD name), the side-map ties that stale token name to
/// the bound entity's id (`linked_entity_id`). Matching every side-map entry
/// whose id equals the bound id recovers those stale names so the existing
/// `rewrite_identifiers` entity pass still rebinds them — the token no longer
/// has to physically match `linked_entity`.
///
/// ADDITIVE: returns an empty map when the side-map or `linked_entity_id` is
/// absent (old imports, id-free callers), leaving the name path — the caller's
/// `trait_def.linked_entity` → `new_entity` substitution — the sole driver, so
/// resolution is byte-identical to before wherever ids are not populated.
pub(crate) fn stale_entity_rebind_subs(
    trait_def: &TraitDefinition,
    new_entity: &str,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let (Some(bound_id), Some(map)) = (
        trait_def.linked_entity_id.as_deref(),
        trait_def.entity_ref_ids.as_ref(),
    ) {
        for (token_name, entity_id) in map {
            if entity_id == bound_id && token_name != new_entity {
                out.insert(token_name.clone(), new_entity.to_string());
            }
        }
    }
    out
}

/// Phase A.1 backwards-compatibility shim: rewrite a single entity name in
/// an SExpression. New code should call `rewrite_identifiers` directly with
/// a HashMap. This function is preserved for any callers that still depend
/// on the positional `(from, to)` API.
#[allow(dead_code)]
pub(crate) fn rewrite_entity_refs(expr: &mut SExpression, from_entity: &str, to_entity: &str) {
    let mut entity_subs = HashMap::new();
    entity_subs.insert(from_entity.to_string(), to_entity.to_string());
    rewrite_identifiers(expr, &entity_subs, &HashMap::new());
}

/// Disambiguate a literal list from an S-expression call.
///
/// A string-headed array bound for a literal-array config slot (declared
/// `field_type` like `[string]`) is structurally identical to an S-expression
/// call — e.g. `["path", "method", …]` is indistinguishable from the `path`
/// operator call `(path method …)`. When the head collides with a registered
/// operator name the runtime mis-evaluates the list and collapses it to a
/// scalar. Wrap such values in the `list` literal constructor (`["list", …]`),
/// which both evaluators return verbatim, so the value is unambiguously data.
///
/// A value that contains a binding anywhere in its tree (`@entity.X`,
/// `@config.X`, `@payload.X`, …) is NOT literal data — it is a live
/// expression the runtime must evaluate (e.g. a reactive `drawables` S-expr
/// composed into `ui-canvas`'s `[json]` slot). Wrapping those froze composed
/// canvases: the painter received the raw `["list", "array/concat", …]` tree
/// instead of evaluated descriptors. Skip the wrap when any string in the
/// tree starts with `@` — the runtime treats those strings as bindings
/// regardless of wrapping, so the value cannot be pure data.
///
/// Type-directed + deterministic — no operator-name heuristic: it fires only
/// for array-typed slots and only for string-headed array values (object- or
/// number-headed arrays are already unambiguous and pass through unchanged).
pub(crate) fn mark_literal_list_in_place(value: &mut SExpression, field_type: &str) {
    if !(field_type.starts_with('[') && field_type.ends_with(']')) {
        return;
    }
    let should_wrap = matches!(
        value,
        SExpression::Array(elems)
            if matches!(elems.first(), Some(SExpression::String(s)) if s != "list")
    ) && !sexpr_tree_contains_binding(value);
    if should_wrap {
        if let SExpression::Array(elems) = value {
            let mut wrapped = Vec::with_capacity(elems.len() + 1);
            wrapped.push(SExpression::String("list".to_string()));
            wrapped.append(elems);
            *value = SExpression::Array(wrapped);
        }
    }
}

/// Twin of `mark_literal_list_in_place`: literal readers (fetch `include`
/// validation + OIR resolution) must accept the `["list", …]` constructor the
/// wrap produces. The evaluators strip the head at eval time (ListOp); readers
/// that treat the array as inert data strip it here instead.
pub(crate) fn strip_literal_list_marker(arr: &[SExpression]) -> &[SExpression] {
    match arr.first() {
        Some(SExpression::String(s)) if s == "list" => &arr[1..],
        _ => arr,
    }
}

/// Deep check: does any leaf in the tree resolve as a binding (`@…`)? A
/// binding makes the value a live expression the evaluator must resolve, not
/// inert data. Leaf test routes to the canonical `SExpression::is_binding`
/// (the same `@`-prefix contract the evaluator keys on) — no local duplicate.
fn sexpr_tree_contains_binding(value: &SExpression) -> bool {
    if value.is_binding() {
        return true;
    }
    match value {
        SExpression::Array(items) => items.iter().any(sexpr_tree_contains_binding),
        SExpression::Object(map) => map.values().any(sexpr_tree_contains_binding),
        _ => false,
    }
}

/// Substitute every `@config.X` binding in an inline trait's transitions
/// (and ticks, if any) with the trait's own declared config-field default.
pub(crate) fn apply_inline_config_defaults(
    trait_def: &mut TraitDefinition,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    let Some(declared) = trait_def.config.as_ref() else {
        return;
    };
    let mut effective_config: HashMap<String, SExpression> = HashMap::new();
    for (k, field) in declared.iter() {
        if let Some(default) = field.default.as_ref() {
            let mut v = default.clone();
            mark_literal_list_in_place(&mut v, &field.field_type);
            effective_config.insert(k.clone(), v);
        }
    }
    if effective_config.is_empty() {
        return;
    }
    let cfg = &effective_config;
    record_config_provenance(trait_def, cfg, recorder);
    for transition in trait_def.state_machine.transitions.iter_mut() {
        if let Some(guard) = transition.guard.as_mut() {
            rewrite_config_bindings(guard, cfg);
        }
        if let Some(effects) = transition.effects.as_mut() {
            for effect in effects.iter_mut() {
                rewrite_config_bindings(effect, cfg);
            }
        }
    }
    if let Some(ticks) = trait_def.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite_config_bindings(guard, cfg);
            }
            for effect in tick.effects.iter_mut() {
                rewrite_config_bindings(effect, cfg);
            }
        }
    }
    // Standalone atom (no call site): `@config.<knob>` emit-name refs
    // resolve to the declared defaults collected above (Option B).
    super::r#trait::resolve_config_ref_emit_names(trait_def, cfg);
}

/// Phase 3.A: Substitute `@config.<param>` binding strings with concrete
/// SExpression values. All other SExpression variants recurse so nested
/// `@config.X` strings inside render-ui payloads, array literals, etc. are
/// rewritten in one walk.
pub(crate) fn rewrite_config_bindings(
    expr: &mut SExpression,
    config_subs: &HashMap<String, SExpression>,
) {
    if config_subs.is_empty() {
        return;
    }
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            // Only `@config.<param>` bindings are substituted. Bare `@config`
            // with no param is NOT a valid form (the binding validator
            // rejects it) so we don't handle it here.
            if let Some(rest) = s.strip_prefix("@config.") {
                // Split on the first `.` so `@config.foo` matches key "foo"
                // and `@config.foo.bar` matches key "foo" with trailing
                // path "bar".
                let (param, trailing) = match rest.find('.') {
                    Some(idx) => (&rest[..idx], Some(&rest[idx + 1..])),
                    None => (rest, None),
                };
                if let Some(value) = config_subs.get(param) {
                    match trailing {
                        None => {
                            // Bare `@config.<param>` — replace the whole
                            // node with the substituted value, then recurse
                            // into the substituted tree so any nested
                            // `@config.X` bindings inside the value (e.g.
                            // a `render-ui` default tree referencing
                            // `@config.cols`) are themselves resolved in
                            // this same pass.
                            let original = s.clone();
                            *expr = value.clone();
                            // Self-reference guard: a config default whose
                            // value is the identical `@config.<param>`
                            // binding (e.g. `startX: @config.startX`)
                            // would recurse forever. Config defaults must
                            // be concrete; stop and leave the binding so
                            // the validator can flag it instead of crashing.
                            let still_same =
                                matches!(expr, SExpression::String(ns) if *ns == *original);
                            if !still_same {
                                rewrite_config_bindings(expr, config_subs);
                            }
                        }
                        Some(path) => {
                            // `@config.<param>.<path>` — if the value is an
                            // object, walk the path and substitute the leaf.
                            // Otherwise leave the binding as-is so the
                            // validator can flag the shape mismatch.
                            if let SExpression::Object(_) = value {
                                if let Some(leaf) = resolve_object_path(value, path) {
                                    *expr = leaf.clone();
                                    rewrite_config_bindings(expr, config_subs);
                                }
                            }
                        }
                    }
                }
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_config_bindings(item, config_subs);
            }
        }
        SExpression::Object(map) => {
            for (_k, v) in map.iter_mut() {
                rewrite_config_bindings(v, config_subs);
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Config→knob provenance recording (Studio contextual-edit source map)
//
// Read-only companion to `rewrite_config_bindings`: BEFORE substitution erases
// the `@config.X` string, walk render-ui effects and record which render-ui
// prop (at which patternPath) was bound to which config knob. `inline_schema`
// emits the records into the schema's `_metadata.configProvenance`. Pure read —
// the proven substitution logic in `rewrite_config_bindings` is untouched.
// ----------------------------------------------------------------------------

/// Record provenance for every `@config.X` binding inside the trait's render-ui
/// effects, for knobs present in `cfg` (those about to be substituted).
pub(crate) fn record_config_provenance(
    trait_def: &TraitDefinition,
    cfg: &HashMap<String, SExpression>,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    let trait_name = trait_def.name.as_str();
    for transition in &trait_def.state_machine.transitions {
        if let Some(effects) = transition.effects.as_ref() {
            for effect in effects {
                record_render_ui_effect(effect, cfg, trait_name, recorder);
            }
        }
    }
    if let Some(ticks) = trait_def.ticks.as_ref() {
        for tick in ticks {
            for effect in &tick.effects {
                record_render_ui_effect(effect, cfg, trait_name, recorder);
            }
        }
    }
}

/// Detect a `["render-ui", slot, {pattern}]` effect and walk its pattern tree.
fn record_render_ui_effect(
    effect: &SExpression,
    cfg: &HashMap<String, SExpression>,
    trait_name: &str,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    if let SExpression::Array(items) = effect {
        if items.len() >= 3 {
            if let (SExpression::String(head), SExpression::Object(pattern)) =
                (&items[0], &items[2])
            {
                if head == "render-ui" {
                    record_pattern_node(pattern, cfg, trait_name, "root", recorder);
                }
            }
        }
    }
}

/// Walk a render-ui pattern node. `children` arrays descend as nested nodes
/// (path `<path>.children.<index>`, matching the runtime `UISlotRenderer`);
/// every other prop is scanned for `@config.X` bindings attributed to this
/// node + prop key.
fn record_pattern_node(
    pattern: &HashMap<String, SExpression>,
    cfg: &HashMap<String, SExpression>,
    trait_name: &str,
    path: &str,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    for (prop_key, value) in pattern.iter() {
        if prop_key == "children" {
            if let SExpression::Array(items) = value {
                for (idx, child) in items.iter().enumerate() {
                    if let SExpression::Object(child_obj) = child {
                        let child_path = format!("{}.children.{}", path, idx);
                        record_pattern_node(child_obj, cfg, trait_name, &child_path, recorder);
                    }
                }
            }
        } else {
            scan_config_in_value(value, cfg, trait_name, path, prop_key, recorder);
        }
    }
}

/// Record `{path, prop, knob}` for each `@config.<knob>` string found anywhere
/// under a node prop's value (including nested data arrays/objects), for knobs
/// present in `cfg`. Does not cross into child render-ui nodes (`children` is
/// walked separately by `record_pattern_node`).
fn scan_config_in_value(
    value: &SExpression,
    cfg: &HashMap<String, SExpression>,
    trait_name: &str,
    path: &str,
    prop_key: &str,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    match value {
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@config.") {
                let knob = rest.split('.').next().unwrap_or(rest);
                if cfg.contains_key(knob) {
                    recorder.push(ConfigProvenanceRecord {
                        trait_name: trait_name.to_string(),
                        pattern_path: Some(path.to_string()),
                        prop: prop_key.to_string(),
                        knob: knob.to_string(),
                    });
                }
            }
        }
        SExpression::Array(items) => {
            for item in items {
                scan_config_in_value(item, cfg, trait_name, path, prop_key, recorder);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                scan_config_in_value(v, cfg, trait_name, path, prop_key, recorder);
            }
        }
        _ => {}
    }
}

/// Walk a dotted path through an SExpression::Object tree. Returns the leaf
/// SExpression if the full path resolves to an object entry; returns None
/// when any intermediate segment is missing or non-object.
fn resolve_object_path<'a>(value: &'a SExpression, path: &str) -> Option<&'a SExpression> {
    let mut current = value;
    for segment in path.split('.') {
        match current {
            SExpression::Object(map) => {
                current = map.get(segment)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Rewrite whole-word entity-name occurrences inside a `PayloadField.field_type`
/// string. Field types are short type expressions like `Matter`, `[Matter]`,
/// `Matter?`, `{ data: [Matter], cursor: string }`. We substitute on word
/// boundaries so `MatterStub` (a fictitious sibling type) doesn't get mauled
/// into `MatterCaseStub` when the rebind is `Matter → MatterCase`.
fn rewrite_entity_in_type_string(s: &str, entity_subs: &HashMap<String, String>) -> String {
    if entity_subs.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut buf = String::new();
    fn is_ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }
    for c in s.chars() {
        if is_ident(c) {
            buf.push(c);
            continue;
        }
        if !buf.is_empty() {
            if let Some(to) = entity_subs.get(&buf) {
                out.push_str(to);
            } else {
                out.push_str(&buf);
            }
            buf.clear();
        }
        out.push(c);
    }
    if !buf.is_empty() {
        if let Some(to) = entity_subs.get(&buf) {
            out.push_str(to);
        } else {
            out.push_str(&buf);
        }
    }
    out
}

/// Apply entity-name substitutions to every `PayloadField.field_type` in the
/// trait's emits / listens / state-machine event declarations.
///
/// Without this, an organism that rebinds an atom's trait via
/// `linkedEntity: NewName` keeps the OLD type literal in the declared payload
/// schema (`data: [OldName]`), and downstream codegen produces
/// `payload as { data: OldName[] }` — a reference to a type that no longer
/// exists. Live symptom from std-legal-case: handlers.ts emitting
/// `payload as { data: Matter[] }` after the `Matter → MatterCase` rebind.
pub(crate) fn rewrite_payload_entity_refs(
    trait_def: &mut TraitDefinition,
    entity_subs: &HashMap<String, String>,
) {
    if entity_subs.is_empty() {
        return;
    }
    if let Some(emits) = trait_def.emits.as_mut() {
        for emit in emits.iter_mut() {
            if let Some(schema) = emit.payload_schema.as_mut() {
                for field in schema.iter_mut() {
                    field.field_type =
                        rewrite_entity_in_type_string(&field.field_type, entity_subs);
                }
            }
        }
    }
    if let Some(events) = trait_def.state_machine.events.as_mut() {
        for ev in events.iter_mut() {
            if let Some(schema) = ev.payload_schema.as_mut() {
                for field in schema.iter_mut() {
                    field.field_type =
                        rewrite_entity_in_type_string(&field.field_type, entity_subs);
                }
            }
        }
    }
}

/// Increment 3-I (`docs/Almadar_LOLO.md` ~line 315/1387): monomorphize the
/// `(E : Entity)` type parameter on an event alias — `type X (E : Entity) =
/// Event { row : E }` — to the using trait's bound entity.
///
/// `orbital-lolo` lowers an unbound param reference in a payload position to
/// the sentinel string `"$E"` (`PayloadField.field_type`, `properties:
/// None`) so the registry `.orb` stays generic (one atom, any entity). Once a
/// trait's FINAL `linked_entity` is known (post rebind, post sibling-pull),
/// every sentinel field in `emits[].payload_schema` and
/// `state_machine.events[].payload_schema` — recursing into nested
/// `properties` — is replaced with `field_type: "object"`, `properties:
/// Some(<entity's fields as PayloadFields>)`.
///
/// `entity_def` is the trait's resolved `linked_entity` looked up in the
/// composed schema; `None` when the trait has no linked entity to
/// substitute — in that case the sentinel is left in place for the
/// defensive sweep (`ORB_T_UNRESOLVED_TYPE_PARAM`) to catch, UNLESS Increment
/// 3-II data resolves it instead (see below). Returns `true` if any sentinel
/// field was rewritten (unused by callers today; kept for future
/// diagnostics/tests to assert on).
///
/// Increment 3-II extension: when this trait's cloned body carries its own
/// declared `type_params` (a `(V : Type, ...)` header on the SOURCE atom) —
/// distinct from a 3-I alias's own single param, which never appears in
/// `type_params` — each declared param resolves independently:
///   - an explicit call-site arg (`trait_def.type_args["V"]`, stashed by
///     `phases::inline::trait::inline_traits` when the reference was
///     resolved) — a PRIMITIVE arg name substitutes a scalar `field_type`;
///     an ENTITY arg name (found in `entities_by_name`) substitutes
///     `field_type: "object"` + flattened `properties`, same shape an
///     Entity-kind default produces;
///   - no arg, `kind == "Entity"` — falls back to `entity_def` (the 3-I
///     default, unchanged);
///   - no arg, `kind == "Type"` (or any other kind) — no substitution; the
///     sentinel survives for the sweep, and is NOT defaulted to `entity_def`
///     even though `entity_def` may be `Some` (the 3-I unconditional default
///     applies only to sentinels with NO declared param — see below).
/// A sentinel whose name is NOT in `type_params` at all is the pre-existing
/// 3-I alias case: unconditionally defaults to `entity_def` exactly as
/// before, byte-for-byte.
/// `trait_def.type_args` is consumed (taken) and cleared here — it is a
/// transient carrier and never appears in a final emitted `.orb`.
pub(crate) fn resolve_type_param_sentinels(
    trait_def: &mut TraitDefinition,
    entity_def: Option<&EntityDefinition>,
    entities_by_name: &HashMap<String, EntityDefinition>,
) -> bool {
    let type_args = trait_def.type_args.take();
    let declared: Vec<orbital_core::schema::TraitTypeParamDef> = trait_def
        .type_params
        .clone()
        .unwrap_or_default();
    let mut subs: HashMap<String, ParamSub> = HashMap::new();
    let mut declared_names: HashSet<String> = HashSet::new();
    for p in &declared {
        declared_names.insert(p.name.clone());
        let arg_value = type_args.as_ref().and_then(|m| m.get(&p.name));
        let sub = match arg_value {
            Some(v) => resolve_type_arg_value(v, entities_by_name),
            None if p.kind == "Entity" => entity_def.map(|e| ParamSub::Entity(e.clone())),
            None => None,
        };
        if let Some(sub) = sub {
            subs.insert(p.name.clone(), sub);
        }
    }
    let mut changed = false;
    if let Some(emits) = trait_def.emits.as_mut() {
        for emit in emits.iter_mut() {
            if let Some(schema) = emit.payload_schema.as_mut() {
                if flatten_bare_entity_payload(schema, entity_def, &subs) {
                    changed = true;
                } else {
                    changed |= resolve_sentinel_fields(schema, entity_def, &subs, &declared_names);
                }
            }
        }
    }
    if let Some(events) = trait_def.state_machine.events.as_mut() {
        for ev in events.iter_mut() {
            if let Some(schema) = ev.payload_schema.as_mut() {
                if flatten_bare_entity_payload(schema, entity_def, &subs) {
                    changed = true;
                } else {
                    changed |= resolve_sentinel_fields(schema, entity_def, &subs, &declared_names);
                }
            }
        }
    }
    changed
}

/// Canonicalize a BARE entity payload so the generic spelling produces the same
/// shape as the concrete one.
///
/// The payload invariant is that an event's declared type IS its access path: an
/// event declaring a row is read as `?field`, an event declaring `{ row : … }` is
/// read as `?row.field`. `orbital-lolo` honors that for a concrete bare alias —
/// `type X = Event Survey` lowers via `resolve_payload_type` to the entity's own
/// fields FLAT at the payload root. But the generic spellings (`Event @entity`,
/// `type X (E : Entity) = Event E`) cannot name an entity at lower time, so they
/// fall through to `type_expr_to_payload_fields`' primitive catch-all and arrive
/// here as a single synthetic field named `"value"` carrying the sentinel. If we
/// let the ordinary nested substitution run, that field becomes
/// `value: object` + properties — a wrapper the author never wrote, read as
/// `?value.field`. Same authored intent, two incompatible access paths.
///
/// So when the sole field IS the whole payload, splice the entity's fields in at
/// the root instead of wrapping — making `Event @entity` and `Event $E` mean
/// exactly what `Event Survey` already means.
///
/// Deliberately narrow:
/// - Only the synthetic `"value"` carrier qualifies. A field the author NAMED
///   (`{ row : @entity }`) is a real wrapper and keeps wrapping — the declared
///   type still describes its own access path.
/// - Array payloads (`Event [@entity]`, `Event [$E]`) keep wrapping: a list is
///   legitimately one value, and flattening rows into the root would be
///   meaningless.
/// - A `Scalar` substitution (`:: V string`) keeps wrapping — a primitive has no
///   fields to splice.
///
/// Returns `true` (and rewrites `fields`) when it flattened, `false` to leave
/// `fields` untouched for the ordinary nested resolution.
fn flatten_bare_entity_payload(
    fields: &mut Vec<PayloadField>,
    entity_def: Option<&EntityDefinition>,
    subs: &HashMap<String, ParamSub>,
) -> bool {
    let [field] = fields.as_slice() else {
        return false;
    };
    if field.name != BARE_PAYLOAD_CARRIER {
        return false;
    }
    let entity = match field.field_type.as_str() {
        // Still-unresolved sentinel: the common single-level case.
        "@entity" => entity_def,
        // An ALREADY-RESOLVED carrier. Reached when the atom is composed
        // through an intermediate atom: the inner level substituted the
        // sentinel and WRAPPED it (`value: object` + properties) before this
        // pass ever saw it, so the un-flattened payload used to survive to
        // codegen and every `?<field>` read on it was undefined. That is what
        // forced `std-version-history`'s `RevisionRolledBack` back to a
        // concrete entity (docs/Almadar_Std_Gaps.md, B8).
        //
        // Re-resolving against THIS level's entity is not just a repair, it is
        // the correct binding: for a `@rebindable` trait the payload must
        // reflect the FINAL bound entity, not whichever one an intermediate
        // level happened to carry.
        "object" if field.properties.is_some() => entity_def,
        ft => match ft.strip_prefix('$').and_then(|p| subs.get(p)) {
            Some(ParamSub::Entity(e)) => Some(e),
            _ => None,
        },
    };
    let Some(entity) = entity else {
        return false;
    };
    *fields = entity_fields_to_payload_fields(entity);
    true
}

/// The synthetic field name `orbital-lolo` gives a payload lowered from a BARE
/// type position — `type_expr_to_payload_fields`' catch-all arm in
/// `lower/emit_listen.rs`. Its presence is what distinguishes "this sentinel is
/// the entire payload" from "the author named a field that happens to hold a
/// row".
///
/// ⚠️ **Known residual ambiguity.** `value` is an authorable field name, so a
/// hand-written sole-field payload `type X = Event { value : SomeEntity }`
/// lowers to the same shape as the synthetic carrier and would be flattened
/// here — silently changing its access path from `?value.f` to `?f`. Verified
/// 2026-08-30 that the corpus contains **zero** such declarations
/// (`= Event { value : <Entity|@entity> }` matches nothing), so nothing is
/// mis-handled today, but the collision is real. The proper fix is to give the
/// synthetic carrier a name authors cannot write (the sentinel `$`-prefix
/// convention already used for type parameters); that changes `.orb` output for
/// every bare-primitive payload (`Event string`), so it is tracked rather than
/// bundled here. See `docs/Almadar_Std_Gaps.md` (B11).
const BARE_PAYLOAD_CARRIER: &str = "value";

/// A resolved Increment 3-II substitution for one declared type param.
enum ParamSub {
    /// A primitive-typed call-site arg (`:: V string`) — the sentinel field
    /// becomes a scalar of this type, no `properties`.
    Scalar(String),
    /// An entity-typed call-site arg (`:: E Booking`) or the Entity-kind
    /// no-arg default (the trait's own `linked_entity`) — the sentinel field
    /// becomes `field_type: "object"` + the entity's flattened fields.
    Entity(EntityDefinition),
}

/// The primitive type-name vocabulary a call-site type-parameter arg may
/// name (mirrors `orbital-lolo`'s `primitive_field_type` name set — the two
/// crates don't share code, `orbital-compiler` doesn't depend on
/// `orbital-lolo`, so this is a deliberate parallel list, not a duplicate of
/// a reachable one).
const PRIMITIVE_TYPE_ARG_NAMES: &[&str] = &[
    "string", "money", "any", "icon", "component", "email", "url", "phone", "uuid", "image",
    "node", "json", "render-ui", "relation", "int", "float", "number", "duration", "bool",
    "boolean", "date", "datetime", "timestamp", "object", "sexpr",
];

/// Resolve a call-site type-parameter argument's VALUE (the string the
/// author wrote after the param name, e.g. `"string"` in `:: V string`, or
/// `"Booking"` in `:: E Booking`) to a substitution. A primitive name is a
/// scalar; an entity name found in `entities_by_name` is an entity
/// substitution; anything else is unresolvable (the sentinel survives for
/// the sweep — the value named neither a primitive nor a known entity).
fn resolve_type_arg_value(
    value: &str,
    entities_by_name: &HashMap<String, EntityDefinition>,
) -> Option<ParamSub> {
    if PRIMITIVE_TYPE_ARG_NAMES.contains(&value) {
        return Some(ParamSub::Scalar(value.to_string()));
    }
    entities_by_name.get(value).map(|e| ParamSub::Entity(e.clone()))
}

/// Recursive worker for `resolve_type_param_sentinels`. A field whose
/// `field_type` is `"$<param>"` or `"[$<param>]"` is the sentinel; every
/// other field's `properties` (if any) is walked for a nested sentinel.
fn resolve_sentinel_fields(
    fields: &mut [PayloadField],
    entity_def: Option<&EntityDefinition>,
    subs: &HashMap<String, ParamSub>,
    declared_names: &HashSet<String>,
) -> bool {
    let mut changed = false;
    for field in fields.iter_mut() {
        // `@entity` — the bound-entity row SELF-reference. Semantically the
        // Entity-kind default without a parameter: substitute the trait's
        // linked entity, or leave for the sweep when there is none.
        let is_self_ref = field.field_type == "@entity" || field.field_type == "[@entity]";
        if is_self_ref {
            if let Some(entity) = entity_def {
                let is_array = field.field_type.starts_with('[');
                field.field_type = if is_array {
                    "[object]".to_string()
                } else {
                    "object".to_string()
                };
                field.properties = Some(entity_fields_to_payload_fields(entity));
                changed = true;
            }
            continue;
        }
        let is_array_sentinel = field.field_type.starts_with("[$") && field.field_type.ends_with(']');
        let is_bare_sentinel = !is_array_sentinel && field.field_type.starts_with('$');
        if is_bare_sentinel || is_array_sentinel {
            let param_name = if is_array_sentinel {
                field.field_type.trim_start_matches("[$").trim_end_matches(']')
            } else {
                field.field_type.trim_start_matches('$')
            };
            if let Some(sub) = subs.get(param_name) {
                match sub {
                    ParamSub::Scalar(prim) => {
                        field.field_type = if is_array_sentinel {
                            format!("[{prim}]")
                        } else {
                            prim.clone()
                        };
                        field.properties = None;
                    }
                    ParamSub::Entity(entity) => {
                        field.field_type = if is_array_sentinel {
                            "[object]".to_string()
                        } else {
                            "object".to_string()
                        };
                        field.properties = Some(entity_fields_to_payload_fields(entity));
                    }
                }
                changed = true;
                continue;
            }
            if declared_names.contains(param_name) {
                // A declared 3-II param with no resolvable substitution
                // (Type-kind with no arg, or an Entity-kind default that
                // itself had no entity_def) — leave the sentinel for the
                // sweep. Does NOT fall through to the unconditional 3-I
                // entity default below; that default is reserved for
                // sentinels with no matching declared param at all.
                continue;
            }
            // Pre-existing Increment 3-I alias sentinel (no declared trait
            // param of this name) — unconditional default to entity_def,
            // unchanged.
            if let Some(entity) = entity_def {
                field.field_type = if is_array_sentinel {
                    "[object]".to_string()
                } else {
                    "object".to_string()
                };
                field.properties = Some(entity_fields_to_payload_fields(entity));
                changed = true;
            }
            // No entity to substitute — leave the sentinel for the
            // end-of-inline defensive sweep to report as
            // ORB_T_UNRESOLVED_TYPE_PARAM.
            continue;
        }
        if let Some(props) = field.properties.as_mut() {
            changed |= resolve_sentinel_fields(props, entity_def, subs, declared_names);
        }
    }
    changed
}

/// Flatten an entity's field list into the `PayloadField` shape a
/// substituted sentinel carries. Mirrors the flat (non-relation) conversion
/// `orbital-lolo`'s `resolve_payload_type` performs at lower time — one
/// field per entity field, no recursion into relations.
/// `Map K V` on an entity field lowers to the `Object` tag with the VALUE shape
/// in `items` and no `properties` — a map has no properties by construction.
/// `PayloadField` carries no `items`, so serializing the enum alone DROPS the
/// value type and a fully-typed map arrives in payload position as a bare
/// `object` — which `generic_types` then reports as shapeless. Spell the map
/// into the type string instead, exactly as config fields already carry it
/// (`Map<string,string>`), so the one representation survives the crossing.
/// The bound-entity row as payload fields.
///
/// Delegates to the ONE converter (`sigil_types::payload_field_of`). This used
/// to be a second, independently-written copy that dropped every field's
/// `properties`, so an entity honestly declaring `heroSpawn : Position`
/// published `heroSpawn : object` on `VIEW.row` and the generic-type gate
/// reported the snapshot as untyped while the entity it snapshots was fully
/// typed. Two spellings of one rule is how they drift.
fn entity_fields_to_payload_fields(entity: &EntityDefinition) -> Vec<PayloadField> {
    entity
        .fields
        .iter()
        .map(super::sigil_types::payload_field_of)
        .collect()
}

/// Run [`resolve_type_param_sentinels`] for every inlined trait in one
/// orbital, looking up each trait's FINAL `linked_entity` against the
/// orbital's own (post-inline) entity set — the primary `entity` plus
/// `auxiliary_entities` (imported atom entities surfaced by the inline
/// phase). Called once per orbital, after `inline_orbital` fully resolves
/// it — see `inline_schema_with_config_diagnostics`.
pub(crate) fn resolve_orbital_type_param_sentinels(orbital: &mut Orbital) {
    let mut entities: HashMap<String, EntityDefinition> = HashMap::new();
    if let EntityRef::Inline(def) = &orbital.entity {
        entities.insert(def.name.clone(), def.clone());
    }
    if let Some(aux) = &orbital.auxiliary_entities {
        for e in aux {
            if let EntityRef::Inline(def) = e {
                entities.insert(def.name.clone(), def.clone());
            }
        }
    }
    for tr in orbital.traits.iter_mut() {
        if let TraitReference::Inline(trait_def) = tr {
            let entity_def = trait_def
                .linked_entity
                .as_deref()
                .and_then(|name| entities.get(name));
            resolve_type_param_sentinels(trait_def, entity_def, &entities);
        }
    }
}

/// Apply event-name substitutions to all structured event-key fields of a
/// `TraitDefinition`. These fields live OUTSIDE SExpressions and so are not
/// reached by `rewrite_identifiers`.
pub(crate) fn rewrite_trait_event_fields(
    trait_def: &mut TraitDefinition,
    event_subs: &HashMap<String, String>,
) {
    if event_subs.is_empty() {
        return;
    }

    // StateMachine.events[*].key — the canonical declaration set.
    if let Some(events) = trait_def.state_machine.events.as_mut() {
        for ev in events.iter_mut() {
            if let Some(to) = event_subs.get(&ev.key) {
                ev.key = to.clone();
            }
        }
    }

    // StateMachine.transitions[*].event — the trigger field.
    for transition in trait_def.state_machine.transitions.iter_mut() {
        if let Some(to) = event_subs.get(&transition.event) {
            transition.event = to.clone();
        }
    }

    // emits[*].event — the trait's outbound event declarations.
    if let Some(emits) = trait_def.emits.as_mut() {
        for emit in emits.iter_mut() {
            if let Some(to) = event_subs.get(&emit.event) {
                emit.event = to.clone();
            }
        }
    }

    // listens[*].event and listens[*].triggers — the trait's inbound event
    // declarations and any internal trigger mapping. A sourced listen's
    // `event` names an event in the SOURCE trait's vocabulary, not this
    // trait's, so the call-site rename map covers only the trigger side;
    // the event side follows the source trait's own renames via
    // `event_renames_by_trait` in `inline_traits`.
    if let Some(listens) = trait_def.listens.as_mut() {
        for listen in listens.iter_mut() {
            let sourced = matches!(
                listen.source,
                Some(ListenSource::Trait { .. }) | Some(ListenSource::Orbital { .. })
            );
            if !sourced {
                if let Some(to) = event_subs.get(&listen.event) {
                    listen.event = to.clone();
                }
            }
            if let Some(triggers) = listen.triggers.as_mut() {
                if let Some(to) = event_subs.get(triggers) {
                    *triggers = to.clone();
                }
            }
        }
    }
}

/// Name of a trait reference regardless of kind (inline name or ref target).
pub(crate) fn trait_ref_name(tr: &TraitReference) -> Option<&str> {
    match tr {
        TraitReference::Inline(td) => Some(&td.name),
        other => other.trait_name(),
    }
}

#[cfg(test)]
mod literal_list_tests {
    use super::*;
    use std::collections::HashMap;

    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items)
    }

    /// The strip twin must undo exactly what the wrap adds — a literal reader
    /// fed `["list","tag"]` sees `["tag"]`; unwrapped arrays pass through.
    #[test]
    fn strip_marker_is_the_wrap_inverse() {
        let wrapped = vec![
            SExpression::String("list".to_string()),
            SExpression::String("tag".to_string()),
        ];
        let stripped = strip_literal_list_marker(&wrapped);
        assert_eq!(stripped, &wrapped[1..]);
        let bare = vec![SExpression::String("tag".to_string())];
        assert_eq!(strip_literal_list_marker(&bare), &bare[..]);
    }

    #[test]
    fn wraps_pure_literal_string_headed_list() {
        let mut v = arr(vec![
            SExpression::String("path".to_string()),
            SExpression::String("method".to_string()),
        ]);
        mark_literal_list_in_place(&mut v, "[string]");
        match &v {
            SExpression::Array(items) => {
                assert_eq!(items[0], SExpression::String("list".to_string()));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn skips_wrap_when_tree_contains_binding() {
        // A reactive drawables S-expr composed into a `[json]` slot: the head
        // is string-headed but the tree carries `@entity` bindings — it must
        // reach the runtime as a call, not as inert data.
        let mut food = HashMap::new();
        food.insert(
            "x".to_string(),
            arr(vec![
                SExpression::String("object/get".to_string()),
                SExpression::String("@entity.food".to_string()),
                SExpression::String("x".to_string()),
            ]),
        );
        let mut cell = HashMap::new();
        cell.insert("type".to_string(), SExpression::String("draw-shape".to_string()));
        cell.insert("position".to_string(), SExpression::Object(food));
        let mut v = arr(vec![
            SExpression::String("array/concat".to_string()),
            arr(vec![SExpression::Object(cell)]),
            arr(vec![]),
        ]);
        mark_literal_list_in_place(&mut v, "[json]");
        match &v {
            SExpression::Array(items) => {
                assert_eq!(items[0], SExpression::String("array/concat".to_string()));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn ignores_non_array_slots_and_non_string_heads() {
        let mut scalar = arr(vec![SExpression::String("path".to_string())]);
        mark_literal_list_in_place(&mut scalar, "string");
        assert_eq!(
            scalar,
            arr(vec![SExpression::String("path".to_string())])
        );
        let mut object_headed = arr(vec![SExpression::Object(HashMap::new())]);
        mark_literal_list_in_place(&mut object_headed, "[json]");
        match &object_headed {
            SExpression::Array(items) => assert_eq!(items.len(), 1),
            _ => panic!("expected array"),
        }
    }
}

#[cfg(test)]
mod conditional_event_list_tests {
    use super::*;

    fn descriptor(event: &str) -> SExpression {
        let mut dm = HashMap::new();
        dm.insert("name".to_string(), SExpression::String("Edit".to_string()));
        dm.insert("event".to_string(), SExpression::String(event.to_string()));
        SExpression::Object(dm)
    }

    fn data_list_with_item_actions(item_actions: SExpression) -> SExpression {
        let mut node = HashMap::new();
        node.insert("type".to_string(), SExpression::String("data-list".to_string()));
        node.insert("itemActions".to_string(), item_actions);
        SExpression::Object(node)
    }

    /// A call-site `events { EDIT_ITEM: EditRow }` rename must reach the
    /// descriptor's `event` field inside BOTH branches of a conditional
    /// (`if`-wrapped) `itemActions` config — not just a literal list.
    #[test]
    fn rename_reaches_descriptors_in_both_conditional_branches() {
        let item_actions = SExpression::Array(vec![
            SExpression::String("if".to_string()),
            SExpression::String("@user.role".to_string()),
            SExpression::Array(vec![descriptor("EDIT_ITEM")]),
            SExpression::Array(vec![descriptor("VIEW_ITEM")]),
        ]);
        let mut expr = data_list_with_item_actions(item_actions);

        let entity_subs = HashMap::new();
        let mut event_subs = HashMap::new();
        event_subs.insert("EDIT_ITEM".to_string(), "EditRow".to_string());
        event_subs.insert("VIEW_ITEM".to_string(), "ViewRow".to_string());
        rewrite_identifiers(&mut expr, &entity_subs, &event_subs);

        let SExpression::Object(node) = &expr else {
            panic!("expected object");
        };
        let SExpression::Array(cond) = &node["itemActions"] else {
            panic!("expected the conditional array to survive the rewrite intact");
        };
        assert_eq!(cond[0], SExpression::String("if".to_string()));
        let then_event = match &cond[2] {
            SExpression::Array(items) => match &items[0] {
                SExpression::Object(dm) => dm.get("event").cloned(),
                _ => None,
            },
            _ => None,
        };
        let else_event = match &cond[3] {
            SExpression::Array(items) => match &items[0] {
                SExpression::Object(dm) => dm.get("event").cloned(),
                _ => None,
            },
            _ => None,
        };
        assert_eq!(then_event, Some(SExpression::String("EditRow".to_string())));
        assert_eq!(else_event, Some(SExpression::String("ViewRow".to_string())));
    }

    /// Literal (non-conditional) `itemActions` rename is unaffected — the
    /// conditional-aware walk must be byte-identical to the old direct-array
    /// path for the common case.
    #[test]
    fn rename_still_reaches_literal_item_actions_list() {
        let item_actions = SExpression::Array(vec![descriptor("EDIT_ITEM")]);
        let mut expr = data_list_with_item_actions(item_actions);

        let entity_subs = HashMap::new();
        let mut event_subs = HashMap::new();
        event_subs.insert("EDIT_ITEM".to_string(), "EditRow".to_string());
        rewrite_identifiers(&mut expr, &entity_subs, &event_subs);

        let SExpression::Object(node) = &expr else {
            panic!("expected object");
        };
        let SExpression::Array(items) = &node["itemActions"] else {
            panic!("expected array");
        };
        let SExpression::Object(dm) = &items[0] else {
            panic!("expected descriptor object");
        };
        assert_eq!(dm["event"], SExpression::String("EditRow".to_string()));
    }
}
