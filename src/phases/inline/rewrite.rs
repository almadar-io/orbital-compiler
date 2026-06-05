//! SExpression / payload identifier rewriting and config-binding substitution.

use crate::embedded::get_pattern;
use crate::phases::validation::sync::patterns::{PATTERN_EVENT_LIST_PROPS, PATTERN_EVENT_PROPS};
use orbital_core::schema::{
    ConfigProvenanceRecord, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

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
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            // Entity binding root rewrite: `@From` or `@From.path` → `@To` or
            // `@To.path`. Boundary-checked so `@FromOther` is left alone.
            for (from, to) in entity_subs {
                let prefix_with_at = format!("@{}", from);
                if let Some(rest) = s.strip_prefix(&prefix_with_at) {
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
                rewrite_identifiers(item, entity_subs, event_subs);
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
            let event_list_for_pattern: Option<&HashMap<&'static str, &'static str>> =
                pattern_type
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
                        .map(|kind| matches!(kind, "event" | "event-ref" | "event-listen" | "callback"))
                        .unwrap_or(false),
                    None => false,
                };
                if is_event_prop || event_props_for_pattern.is_some_and(|set| set.contains(k.as_str())) {
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
                        if let SExpression::Array(items) = v {
                            for item in items.iter_mut() {
                                if let SExpression::Object(entry) = item {
                                    if let Some(inner) = entry.get_mut(*field) {
                                        if let SExpression::String(s) = inner {
                                            if !s.starts_with('@') {
                                                if let Some(to) = event_subs.get(s) {
                                                    *s = to.clone();
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
                rewrite_identifiers(v, entity_subs, event_subs);
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
                let split_at = rest
                    .find(|c| c == '.' || c == '[')
                    .unwrap_or(rest.len());
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
/// Type-directed + deterministic — no operator-name heuristic: it fires only
/// for array-typed slots and only for string-headed array values (object- or
/// number-headed arrays are already unambiguous and pass through unchanged).
pub(crate) fn mark_literal_list_in_place(value: &mut SExpression, field_type: &str) {
    if !(field_type.starts_with('[') && field_type.ends_with(']')) {
        return;
    }
    if let SExpression::Array(elems) = value {
        let is_string_headed =
            matches!(elems.first(), Some(SExpression::String(s)) if s != "list");
        if is_string_headed {
            let mut wrapped = Vec::with_capacity(elems.len() + 1);
            wrapped.push(SExpression::String("list".to_string()));
            wrapped.append(elems);
            *value = SExpression::Array(wrapped);
        }
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
                            *expr = value.clone();
                            rewrite_config_bindings(expr, config_subs);
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
    let mut chars = s.chars().peekable();
    let mut buf = String::new();
    fn is_ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }
    while let Some(c) = chars.next() {
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
    // declarations and any internal trigger mapping.
    if let Some(listens) = trait_def.listens.as_mut() {
        for listen in listens.iter_mut() {
            if let Some(to) = event_subs.get(&listen.event) {
                listen.event = to.clone();
            }
            if let Some(triggers) = listen.triggers.as_mut() {
                if let Some(to) = event_subs.get(triggers) {
                    *triggers = to.clone();
                }
            }
        }
    }
}

/// Rewrite `@trait.<old>` string literals inside a trait's state machine to
/// `@trait.<new>` so a renamed sibling stays referenced.
pub(crate) fn rewrite_trait_literal_refs(td: &mut TraitDefinition, old: &str, new: &str) {
    let from = format!("@trait.{}", old);
    let to = format!("@trait.{}", new);
    for transition in td.state_machine.transitions.iter_mut() {
        if let Some(effects) = transition.effects.as_mut() {
            for eff in effects.iter_mut() {
                rewrite_sexpr_literal(eff, &from, &to);
            }
        }
        if let Some(guard) = transition.guard.as_mut() {
            rewrite_sexpr_literal(guard, &from, &to);
        }
    }
}

/// Replace every string-literal `from` with `to` everywhere in an SExpression.
fn rewrite_sexpr_literal(expr: &mut SExpression, from: &str, to: &str) {
    match expr {
        SExpression::String(s) => {
            if s == from {
                *s = to.to_string();
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_sexpr_literal(item, from, to);
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                rewrite_sexpr_literal(v, from, to);
            }
        }
        _ => {}
    }
}

/// Name of a trait reference regardless of kind (inline name or ref target).
pub(crate) fn trait_ref_name(tr: &TraitReference) -> Option<&str> {
    match tr {
        TraitReference::Inline(td) => Some(&td.name),
        other => other.trait_name(),
    }
}
