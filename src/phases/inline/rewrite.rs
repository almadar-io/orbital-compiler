//! SExpression / payload identifier rewriting and config-binding substitution.

use orbital_core_eval::evaluator::{parse_iso, IsoShape};
use super::InlineError;
use std::sync::Arc;
use crate::embedded::get_pattern;
use crate::phases::validation::effect::persist_entity_index;
use crate::phases::validation::sync::patterns::{PATTERN_EVENT_LIST_PROPS, PATTERN_EVENT_PROPS};
use crate::phases::validation::user_identity::{user_field_of, EQUALITY_OPS};
use crate::phases::validation::value_walk::all_sources;
use orbital_core_base::schema::{
    compose_nullability, ConfigField, ConfigProvenanceRecord, EntityDefinition, EntityRef,
    FieldDefinition, FieldType, ListenSource, Orbital, PayloadField, SExpression, TraitDefinition,
    TraitReference,
};
use std::collections::{BTreeMap, HashMap, HashSet};

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
    items: Option<&orbital_core_base::schema::FieldDefinition>,
    event_subs: &orbital_core_base::collections::IndexMap<String, String>,
) -> bool {
    map_event_typed_members(value, items, None, &mut |current| {
        event_subs.get(current).cloned()
    })
}

/// Apply `f` to every literal value at an `event`-typed member of a config
/// knob value; `f` returns the replacement, or `None` to keep it. `@`-bindings
/// resolve elsewhere and `""` is the default-off outlet, so neither reaches `f`.
pub(crate) fn map_event_typed_members(
    value: &mut SExpression,
    items: Option<&orbital_core_base::schema::FieldDefinition>,
    properties: Option<&orbital_core_base::collections::IndexMap<String, FieldDefinition>>,
    f: &mut dyn FnMut(&str) -> Option<String>,
) -> bool {
    let properties = items
        .and_then(|schema| schema.properties.as_ref())
        .or(properties);
    match value {
        SExpression::Array(entries) => {
            let mut touched = false;
            for entry in entries.iter_mut() {
                touched |= map_event_typed_members(entry, items, properties, f);
            }
            touched
        }
        SExpression::Object(map) => {
            let Some(properties) = properties else {
                return false;
            };
            // A union's `properties` are its VARIANTS; each element carries one
            // variant's members, so walk every variant's members, each key once.
            if items.is_some_and(|schema| {
                schema.field_type == orbital_core_base::schema::FieldType::Union
            }) {
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut touched = false;
                for variant in properties.values() {
                    let Some(members) = variant.properties.as_ref() else {
                        continue;
                    };
                    for (member, decl) in members.iter() {
                        if !is_event_typed(&decl.field_type) || !seen.insert(member.clone()) {
                            continue;
                        }
                        if let Some(SExpression::String(current)) = map.get_mut(member.as_str()) {
                            if !current.is_empty() && !current.starts_with('@') {
                                if let Some(to) = f(current.as_str()) {
                                    *current = to.into();
                                    touched = true;
                                }
                            }
                        }
                    }
                }
                return touched;
            }
            let mut touched = false;
            for (member, decl) in properties.iter() {
                if is_event_typed(&decl.field_type) {
                    if let Some(SExpression::String(current)) = map.get_mut(member.as_str()) {
                        if !current.is_empty() && !current.starts_with('@') {
                            if let Some(to) = f(current.as_str()) {
                                *current = to.into();
                                touched = true;
                            }
                        }
                    }
                    continue;
                }
                // A descriptor can hold descriptors.
                if decl.items.is_some() || decl.properties.is_some() {
                    if let Some(nested) = map.get_mut(member.as_str()) {
                        touched |= map_event_typed_members(
                            nested,
                            decl.items.as_deref(),
                            decl.properties.as_ref(),
                            f,
                        );
                    }
                }
            }
            touched
        }
        _ => false,
    }
}

fn is_event_typed(ft: &orbital_core_base::schema::FieldType) -> bool {
    matches!(
        ft,
        orbital_core_base::schema::FieldType::Event
            | orbital_core_base::schema::FieldType::EventAddress
    )
}

fn is_event_type_name(name: &str) -> bool {
    name == "event" || name == "EventAddress"
}

/// Apply `f` to every `listens`-addressed event value (`Trait.EVENT`,
/// `Orbital.Trait.EVENT`) a trait's effects emit at an event-typed position of
/// the emitted event's declared payload schema. Type-directed: a string that
/// merely spells an address at a `string` position is left alone.
pub(crate) fn map_effect_event_addresses(td: &mut TraitDefinition, f: &mut AddressMap<'_>) {
    use orbital_core_base::schema::{join_event_address, split_event_address};
    let Some(emits) = td.emits.as_ref() else {
        return;
    };
    let schemas: HashMap<String, Vec<PayloadField>> = emits
        .iter()
        .filter_map(|e| Some((e.event.clone(), e.payload_schema.clone()?)))
        .collect();
    if schemas.is_empty() {
        return;
    }
    let mut on_value = |current: &str| -> Option<String> {
        let (event, Some(source)) = split_event_address(current) else {
            return None;
        };
        if matches!(source, ListenSource::Any) {
            return None;
        }
        let (event, source) = f(&event, &source)?;
        Some(join_event_address(&event, Some(&source)))
    };
    for transition in td.state_machine.transitions.iter_mut() {
        for effect in transition.effects.iter_mut().flatten() {
            map_emitted_addresses(effect, &schemas, &mut on_value);
        }
    }
    for tick in td.ticks.iter_mut().flatten() {
        for effect in tick.effects.iter_mut() {
            map_emitted_addresses(effect, &schemas, &mut on_value);
        }
    }
}

fn map_emitted_addresses(
    expr: &mut SExpression,
    schemas: &HashMap<String, Vec<PayloadField>>,
    on_value: &mut dyn FnMut(&str) -> Option<String>,
) {
    let SExpression::Array(items) = expr else {
        return;
    };
    if let (Some(SExpression::String(head)), Some(SExpression::String(event))) =
        (items.first(), items.get(1))
    {
        if head.as_str() == "emit" {
            if let (Some(fields), Some(payload)) = (schemas.get(event.as_str()), items.get_mut(2)) {
                map_payload_addresses(payload, fields, on_value);
            }
        }
    }
    for item in items.iter_mut() {
        map_emitted_addresses(item, schemas, on_value);
    }
}

fn map_payload_addresses(
    value: &mut SExpression,
    fields: &[PayloadField],
    on_value: &mut dyn FnMut(&str) -> Option<String>,
) {
    match value {
        SExpression::Array(entries) => {
            for entry in entries.iter_mut() {
                map_payload_addresses(entry, fields, on_value);
            }
        }
        SExpression::Object(map) => {
            for field in fields {
                let Some(member) = map.get_mut(field.name.as_str()) else {
                    continue;
                };
                if is_event_type_name(&field.field_type) {
                    if let SExpression::String(current) = member {
                        if !current.is_empty() && !current.starts_with('@') {
                            if let Some(to) = on_value(current.as_str()) {
                                *current = to.into();
                            }
                        }
                    }
                } else if let Some(nested) = field.properties.as_deref() {
                    map_payload_addresses(member, nested, on_value);
                }
            }
        }
        _ => {}
    }
}

/// Maps a split event address to its replacement event and source, or `None` to keep it.
pub(crate) type AddressMap<'a> =
    dyn FnMut(&str, &ListenSource) -> Option<(String, ListenSource)> + 'a;

/// Apply `f` to every `listens`-addressed event value (`Trait.EVENT`,
/// `Orbital.Trait.EVENT`) in a trait's config — scalar `event` knobs and
/// `event`-typed struct members alike. `f` gets the split address and returns
/// the replacement source and event, or `None` to keep it.
pub(crate) fn map_config_event_addresses(td: &mut TraitDefinition, f: &mut AddressMap<'_>) {
    use orbital_core_base::schema::{join_event_address, split_event_address};
    let mut on_value = |current: &str| -> Option<String> {
        let (event, Some(source)) = split_event_address(current) else {
            return None;
        };
        if matches!(source, ListenSource::Any) {
            return None;
        }
        let (event, source) = f(&event, &source)?;
        Some(join_event_address(&event, Some(&source)))
    };
    let Some(cfg) = td.config.as_mut() else {
        return;
    };
    map_fields_event_values(cfg, &mut on_value);
}

/// [`map_config_event_addresses`] over a bare knob map: an import's folded
/// orbital-level config, before its values are substituted into effects.
pub(crate) fn map_config_fields_event_addresses(
    cfg: &mut orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>,
    f: &mut AddressMap<'_>,
) {
    use orbital_core_base::schema::{join_event_address, split_event_address};
    let mut on_value = |current: &str| -> Option<String> {
        let (event, Some(source)) = split_event_address(current) else {
            return None;
        };
        if matches!(source, ListenSource::Any) {
            return None;
        }
        let (event, source) = f(&event, &source)?;
        Some(join_event_address(&event, Some(&source)))
    };
    map_fields_event_values(cfg, &mut on_value);
}

pub(crate) fn map_fields_event_values(
    cfg: &mut orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>,
    on_value: &mut dyn FnMut(&str) -> Option<String>,
) {
    for field in cfg.values_mut() {
        if is_event_type_name(&field.field_type) {
            if let Some(SExpression::String(current)) = field.default.as_mut() {
                if !current.starts_with('@') {
                    if let Some(to) = on_value(current) {
                        *current = to.into();
                    }
                }
            }
        }
        if field.items.is_some() || field.properties.is_some() {
            let items = field.items.clone();
            let properties = field.properties.clone();
            if let Some(default) = field.default.as_mut() {
                map_event_typed_members(
                    default,
                    items.as_deref(),
                    properties.as_ref(),
                    &mut *on_value,
                );
            }
        }
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
/// 1. **Entity binding root `@EntityName`** — when an `SExpression::String`
///    begins with `"@<from>"` AND the next character is `.` or end-of-string,
///    the binding root is rewritten to `@<to>` plus the trailing path. The
///    suffix-boundary check prevents matching `@EntityNameSomethingElse`.
///
/// 2. **Emit list `["emit", "EVENT_NAME", ...]`** — when the array's first
///    element is `"emit"` and the second is a string in `event_subs`, the
///    event name is rewritten. Matches both the bare 2-element form and
///    the 3-element form `["emit", "EVENT", payload]` where a payload
///    object trails. The walk falls through into the payload tail so any
///    nested `["fetch", X]` inside the payload still gets entity-rewritten.
///
/// 3. **Fetch effect `["fetch", "EntityName", ...]`** (GAP-COMP-11) — when
///    the array's first element is `"fetch"` and position 1 is a string in
///    `entity_subs`, the entity-name slot is rewritten. Walk continues into
///    trailing elements (criteria, etc.) so nested refs are still visited.
///
/// 4. **Persist effect `["persist", action, "EntityName", ...]`** (GAP-COMP-11)
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
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    event_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    let names_a_sub = |text: &str| {
        entity_subs.contains_key(text)
            || event_subs.contains_key(text)
            || text.strip_prefix('@').is_some_and(|rest| {
                entity_subs.contains_key(&rest[..rest.find('.').unwrap_or(rest.len())])
            })
    };
    if expr.any_string(names_a_sub) {
        rewrite_identifiers_inner(expr, entity_subs, event_subs);
    }
}

/// The position rule for "this SExpression node names an entity at exactly
/// this spot" — the `@EntityName[.path]` binding root (boundary-checked so
/// `@EntityNameOther` never matches candidate `EntityName`: the identifier is
/// the substring up to the first `.`, so a differently-spelled name can never
/// partially match), `["fetch", "EntityName", …]`,
/// and `["persist", "create"|"upsert"|"update"|"delete", "EntityName", …]`
/// (GAP-COMP-11), and the entity of each op tuple a `["persist", "batch", <ops>]`
/// writes out (a literal list element, or the tuple an `array/map` lambda returns). Shared by the RENAME walk (`rewrite_identifiers_inner`,
/// via `f` that substitutes through a rename map) and the read-only
/// COLLECTOR (`collect_entity_slots`, via `f` that just records membership) —
/// one position rule, no second copy to drift apart. Never recurses into
/// children itself (both callers already do their own full recursive walk;
/// `["fetch", X, {…criteria…}]`'s criteria tail, for instance, still needs
/// visiting after this fn handles position 1).
pub(crate) fn visit_entity_slots(expr: &mut SExpression, f: &mut dyn FnMut(&mut String)) {
    match expr {
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix('@') {
                let split_at = rest.find('.').unwrap_or(rest.len());
                let (name_part, suffix) = rest.split_at(split_at);
                let mut name = name_part.to_string();
                f(&mut name);
                *s = format!("@{name}{suffix}").into();
            }
        }
        SExpression::Array(items) => {
            if let Some(SExpression::String(op)) = items.first() {
                match op.as_str() {
                    "fetch" => {
                        if let Some(SExpression::String(name)) = items.get_mut(1) {
                            f(name);
                        }
                    }
                    "persist" if matches!(items.get(1), Some(SExpression::String(a)) if a == "batch") => {
                        if let Some(ops) = items.get_mut(2) {
                            visit_batch_op_entities(ops, f);
                        }
                    }
                    "persist" => {
                        let is_single_record = matches!(
                            items.get(1),
                            Some(SExpression::String(action))
                                if matches!(action.as_str(), "create" | "upsert" | "update" | "delete")
                        );
                        if is_single_record {
                            if let Some(SExpression::String(name)) = items.get_mut(2) {
                                f(name);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Apply `f` to the entity name of every op tuple a batch's `<ops>` writes out.
fn visit_batch_op_entities(ops: &mut SExpression, f: &mut dyn FnMut(&mut String)) {
    use orbital_core_base::persist_batch::is_op_tuple;
    fn tuple(t: &mut SExpression, f: &mut dyn FnMut(&mut String)) {
        if !is_op_tuple(t) {
            return;
        }
        if let SExpression::Array(items) = t {
            if let Some(SExpression::String(name)) = items.get_mut(1) {
                f(name);
            }
        }
    }
    let SExpression::Array(items) = ops else {
        return;
    };
    match items.first() {
        Some(SExpression::String(head)) if head == "array/map" && items.len() == 3 => {
            if let SExpression::Array(lambda) = &mut items[2] {
                let is_fn = matches!(lambda.first(), Some(SExpression::String(h)) if h == "fn");
                if is_fn && lambda.len() >= 3 {
                    if let Some(body) = lambda.last_mut() {
                        tuple(body, f);
                    }
                }
            }
        }
        Some(SExpression::String(head)) if head == "list" => {
            items[1..].iter_mut().for_each(|e| tuple(e, f))
        }
        Some(SExpression::String(_)) => {}
        _ => items.iter_mut().for_each(|e| tuple(e, f)),
    }
}

/// Read-only twin of `visit_entity_slots`: every entity name at one of its
/// positions that is also a member of `candidates` is added to `out`.
///
/// Rather than hand-writing a second (immutable) copy of the position match —
/// exactly the kind of drift the two-walker split above exists to prevent —
/// this clones the CURRENT node (children included; they are visited by this
/// fn's own recursion below regardless, so the clone's cost is bounded by the
/// same tree this call already walks) and runs the real mutating dispatcher
/// on the clone, capturing membership through the callback instead of
/// rewriting anything. One position rule.
pub(crate) fn collect_entity_slots(
    expr: &SExpression,
    candidates: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    let mut probe = expr.clone();
    visit_entity_slots(&mut probe, &mut |name| {
        if candidates.contains(name.as_str()) {
            out.insert(name.clone());
        }
    });
    match expr {
        SExpression::Array(items) => {
            for item in items {
                collect_entity_slots(item, candidates, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_entity_slots(v, candidates, out);
            }
        }
        _ => {}
    }
}

/// Rewrite a role literal crossing an orbital-import boundary. Mirrors
/// `validation::user_identity::check_comparison`'s shape rules for what
/// counts as a role-literal comparison — the two must stay shape-identical
/// (each cites the other in its doc comment):
///
/// (a) An equality node (`EQUALITY_OPS`) with exactly one operand a direct
///     `@user.<f>` where `f` is a role-vocabulary field (a key of
///     `role_fields`) and the other a non-`@` string literal that is a key
///     of `roles`: a single target swaps the literal in place; more than one
///     target replaces the WHOLE node with `["array/includes", [targets…],
///     "@user.<f>"]`, wrapped in `["not", …]` when the original op was a
///     negation (`!=`/`!==`/`neq`).
/// (b) An `array/includes` node whose first argument is an array of string
///     literals and whose second is a direct `@user.<f>` role field: each
///     literal is mapped through `roles` (a list target expands in place;
///     the result is deduped, first-seen order kept; an unmapped literal is
///     left as itself for `ORB_S_ROLE_LITERAL_NOT_MEMBER` to catch).
///
/// Recurses into every other array/object so a match anywhere in the tree is
/// found (transition/tick guards+effects, listen guard/payloadMapping, and
/// config-field defaults all funnel through here from `orbital.rs`).
pub(crate) fn rewrite_role_literals(
    expr: &mut SExpression,
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
) {
    if let SExpression::Array(items) = expr {
        if let Some(rewritten) = try_rewrite_role_equality(items, role_fields, roles) {
            *expr = rewritten;
            return;
        }
        if try_rewrite_role_includes(items, role_fields, roles) {
            return;
        }
        for item in items.iter_mut() {
            rewrite_role_literals(item, role_fields, roles);
        }
        return;
    }
    if let SExpression::Object(map) = expr {
        for v in map.values_mut() {
            rewrite_role_literals(v, role_fields, roles);
        }
    }
}

/// A direct `@user.<f>` read whose field is a role-vocabulary field —
/// `user_field_of` (the same string-shape check `check_comparison` uses)
/// filtered to fields `role_fields` actually declares.
fn direct_role_user_field<'a>(
    expr: &SExpression,
    role_fields: &'a orbital_core_base::collections::IndexMap<String, Vec<String>>,
) -> Option<&'a str> {
    let SExpression::String(s) = expr else {
        return None;
    };
    let field = user_field_of(s)?;
    role_fields.keys().map(String::as_str).find(|k| *k == field)
}

/// Case (a): an equality/inequality node comparing a role field to a literal
/// that `roles` remaps. Returns the replacement node when it matches, `None`
/// (node untouched) otherwise.
fn try_rewrite_role_equality(
    items: &[SExpression],
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
) -> Option<SExpression> {
    let SExpression::String(op) = items.first()? else {
        return None;
    };
    if !EQUALITY_OPS.contains(&op.as_str()) || items.len() < 3 {
        return None;
    }
    let (sigil_idx, field) = [1usize, 2usize]
        .into_iter()
        .find_map(|i| direct_role_user_field(items.get(i)?, role_fields).map(|f| (i, f)))?;
    let literal_idx = if sigil_idx == 1 { 2 } else { 1 };
    let SExpression::String(literal) = items.get(literal_idx)? else {
        return None;
    };
    if literal.starts_with('@') {
        return None;
    }
    let targets = roles.get(literal.as_str())?;
    let is_negated = matches!(op.as_str(), "!=");
    let node = if targets.len() == 1 {
        let mut new_items = items.to_vec();
        new_items[literal_idx] = SExpression::string(targets[0].clone());
        SExpression::Array(new_items.into())
    } else {
        SExpression::Array(vec![
            SExpression::string("array/includes"),
            SExpression::Array(targets.iter().cloned().map(SExpression::string).collect()),
            SExpression::string(format!("@user.{field}")),
        ].into())
    };
    if targets.len() > 1 && is_negated {
        Some(SExpression::Array(vec![SExpression::string("not"), node].into()))
    } else {
        Some(node)
    }
}

/// Case (b): an `array/includes` node whose haystack (position 1) is a
/// literal string array and whose needle (position 2) is a direct role-field
/// read — each haystack literal is expanded through `roles` in place.
/// Returns `true` when this node matched the shape (rewritten, whether or
/// not any literal actually changed) so the caller does not also recurse
/// into a node this fn already fully handled.
fn try_rewrite_role_includes(
    items: &mut [SExpression],
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
) -> bool {
    if items.len() < 3 {
        return false;
    }
    let SExpression::String(op) = &items[0] else {
        return false;
    };
    if op != "array/includes" {
        return false;
    }
    if direct_role_user_field(&items[2], role_fields).is_none() {
        return false;
    }
    let SExpression::Array(haystack) = &items[1] else {
        return false;
    };
    // Mirror `validation::user_identity::literal_haystack_members` (the two
    // cite each other): a `list` call's TAIL is the literal member set (the
    // head is the array-construction marker `mark_literal_list_in_place`
    // wraps a config-array-override value with, not a member itself); a
    // bare array is literal only when EVERY element is a non-`@` string
    // literal — the shape this fn's own multi-target expansion produces
    // (`["array/includes", [targets…], "@user.<f>"]`). Any other shape — a
    // nested call like `["object/get", ["array/nth", …], "allowedRoles"]`, a
    // per-step dynamic role list — is left untouched.
    let is_list_call = matches!(haystack.first(), Some(SExpression::String(s)) if s == "list");
    let members: &[SExpression] = if is_list_call {
        &haystack[1..]
    } else {
        haystack
    };
    let all_literals = members
        .iter()
        .all(|v| matches!(v, SExpression::String(s) if !s.starts_with('@')));
    if !all_literals {
        return false;
    }
    let expanded = expand_role_members(members, roles);
    let mut new_haystack: Vec<SExpression> = Vec::with_capacity(expanded.len() + 1);
    if is_list_call {
        new_haystack.push(SExpression::string("list"));
    }
    new_haystack.extend(expanded);
    items[1] = SExpression::Array(new_haystack.into());
    true
}

/// Map each literal member through `roles` (a list target expands in place),
/// deduped with first-seen order kept; an unmapped literal stays as itself
/// for `ORB_S_ROLE_LITERAL_NOT_MEMBER` to catch. Non-string members are
/// dropped by callers' all-literal precheck, never reached here.
fn expand_role_members(
    members: &[SExpression],
    roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
) -> Vec<SExpression> {
    let mut expanded: Vec<String> = Vec::new();
    for v in members.iter() {
        let SExpression::String(lit) = v else {
            continue;
        };
        let mapped: Vec<String> = roles
            .get(lit.as_str())
            .cloned()
            .unwrap_or_else(|| vec![lit.clone().into_inner()]);
        for m in mapped {
            if !expanded.contains(&m) {
                expanded.push(m);
            }
        }
    }
    expanded.into_iter().map(SExpression::string).collect()
}

/// Where a role list lives inside a config knob: the knob, the object keys
/// walked from the knob's array elements down to the array whose elements
/// carry the list, and the key holding it (`None` = the knob itself is the
/// list).
#[derive(PartialEq)]
pub(crate) struct RoleListSite {
    pub(crate) knob: String,
    pub(crate) path: Vec<String>,
    pub(crate) key: Option<String>,
    /// The role-vocabulary identity field the list is tested against.
    pub(crate) field: String,
}

/// A role source: a direct `@user.<f>` read of a role-vocabulary field, or
/// `@config.<v>` whose settled default is exactly such a read (the
/// `viewerRole: @user.role` binding).
fn role_source_field<'a>(
    expr: &SExpression,
    cfg: &orbital_core_base::collections::IndexMap<String, ConfigField>,
    role_fields: &'a orbital_core_base::collections::IndexMap<String, Vec<String>>,
) -> Option<&'a str> {
    if let Some(f) = direct_role_user_field(expr, role_fields) {
        return Some(f);
    }
    let SExpression::String(s) = expr else {
        return None;
    };
    let knob = s.strip_prefix("@config.")?;
    let default = cfg.get(knob)?.default.as_ref()?;
    direct_role_user_field(default, role_fields)
}

type ItemScope = HashMap<String, (String, Vec<String>)>;

/// The knob (and object-key path below its elements) a collection expression
/// denotes: `@config.<knob>`, a pass-through of one (an operator declaring
/// `identity-of-arg<N>` only narrows or reorders, so it denotes the same
/// elements), or `(object/get @<param> <key> …)` where `<param>` is bound to an
/// element of a resolved collection.
fn resolve_role_collection(expr: &SExpression, scope: &ItemScope) -> Option<(String, Vec<String>)> {
    if let Some(inner) = expr.passthrough_arg(&all_sources()) {
        return resolve_role_collection(inner, scope);
    }
    match expr {
        SExpression::String(s) => {
            let knob = s.strip_prefix("@config.")?;
            (!knob.contains('.')).then(|| (knob.to_string(), Vec::new()))
        }
        SExpression::Array(items) => match items.first()? {
            SExpression::String(op) if op == "object/get" => {
                let SExpression::String(param) = items.get(1)? else {
                    return None;
                };
                let SExpression::String(key) = items.get(2)? else {
                    return None;
                };
                let (knob, mut path) = scope.get(param.strip_prefix('@')?)?.clone();
                path.push(key.clone().into_inner());
                Some((knob, path))
            }
            _ => None,
        },
        _ => None,
    }
}

fn collect_role_list_sites(
    expr: &SExpression,
    scope: &ItemScope,
    cfg: &orbital_core_base::collections::IndexMap<String, ConfigField>,
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    out: &mut Vec<RoleListSite>,
) {
    match expr {
        SExpression::Array(items) => {
            if let (Some(SExpression::String(op)), true) = (items.first(), items.len() == 3) {
                let role_field = role_source_field(&items[2], cfg, role_fields);
                if let (true, Some(role_field)) = (op == "array/includes", role_field) {
                    let field = role_field.to_string();
                    match &items[1] {
                        SExpression::String(_) => {
                            if let Some((knob, path)) = resolve_role_collection(&items[1], scope) {
                                if path.is_empty() {
                                    out.push(RoleListSite {
                                        knob,
                                        path,
                                        key: None,
                                        field: field.clone(),
                                    });
                                }
                            }
                        }
                        SExpression::Array(get) if matches!(get.first(), Some(SExpression::String(o)) if o == "object/get") => {
                            if let (
                                Some(SExpression::String(param)),
                                Some(SExpression::String(key)),
                            ) = (get.get(1), get.get(2))
                            {
                                if let Some((knob, path)) =
                                    param.strip_prefix('@').and_then(|p| scope.get(p)).cloned()
                                {
                                    out.push(RoleListSite {
                                        knob,
                                        path,
                                        key: Some(key.clone().into_inner()),
                                        field: field.clone(),
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if items.len() >= 3 {
                if let SExpression::Array(lambda) = &items[2] {
                    if let (
                        Some(SExpression::String(head)),
                        Some(SExpression::String(param)),
                        Some(body),
                    ) = (lambda.first(), lambda.get(1), lambda.get(2))
                    {
                        if head == "fn" {
                            if let Some(coll) = resolve_role_collection(&items[1], scope) {
                                collect_role_list_sites(&items[1], scope, cfg, role_fields, out);
                                let mut inner = scope.clone();
                                inner.insert(param.clone().into_inner(), coll);
                                collect_role_list_sites(body, &inner, cfg, role_fields, out);
                                return;
                            }
                        }
                    }
                }
            }
            for item in items {
                collect_role_list_sites(item, scope, cfg, role_fields, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_role_list_sites(v, scope, cfg, role_fields, out);
            }
        }
        _ => {}
    }
}

fn list_members_mut(value: &mut SExpression) -> Option<&mut [SExpression]> {
    let SExpression::Array(items) = value else {
        return None;
    };
    let is_list_call = matches!(items.first(), Some(SExpression::String(s)) if s == "list");
    Some(if is_list_call { &mut items[1..] } else { items })
}

/// The all-literal members of a role list (`["list", …]` marker skipped);
/// `None` when any member is a non-literal (dynamic) value.
fn literal_role_members(value: &SExpression) -> Option<&[SExpression]> {
    let SExpression::Array(items) = value else {
        return None;
    };
    let is_list_call = matches!(items.first(), Some(SExpression::String(s)) if s == "list");
    let members = if is_list_call {
        &items[1..]
    } else {
        &items[..]
    };
    members
        .iter()
        .all(|v| matches!(v, SExpression::String(s) if !s.starts_with('@')))
        .then_some(members)
}

fn expand_role_list_in_place(value: &mut SExpression, roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>) {
    let Some(members) = literal_role_members(value) else {
        return;
    };
    let mut expanded = expand_role_members(members, roles);
    let SExpression::Array(items) = value else {
        return;
    };
    if matches!(items.first(), Some(SExpression::String(s)) if s == "list") {
        expanded.insert(0, SExpression::string("list"));
    }
    *items = expanded.into();
}

fn visit_role_lists_at(
    value: &mut SExpression,
    path: &[String],
    key: Option<&str>,
    f: &mut dyn FnMut(&mut SExpression),
) {
    let Some(key) = key else {
        f(value);
        return;
    };
    let Some(elements) = list_members_mut(value) else {
        return;
    };
    for elem in elements.iter_mut() {
        let SExpression::Object(map) = elem else {
            continue;
        };
        match path.split_first() {
            Some((head, rest)) => {
                if let Some(child) = map.get_mut(head) {
                    visit_role_lists_at(child, rest, Some(key), f);
                }
            }
            None => {
                if let Some(list) = map.get_mut(key) {
                    f(list);
                }
            }
        }
    }
}

/// Every deduped config-held role-list site of `td`: a knob list the trait
/// tests membership in against a role source. The one detector behind both the
/// import remap (`rewrite_role_literals_in_config_lists`) and the validator's
/// literal-membership check.
pub(crate) fn role_list_sites(
    td: &TraitDefinition,
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
) -> Vec<RoleListSite> {
    let Some(cfg) = td.config.as_ref() else {
        return Vec::new();
    };
    let scope = ItemScope::new();
    let mut sites: Vec<RoleListSite> = Vec::new();
    let mut walk =
        |e: &SExpression| collect_role_list_sites(e, &scope, cfg, role_fields, &mut sites);
    for t in &td.state_machine.transitions {
        t.guard.iter().for_each(&mut walk);
        t.effects.iter().flatten().for_each(&mut walk);
    }
    for tick in td.ticks.iter().flatten() {
        tick.guard.iter().for_each(&mut walk);
        tick.effects.iter().for_each(&mut walk);
    }
    for l in td.listens.iter().flatten() {
        l.guard.iter().for_each(&mut walk);
        l.payload_mapping
            .iter()
            .flat_map(|m| m.values())
            .for_each(&mut walk);
    }
    // Role checks inside another knob's default — a detail-panel action's
    // `when: (fn row (array/includes @config.K @config.viewerRole))` held in
    // `detailActions` — reach the render through config folding.
    cfg.values()
        .filter_map(|field| field.default.as_ref())
        .for_each(&mut walk);
    let mut unique: Vec<RoleListSite> = Vec::new();
    for site in sites {
        if !unique.contains(&site) {
            unique.push(site);
        }
    }
    unique
}

/// The string literals of every role list `site` denotes inside `default`
/// (the knob's settled default). Lists holding a dynamic member are skipped,
/// exactly as the remap skips them.
pub(crate) fn role_list_literals(default: &SExpression, site: &RoleListSite) -> Vec<String> {
    let mut probe = default.clone();
    let mut out: Vec<String> = Vec::new();
    visit_role_lists_at(&mut probe, &site.path, site.key.as_deref(), &mut |list| {
        if let Some(members) = literal_role_members(list) {
            for m in members {
                if let SExpression::String(s) = m {
                    if !out.contains(s) {
                        out.push(s.clone().into_inner());
                    }
                }
            }
        }
    });
    out
}

/// Stage B `roles {}` over config-held role lists. A role literal sitting in a
/// knob value is only a role COMPARISON once the trait's own guard/render
/// reads it against a role source, so `rewrite_role_literals` (which needs the
/// literal inside the comparison node) cannot see it.
///
/// Structural rule: a knob list is remapped exactly when the trait tests
/// membership in it against a role source (`role_source_field`) — either
/// `(array/includes @config.K <role source>)` (the knob IS the list), or
/// `(array/includes (object/get @p <key> …) <role source>)` inside a
/// `(fn p …)` over a collection that resolves to knob `K` (each element's
/// `<key>` list, nested through `(object/get @p <k> …)` child collections).
/// Members expand through `roles` exactly as `try_rewrite_role_includes`
/// does (dedupe, order kept, unmapped literal untouched so
/// `ORB_S_ROLE_LITERAL_NOT_MEMBER` still fires). Runs after the config
/// forward; knobs that resolved out to the CONSUMER's own `config {}`
/// (`schema_only_keys`) are the consumer's vocabulary and are never touched.
pub(crate) fn rewrite_role_literals_in_config_lists(
    td: &mut TraitDefinition,
    role_fields: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    roles: &orbital_core_base::collections::IndexMap<String, Vec<String>>,
    schema_only_keys: &HashSet<String>,
) {
    if roles.is_empty() {
        return;
    }
    let unique = role_list_sites(td, role_fields);
    let Some(cfg) = td.config.as_mut() else {
        return;
    };
    for site in unique {
        if schema_only_keys.contains(&site.knob) {
            continue;
        }
        let Some(default) = cfg.get_mut(&site.knob).and_then(|f| f.default.as_mut()) else {
            continue;
        };
        visit_role_lists_at(default, &site.path, site.key.as_deref(), &mut |list| {
            expand_role_list_in_place(list, roles)
        });
    }
}

fn rewrite_identifiers_inner(
    expr: &mut SExpression,
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    event_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    visit_entity_slots(expr, &mut |name| {
        if let Some(to) = entity_subs.get(name.as_str()) {
            *name = to.clone();
        }
    });
    match expr {
        SExpression::Null
        | SExpression::Bool(_)
        | SExpression::Number(_)
        | SExpression::String(_) => {}
        SExpression::Array(items) => {
            // Detect `["emit", "EVENT", ...]` and rewrite the event name in
            // position 1. The form is canonically 2 or 3 elements (`emit
            // EVENT` or `emit EVENT payload`); both must be rewritten when
            // the event is in `event_subs`. Falling through to the
            // recursive walk below ensures any nested `["fetch", X]` inside
            // a payload tail still gets entity-rewritten.
            if items.len() >= 2 {
                if let (SExpression::String(op), SExpression::String(arg)) = (&items[0], &items[1])
                {
                    if op == "emit" {
                        if let Some(to) = event_subs.get(arg.as_str()) {
                            items[1] = SExpression::string(to.clone());
                            // Don't return — let the recursive walk visit
                            // any payload tail so nested entity refs in
                            // `(emit EVENT { ref: [...] })` still rewrite.
                        }
                    }
                }
            }
            for item in items.iter_mut() {
                rewrite_identifiers_inner(item, entity_subs, event_subs);
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
                Some(SExpression::String(ty)) => Some(ty.to_string()),
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
                // 0) Effect emit-options map. fetch/persist/call-service/
                //    os-watch/set effects carry `emit: {success|failure|
                //    on_message|cancelled: "EVENT"}` (the slot-key universe of
                //    every `validate_emit_config` call site) and the string
                //    form `emit: "EVENT"`. A call-site `events {OLD: NEW}`
                //    rename that skips these leaves the atom's fetch success
                //    emit firing the pre-rename name →
                //    ORB_EFF_EMIT_UNDECLARED_EVENT per fetch site
                //    (C-EVENTS-RENAME-MISSES-FETCH-EMIT-MAP).
                if k == "emit" {
                    match v {
                        SExpression::String(ev) => {
                            if let Some(to) = event_subs.get(ev.as_str()) {
                                *ev = to.clone().into();
                            }
                        }
                        SExpression::Object(emit_map) => {
                            let slots = orbital_core_base::language::registry::emit_slots();
                            for (slot, slot_v) in emit_map.iter_mut() {
                                if slots.contains(slot.as_str()) {
                                    if let SExpression::String(ev) = slot_v {
                                        if let Some(to) = event_subs.get(ev.as_str()) {
                                            *ev = to.clone().into();
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
                            if let Some(to) = event_subs.get(s.as_str()) {
                                *s = to.clone().into();
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
                            for leaf in v.value_leaves_mut(&all_sources()) {
                                let items = match leaf {
                                    SExpression::Array(items) => items.as_mut_slice(),
                                    SExpression::Object(_) => std::slice::from_mut(leaf),
                                    _ => &mut [],
                                };
                                for item in items.iter_mut() {
                                    if let SExpression::Object(entry) = item {
                                        if let Some(SExpression::String(s)) = entry.get_mut(*field)
                                        {
                                            if !s.starts_with('@') {
                                                if let Some(to) = event_subs.get(s.as_str()) {
                                                    *s = to.clone().into();
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
                        if let Some(to) = entity_subs.get(s.as_str()) {
                            *s = to.clone().into();
                            continue;
                        }
                    }
                }
                rewrite_identifiers_inner(v, entity_subs, event_subs);
            }
        }
    }
}

/// Sigils whose `.field` suffix addresses a field of the trait's bound
/// entity/row — `@entity` (the bound entity itself) plus `@item`/`@row`,
/// the two row-iteration binding roots a `*Binding`-suffixed config default
/// hands out for override (`cardTitleBinding : string = @item.title`,
/// `Board.traits.BoardItemBoard`; `filter: (fn row (= @row.stage won))`,
/// `Stats.traits.StatsItemStats`) — the only two named alongside `@entity`
/// in `Almadar_LOLO.md`'s own sigil list (§ "Bindings": "`@user.*`,
/// `@row.*`, …"; ELOLO_MATCH_EXPR_SCRUTINEE_IMPURE). An atom-specific
/// lambda alias picked by that atom's own author (`@card`, `@line`, `@bar`,
/// … — `fn`/`lambda` parameter names are open-ended, not a fixed sigil
/// vocabulary) is out of scope: there is no enumerable "every row alias"
/// set to dispatch on, only the two orbital-core documents by name.
const ROW_ALIAS_SIGILS: &[&str] = &["@entity.", "@item.", "@row.", "@fromEntity."];

/// Rewrite `@entity.<old>` / `@item.<old>` / `@row.<old>` reads to their
/// `<new>` counterpart per the call-site `fields` remap map (G3, `docs/
/// Almadar_Compiler_Gaps.md` §82 — `@item.<f>` widened the original
/// `@entity.`-only scope). Mirrors the entity-root rewrite in
/// `rewrite_identifiers` but targets the FIELD segment (the part after the
/// sigil) rather than the entity root. Boundary-checked on the field name so
/// `@entity.titleSuffix` is left alone when remapping `title`; nested paths
/// (`@entity.title.x`) and array indices (`@entity.title[0]`) are preserved.
/// Also rewrites the call-form of the same read, `(object/get <sigil>
/// "<old>")` (an access-policy/guard comparison like `(object/get @entity
/// csm)`, which never reaches the sigil-string case above since the field
/// name sits in its own positional argument) — mirrors the shape
/// `collect_extend_field_reads`/`check_object_get` already treat as the
/// canonical `object/get` field-read recognizer, generalized from
/// `@entity`-only to the same sigil set above. And rewrites a struct
/// descriptor's `key`/`name`/`field` member (`{ field: "title", header:
/// "Title" }` — a `ColumnSpec`/`AppSearchFieldSpec`-style column/form/metric
/// entry) when its value matches — the SAME member-key set
/// `collect_extend_field_reads`/`config_value_names_field` already treat as
/// the canonical "this object entry names a field" shape, generalized here
/// from detection to rewriting. Empty map = identity walk.
pub(crate) fn rewrite_entity_field_paths(
    expr: &mut SExpression,
    field_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    if field_subs.is_empty()
        || !expr.any_string(|text| {
            field_subs.contains_key(text)
                || ROW_ALIAS_SIGILS.iter().any(|sigil| {
                    text.strip_prefix(sigil).is_some_and(|rest| {
                        field_subs.contains_key(&rest[..rest.find(['.', '[']).unwrap_or(rest.len())])
                    })
                })
        })
    {
        return;
    }
    rewrite_entity_field_paths_inner(expr, field_subs);
}

fn rewrite_entity_field_paths_inner(
    expr: &mut SExpression,
    field_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            for sigil in ROW_ALIAS_SIGILS {
                let Some(rest) = s.strip_prefix(sigil) else {
                    continue;
                };
                // Split off the first segment (field name), keeping any nested
                // path / index suffix intact.
                let split_at = rest.find(['.', '[']).unwrap_or(rest.len());
                let (field, suffix) = rest.split_at(split_at);
                if let Some(to) = field_subs.get(field) {
                    *s = format!("{sigil}{to}{suffix}").into();
                }
                break;
            }
        }
        SExpression::Array(items) => {
            if let [SExpression::String(op), SExpression::String(root), SExpression::String(f), ..] =
                items.as_mut_slice()
            {
                if op == "object/get" && ROW_ALIAS_SIGILS.contains(&format!("{root}.").as_str()) {
                    if let Some(to) = field_subs.get(f.as_str()) {
                        *f = to.clone().into();
                    }
                }
            }
            for item in items.iter_mut() {
                rewrite_entity_field_paths_inner(item, field_subs);
            }
        }
        SExpression::Object(map) => {
            for key in ["key", "name", "field"] {
                if let Some(SExpression::String(v)) = map.get_mut(key) {
                    if let Some(to) = field_subs.get(v.as_str()) {
                        *v = to.clone().into();
                    }
                }
            }
            for v in map.values_mut() {
                rewrite_entity_field_paths_inner(v, field_subs);
            }
        }
    }
}

/// A `fields {}` remap renames the keys of the data literal of a `persist` that writes the bound entity, so the
/// written columns follow the `@entity.<field>` reads rewritten beside them. A persist to another entity keeps its keys.
pub(crate) fn rewrite_persist_data_keys(
    expr: &mut SExpression,
    field_subs: &orbital_core_base::collections::IndexMap<String, String>,
    bound_entity: &str,
) {
    if field_subs.is_empty() {
        return;
    }
    match expr {
        SExpression::Array(items) => {
            if matches!(items.first(), Some(SExpression::String(op)) if op == "persist") {
                let entity_index = persist_entity_index(items);
                let writes_bound = matches!(
                    items.get(entity_index),
                    Some(SExpression::String(name)) if name == bound_entity
                );
                if writes_bound {
                    if let Some(SExpression::Object(data)) = items.get_mut(entity_index + 1) {
                        let renamed: Vec<(String, String)> = data
                            .keys()
                            .filter_map(|k| field_subs.get(k.as_str()).map(|to| (k.to_string(), to.clone())))
                            .collect();
                        for (from, to) in renamed {
                            if let Some(value) = data.shift_remove(from.as_str()) {
                                data.insert(to.into(), value);
                            }
                        }
                    }
                }
            }
            for item in items.iter_mut() {
                rewrite_persist_data_keys(item, field_subs, bound_entity);
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                rewrite_persist_data_keys(v, field_subs, bound_entity);
            }
        }
        _ => {}
    }
}

/// A `fields {}` rename must also reach a config knob's OWN default value
/// when the default value itself IS the field-name reference: a bare
/// string equal to the renamed field (`searchField : string = title`,
/// `std-app-search`) or a bare list of them (`formFields : [string] =
/// (title contactId amount stage closedAt)`, `std-board`) — G3, `docs/
/// Almadar_Compiler_Gaps.md` §82. The struct/array-of-struct case
/// (`columns : [ColumnSpec]`, `resultFields : [AppSearchFieldSpec]`,
/// `metrics : [MetricSpec]`) is already covered by
/// `rewrite_entity_field_paths`'s `key`/`name`/`field` member rule above,
/// which every call site of this function also runs on the same default.
///
/// A call-site `config {}` override collapses the atom's own declared
/// `field_type` to `"unknown"` on the materialized trait (a separate,
/// pre-existing gap in the override fold, not this one — the type is
/// genuinely unrecoverable here), so this dispatches on the DEFAULT
/// VALUE's shape (bare string / bare array of strings), not the declared
/// type string. Deliberately scoped to config DEFAULTS ONLY — never
/// guards/effects, where a bare string literal is far more likely to be an
/// unrelated comparison value than a field name. `field_subs`'s domain is
/// the small, closed set of field names actually being renamed on THIS
/// import, so the residual risk this accepts is a config default whose
/// value happens to literally equal one of those few old names.
pub(crate) fn rewrite_bare_field_name_config_default(
    default: &mut SExpression,
    field_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    if field_subs.is_empty() {
        return;
    }
    match default {
        SExpression::String(s) => {
            if !s.is_empty() && !s.starts_with('@') {
                if let Some(to) = field_subs.get(s.as_str()) {
                    *s = to.clone().into();
                }
            }
        }
        SExpression::Array(items) if items.iter().all(|i| matches!(i, SExpression::String(_))) => {
            for item in items.iter_mut() {
                if let SExpression::String(s) = item {
                    if !s.is_empty() && !s.starts_with('@') {
                        if let Some(to) = field_subs.get(s.as_str()) {
                            *s = to.clone().into();
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Read-only, POST-rewrite completeness check for G3's `fields {}` rename
/// (`docs/Almadar_Compiler_Gaps.md` §82, rung 2): does `default` still
/// contain a bare string exactly equal to one of the PRE-rename field
/// names, in ANY position — not just the `key`/`name`/`field` member set
/// `rewrite_entity_field_paths` rewrites, or the bare string/list shape
/// `rewrite_bare_field_name_config_default` rewrites? A struct member
/// spelled outside that set (e.g. a pattern-specific `sourceField`) is
/// invisible to both rewriters while still holding a genuine leftover
/// reference; called AFTER they run, so a match here means the rename
/// could not be safely applied — `ORB_O_FIELD_RENAME_UNAPPLIED` turns that
/// into a diagnostic instead of a silent no-op. Recurses through
/// arrays/objects; a `@`-prefixed string is a live binding, never a
/// leftover literal, and `field_subs`'s domain (the few old names actually
/// being renamed) bounds how broad a match can be.
pub(crate) fn collect_stale_field_name_literals(
    default: &SExpression,
    field_subs: &orbital_core_base::collections::IndexMap<String, String>,
    out: &mut HashSet<String>,
) {
    match default {
        SExpression::String(s) => {
            if !s.is_empty() && !s.starts_with('@') && field_subs.contains_key(s.as_str()) {
                out.insert(s.clone().into_inner());
            }
        }
        SExpression::Array(items) => {
            for item in items {
                collect_stale_field_name_literals(item, field_subs, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_stale_field_name_literals(v, field_subs, out);
            }
        }
        _ => {}
    }
}

/// V4 leverage-ids reader: given a resolved inline trait and a rebind target
/// entity name, return the extra `stale-token-name → new_entity` entity
/// substitutions implied by the trait's `entity_ref_ids` side-map.
///
/// When the trait's bound entity declaration was RENAMED (its stable id is
/// unchanged but the `@entity`/`fetch`/`persist` tokens in the
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
///
/// Deliberately does NOT exclude an entry whose `token_name` already equals
/// `new_entity`: the atom's own copy of that entry still carries the ATOM's
/// bound-entity id (e.g. baked when a shared generic was last stamped
/// standalone), which is foreign to whatever schema this trait is being
/// resolved into now — this is what lets the identity entry through so
/// `rewrite_entity_ref_ids` (the caller's next step) still refreshes its
/// VALUE to the consumer's `entity_ids[new_entity]`, even though the token's
/// NAME needs no rewrite. Without this, a same-name rebind (the atom's own
/// entity happens to already be called `new_entity`) left the id stale —
/// found on `std-notes`' `NoteSubpages = PageAtom.traits.NoteSubpages ->
/// Note`, whose `entityRefIds["Note"]` kept naming an id no orbital in the
/// composed schema carries, exactly the class of bug `ORB_ID_UNKNOWN_REF`'s
/// new `entityRefIds` coverage was added to catch.
pub(crate) fn stale_entity_rebind_subs(
    trait_def: &TraitDefinition,
    new_entity: &str,
) -> orbital_core_base::collections::IndexMap<String, String> {
    let mut out = orbital_core_base::collections::IndexMap::new();
    if let (Some(bound_id), Some(map)) = (
        trait_def.linked_entity_id.as_deref(),
        trait_def.entity_ref_ids.as_ref(),
    ) {
        for (token_name, entity_id) in map {
            if entity_id == bound_id {
                out.insert(token_name.clone(), new_entity.to_string());
            }
        }
    }
    out
}

/// General case of `stale_entity_rebind_subs`, for a positional (non-`@entity`,
/// non-`linked_entity`) reference a trait carries to a SECONDARY entity — one
/// its own rebind never touches because that rebind only retargets the
/// PRIMARY `linked_entity`. `entity_ref_ids` already carries this reference's
/// id (the atom's own side-map, unrelated to `linked_entity_id`); when the
/// CONSUMER orbital independently declares an entity with that SAME id under
/// a DIFFERENT name (`consumer_entity_ids`, this orbital's own name→id
/// roster), the atom's positional token is stale and must follow the
/// consumer's name — the id is the only honest join key across a rename, and
/// this is the reachable-anywhere-in-effects general case `stale_entity_
/// rebind_subs` only covers for the trait's OWN bound entity.
///
/// Reproduces `WikiRevisionCapture` (std-wiki): rebound `-> WikiPage`
/// (`linked_entity`, a no-op — the atom's own copy is already `WikiPage`), so
/// `linked_entity`-driven `entity_subs` is empty, yet its `(persist create
/// WikiPageRevision {…})` positional target names the SAME id the SIBLING
/// `WikiHistory` pull rebound to `WikiRevision` (their `entity_ref_ids`
/// values coincide — both trace to the ORGANISM's single revision entity).
/// No cross-trait bookkeeping needed: each pull independently resolves its
/// own stale positional names against the one CONSUMER roster every pull
/// already receives.
pub(crate) fn consumer_known_entity_rebind_subs(
    trait_def: &TraitDefinition,
    consumer_entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
) -> orbital_core_base::collections::IndexMap<String, String> {
    let mut out = orbital_core_base::collections::IndexMap::new();
    let Some(refs) = trait_def.entity_ref_ids.as_ref() else {
        return out;
    };
    // id -> consumer name. Built once per call; a genuine id collision across
    // two differently-named consumer entities is a separate id-integrity bug
    // for another gate to catch, not something to guess between here.
    let mut id_to_consumer_name: HashMap<&str, &str> = HashMap::new();
    for (name, id) in consumer_entity_ids {
        id_to_consumer_name
            .entry(id.as_str())
            .or_insert(name.as_str());
    }
    for (token_name, entity_id) in refs {
        if let Some(consumer_name) = id_to_consumer_name.get(entity_id.as_str()) {
            if *consumer_name != token_name {
                out.insert(token_name.clone(), (*consumer_name).to_string());
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
    let mut entity_subs = orbital_core_base::collections::IndexMap::new();
    entity_subs.insert(from_entity.to_string(), to_entity.to_string());
    rewrite_identifiers(expr, &entity_subs, &orbital_core_base::collections::IndexMap::new());
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
/// Type-directed + deterministic: the declared array type decides, never the
/// kind of the first item. Every non-empty literal in an array-typed slot is
/// marked; the empty list stays `[]`.
pub(crate) fn mark_literal_list_in_place(value: &mut SExpression, field_type: &str) {
    if !(field_type.starts_with('[') && field_type.ends_with(']')) {
        return;
    }
    let should_wrap = matches!(
        value,
        SExpression::Array(elems)
            if !elems.is_empty() && !matches!(elems.first(), Some(SExpression::String(s)) if s == "list")
    ) && !sexpr_tree_contains_binding(value);
    if should_wrap {
        if let SExpression::Array(elems) = value {
            let mut wrapped = Vec::with_capacity(elems.len() + 1);
            wrapped.push(SExpression::string("list"));
            wrapped.append(elems);
            *value = SExpression::Array(wrapped.into());
        }
    }
}

/// `mark_literal_list_in_place` applied through a config value's whole declared
/// shape: the top-level slot, then every nested array/object member the typed
/// carriers (`items` / `properties`) describe. A string-headed array nested in a
/// struct (`sections: [{ fields: ["count", "slug"] }]`) is as ambiguous as a
/// top-level one — unwrapped, `(count slug)` evaluates as a call.
pub(crate) fn mark_config_literal_lists(value: &mut SExpression, field: &ConfigField) {
    if field.items.as_deref().is_none_or(element_admits_string) {
        mark_literal_list_in_place(value, &field.field_type);
    }
    let Some(shape) = field.items.as_deref() else {
        if let (Some(props), SExpression::Object(map)) = (&field.properties, value) {
            mark_object_members(map, props);
        }
        return;
    };
    if field.field_type.starts_with("Map<") {
        if let SExpression::Object(map) = value {
            for entry in map.values_mut() {
                mark_field_literal_lists(entry, shape);
            }
        }
        return;
    }
    if let SExpression::Array(elems) = value {
        for elem in literal_list_elements_mut(elems) {
            mark_field_literal_lists(elem, shape);
        }
    }
}

/// Whether a string can be an element of this array: when it cannot, a
/// string-headed value is a call, never literal data, and is left unwrapped.
fn element_admits_string(shape: &FieldDefinition) -> bool {
    !matches!(
        shape.field_type,
        FieldType::Number
            | FieldType::Int
            | FieldType::Instant
            | FieldType::Boolean
            | FieldType::Money
            | FieldType::File
            | FieldType::Bytes
            | FieldType::Array
            | FieldType::Object
            | FieldType::Tuple
    )
}

fn mark_field_literal_lists(value: &mut SExpression, shape: &FieldDefinition) {
    match shape.field_type {
        FieldType::Array => {
            if shape.items.as_deref().is_none_or(element_admits_string) {
                mark_literal_list_in_place(value, "[]");
            }
            if let (Some(inner), SExpression::Array(elems)) = (shape.items.as_deref(), value) {
                for elem in literal_list_elements_mut(elems) {
                    mark_field_literal_lists(elem, inner);
                }
            }
        }
        FieldType::Object => {
            if let (Some(props), SExpression::Object(map)) = (&shape.properties, value) {
                mark_object_members(map, props);
            }
        }
        _ => {}
    }
}

fn mark_object_members(
    map: &mut orbital_core_base::schema::SObject,
    props: &orbital_core_base::collections::IndexMap<String, FieldDefinition>,
) {
    for (key, member) in map.iter_mut() {
        if let Some(member_shape) = props.get(key.as_str()) {
            mark_field_literal_lists(member, member_shape);
        }
    }
}

fn literal_list_elements_mut(elems: &mut [SExpression]) -> &mut [SExpression] {
    match elems.first() {
        Some(SExpression::String(s)) if s == "list" => &mut elems[1..],
        _ => elems,
    }
}

/// Twin of `mark_literal_list_in_place`: literal readers (fetch `include`
/// validation + OIR resolution) must accept the `["list", …]` constructor the
/// wrap produces. The evaluators strip the head at eval time (ListOp); readers
/// that treat the array as inert data strip it here instead.
pub(crate) use orbital_core_eval::lower::strip_literal_list_marker;

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
/// A `secret` knob's reads stay `@config.<knob>` in the resolved program, so its secret-flow rules hold there too; the
/// knob's effective value is its declared default, which the kernel binds at run time.
pub(crate) const SECRET_KNOB_TYPE: &str = "secret";

pub(crate) fn apply_inline_config_defaults(
    trait_def: &mut TraitDefinition,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    let Some(declared) = trait_def.config.as_ref() else {
        return;
    };
    let mut effective_config: orbital_core_base::schema::SObject =
        orbital_core_base::schema::SObject::new();
    let mut trait_knobs: HashSet<String> = HashSet::new();
    for (k, field) in declared.iter() {
        if field.field_type == "trait" {
            trait_knobs.insert(k.clone());
        }
        if field.field_type == SECRET_KNOB_TYPE {
            continue;
        }
        if let Some(default) = field.default.as_ref() {
            let mut v = default.clone();
            mark_config_literal_lists(&mut v, field);
            effective_config.insert(k.as_str().into(), v);
        }
    }
    if effective_config.is_empty() {
        return;
    }
    // A `trait`-typed knob's raw value (a `@trait.<Name>` embed token, or
    // `Null` for the unbound `none` sentinel) has no meaning as a runtime
    // boolean, so resolve the CONDITION slot of every `(if|when
    // @config.<knob> ...)` to whether the knob is bound BEFORE the general
    // substitution below bakes the raw value into every other position
    // (correctly — e.g. a render-ui children entry). See
    // `coerce_trait_knob_conditions`'s doc comment for the full rationale.
    coerce_trait_knob_conditions(trait_def, &trait_knobs, &effective_config);
    let declared_types = declared_list_types(trait_def.config.as_ref(), &effective_config);
    let cfg = &effective_config;
    record_config_provenance(trait_def, cfg, recorder);
    rewrite_trait_config_bindings(trait_def, cfg, &declared_types);
    // Standalone atom (no call site): `@config.<knob>` emit-name refs
    // resolve to the declared defaults collected above (Option B).
    super::r#trait::resolve_config_ref_emit_names(trait_def, cfg);
}

/// A `trait`-typed config knob's raw value is a render-only composition
/// token — a `@trait.<Name>` embed string, or `Null` for the unbound `none`
/// sentinel (owner ruling 2026-09-13) — never a runtime boolean. But the
/// generic `@config.<knob>` substitution below (`rewrite_config_bindings`) is
/// a blind, position-agnostic text walk: baking the raw value into a
/// `(if @config.<knob> ...)` guard condition would land a bare `@trait.X`
/// string (or a bare `Null`, itself not a `boolean`-typed literal per the
/// `if` operator's declared param type) directly in a boolean slot, which
/// `ORB_BINDING_TRAIT_OUTSIDE_RENDER_UI` correctly rejects — a render-only
/// primitive has no truth value.
///
/// This pass runs FIRST and resolves ONLY the condition slot of every
/// `(if|when @config.<knob> ...)` node — for each `<knob>` in `trait_knobs`
/// — to a literal `Bool`: `true` when the knob's `effective_config` value is
/// a bound `@trait.<Name>` string, `false` when it is `Null` (or otherwise
/// unresolved). Every OTHER occurrence of `@config.<knob>` (e.g. a
/// render-ui `children` entry) is untouched here and flows through the
/// normal substitution afterward exactly as before — which is what lets the
/// compiled TypeScript backend keep baking a bound knob into a static
/// `<TraitFrame>`-equivalent import (`codegen/pattern/registry.rs`'s
/// `child_to_jsx`) and the interpreted path keep resolving it via
/// `@almadar/ui`'s `trait-binding-resolver.ts`.
pub(crate) fn coerce_trait_knob_conditions(
    trait_def: &mut TraitDefinition,
    trait_knobs: &HashSet<String>,
    effective_config: &orbital_core_base::schema::SObject,
) {
    if trait_knobs.is_empty() {
        return;
    }
    fn is_bound(value: Option<&SExpression>) -> bool {
        matches!(value, Some(SExpression::String(_)))
    }
    fn walk(
        expr: &mut SExpression,
        trait_knobs: &HashSet<String>,
        effective_config: &orbital_core_base::schema::SObject,
    ) {
        let SExpression::Array(arr) = expr else {
            return;
        };
        if arr.len() >= 2 {
            if let Some(SExpression::String(op)) = arr.first() {
                if op == "if" || op == "when" {
                    if let SExpression::String(cond) = &arr[1] {
                        if let Some(knob) = cond.strip_prefix("@config.") {
                            if trait_knobs.contains(knob) {
                                let bound = is_bound(effective_config.get(knob));
                                arr[1] = SExpression::Bool(bound);
                            }
                        }
                    }
                }
            }
        }
        for item in arr.iter_mut() {
            walk(item, trait_knobs, effective_config);
        }
    }
    trait_def.state_machine.edit(|machine| {
        for p in machine.propositions_mut() {
            walk(p, trait_knobs, effective_config);
        }
        for transition in machine.transitions.iter_mut() {
            if let Some(guard) = transition.guard.as_mut() {
                walk(guard, trait_knobs, effective_config);
            }
            if let Some(effects) = transition.effects.as_mut() {
                for effect in effects.iter_mut() {
                    walk(effect, trait_knobs, effective_config);
                }
            }
        }
    });
    if let Some(ticks) = trait_def.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                walk(guard, trait_knobs, effective_config);
            }
            for effect in tick.effects.iter_mut() {
                walk(effect, trait_knobs, effective_config);
            }
        }
    }
}

/// Whether a literal default of the declared type `ty` is inferred narrower than the type: a `Map` (the literal reads as
/// a record of its keys), or a record, or a list of records, that declares an optional field the literal may omit.
fn literal_reads_narrower(ty: &orbital_core_base::ir::OirType) -> bool {
    use orbital_core_base::ir::OirType;
    fn peel(t: &OirType) -> &OirType {
        match t {
            OirType::Optional { inner } => peel(inner),
            other => other,
        }
    }
    fn record(t: &OirType) -> bool {
        match peel(t) {
            // A field name written as a string literal reads as a string, not as the field reference it is.
            OirType::Map { .. } | OirType::FieldRef { .. } => true,
            OirType::Struct { fields, .. } => {
                fields.iter().any(|f| !f.required || record(&f.field_type))
            }
            _ => false,
        }
    }
    match peel(ty) {
        OirType::Array { inner } => record(inner),
        other => record(other),
    }
}

/// The declared-type side-map: for each knob whose declared type is a list whose items may differ in kind
/// (`[CliCommand]`, `[SExpr]`) and whose value is a non-empty literal, the declared type as JSON text. An operator
/// reading the knob gets its literal bound by a declaring `let` (see [`declared_read`]), so the checker reads the
/// knob's type instead of one inferred from the items.
pub(crate) fn declared_list_types(
    declared: Option<&orbital_core_base::collections::IndexMap<String, ConfigField>>,
    config_subs: &orbital_core_base::schema::SObject,
) -> orbital_core_base::schema::SObject {
    let mut out = orbital_core_base::schema::SObject::new();
    let Some(declared) = declared else {
        return out;
    };
    for (knob, value) in config_subs.iter() {
        let Some(field) = declared.get(knob.as_str()) else {
            continue;
        };
        let point_in_time = match (value, field.field_type.as_str()) {
            (SExpression::String(text), "date") => {
                parse_iso(text).is_some_and(|(_, shape)| shape == IsoShape::Date)
            }
            (SExpression::String(text), "datetime" | "instant") => {
                parse_iso(text).is_some_and(|(_, shape)| shape == IsoShape::DateTime)
            }
            _ => false,
        };
        if point_in_time {
            // A knob type with no shape is refused where the trait's config schema resolves.
            let Ok(ty) = crate::phases::resolve::config_field_carried_type(field) else {
                continue;
            };
            if let Ok(text) = serde_json::to_string(&ty) {
                out.insert(knob.clone(), SExpression::string(text));
            }
            continue;
        }
        let literal = match value {
            SExpression::Array(items) => !items.is_empty(),
            SExpression::Object(_) => true,
            _ => false,
        };
        if !literal || sexpr_tree_contains_binding(value) {
            continue;
        }
        let mut field = field.clone();
        let named = |record: &Option<String>| -> Option<String> {
            let knob = record
                .as_deref()?
                .strip_prefix("@entity(")?
                .strip_suffix(')')?;
            match config_subs.get(knob)? {
                SExpression::String(name) => Some(name.clone().into_inner()),
                _ => None,
            }
        };
        if let Some(item) = field.items.as_deref_mut() {
            super::sigil_types::name_knob_records(item, &named);
        }
        for member in field.properties.iter_mut().flat_map(|p| p.values_mut()) {
            super::sigil_types::name_knob_records(member, &named);
        }
        let Ok(ty) = crate::phases::resolve::config_field_carried_type(&field) else {
            continue;
        };
        let list_type = matches!(&ty, orbital_core_base::ir::OirType::Array { .. })
            || matches!(&ty, orbital_core_base::ir::OirType::Optional { inner } if matches!(**inner, orbital_core_base::ir::OirType::Array { .. }));
        if matches!(value, SExpression::Array(_)) != list_type
            || (!crate::phases::validation::effect::needs_declaration(&ty)
                && !literal_reads_narrower(&ty))
        {
            continue;
        }
        let Ok(text) = serde_json::to_string(&ty) else {
            continue;
        };
        out.insert(knob.clone(), SExpression::string(text));
    }
    out
}

/// The local a knob's declared literal is bound to in [`declared_read`].
fn declared_local(knob: &str) -> String {
    format!("knob_{knob}")
}

/// `(let ((knob_<k> <literal> <declared type>)) @knob_<k>)`: the knob's literal read as its declared type.
fn declared_read(knob: &str, literal: &SExpression, shape: &SExpression) -> SExpression {
    let local = declared_local(knob);
    SExpression::Array(vec![
        SExpression::string("let"),
        SExpression::Array(vec![SExpression::Array(vec![
            SExpression::string(local.clone()),
            literal.clone(),
            shape.clone(),
        ].into())].into()),
        SExpression::string(format!("@{local}")),
    ].into())
}

/// [`rewrite_config_bindings`] over every expression a trait carries.
pub(crate) fn rewrite_trait_config_bindings(
    trait_def: &mut TraitDefinition,
    cfg: &orbital_core_base::schema::SObject,
    declared: &orbital_core_base::schema::SObject,
) {
    rewrite_trait_expressions(trait_def, &mut |expr| {
        rewrite_config_bindings(expr, cfg, declared)
    });
}

fn rewrite_trait_expressions(
    trait_def: &mut TraitDefinition,
    rewrite: &mut impl FnMut(&mut SExpression),
) {
    trait_def.state_machine.edit(|machine| {
        for p in machine.propositions_mut() {
            rewrite(p);
        }
        for transition in machine.transitions.iter_mut() {
            if let Some(guard) = transition.guard.as_mut() {
                rewrite(guard);
            }
            if let Some(effects) = transition.effects.as_mut() {
                for effect in effects.iter_mut() {
                    rewrite(effect);
                }
            }
        }
    });
    if let Some(ticks) = trait_def.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite(guard);
            }
            for effect in tick.effects.iter_mut() {
                rewrite(effect);
            }
        }
    }
    if let Some(listens) = trait_def.listens.as_mut() {
        listens.edit(|listens| {
            for listen in listens.iter_mut() {
                if let Some(guard) = listen.guard.as_mut() {
                    rewrite(guard);
                }
            }
        });
    }
}

/// Phase 3.A: Substitute `@config.<param>` binding strings with concrete
/// SExpression values. All other SExpression variants recurse so nested
/// `@config.X` strings inside render-ui payloads, array literals, etc. are
/// rewritten in one walk. An operator's operand reading a knob of `declared` takes that knob's literal bound by a
/// declaring `let`, and a `let` binding of one carries the declared type as its third item (see [`declared_list_types`]).
pub(crate) fn rewrite_config_bindings(
    expr: &mut SExpression,
    config_subs: &orbital_core_base::schema::SObject,
    declared: &orbital_core_base::schema::SObject,
) {
    if config_subs.is_empty()
        || !expr.any_string(|text| {
            text.strip_prefix("@config.").is_some_and(|rest| {
                config_subs.contains_key(rest)
                    || config_subs.contains_key(&rest[..rest.find('.').unwrap_or(rest.len())])
            })
        })
    {
        return;
    }
    rewrite_config_bindings_inner(expr, config_subs, declared, &mut Vec::new());
}

fn rewrite_config_bindings_inner(
    expr: &mut SExpression,
    config_subs: &orbital_core_base::schema::SObject,
    declared: &orbital_core_base::schema::SObject,
    in_progress: &mut Vec<String>,
) {
    if config_subs.is_empty() {
        return;
    }
    // The operands an operator's result is drawn from (`if` branches, `default` operands, …) hand a literal on to
    // whatever reads the result (a rendered prop), so there it stays the plain literal.
    let result_operands: Vec<usize> = expr
        .value_source_slots(&crate::phases::validation::value_walk::all_sources())
        .into_iter()
        .flatten()
        .map(|slot| match slot {
            orbital_core_base::schema::ValueSlot::Arg(i)
            | orbital_core_base::schema::ValueSlot::LambdaBody(i) => i,
        })
        .collect();
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
                if let Some(value) = config_subs
                    .get(param)
                    .filter(|_| !in_progress.iter().any(|active| active == param))
                {
                    match trailing {
                        None => {
                            // Bare `@config.<param>` — replace the whole node with the
                            // value and resolve the knobs the value reads. A knob already
                            // being substituted is not read again: its value's own
                            // `@config.<param>` belongs to the embedder's rung, so the walk
                            // ends at one substitution per knob (a self-read, or a cycle
                            // through other knobs, never recurses).
                            let param = param.to_string();
                            *expr = value.clone();
                            in_progress.push(param);
                            rewrite_config_bindings_inner(expr, config_subs, declared, in_progress);
                            in_progress.pop();
                        }
                        Some(path) => {
                            // `@config.<param>.<path>` — if the value is an
                            // object, walk the path and substitute the leaf.
                            // Otherwise leave the binding as-is so the
                            // validator can flag the shape mismatch.
                            if let SExpression::Object(_) = value {
                                if let Some(leaf) = resolve_object_path(value, path) {
                                    let leaf = leaf.clone();
                                    in_progress.push(param.to_string());
                                    *expr = leaf;
                                    rewrite_config_bindings_inner(expr, config_subs, declared, in_progress);
                                    in_progress.pop();
                                }
                            }
                        }
                    }
                }
            }
        }
        SExpression::Array(items) => {
            // An operator checks each argument against its signature. An empty list read there stays
            // `@config.<knob>`, typed by the knob's declaration: `[]` has no element type of its own. A list with
            // elements types itself and is substituted, as is any literal anywhere else (a render prop, an effect
            // option, a written row).
            let head = items.first().and_then(|h| h.as_str()).map(str::to_string);
            let is_let = head.as_deref() == Some("let");
            let reads_as_operand = head
                .as_deref()
                .is_some_and(crate::type_infer::is_value_operator);
            for (at, item) in items.iter_mut().enumerate() {
                if reads_as_operand && is_empty_list_knob(item, config_subs) {
                    continue;
                }
                // `(let ((name <value>) …) body)`: each value is read by the body as the name's type. A binding
                // whose value is a declared knob takes the knob's declared type as its own third item.
                if is_let && at == 1 {
                    if let SExpression::Array(pairs) = item {
                        for pair in pairs.iter_mut() {
                            if let SExpression::Array(parts) = pair {
                                if parts.len() == 2 {
                                    if let Some((knob, shape)) = declared_knob(&parts[1], declared)
                                    {
                                        if let Some(literal) = config_subs.get(knob) {
                                            parts[1] = literal.clone();
                                            parts.push(shape);
                                            continue;
                                        }
                                    }
                                }
                                for part in parts.iter_mut().skip(1) {
                                    if !is_empty_list_knob(part, config_subs) {
                                        rewrite_config_bindings_inner(part, config_subs, declared, in_progress);
                                    }
                                }
                                continue;
                            }
                            rewrite_config_bindings_inner(pair, config_subs, declared, in_progress);
                        }
                        continue;
                    }
                }
                if reads_as_operand
                    && !(at >= 1 && result_operands.contains(&(at - 1)))
                    && take_declared(item, config_subs, declared)
                {
                    continue;
                }
                rewrite_config_bindings_inner(item, config_subs, declared, in_progress);
            }
        }
        SExpression::Object(map) => {
            for (_k, v) in map.iter_mut() {
                rewrite_config_bindings_inner(v, config_subs, declared, in_progress);
            }
        }
    }
}

/// Whether `expr` is a bare `@config.<knob>` whose value is the empty list.
/// The knob of a bare `@config.<knob>` that `declared` holds a declared type for.
fn declared_knob<'a>(
    expr: &'a SExpression,
    declared: &orbital_core_base::schema::SObject,
) -> Option<(&'a str, SExpression)> {
    let SExpression::String(s) = expr else {
        return None;
    };
    let knob = s.strip_prefix("@config.")?;
    declared.get(knob).map(|shape| (knob, shape.clone()))
}

/// Replace a bare `@config.<knob>` of `declared` with the knob's literal bound by a declaring `let`; whether it did.
fn take_declared(
    expr: &mut SExpression,
    config_subs: &orbital_core_base::schema::SObject,
    declared: &orbital_core_base::schema::SObject,
) -> bool {
    let Some((knob, shape)) = declared_knob(expr, declared) else {
        return false;
    };
    let Some(literal) = config_subs.get(knob) else {
        return false;
    };
    *expr = declared_read(knob, literal, &shape);
    true
}

fn is_empty_list_knob(
    expr: &SExpression,
    config_subs: &orbital_core_base::schema::SObject,
) -> bool {
    let SExpression::String(s) = expr else {
        return false;
    };
    s.strip_prefix("@config.")
        .and_then(|knob| config_subs.get(knob))
        .is_some_and(|value| matches!(value, SExpression::Array(items) if items.is_empty()))
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
    cfg: &orbital_core_base::schema::SObject,
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
    cfg: &orbital_core_base::schema::SObject,
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
    pattern: &orbital_core_base::schema::SObject,
    cfg: &orbital_core_base::schema::SObject,
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
    cfg: &orbital_core_base::schema::SObject,
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
fn rewrite_entity_in_type_string(s: &str, entity_subs: &orbital_core_base::collections::IndexMap<String, String>) -> String {
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

/// Apply entity-name substitutions to every `PayloadField.field_type` AND
/// `PayloadField.entity` in the trait's emits / state-machine event
/// declarations, recursing into nested `properties` (an entity marker can
/// sit on a field several levels deep inside an anonymous struct payload).
///
/// Without this, an organism that rebinds an atom's trait via
/// `linkedEntity: NewName` keeps the OLD type literal in the declared payload
/// schema (`data: [OldName]`), and downstream codegen produces
/// `payload as { data: OldName[] }` — a reference to a type that no longer
/// exists. Live symptom from std-legal-case: handlers.ts emitting
/// `payload as { data: Matter[] }` after the `Matter → MatterCase` rebind.
///
/// `PayloadField.entity` is a SEPARATE, later-added marker (B4-C: "the
/// declared entity this field's value IS an instance of") — `field_type`
/// alone flattens to `"object"`/`"[object]"` and carries no entity name once
/// lowering flattens an entity-typed field, so `entity` is the only surviving
/// identity a consumer (e.g. `wholeRow: f.entity === entityName`) can read.
/// Missing this rewrite left a rebound trait's payload field naming its
/// ATOM's original entity after the rebind (std-notes' `NoteDelete`, rebound
/// `ConfirmAction → Note`, still had `DELETE.row.entity == "ConfirmAction"`).
/// ONE owner for both markers, so a future third one lands here too.
pub(crate) fn rewrite_payload_entity_refs(
    trait_def: &mut TraitDefinition,
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
) {
    if entity_subs.is_empty() {
        return;
    }
    fn rewrite_fields(fields: &mut [PayloadField], entity_subs: &orbital_core_base::collections::IndexMap<String, String>) {
        for field in fields.iter_mut() {
            field.field_type = rewrite_entity_in_type_string(&field.field_type, entity_subs);
            if let Some(entity) = field.entity.as_ref() {
                if let Some(renamed) = entity_subs.get(entity) {
                    field.entity = Some(renamed.clone());
                }
            }
            if let Some(projection) = field.projected_from.as_mut() {
                if let Some(renamed) = entity_subs.get(&projection.type_name) {
                    projection.type_name = renamed.clone();
                }
            }
            if let Some(props) = field.properties.as_mut() {
                rewrite_fields(props, entity_subs);
            }
        }
    }
    if let Some(emits) = trait_def.emits.as_mut() {
        emits.edit(|emits| {
            for emit in emits.iter_mut() {
                if let Some(schema) = emit.payload_schema.as_mut() {
                    rewrite_fields(schema, entity_subs);
                }
                if let Some(name) = emit.payload_entity.as_ref() {
                    if let Some(renamed) = entity_subs.get(name) {
                        emit.payload_entity = Some(renamed.clone());
                    }
                }
            }
        });
    }
    trait_def.state_machine.edit(|machine| {
        for ev in machine.events.iter_mut().flatten() {
            if let Some(schema) = ev.payload_schema.as_mut() {
                rewrite_fields(schema, entity_subs);
            }
            if let Some(name) = ev.payload_entity.as_ref() {
                if let Some(renamed) = entity_subs.get(name) {
                    ev.payload_entity = Some(renamed.clone());
                }
            }
        }
    });
    rewrite_trait_expressions(trait_def, &mut |expr| {
        rewrite_recorded_shapes(expr, &mut |shape| {
            let old = shape.clone();
            let mut renamed = BTreeMap::new();
            let referenced = std::cell::RefCell::new(HashSet::new());
            let replace = |ty: &orbital_core_base::ir::OirType| match ty {
                orbital_core_base::ir::OirType::Reference { entity } => {
                    entity_subs.get(entity).map(|name| {
                        referenced.borrow_mut().insert(entity.clone());
                        orbital_core_base::ir::OirType::Reference {
                            entity: name.clone(),
                        }
                    })
                }
                orbital_core_base::ir::OirType::Row { entity } => {
                    entity_subs.get(entity).map(|name| {
                        referenced.borrow_mut().insert(entity.clone());
                        orbital_core_base::ir::OirType::Row {
                            entity: name.clone(),
                        }
                    })
                }
                _ => None,
            };
            if let orbital_core_base::ir::OirType::Reference { entity }
            | orbital_core_base::ir::OirType::Row { entity } = &shape.ty
            {
                if let Some(name) = entity_subs.get(entity) {
                    if shape.name.as_ref() == Some(entity) {
                        shape.name = Some(name.clone());
                    }
                }
            }
            shape.ty.rewrite(&replace);
            for ty in shape.named.values_mut() {
                ty.rewrite(&replace);
            }
            for (name, ty) in std::mem::take(&mut shape.named) {
                let rewritten_name = if referenced.borrow().contains(&name) {
                    entity_subs
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| name.clone())
                } else {
                    name
                };
                renamed.insert(rewritten_name, ty);
            }
            shape.named = renamed;
            *shape != old
        });
    });
}

/// Rewrite `TraitDefinition.entity_ref_ids` (the V4 leverage-id side-map:
/// token NAME → the entity's stable id) the same way every other entity
/// substitution site is applied: a key naming an entity `entity_subs`
/// renamed becomes the RENAMED name; EVERY key (renamed or not) then has its
/// value refreshed to that (possibly renamed) name's CURRENT id in
/// `entity_ids` (the consumer-scope name→id map `linked_entity_id` is
/// re-stamped from at every call site) — never the stale id the map
/// originally carried.
///
/// The by-name lookup runs even for a key `entity_subs` does not touch:
/// `entity_subs` only covers entries a REBIND resolved (the trait's own
/// `linked_entity`, or an id-equality join via
/// `consumer_known_entity_rebind_subs`/`stale_entity_rebind_subs`). A
/// SECONDARY reference whose name happens to be unchanged between the atom
/// and the consumer (e.g. an atom's own `WikiPage` next to a consumer that
/// also declares a `WikiPage`, but as a DIFFERENT declaration with a
/// DIFFERENT id — std-wiki's `WikiAttachmentList = PageAtom.traits.
/// WikiAttachmentList -> WikiAttachment`, whose secondary `entityRefIds
/// [WikiPage]` names the ATOM's own foreign `WikiPage` id) is exactly this
/// case: no rename happens (same name both sides), so `entity_subs` never
/// carries a "WikiPage" entry, yet the id is genuinely foreign and must
/// still be joined by NAME against the consumer roster. A name `entity_ids`
/// does not know at all (a true atom-internal reference no consumer node
/// covers) falls back to the old id unchanged — the only honest choice left.
///
/// Every entity-rename call site in this crate (orbital-import splice,
/// aux-clone/`entities {}` remap, and the ordinary trait-ref `entity X`
/// rebind) must call this alongside `rewrite_identifiers`/
/// `rewrite_payload_entity_refs`/`rewrite_entity_typed_config_values`, or the
/// side-map goes stale: its keys keep naming an entity the renamed trait no
/// longer declares, and its values keep pointing at that entity's OLD id —
/// both of which `identity_normalize.rs` (and `inline/mod.rs`'s documented
/// "side-maps always cover their nodes" invariant) assume never happens.
pub(crate) fn rewrite_entity_ref_ids(
    trait_def: &mut TraitDefinition,
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
) {
    let Some(refs) = trait_def.entity_ref_ids.take() else {
        return;
    };
    let mut out: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::with_capacity(refs.len());
    for (from, old_id) in refs {
        let name = entity_subs
            .get(&from)
            .cloned()
            .unwrap_or_else(|| from.clone());
        match entity_ids.get(&name) {
            Some(id) => {
                out.insert(name, id.clone());
            }
            // A renamed key never inherits its predecessor's id: the id-first
            // reader would resolve it straight back to the pre-rename entity.
            None if name == from => {
                out.insert(name, old_id);
            }
            None => {}
        }
    }
    trait_def.entity_ref_ids = (!out.is_empty()).then_some(out);
}

/// Rewrite a trait's `config[].default` VALUE for every knob whose DECLARED
/// type is `entity` (`targetEntity : entity`, `Almadar_LOLO.md` §12) through
/// the same `entity_subs` map every other entity-rename call site applies —
/// dispatch is on `field.field_type`, never on the knob's NAME (G2,
/// `docs/Almadar_Compiler_Gaps.md` §82: `entity <Name>` renamed the primary
/// but left `targetEntity: TimeOff` naming the pre-rename upstream entity).
/// An `entity`-typed knob's default is a bare entity-name string, so a plain
/// substitution is the whole rewrite. A rewritten field's V4-W5 dual-carry
/// `ref_id` (`identity_normalize.rs`) is refreshed to the RENAMED entity's
/// current id from `entity_ids` (or cleared, same "no reliable id to carry"
/// rule `rewrite_entity_ref_ids` already applies) — never left pointing at
/// the pre-rename upstream entity's id. Belongs alongside
/// `rewrite_identifiers`/`rewrite_payload_entity_refs`/`rewrite_entity_ref_ids`
/// at every call site that builds an `entity_subs` map — the orbital-import
/// splice's `rename_entities_in_trait` and the ordinary trait-ref `entity X`
/// rebind's `apply_overrides_to_trait` both call this now, one owner.
pub(crate) fn rewrite_entity_typed_config_values(
    trait_def: &mut TraitDefinition,
    entity_subs: &orbital_core_base::collections::IndexMap<String, String>,
    entity_ids: &orbital_core_base::collections::IndexMap<String, String>,
) {
    if entity_subs.is_empty() {
        return;
    }
    let Some(config) = trait_def.config.as_mut() else {
        return;
    };
    for field in config.values_mut() {
        if field.field_type != "entity" {
            continue;
        }
        if let Some(SExpression::String(name)) = field.default.as_ref() {
            if let Some(to) = entity_subs.get(name.as_str()) {
                field.default = Some(SExpression::string(to.clone()));
                field.ref_id = entity_ids.get(to).cloned();
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
///
/// Gap C: when a call-site arg is absent, the chain falls through — for
/// Entity-kind params, first to `entity_def` (the 3-I linked-entity default,
/// unchanged), then — for ANY kind, Entity included — to the param's own
/// declared `(name : Kind = <default>)` value, resolved through the SAME
/// `resolve_type_arg_value` vocabulary a call-site arg uses. Only then does
/// the sentinel survive unresolved for the sweep. This is what lets a
/// Type-kind param validate on the declaring atom's own (parameter-less)
/// self-test page.
pub(crate) fn resolve_type_param_sentinels(
    trait_def: &mut TraitDefinition,
    entity_def: Option<&EntityDefinition>,
    entities_by_name: &HashMap<String, EntityDefinition>,
    types_by_name: &BTreeMap<String, Vec<PayloadField>>,
) -> bool {
    let type_args = trait_def.type_args.take();
    let declared: Vec<orbital_core_base::schema::TraitTypeParamDef> =
        trait_def.type_params.clone().unwrap_or_default();
    let mut subs: HashMap<String, ParamSub> = HashMap::new();
    let mut declared_names: HashSet<String> = HashSet::new();
    for p in &declared {
        declared_names.insert(p.name.clone());
        let arg_value = type_args.as_ref().and_then(|m| m.get(&p.name));
        let sub = match arg_value {
            Some(v) => resolve_type_arg_value(v, entities_by_name, types_by_name),
            None => {
                let entity_default = if p.kind == "Entity" {
                    entity_def.map(|e| ParamSub::Entity(e.clone()))
                } else {
                    None
                };
                entity_default.or_else(|| {
                    p.default
                        .as_deref()
                        .and_then(|d| resolve_type_arg_value(d, entities_by_name, types_by_name))
                })
            }
        };
        if let Some(sub) = sub {
            subs.insert(p.name.clone(), sub);
        }
    }
    let mut changed = false;
    let bindings: orbital_core_base::collections::IndexMap<String, FieldDefinition> = subs
        .iter()
        .filter_map(|(name, sub)| param_sub_field(sub).map(|f| (name.clone(), f)))
        .collect();
    let declared_bindings: BTreeMap<String, orbital_core_base::ir::OirType> = subs
        .iter()
        .filter_map(|(name, sub)| {
            // A type argument with no shape is refused where its alias or entity is declared.
            let ty = match sub {
                ParamSub::Scalar(_) => orbital_core_base::ir::field_shape(bindings.get(name)?).ok()?,
                ParamSub::Struct(fields) => {
                    orbital_core_base::ir::types::declared_alias_shape(fields).ok()?
                }
                ParamSub::Entity(entity) => orbital_core_base::ir::OirType::Reference {
                    entity: entity.name.clone(),
                },
            };
            Some((name.clone(), ty))
        })
        .collect();
    rewrite_trait_expressions(trait_def, &mut |expr| {
        changed |= rewrite_recorded_shapes(expr, &mut |shape| {
            let old = shape.clone();
            let root_variable = matches!(&shape.ty, orbital_core_base::ir::OirType::Var { name } if declared_bindings.contains_key(name.as_str()));
            shape.ty.substitute_type_vars(&declared_bindings);
            for ty in shape.named.values_mut() {
                ty.substitute_type_vars(&declared_bindings);
            }
            if root_variable {
                shape.name = match &shape.ty {
                    orbital_core_base::ir::OirType::Reference { entity } => Some(entity.clone()),
                    _ => None,
                };
            }
            let recorded = &shape.named;
            // A named type with no shape is refused where it is declared; the recorded shape stays as it was.
            let Ok(closed) = orbital_core_base::ir::types::DeclaredShape::with_resolver(
                shape.name.clone(),
                shape.ty.clone(),
                |name| {
                    if let Some(entity) = entities_by_name.get(name) {
                        return orbital_core_base::ir::types::declared_alias_shape(
                            &entity_fields_to_payload_fields(entity),
                        )
                        .map(Some);
                    }
                    if let Some(recorded) = recorded.get(name) {
                        return Ok(Some(recorded.clone()));
                    }
                    types_by_name
                        .get(name)
                        .map(|fields| orbital_core_base::ir::types::declared_alias_shape(fields))
                        .transpose()
                },
            ) else {
                return false;
            };
            *shape = closed;
            *shape != old
        });
    });
    // `field r T`: the record a type variable stands for is the entity the call site binds it to.
    if let Some(config) = trait_def.config.as_mut() {
        let bind_record = |record: &mut Option<String>| {
            let bound = record
                .as_deref()
                .and_then(|text| text.strip_prefix('$'))
                .and_then(|name| match subs.get(name) {
                    Some(ParamSub::Entity(entity)) => Some(entity.name.clone()),
                    _ => None,
                });
            if let Some(name) = bound {
                *record = Some(name);
                true
            } else {
                false
            }
        };
        for knob in config.values_mut() {
            changed |= bind_record(&mut knob.record);
            if let Some(items) = knob.items.as_deref_mut() {
                changed |= bind_record(&mut items.record);
            }
        }
    }
    if let Some(config) = trait_def.config.as_mut() {
        for knob in config.values_mut() {
            changed |= resolve_config_sentinel(knob, &subs, &bindings);
        }
    }
    if let (false, Some(config)) = (bindings.is_empty(), trait_def.config.as_mut()) {
        for knob in config.values_mut() {
            if let Some(items) = knob.items.as_deref_mut() {
                items.substitute_type_vars(&bindings);
            }
            for member in knob.properties.iter_mut().flat_map(|p| p.values_mut()) {
                member.substitute_type_vars(&bindings);
            }
        }
    }
    if let Some(emits) = trait_def.emits.as_mut() {
        emits.edit(|emits| {
            for emit in emits.iter_mut() {
                if resolve_payload_entity_schema(
                    &mut emit.payload_entity,
                    &mut emit.payload_schema,
                    entities_by_name,
                    entity_def,
                    &subs,
                    &declared_names,
                    type_args.as_ref(),
                ) {
                    changed = true;
                    continue;
                }
                if let Some(schema) = emit.payload_schema.as_mut() {
                    changed |= resolve_sentinel_fields(
                        schema,
                        entity_def,
                        &subs,
                        &declared_names,
                        entities_by_name,
                    );
                }
            }
        });
    }
    trait_def.state_machine.edit(|machine| {
        if let Some(events) = machine.events.as_mut() {
            for ev in events.iter_mut() {
                if resolve_payload_entity_schema(
                    &mut ev.payload_entity,
                    &mut ev.payload_schema,
                    entities_by_name,
                    entity_def,
                    &subs,
                    &declared_names,
                    type_args.as_ref(),
                ) {
                    changed = true;
                    continue;
                }
                if let Some(schema) = ev.payload_schema.as_mut() {
                    changed |= resolve_sentinel_fields(
                        schema,
                        entity_def,
                        &subs,
                        &declared_names,
                        entities_by_name,
                    );
                }
            }
        }
    });
    changed
}

fn rewrite_recorded_shapes(
    expr: &mut SExpression,
    rewrite: &mut impl FnMut(&mut orbital_core_base::ir::types::DeclaredShape) -> bool,
) -> bool {
    use orbital_core_base::schema::RewriteVisit;
    let mut changed = false;
    let output: Result<_, std::convert::Infallible> = expr.try_rewrite(
        |node| {
            Ok(if matches!(node.operator(), Some("quote" | "quasiquote")) {
                RewriteVisit::Keep
            } else {
                RewriteVisit::Descend
            })
        },
        |mut node| {
            if let SExpression::Array(items) = &mut node {
                if items.first().and_then(SExpression::as_str) == Some("let") {
                    if let Some(SExpression::Array(bindings)) = items.get_mut(1) {
                        for binding in bindings {
                            let SExpression::Array(parts) = binding else {
                                continue;
                            };
                            if let [_, _, SExpression::String(declaration)] = parts.as_mut_slice() {
                                if let Some(mut shape) =
                                    orbital_core_base::ir::types::DeclaredShape::parse(declaration)
                                {
                                    if rewrite(&mut shape) {
                                        *declaration = serde_json::to_string(&shape)
                                            .expect("declared shape serializes")
                                            .into();
                                        changed = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(node)
        },
    );
    match output {
        Ok(output) => *expr = output,
        Err(never) => match never {},
    }
    changed
}

/// G4 (`Almadar_Compiler_Gaps.md` §82): re-derive an entity-shaped payload
/// (`EmitDefinition`/`EventDefinition.payload_entity: Some(name)` — the
/// bare `type X = Event <EntityName>` alias form, `orbital-lolo`'s
/// `bare_entity_alias_name`) from that entity's CURRENT field set —
/// `entities_by_name` is this trait's own orbital, already post
/// `fields{}`-rename / `extend{}` by the time this runs (called after
/// `inline_orbital` fully resolves the orbital, see
/// `resolve_orbital_type_param_sentinels`). Without this, an orbital
/// import's or trait-reference's `fields{}`/`extend{}` override renamed or
/// grew the entity but the payload flattened at the ATOM's OWN compile time
/// kept describing the entity's ORIGINAL shape — `persist`'s actually-
/// emitted fields then disagree with the declared `payloadSchema`
/// (`ORB_EMIT_PAYLOAD_NOT_PRODUCED`).
///
/// Whole-payload provenance resolves concrete, implicit and explicit generic bindings here.
fn resolve_payload_entity_schema(
    payload_entity: &mut Option<String>,
    payload_schema: &mut Option<Vec<PayloadField>>,
    entities_by_name: &HashMap<String, EntityDefinition>,
    entity_def: Option<&EntityDefinition>,
    subs: &HashMap<String, ParamSub>,
    declared_names: &HashSet<String>,
    explicit_args: Option<&orbital_core_base::collections::IndexMap<String, String>>,
) -> bool {
    let Some(origin) = payload_entity.clone() else {
        return false;
    };
    let entity = if origin == "@entity" {
        entity_def
    } else if let Some(param) = origin.strip_prefix('$') {
        match subs.get(param) {
            Some(ParamSub::Entity(entity)) => {
                if explicit_args.is_some_and(|args| args.contains_key(param)) {
                    *payload_entity = Some(entity.name.clone());
                }
                Some(entity)
            }
            Some(ParamSub::Scalar(_) | ParamSub::Struct(_)) => {
                *payload_entity = None;
                None
            }
            None if !declared_names.contains(param) => entity_def,
            None => None,
        }
    } else {
        entities_by_name.get(&origin)
    };
    let Some(entity) = entity else {
        return false;
    };
    // A row payload is row-BEARING, not a complete row: the re-derived list
    // takes its names and types from the entity as it stands now, but a
    // field is required only if the contract already required it (the
    // emitting atom's own entity — a host's merged `name!` is not something a
    // `persist create` on this trait ever supplies). `@intrinsic` fields
    // never reach a stored row, so they never appear on one.
    let previous_required: HashMap<String, Option<bool>> = payload_schema
        .as_ref()
        .map(|schema| {
            schema
                .iter()
                .map(|f| (f.name.clone(), f.required))
                .collect()
        })
        .unwrap_or_default();
    let fields: Vec<PayloadField> = entity
        .fields
        .iter()
        .filter(|f| f.intrinsic != Some(true))
        .map(|f| {
            let mut field = super::sigil_types::payload_field_of(f);
            field.required = previous_required.get(&field.name).copied().flatten();
            field
        })
        .collect();
    *payload_schema = Some(fields);
    true
}

/// A resolved Increment 3-II substitution for one declared type param.
enum ParamSub {
    /// A primitive-typed call-site arg (`:: V string`) — the sentinel field
    /// becomes a scalar of this type, no `properties`.
    Scalar(String),
    /// An entity-typed call-site arg (`:: E Booking`) or the Entity-kind
    /// no-arg default (the trait's own `linked_entity`) — the sentinel field
    /// becomes `field_type: "object"` + the entity's flattened fields.
    Entity(EntityDefinition),
    /// A struct-alias-typed call-site arg (`:: p PoApprovalPayload`) —
    /// resolved against the consuming orbital's `types` map (Gap A). The
    /// sentinel field becomes `field_type: "object"` + this flattened shape,
    /// same treatment an Entity substitution gets.
    Struct(Vec<PayloadField>),
}

/// A config knob typed by a trait parameter (`config { result : V }` lowers to
/// `"$V"`, `[V]` to `"[$V]"`) takes the parameter's binding, spelled as the payload
/// sentinel's is: the primitive, or `object` with the bound shape. Unbound stays for the sweep.
fn resolve_config_sentinel(
    knob: &mut ConfigField,
    subs: &HashMap<String, ParamSub>,
    bindings: &orbital_core_base::collections::IndexMap<String, FieldDefinition>,
) -> bool {
    let (param, is_array) = match knob
        .field_type
        .strip_prefix("[$")
        .and_then(|s| s.strip_suffix(']'))
    {
        Some(p) => (p.to_string(), true),
        None => match knob.field_type.strip_prefix('$') {
            Some(p) => (p.to_string(), false),
            None => return false,
        },
    };
    let Some(sub) = subs.get(&param) else {
        return false;
    };
    let mut tag = match sub {
        ParamSub::Scalar(prim) => prim.clone(),
        ParamSub::Entity(_) | ParamSub::Struct(_) => "object".to_string(),
    };
    let shape = bindings.get(&param).cloned();
    if let Some(shape) = shape.as_ref() {
        tag = super::sigil_types::payload_field_of(shape).field_type;
    }
    if is_array {
        knob.field_type = format!("[{tag}]");
        knob.items = shape.map(Box::new);
        knob.properties = None;
        knob.additional_properties = None;
    } else {
        knob.field_type = tag;
        if let Some(shape) = shape {
            knob.nullable = compose_nullability(knob.nullable, shape.nullable);
            knob.properties = shape.properties;
            knob.items = shape.items;
            knob.additional_properties = shape.additional_properties;
            knob.values = shape.values;
        }
    }
    true
}

/// A parameter's binding as the field shape that replaces its type variable:
/// a primitive's tag, or an entity row / struct alias as an object of its fields.
fn param_sub_field(sub: &ParamSub) -> Option<FieldDefinition> {
    if let ParamSub::Struct(fields) = sub {
        if let [shape] = fields.as_slice() {
            if shape.name.is_empty() {
                let mut shape = super::sigil_types::field_of(shape);
                shape.name = None;
                return Some(shape);
            }
        }
    }
    let (field_type, properties) = match sub {
        ParamSub::Scalar(prim) => (FieldType::from_primitive_name(prim)?, None),
        ParamSub::Entity(entity) => (
            FieldType::Object,
            Some(
                entity
                    .fields
                    .iter()
                    .filter_map(|f| f.name.clone().map(|n| (n, f.clone())))
                    .collect(),
            ),
        ),
        ParamSub::Struct(fields) => (
            FieldType::Object,
            Some(
                fields
                    .iter()
                    .map(|f| (f.name.clone(), super::sigil_types::field_of(f)))
                    .collect(),
            ),
        ),
    };
    Some(FieldDefinition {
        nullable: None,
        additional_properties: None,
        mock: None,
        merged_from: None,
        name: None,
        field_type,
        required: None,
        primary_key: None,
        unique: None,
        default: None,
        values: None,
        relation: None,
        items: None,
        properties,
        intrinsic: None,
        description: None,
        synonyms: None,
        projected_from: None,
        record: None,
        derived: None,
    })
}

/// The primitive type-name vocabulary a call-site type-parameter arg may
/// name (mirrors `orbital-lolo`'s `primitive_field_type` name set — the two
/// crates don't share code, `orbital-compiler` doesn't depend on
/// `orbital-lolo`, so this is a deliberate parallel list, not a duplicate of
/// a reachable one).
const PRIMITIVE_TYPE_ARG_NAMES: &[&str] = &[
    "string",
    "money",
    "any",
    "icon",
    "component",
    "email",
    "url",
    "phone",
    "uuid",
    "image",
    "node",
    "json",
    "render-ui",
    "relation",
    "int",
    "float",
    "number",
    "duration",
    "bool",
    "boolean",
    "date",
    "datetime",
    "timestamp",
    "instant",
    "object",
    "sexpr",
];

/// Resolve a call-site type-parameter argument's VALUE (the string the
/// author wrote after the param name, e.g. `"string"` in `:: V string`, or
/// `"Booking"` in `:: E Booking`) to a substitution. A primitive name is a
/// scalar; an entity name found in `entities_by_name` is an entity
/// substitution; a struct-alias name found in `types_by_name` (Gap A) is a
/// struct substitution; anything else is unresolvable (the sentinel survives
/// for the sweep — the value named neither a primitive, a known entity, nor
/// a known struct alias).
fn resolve_type_arg_value(
    value: &str,
    entities_by_name: &HashMap<String, EntityDefinition>,
    types_by_name: &BTreeMap<String, Vec<PayloadField>>,
) -> Option<ParamSub> {
    if PRIMITIVE_TYPE_ARG_NAMES.contains(&value) {
        return Some(ParamSub::Scalar(value.to_string()));
    }
    if let Some(e) = entities_by_name.get(value) {
        return Some(ParamSub::Entity(e.clone()));
    }
    types_by_name
        .get(value)
        .map(|shape| ParamSub::Struct(shape.clone()))
}

/// Recursive worker for `resolve_type_param_sentinels`. A field whose
/// `field_type` is `"$<param>"` or `"[$<param>]"` is the sentinel; every
/// other field's `properties` (if any) is walked for a nested sentinel.
fn resolve_sentinel_fields(
    fields: &mut [PayloadField],
    entity_def: Option<&EntityDefinition>,
    subs: &HashMap<String, ParamSub>,
    declared_names: &HashSet<String>,
    entities_by_name: &HashMap<String, EntityDefinition>,
) -> bool {
    let mut changed = false;
    for field in fields.iter_mut() {
        if matches!(field.field_type.as_str(), "object" | "[object]")
            && resolve_payload_entity_schema(
                &mut field.entity,
                &mut field.properties,
                entities_by_name,
                entity_def,
                subs,
                declared_names,
                None,
            )
        {
            changed = true;
            continue;
        }
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
                // B4-V4: `PayloadField.entity` (B4-C's "the declared entity
                // this field's value IS an instance of") must be stamped
                // here too, exactly like the lowering-time literal `row :
                // Note` annotation stamps it — `field_type` alone flattens
                // to `object`/`[object]` and carries no entity name once
                // resolved, so `entity` is the only surviving identity a
                // consumer (`wholeRow: f.entity === entityName`,
                // `rewrite_payload_entity_refs`'s own rename walk) can read.
                // Stamping it here, at the trait's FINAL entity_def (post
                // rebind/sibling-pull — see this fn's caller doc), means
                // any LATER rename (orbital-import materialization, an
                // outer `-> Entity` rebind) picks it up for free through
                // the SAME `rewrite_payload_entity_refs` walk that already
                // renames the literal-annotation marker.
                field.entity = Some(entity.name.clone());
                field.items = None;
                changed = true;
            }
            continue;
        }
        let is_array_sentinel =
            field.field_type.starts_with("[$") && field.field_type.ends_with(']');
        let is_bare_sentinel = !is_array_sentinel && field.field_type.starts_with('$');
        if is_bare_sentinel || is_array_sentinel {
            let param_name = if is_array_sentinel {
                field
                    .field_type
                    .trim_start_matches("[$")
                    .trim_end_matches(']')
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
                        field.items = None;
                    }
                    ParamSub::Entity(entity) => {
                        field.field_type = if is_array_sentinel {
                            "[object]".to_string()
                        } else {
                            "object".to_string()
                        };
                        field.properties = Some(entity_fields_to_payload_fields(entity));
                        // Same class as the `@entity` self-ref stamp above —
                        // an Entity-kind type-param substitution flattens to
                        // `object`/`[object]` too and needs the SAME marker.
                        field.entity = Some(entity.name.clone());
                        field.items = None;
                    }
                    ParamSub::Struct(_) => {
                        let mut bound = super::sigil_types::payload_field_of(
                            &param_sub_field(sub).expect("a structural binding has a field shape"),
                        );
                        if is_array_sentinel {
                            field.field_type = format!("[{}]", bound.field_type);
                            field.properties = None;
                            field.additional_properties = None;
                            bound.name.clear();
                            field.items = Some(Box::new(bound));
                        } else {
                            field.field_type = bound.field_type;
                            field.nullable = compose_nullability(field.nullable, bound.nullable);
                            field.properties = bound.properties;
                            field.items = bound.items;
                            field.additional_properties = bound.additional_properties;
                            field.values = bound.values;
                        }
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
            // param of this name) — unconditional default to entity_def.
            // Stamps `field.entity` too (asymmetry fix, Stage C): arms 1
            // (`@entity` self-ref) and 2 (`ParamSub::Entity`) above already
            // stamp it — this arm resolves to the SAME `object`/`[object]`
            // + flattened-properties shape and needs the same marker so a
            // consumer (`wholeRow`, `rewrite_payload_entity_refs`'s rename
            // walk) can read it.
            if let Some(entity) = entity_def {
                field.field_type = if is_array_sentinel {
                    "[object]".to_string()
                } else {
                    "object".to_string()
                };
                field.properties = Some(entity_fields_to_payload_fields(entity));
                field.entity = Some(entity.name.clone());
                field.items = None;
                changed = true;
            }
            // No entity to substitute — leave the sentinel for the
            // end-of-inline defensive sweep to report as
            // ORB_T_UNRESOLVED_TYPE_PARAM.
            continue;
        }
        if let Some(props) = field.properties.as_mut() {
            changed |=
                resolve_sentinel_fields(props, entity_def, subs, declared_names, entities_by_name);
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
/// it — see `inline_schema_with_orbital_diagnostics`.
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
    // Gap A: cloned ahead of the mutable trait-iteration below (both would
    // otherwise borrow `orbital`) — this orbital's own `types` map is the
    // ONLY scope a `::`/default struct-alias arg resolves against; a sibling
    // orbital's aliases are not visible here (matches how `entities` above is
    // scoped to this orbital's own + auxiliary entities, not the schema).
    let types_by_name: BTreeMap<String, Vec<PayloadField>> =
        orbital.types.clone().unwrap_or_default();
    for tr in orbital.traits.iter_mut() {
        if let TraitReference::Inline(trait_def) = tr {
            let trait_def = Arc::make_mut(trait_def);
            let entity_def = trait_def
                .linked_entity
                .as_deref()
                .and_then(|name| entities.get(name));
            resolve_type_param_sentinels(trait_def, entity_def, &entities, &types_by_name);
        }
    }
}

/// One `events { FROM: TO }` rename resolved to the declaring event's stable id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventIdRename {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// An `events { FROM: TO }` map resolved once, at the scope it is declared,
/// to the stable ids of the events it renames.
#[derive(Debug, Clone, Default)]
pub(crate) struct EventScopeRenames {
    pub renames: Vec<EventIdRename>,
    to_by_id: orbital_core_base::collections::IndexMap<String, String>,
    renamed_from: HashSet<String>,
}

/// Resolve `event_subs` against every trait of one scope (a single trait for a
/// trait-scope rename, every kept trait of an import for an orbital-scope one).
/// A FROM resolves to the id of each `state_machine.events[]` declaration keyed
/// FROM and of each `emits[]` entry named FROM that carries an id. A declaration
/// keyed FROM without an id is an unstamped program: a typed error.
pub(crate) fn resolve_event_scope_renames<'a>(
    traits: impl IntoIterator<Item = &'a TraitDefinition>,
    event_subs: &orbital_core_base::collections::IndexMap<String, String>,
) -> Result<EventScopeRenames, InlineError> {
    let mut scope = EventScopeRenames::default();
    if event_subs.is_empty() {
        return Ok(scope);
    }
    for td in traits {
        for ev in td.state_machine.events.iter().flatten() {
            if let Some(to) = event_subs.get(&ev.key) {
                let id = ev.id.clone().ok_or_else(|| InlineError::UnknownEventRef {
                    trait_name: td.name.clone(),
                    event: ev.key.clone(),
                    site: "stateMachine.events[].id".to_string(),
                })?;
                scope.add(id, &ev.key, to);
            }
        }
        for emit in td.emits.iter().flatten() {
            if let (Some(to), Some(id)) = (event_subs.get(&emit.event), emit.event_id.as_ref()) {
                scope.add(id.clone(), &emit.event, to);
            }
        }
    }
    Ok(scope)
}

impl EventScopeRenames {
    fn add(&mut self, id: String, from: &str, to: &str) {
        if self.to_by_id.insert(id.clone(), to.to_string()).is_none() {
            self.renames.push(EventIdRename {
                id,
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        self.renamed_from.insert(from.to_string());
    }

    fn to_for(&self, id: Option<&String>) -> Option<&String> {
        id.and_then(|id| self.to_by_id.get(id))
    }
}

/// Apply a resolved event rename to one `TraitDefinition`. Every structured
/// reference (`state_machine.events[].id`, `transition.event_id`,
/// `emits[].event_id`, `listens[].event_id`, `listens[].triggers_id`) is
/// matched by id, never by its name string; ids are unchanged. A sourced
/// listen's `event` is renamed only when its source trait's name is in
/// `sourced_in_scope`. A transition, emit or trigger with no id whose name is a
/// renamed FROM is an unstamped program: a typed error, no name fallback; the
/// trait is untouched on error.
pub(crate) fn rewrite_trait_event_fields(
    trait_def: &mut TraitDefinition,
    scope: &EventScopeRenames,
    sourced_in_scope: Option<&HashSet<&str>>,
) -> Result<(), InlineError> {
    if scope.to_by_id.is_empty() {
        return Ok(());
    }
    let unresolved = |site: &str, event: &str| InlineError::UnknownEventRef {
        trait_name: trait_def.name.clone(),
        event: event.to_string(),
        site: site.to_string(),
    };
    let is_renamed_from = |name: &str| scope.renamed_from.contains(name);

    for t in trait_def.state_machine.transitions.iter() {
        if t.event_id.is_none() && is_renamed_from(&t.event) {
            return Err(unresolved("transitions[].eventId", &t.event));
        }
    }
    for emit in trait_def.emits.iter().flatten() {
        if emit.event_id.is_none() && is_renamed_from(&emit.event) {
            return Err(unresolved("emits[].eventId", &emit.event));
        }
    }
    for listen in trait_def.listens.iter().flatten() {
        if let (Some(triggers), None) = (listen.triggers.as_deref(), listen.triggers_id.as_ref()) {
            if is_renamed_from(triggers) {
                return Err(unresolved("listens[].triggersId", triggers));
            }
        }
    }

    trait_def.state_machine.edit(|machine| {
        if let Some(events) = machine.events.as_mut() {
            for ev in events.iter_mut() {
                if let Some(to) = scope.to_for(ev.id.as_ref()) {
                    ev.key = to.clone();
                }
            }
        }
        for transition in machine.transitions.iter_mut() {
            if let Some(to) = scope.to_for(transition.event_id.as_ref()) {
                transition.event = to.clone();
            }
        }
    });
    if let Some(emits) = trait_def.emits.as_mut() {
        emits.edit(|emits| {
            for emit in emits.iter_mut() {
                if let Some(to) = scope.to_for(emit.event_id.as_ref()) {
                    emit.event = to.clone();
                }
            }
        });
    }
    if let Some(listens) = trait_def.listens.as_mut() {
        listens.edit(|listens| {
            for listen in listens.iter_mut() {
                let event_in_scope = match listen.source.as_ref() {
                    Some(ListenSource::Trait { trait_name, .. })
                    | Some(ListenSource::Orbital { trait_name, .. }) => {
                        sourced_in_scope.is_some_and(|names| names.contains(trait_name.as_str()))
                    }
                    Some(ListenSource::Any) | None => true,
                };
                if event_in_scope {
                    if let Some(to) = scope.to_for(listen.event_id.as_ref()) {
                        listen.event = to.clone();
                    }
                }
                if let Some(to) = scope.to_for(listen.triggers_id.as_ref()) {
                    listen.triggers = Some(to.clone());
                }
            }
        });
    }
    Ok(())
}

/// Name of a trait reference regardless of kind (inline name or ref target).
pub(crate) fn trait_ref_name(tr: &TraitReference) -> Option<&str> {
    match tr {
        TraitReference::Inline(td) => Some(&td.name),
        other => other.trait_name(),
    }
}

#[cfg(test)]
mod config_self_read_tests {
    use super::*;
    use serde_json::json;

    fn expr(v: serde_json::Value) -> SExpression {
        serde_json::from_value(v).expect("expression fixture")
    }

    fn subs(items: serde_json::Value) -> orbital_core_base::schema::SObject {
        serde_json::from_value(items).expect("subs fixture")
    }

    #[test]
    fn a_knob_value_reading_its_own_name_is_substituted_once_and_the_inner_read_stays() {
        let mut body = expr(json!(["str/join", "@config.items", ","]));
        let inner = json!(["array/map", "@config.items", ["fn", "x", "@x"]]);
        let cfg = subs(json!({ "items": inner }));
        rewrite_config_bindings(&mut body, &cfg, &orbital_core_base::schema::SObject::new());
        assert_eq!(
            body,
            expr(json!(["str/join", ["array/map", "@config.items", ["fn", "x", "@x"]], ","]))
        );
    }

    #[test]
    fn a_two_knob_cycle_stops_at_the_knob_being_substituted() {
        let mut body = expr(json!(["+", "@config.a", 1]));
        let cfg = subs(json!({ "a": ["+", "@config.b", 1], "b": ["+", "@config.a", 1] }));
        rewrite_config_bindings(&mut body, &cfg, &orbital_core_base::schema::SObject::new());
        assert_eq!(body, expr(json!(["+", ["+", ["+", "@config.a", 1], 1], 1])));
    }

    #[test]
    fn control_a_knob_value_reading_another_knob_resolves_through_it() {
        let mut body = expr(json!(["str/concat", "@config.a", "!"]));
        let cfg = subs(json!({ "a": ["str/concat", "@config.b", "?"], "b": "x" }));
        rewrite_config_bindings(&mut body, &cfg, &orbital_core_base::schema::SObject::new());
        assert_eq!(body, expr(json!(["str/concat", ["str/concat", "x", "?"], "!"])));
    }
}

#[cfg(test)]
mod literal_list_tests {
    use super::*;

    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items.into())
    }

    /// The strip twin must undo exactly what the wrap adds — a literal reader
    /// fed `["list","tag"]` sees `["tag"]`; unwrapped arrays pass through.
    #[test]
    fn strip_marker_is_the_wrap_inverse() {
        let wrapped = vec![
            SExpression::String(("list".to_string()).into()),
            SExpression::String(("tag".to_string()).into()),
        ];
        let stripped = strip_literal_list_marker(&wrapped);
        assert_eq!(stripped, &wrapped[1..]);
        let bare = vec![SExpression::String(("tag".to_string()).into())];
        assert_eq!(strip_literal_list_marker(&bare), &bare[..]);
    }

    #[test]
    fn wraps_pure_literal_string_headed_list() {
        let mut v = arr(vec![
            SExpression::String(("path".to_string()).into()),
            SExpression::String(("method".to_string()).into()),
        ]);
        mark_literal_list_in_place(&mut v, "[string]");
        match &v {
            SExpression::Array(items) => {
                assert_eq!(items[0], SExpression::String(("list".to_string()).into()));
            }
            _ => panic!("expected array"),
        }
    }

    /// A rebind onto an entity whose id is not in scope must not keep the
    /// atom's entity id under the new name (JS twin: reference-resolver
    /// `rewriteEntityRefIds`; project-friday's imported chat rail resolved
    /// back to `BrowseItem`).
    #[test]
    fn renamed_entity_ref_without_known_id_drops_the_predecessor_id() {
        let mut t: TraitDefinition = serde_json::from_value(serde_json::json!({
            "name": "Rail",
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] },
            "entityRefIds": { "BrowseItem": "ent_browse", "Other": "ent_other" }
        }))
        .expect("trait");
        let subs: orbital_core_base::collections::IndexMap<String, String> =
            [("BrowseItem".to_string(), "ChannelMember".to_string())]
                .into_iter()
                .collect();
        rewrite_entity_ref_ids(&mut t, &subs, &orbital_core_base::collections::IndexMap::new());
        let refs = t.entity_ref_ids.expect("refs");
        assert!(!refs.contains_key("ChannelMember"), "{refs:?}");
        assert_eq!(refs.get("Other").map(String::as_str), Some("ent_other"));
    }

    /// §72: a string-headed array NESTED in a struct-typed config value is as
    /// ambiguous as a top-level one — `["count", "slug"]` must reach the effect
    /// body as data, not as a `(count slug)` call.
    #[test]
    fn nested_struct_array_members_are_marked_as_literal_lists() {
        let field: ConfigField = serde_json::from_value(serde_json::json!({
            "type": "[RecordSection]",
            "items": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "fields": { "type": "array", "items": { "type": "string" } }
                }
            }
        }))
        .expect("config field");
        let mut v: SExpression = serde_json::from_value(serde_json::json!([
            { "title": "Meta", "fields": ["count", "slug"] }
        ]))
        .expect("value");
        mark_config_literal_lists(&mut v, &field);
        let SExpression::Array(sections) = &v else {
            panic!("expected array")
        };
        let SExpression::Object(section) = &sections[0] else {
            panic!("expected object")
        };
        let Some(SExpression::Array(fields)) = section.get("fields") else {
            panic!("expected fields array")
        };
        assert_eq!(fields[0], SExpression::String(("list".to_string()).into()));
        assert_eq!(fields[1], SExpression::String(("count".to_string()).into()));
    }

    /// A string-headed value can never be a literal of a struct-element array
    /// (`[NavLink]` elements are objects), so it is a call — e.g. an
    /// `(if (== "kflow" "rawi") [...] [...])` whose `@config` read was folded to
    /// a literal — and must reach the runtime unwrapped.
    #[test]
    fn struct_element_slot_keeps_a_call_value_unwrapped() {
        let field: ConfigField = serde_json::from_value(serde_json::json!({
            "type": "[NavLink]",
            "items": { "type": "object", "properties": { "label": { "type": "string" }, "href": { "type": "string" } } }
        }))
        .expect("config field");
        let call = serde_json::json!(["if", ["==", "kflow", "rawi"], [{ "href": "#a", "label": "A" }], [{ "href": "#b", "label": "B" }]]);
        let mut v: SExpression = serde_json::from_value(call.clone()).expect("value");
        mark_config_literal_lists(&mut v, &field);
        let expected: SExpression = serde_json::from_value(call).expect("value");
        assert_eq!(v, expected);
    }

    #[test]
    fn control_string_element_slot_still_wraps_a_string_headed_list() {
        let field: ConfigField = serde_json::from_value(serde_json::json!({
            "type": "[string]",
            "items": { "type": "string" }
        }))
        .expect("config field");
        let mut v: SExpression =
            serde_json::from_value(serde_json::json!(["if", "else"])).expect("value");
        mark_config_literal_lists(&mut v, &field);
        let SExpression::Array(items) = &v else {
            panic!("expected array")
        };
        assert_eq!(items[0], SExpression::String(("list".to_string()).into()));
    }

    #[test]
    fn control_number_element_slot_keeps_a_call_value_unwrapped() {
        let field: ConfigField = serde_json::from_value(serde_json::json!({
            "type": "[number]",
            "items": { "type": "number" }
        }))
        .expect("config field");
        let call = serde_json::json!(["if", true, [1, 2], [3]]);
        let mut v: SExpression = serde_json::from_value(call.clone()).expect("value");
        mark_config_literal_lists(&mut v, &field);
        let expected: SExpression = serde_json::from_value(call).expect("value");
        assert_eq!(v, expected);
    }

    #[test]
    fn skips_wrap_when_tree_contains_binding() {
        // A reactive drawables S-expr composed into a `[json]` slot: the head
        // is string-headed but the tree carries `@entity` bindings — it must
        // reach the runtime as a call, not as inert data.
        let mut food = orbital_core_base::schema::SObject::new();
        food.insert(
            "x".into(),
            arr(vec![
                SExpression::String(("object/get".to_string()).into()),
                SExpression::String(("@entity.food".to_string()).into()),
                SExpression::String(("x".to_string()).into()),
            ]),
        );
        let mut cell = orbital_core_base::schema::SObject::new();
        cell.insert(
            "type".into(),
            SExpression::String(("draw-shape".to_string()).into()),
        );
        cell.insert("position".into(), SExpression::object(food));
        let mut v = arr(vec![
            SExpression::String(("array/concat".to_string()).into()),
            arr(vec![SExpression::object(cell)]),
            arr(vec![]),
        ]);
        mark_literal_list_in_place(&mut v, "[json]");
        match &v {
            SExpression::Array(items) => {
                assert_eq!(
                    items[0],
                    SExpression::String(("array/concat".to_string()).into())
                );
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn ignores_non_array_slots() {
        let mut scalar = arr(vec![SExpression::String(("path".to_string()).into())]);
        mark_literal_list_in_place(&mut scalar, "string");
        assert_eq!(
            scalar,
            arr(vec![SExpression::String(("path".to_string()).into())])
        );
    }

    #[test]
    fn a_list_knob_whose_first_item_is_an_object_is_marked_as_a_list() {
        let mut object_headed = arr(vec![
            SExpression::object(orbital_core_base::schema::SObject::new()),
            SExpression::String(("tool".to_string()).into()),
        ]);
        mark_literal_list_in_place(&mut object_headed, "[json]");
        match &object_headed {
            SExpression::Array(items) => {
                assert_eq!(items.len(), 3, "{object_headed:?}");
                assert_eq!(items[0], SExpression::String(("list".to_string()).into()));
            }
            _ => panic!("expected array"),
        }
        let mut empty = arr(vec![]);
        mark_literal_list_in_place(&mut empty, "[json]");
        assert_eq!(
            empty,
            arr(vec![]),
            "control: the empty list stays as written"
        );
    }

    fn mixed_commands_trait(effect: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Runner",
            "config": { "commands": {
                "type": "[CliCommand]",
                "default": [{ "anchor": { "env": "TOOL_PATH" } }, { "anchor": "exe-dir", "path": "tool" }, "tool"],
                "items": { "type": "union", "values": ["string", "AnchoredEnv", "AnchoredPath"], "properties": {
                    "string": { "name": "string", "type": "string" },
                    "AnchoredEnv": { "name": "AnchoredEnv", "type": "object", "properties": {
                        "anchor": { "name": "anchor", "type": "object", "required": true, "properties": {
                            "env": { "name": "env", "type": "string", "required": true } } } } },
                    "AnchoredPath": { "name": "AnchoredPath", "type": "object", "properties": {
                        "anchor": { "name": "anchor", "type": "string", "required": true },
                        "path": { "name": "path", "type": "string", "required": true } } }
                } }
            } },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "RUN", "effects": [effect] }]
            }
        }))
        .expect("trait")
    }

    fn first_effect(t: &TraitDefinition) -> SExpression {
        t.state_machine.transitions[0]
            .effects
            .as_ref()
            .expect("effects")[0]
            .clone()
    }

    #[test]
    fn a_mixed_list_knob_read_by_an_operator_keeps_its_declared_type() {
        let mut t = mixed_commands_trait(serde_json::json!([
            "set",
            "@entity.n",
            ["array/len", "@config.commands"]
        ]));
        apply_inline_config_defaults(&mut t, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&t)).unwrap();
        let arg = &effect[2][1];
        assert_eq!(arg[0], "let", "{effect}");
        let binding = &arg[1][0];
        assert!(
            binding[2].is_string(),
            "the binding declares its type: {effect}"
        );
        assert_eq!(
            arg[2], "@knob_commands",
            "the read is the declared local: {effect}"
        );
        assert_eq!(
            binding[1][0], "list",
            "the value is the marked literal: {effect}"
        );
        let declared: orbital_core_base::ir::OirType =
            serde_json::from_str(binding[2].as_str().expect("type text")).expect("an IR type");
        assert!(
            matches!(&declared, orbital_core_base::ir::OirType::Array { inner } if matches!(**inner, orbital_core_base::ir::OirType::Union { .. })),
            "{declared:?}"
        );
    }

    #[test]
    fn a_let_binding_of_a_mixed_list_knob_carries_the_declared_type_as_its_third_item() {
        let mut t = mixed_commands_trait(serde_json::json!([
            "set",
            "@entity.n",
            [
                "let",
                [["cmds", "@config.commands"]],
                ["array/len", "@cmds"]
            ]
        ]));
        apply_inline_config_defaults(&mut t, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&t)).unwrap();
        let binding = &effect[2][1][0];
        assert_eq!(binding.as_array().map(Vec::len), Some(3), "{effect}");
        assert_eq!(binding[1][0], "list", "{effect}");
        let declared: orbital_core_base::ir::OirType =
            serde_json::from_str(binding[2].as_str().expect("type text")).expect("an IR type");
        assert!(
            matches!(declared, orbital_core_base::ir::OirType::Array { .. }),
            "{declared:?}"
        );
        let mut plain = mixed_commands_trait(serde_json::json!([
            "set",
            "@entity.n",
            ["let", [["cmds", ["list", "a"]]], ["array/len", "@cmds"]]
        ]));
        apply_inline_config_defaults(&mut plain, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&plain)).unwrap();
        assert_eq!(
            effect[2][1][0].as_array().map(Vec::len),
            Some(2),
            "control: a binding of another value stays a pair: {effect}"
        );
    }

    #[test]
    fn a_declared_knob_under_an_operator_that_passes_operands_on_stays_the_plain_literal() {
        let mut t = mixed_commands_trait(serde_json::json!([
            "render-ui", "main", { "type": "data-list", "itemActions": ["if", true, "@config.commands", []] }
        ]));
        apply_inline_config_defaults(&mut t, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&t)).unwrap();
        assert_eq!(effect[2]["itemActions"][2][0], "list", "{effect}");
        let mut counted = mixed_commands_trait(serde_json::json!([
            "set",
            "@entity.n",
            ["array/len", "@config.commands"]
        ]));
        apply_inline_config_defaults(&mut counted, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&counted)).unwrap();
        assert_eq!(
            effect[2][1][0], "let",
            "control: an operand the result is not drawn from keeps the declaration: {effect}"
        );
    }

    fn knob_trait(knob: serde_json::Value, effect: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Runner",
            "config": { "knob": knob },
            "stateMachine": {
                "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "RUN", "effects": [effect] }]
            }
        }))
        .expect("trait")
    }

    fn edge_items(color_required: bool) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {
            "from": { "name": "from", "type": "string", "required": true },
            "color": { "name": "color", "type": "string", "required": color_required }
        } })
    }

    #[test]
    fn a_literal_default_that_may_omit_a_declared_optional_field_keeps_the_declared_type() {
        let read = serde_json::json!(["set", "@entity.n", ["array/len", "@config.knob"]]);
        let mut optional = knob_trait(
            serde_json::json!({ "type": "[Edge]", "default": [{ "from": "a" }], "items": edge_items(false) }),
            read.clone(),
        );
        apply_inline_config_defaults(&mut optional, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&optional)).unwrap();
        assert_eq!(effect[2][1][0], "let", "{effect}");
        let mut required = knob_trait(
            serde_json::json!({ "type": "[Edge]", "default": [{ "from": "a", "color": "red" }], "items": edge_items(true) }),
            read,
        );
        apply_inline_config_defaults(&mut required, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&required)).unwrap();
        assert_ne!(
            effect[2][1][0], "let",
            "control: a literal that fills every field reads as itself: {effect}"
        );
    }

    #[test]
    fn a_map_knob_with_a_literal_default_keeps_its_declared_map_type() {
        let mut t = knob_trait(
            serde_json::json!({ "type": "Map<string,Edge>", "default": {}, "items": edge_items(true) }),
            serde_json::json!(["set", "@entity.n", ["object/keys", "@config.knob"]]),
        );
        apply_inline_config_defaults(&mut t, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&t)).unwrap();
        assert_eq!(effect[2][1][0], "let", "{effect}");
        let declared: orbital_core_base::ir::OirType =
            serde_json::from_str(effect[2][1][1][0][2].as_str().expect("type text"))
                .expect("an IR type");
        assert!(
            matches!(declared, orbital_core_base::ir::OirType::Map { .. }),
            "{declared:?}"
        );
    }

    #[test]
    fn control_a_mixed_list_knob_outside_an_operand_is_the_plain_value() {
        let mut t = mixed_commands_trait(
            serde_json::json!(["render-ui", "main", { "type": "list", "items": "@config.commands" }]),
        );
        apply_inline_config_defaults(&mut t, &mut Vec::new());
        let effect = serde_json::to_value(first_effect(&t)).unwrap();
        assert_eq!(effect[2]["items"][0], "list", "{effect}");
    }

    /// `array/len` of a `[Cell]` knob (`type Cell = string | [Cell]`, the orbital's own recursive union) after
    /// inline and resolve, evaluated by the kernel.
    fn recursive_union_knob_len(
        default: serde_json::Value,
    ) -> Result<orbital_core_base::value::Value, String> {
        let mut schema: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "probe",
            "orbitals": [{
                "name": "ProbeOrbital",
                "types": { "Cell": [{ "name": "", "type": "union", "properties": [
                    { "name": "string", "type": "string" },
                    { "name": "array", "type": "[Cell]" }
                ] }] },
                "entity": { "name": "Probe", "persistence": "runtime", "fields": [
                    { "name": "id", "type": "string", "required": true },
                    { "name": "n", "type": "number" }
                ] },
                "traits": [{
                    "name": "Prober",
                    "linkedEntity": "Probe",
                    "config": { "items": {
                        "type": "[Cell]",
                        "default": default,
                        "items": { "type": "union", "values": ["string", "array"], "properties": {
                            "string": { "name": "string", "type": "string" },
                            "array": { "name": "array", "type": "array", "items": { "type": "union", "values": ["Cell"] } }
                        } }
                    } },
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "RUN",
                            "effects": [["set", "@entity.n", ["array/len", "@config.items"]]] }]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("probe program");
        let TraitReference::Inline(t) = &mut schema.orbitals[0].traits[0] else {
            panic!("inline trait")
        };
        let t = Arc::make_mut(t);
        apply_inline_config_defaults(t, &mut Vec::new());
        crate::phases::validation::effect::record_declared_shapes(&mut schema).expect("the probe types have shapes");
        let TraitReference::Inline(t) = &schema.orbitals[0].traits[0] else {
            panic!("inline trait")
        };
        let effect = first_effect(t);
        let SExpression::Array(parts) = &effect else {
            panic!("set effect: {effect:?}")
        };
        orbital_core_eval::evaluator::Evaluator::new()
            .evaluate(
                &parts[2],
                &orbital_core_eval::evaluator::context::BindingContext::new(),
            )
            .map_err(|e| e.to_string())
    }

    #[test]
    fn a_recursive_union_knob_list_is_checked_element_by_element() {
        assert_eq!(
            recursive_union_knob_len(serde_json::json!(["a", ["b", ["c"]], "d"])),
            Ok(orbital_core_base::value::Value::Number(3.0))
        );
    }

    #[test]
    fn control_the_kernel_does_not_check_a_knob_list_against_its_declared_union() {
        // The declared type is static: refusing an element outside the union is the validator's knob check.
        assert_eq!(
            recursive_union_knob_len(serde_json::json!(["a", ["b", 1]])),
            Ok(orbital_core_base::value::Value::Number(2.0))
        );
    }
}

#[cfg(test)]
mod conditional_event_list_tests {
    use super::*;

    fn descriptor(event: &str) -> SExpression {
        let mut dm = orbital_core_base::schema::SObject::new();
        dm.insert(
            "name".into(),
            SExpression::String(("Edit".to_string()).into()),
        );
        dm.insert(
            "event".into(),
            SExpression::String(event.to_string().into()),
        );
        SExpression::object(dm)
    }

    fn data_list_with_item_actions(item_actions: SExpression) -> SExpression {
        let mut node = orbital_core_base::schema::SObject::new();
        node.insert(
            "type".into(),
            SExpression::String(("data-list".to_string()).into()),
        );
        node.insert("itemActions".into(), item_actions);
        SExpression::object(node)
    }

    /// A call-site `events { EDIT_ITEM: EditRow }` rename must reach the
    /// descriptor's `event` field inside BOTH branches of a conditional
    /// (`if`-wrapped) `itemActions` config — not just a literal list.
    #[test]
    fn rename_reaches_descriptors_in_both_conditional_branches() {
        let item_actions = SExpression::Array(vec![
            SExpression::String(("if".to_string()).into()),
            SExpression::String(("@user.role".to_string()).into()),
            SExpression::Array(vec![descriptor("EDIT_ITEM")].into()),
            SExpression::Array(vec![descriptor("VIEW_ITEM")].into()),
        ].into());
        let mut expr = data_list_with_item_actions(item_actions);

        let entity_subs = orbital_core_base::collections::IndexMap::new();
        let mut event_subs = orbital_core_base::collections::IndexMap::new();
        event_subs.insert("EDIT_ITEM".to_string(), "EditRow".to_string());
        event_subs.insert("VIEW_ITEM".to_string(), "ViewRow".to_string());
        rewrite_identifiers(&mut expr, &entity_subs, &event_subs);

        let SExpression::Object(node) = &expr else {
            panic!("expected object");
        };
        let SExpression::Array(cond) = &node["itemActions"] else {
            panic!("expected the conditional array to survive the rewrite intact");
        };
        assert_eq!(cond[0], SExpression::String(("if".to_string()).into()));
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
        assert_eq!(
            then_event,
            Some(SExpression::String(("EditRow".to_string()).into()))
        );
        assert_eq!(
            else_event,
            Some(SExpression::String(("ViewRow".to_string()).into()))
        );
    }

    /// Literal (non-conditional) `itemActions` rename is unaffected — the
    /// conditional-aware walk must be byte-identical to the old direct-array
    /// path for the common case.
    #[test]
    fn rename_still_reaches_literal_item_actions_list() {
        let item_actions = SExpression::Array(vec![descriptor("EDIT_ITEM")].into());
        let mut expr = data_list_with_item_actions(item_actions);

        let entity_subs = orbital_core_base::collections::IndexMap::new();
        let mut event_subs = orbital_core_base::collections::IndexMap::new();
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
        assert_eq!(
            dm["event"],
            SExpression::String(("EditRow".to_string()).into())
        );
    }
}

#[cfg(test)]
mod role_literal_tests {
    use super::*;
    use serde_json::json;

    fn role_fields() -> orbital_core_base::collections::IndexMap<String, Vec<String>> {
        orbital_core_base::collections::IndexMap::from([(
            "role".to_string(),
            vec!["employee".to_string(), "approver".to_string()],
        )])
    }

    fn roles_single() -> orbital_core_base::collections::IndexMap<String, Vec<String>> {
        orbital_core_base::collections::IndexMap::from([("approver".to_string(), vec!["owner".to_string()])])
    }

    fn roles_list() -> orbital_core_base::collections::IndexMap<String, Vec<String>> {
        orbital_core_base::collections::IndexMap::from([(
            "approver".to_string(),
            vec!["owner".to_string(), "project_manager".to_string()],
        )])
    }

    fn sexpr(v: serde_json::Value) -> SExpression {
        serde_json::from_value(v).expect("valid SExpression fixture")
    }

    /// A single-target remap swaps the literal in place — the op and the
    /// sigil operand are untouched.
    #[test]
    fn equality_single_target_swaps_literal_in_place() {
        let mut expr = sexpr(json!(["=", "@user.role", "approver"]));
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(expr, sexpr(json!(["=", "@user.role", "owner"])));
    }

    /// A multi-target remap replaces the WHOLE comparison node with
    /// `array/includes`.
    #[test]
    fn equality_list_target_becomes_array_includes() {
        let mut expr = sexpr(json!(["=", "@user.role", "approver"]));
        rewrite_role_literals(&mut expr, &role_fields(), &roles_list());
        assert_eq!(
            expr,
            sexpr(json!([
                "array/includes",
                ["owner", "project_manager"],
                "@user.role"
            ]))
        );
    }

    /// The sigil may be on EITHER side of the equality.
    #[test]
    fn equality_sigil_on_the_left_is_also_matched() {
        let mut expr = sexpr(json!(["==", "approver", "@user.role"]));
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(expr, sexpr(json!(["==", "owner", "@user.role"])));
    }

    /// `!=`/`!==`/`neq` with a multi-target remap wraps the produced
    /// `array/includes` in `not` — a single-target remap keeps the original
    /// negated op untouched (the op itself already encodes the negation).
    #[test]
    fn negated_equality_list_target_wraps_in_not() {
        for op in ["!="] {
            let mut expr = sexpr(json!([op, "@user.role", "approver"]));
            rewrite_role_literals(&mut expr, &role_fields(), &roles_list());
            assert_eq!(
                expr,
                sexpr(json!([
                    "not",
                    ["array/includes", ["owner", "project_manager"], "@user.role"]
                ])),
                "op {op}"
            );
        }
    }

    #[test]
    fn negated_equality_single_target_keeps_op_no_wrap() {
        let mut expr = sexpr(json!(["!=", "@user.role", "approver"]));
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(expr, sexpr(json!(["!=", "@user.role", "owner"])));
    }

    /// `array/includes` whose haystack is a literal array and whose needle
    /// is a direct role field: each haystack literal is mapped through
    /// `roles` (a list target expands in place); an unmapped literal is
    /// left as itself; duplicates introduced by the expansion are deduped,
    /// first-seen order kept.
    #[test]
    fn array_includes_haystack_literals_expand_through_roles() {
        let mut roles = roles_list();
        roles.insert("viewer".to_string(), vec!["project_manager".to_string()]);
        let mut expr = sexpr(json!([
            "array/includes",
            ["approver", "viewer", "ghost"],
            "@user.role"
        ]));
        rewrite_role_literals(&mut expr, &role_fields(), &roles);
        assert_eq!(
            expr,
            sexpr(json!([
                "array/includes",
                ["owner", "project_manager", "ghost"],
                "@user.role"
            ])),
            "approver+viewer both expand to project_manager, deduped to one occurrence; \
             unmapped \"ghost\" survives for ORB_S_ROLE_LITERAL_NOT_MEMBER to catch"
        );
    }

    /// A literal not present in `roles` is left completely alone.
    #[test]
    fn unmapped_literal_is_untouched() {
        let mut expr = sexpr(json!(["=", "@user.role", "ghost"]));
        let before = expr.clone();
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(expr, before);
    }

    /// A field not in `role_fields` (not a role-vocabulary field) is never
    /// touched even if the literal happens to match a `roles` key.
    #[test]
    fn non_role_field_is_untouched() {
        let mut expr = sexpr(json!(["=", "@user.email", "approver"]));
        let before = expr.clone();
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(expr, before);
    }

    /// The rewrite recurses into every nested position — an `and`-wrapped
    /// guard tree still finds the role comparison inside it.
    #[test]
    fn recurses_into_nested_structures() {
        let mut expr = sexpr(json!([
            "and",
            ["=", "@user.role", "approver"],
            ["=", "@entity.active", true]
        ]));
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(
            expr,
            sexpr(json!([
                "and",
                ["=", "@user.role", "owner"],
                ["=", "@entity.active", true]
            ]))
        );
    }

    /// A dynamic per-step haystack (std-step-flow's role guard idiom,
    /// `(object/get (array/nth @config.steps i) "allowedRoles")`) is not a
    /// literal role list — the node is left completely untouched. Mirrors
    /// `validation::user_identity::literal_haystack_members` returning
    /// `None` for the identical shape.
    #[test]
    fn array_includes_dynamic_object_get_haystack_is_untouched() {
        let mut expr = sexpr(json!([
            "array/includes",
            [
                "object/get",
                ["array/nth", "@config.steps", 0],
                "allowedRoles"
            ],
            "@user.role"
        ]));
        let before = expr.clone();
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(
            expr, before,
            "a nested call in the haystack must never be treated as literal members"
        );
    }

    /// A `["list", …]` array-construction wrap (`mark_literal_list_in_place`
    /// disambiguating a config-array-override default from an operator
    /// call — both `.lolo` spellings `(a b)`/`[a, b]` lower through it) is a
    /// literal haystack whose TAIL is the member set: the `list` head is
    /// re-emitted on the rewritten node, never fed through `roles` as if it
    /// were a role name itself.
    #[test]
    fn array_includes_list_call_haystack_expands_tail_and_keeps_wrap() {
        let mut expr = sexpr(json!([
            "array/includes",
            ["list", "approver", "ghost"],
            "@user.role"
        ]));
        rewrite_role_literals(&mut expr, &role_fields(), &roles_single());
        assert_eq!(
            expr,
            sexpr(json!([
                "array/includes",
                ["list", "owner", "ghost"],
                "@user.role"
            ])),
            "\"list\" itself must never be treated as a member; \"approver\" maps to \
             \"owner\", unmapped \"ghost\" survives, and the wrap is preserved so the \
             evaluator still reads the result as inert data"
        );
    }

    fn hr_roles() -> orbital_core_base::collections::IndexMap<String, Vec<String>> {
        orbital_core_base::collections::IndexMap::from([
            (
                "hr-director".to_string(),
                vec!["owner".to_string(), "operations_director".to_string()],
            ),
            ("cfo".to_string(), vec!["owner".to_string()]),
        ])
    }

    /// A trait whose guard tests `@config.allowedRoles` against the
    /// `viewerRole` knob (bound to `@user.role`) and whose render filters
    /// `@config.navItems` by each item's (and child's) `roles`.
    fn gated_trait(
        allowed: serde_json::Value,
        nav: serde_json::Value,
        viewer_default: &str,
    ) -> TraitDefinition {
        let includes_item = |p: &str| {
            json!([
                "array/includes",
                ["object/get", format!("@{p}"), "roles", []],
                "@config.viewerRole"
            ])
        };
        serde_json::from_value(json!({
            "name": "Layout",
            "linkedEntity": "Person",
            "config": {
                "allowedRoles": { "type": "unknown", "default": allowed },
                "viewerRole": { "type": "unknown", "default": viewer_default },
                "navItems": { "type": "unknown", "default": nav }
            },
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "events": [ { "key": "INIT" } ],
                "transitions": [ {
                    "from": "idle", "to": "idle", "event": "INIT",
                    "guard": ["or",
                        ["=", ["array/len", "@config.allowedRoles"], 0],
                        ["array/includes", "@config.allowedRoles", "@config.viewerRole"]],
                    "effects": [ ["render-ui", "main", {
                        "type": "app-layout",
                        "navItems": ["array/filter", "@config.navItems",
                            ["fn", "item", ["or", ["=", ["array/len", ["object/get", "@item", "roles", []]], 0],
                                includes_item("item"),
                                ["array/some",
                                    ["array/filter", ["object/get", "@item", "children", []],
                                        ["fn", "child", includes_item("child")]],
                                    ["fn", "c", true]]]]]
                    }] ]
                } ]
            }
        }))
        .expect("valid trait fixture")
    }

    fn knob(td: &TraitDefinition, name: &str) -> serde_json::Value {
        serde_json::to_value(td.config.as_ref().unwrap()[name].default.clone().unwrap()).unwrap()
    }

    /// The knob feeding `(array/includes @config.K @config.viewerRole)` is
    /// remapped: list targets expand in place, deduped, order kept; an
    /// unmapped literal is left for `ORB_S_ROLE_LITERAL_NOT_MEMBER`.
    #[test]
    fn config_role_list_compared_to_role_source_is_remapped() {
        let mut td = gated_trait(
            json!(["hr-director", "cfo", "ghost"]),
            json!([]),
            "@user.role",
        );
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &hr_roles(),
            &HashSet::new(),
        );
        assert_eq!(
            knob(&td, "allowedRoles"),
            json!(["owner", "operations_director", "ghost"])
        );
    }

    /// A role knob read only inside ANOTHER knob's default — a detail-panel
    /// action's `when: (fn row (array/includes @config.K @config.viewerRole))`
    /// in `detailActions` — is a role-list site too, so the import remap
    /// rewrites it before config folding copies it into the render.
    fn action_gated_trait(roles: serde_json::Value, other: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(json!({
            "name": "Ledger",
            "linkedEntity": "Person",
            "config": {
                "reconcileRoles": { "type": "unknown", "default": roles },
                "otherList": { "type": "unknown", "default": other },
                "viewerRole": { "type": "unknown", "default": "@user.role" },
                "detailActions": { "type": "unknown", "default": [{
                    "label": "Reconcile",
                    "event": "RECONCILE",
                    "when": ["fn", "row", ["or",
                        ["=", ["array/len", "@config.reconcileRoles"], 0],
                        ["array/includes", "@config.reconcileRoles", "@config.viewerRole"]]]
                }] }
            },
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "events": [ { "key": "INIT" } ],
                "transitions": [ {
                    "from": "idle", "to": "idle", "event": "INIT",
                    "effects": [ ["render-ui", "main", { "type": "detail-panel", "actions": "@config.detailActions" }] ]
                } ]
            }
        }))
        .expect("valid trait fixture")
    }

    #[test]
    fn role_knob_read_inside_an_action_when_is_remapped() {
        let mut td = action_gated_trait(json!(["cfo", "hr-director"]), json!(["cfo"]));
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &hr_roles(),
            &HashSet::new(),
        );
        assert_eq!(
            knob(&td, "reconcileRoles"),
            json!(["owner", "operations_director"])
        );
        // Control: a list no role check reads is data, not a role list.
        assert_eq!(knob(&td, "otherList"), json!(["cfo"]));
    }

    /// The `["list", …]` array-construction wrap keeps its marker.
    #[test]
    fn config_role_list_keeps_list_marker() {
        let mut td = gated_trait(
            json!(["list", "cfo", "hr-director"]),
            json!([]),
            "@user.role",
        );
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &hr_roles(),
            &HashSet::new(),
        );
        assert_eq!(
            knob(&td, "allowedRoles"),
            json!(["list", "owner", "operations_director"])
        );
    }

    /// Item `roles` (and nested child `roles`) inside `array/filter … (fn …)`
    /// over the nav knob are remapped; items without `roles` and sibling
    /// keys are untouched.
    #[test]
    fn config_item_roles_through_filter_are_remapped() {
        let nav = json!([
            { "label": "A", "href": "/a", "roles": ["hr-director"],
              "children": [ { "label": "A1", "roles": ["cfo", "hr-director"] } ] },
            { "label": "B", "href": "/b" }
        ]);
        let mut td = gated_trait(json!([]), nav, "@user.role");
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &hr_roles(),
            &HashSet::new(),
        );
        assert_eq!(
            knob(&td, "navItems"),
            json!([
                { "label": "A", "href": "/a", "roles": ["owner", "operations_director"],
                  "children": [ { "label": "A1", "roles": ["owner", "operations_director"] } ] },
                { "label": "B", "href": "/b" }
            ])
        );
    }

    /// Paired control: the same lists compared against something that is not
    /// a role source (a plain string knob) are never touched.
    #[test]
    fn config_lists_not_compared_to_role_source_are_untouched() {
        let mut td = gated_trait(
            json!(["hr-director"]),
            json!([{ "label": "A", "roles": ["hr-director"] }]),
            "not-a-user-read",
        );
        let before_allowed = knob(&td, "allowedRoles");
        let before_nav = knob(&td, "navItems");
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &hr_roles(),
            &HashSet::new(),
        );
        assert_eq!(knob(&td, "allowedRoles"), before_allowed);
        assert_eq!(knob(&td, "navItems"), before_nav);
    }

    /// A non-role `@user` field (`email`) as needle is not a role source.
    #[test]
    fn config_list_compared_to_non_role_user_field_is_untouched() {
        let mut td = gated_trait(json!(["hr-director"]), json!([]), "@user.email");
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &hr_roles(),
            &HashSet::new(),
        );
        assert_eq!(knob(&td, "allowedRoles"), json!(["hr-director"]));
    }

    /// A knob that resolved out to the consumer's own app config is the
    /// consumer's vocabulary — skipped; other knobs still remap.
    #[test]
    fn schema_only_knobs_are_skipped() {
        let mut td = gated_trait(
            json!(["hr-director"]),
            json!([{ "label": "A", "roles": ["hr-director"] }]),
            "@user.role",
        );
        let skip = HashSet::from(["navItems".to_string()]);
        rewrite_role_literals_in_config_lists(&mut td, &role_fields(), &hr_roles(), &skip);
        assert_eq!(
            knob(&td, "navItems"),
            json!([{ "label": "A", "roles": ["hr-director"] }])
        );
        assert_eq!(
            knob(&td, "allowedRoles"),
            json!(["owner", "operations_director"])
        );
    }

    /// An empty `roles` map is a no-op; a non-list knob value is left alone.
    #[test]
    fn empty_roles_map_and_non_list_values_are_noops() {
        let mut td = gated_trait(
            json!(["hr-director"]),
            json!("@config.navItems"),
            "@user.role",
        );
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &orbital_core_base::collections::IndexMap::new(),
            &HashSet::new(),
        );
        assert_eq!(knob(&td, "allowedRoles"), json!(["hr-director"]));
        rewrite_role_literals_in_config_lists(
            &mut td,
            &role_fields(),
            &hr_roles(),
            &HashSet::new(),
        );
        assert_eq!(knob(&td, "navItems"), json!("@config.navItems"));
    }
}

#[cfg(test)]
mod entity_slot_tests {
    use super::*;
    use serde_json::json;

    fn sexpr(v: serde_json::Value) -> SExpression {
        serde_json::from_value(v).expect("valid SExpression fixture")
    }

    fn candidates() -> HashSet<String> {
        HashSet::from(["Other".to_string()])
    }

    #[test]
    fn collects_at_root_fetch_and_persist_positions() {
        let mut out = HashSet::new();
        collect_entity_slots(&sexpr(json!("@Other.id")), &candidates(), &mut out);
        collect_entity_slots(
            &sexpr(json!(["fetch", "Other", {}])),
            &candidates(),
            &mut out,
        );
        collect_entity_slots(
            &sexpr(json!(["persist", "create", "Other", {}])),
            &candidates(),
            &mut out,
        );
        assert_eq!(out, HashSet::from(["Other".to_string()]));
    }

    /// A name outside `candidates` (even one that structurally matches an
    /// entity-slot position) is never collected.
    #[test]
    fn ignores_non_candidate_names() {
        let mut out = HashSet::new();
        collect_entity_slots(
            &sexpr(json!(["fetch", "Unrelated", {}])),
            &candidates(),
            &mut out,
        );
        assert!(out.is_empty());
    }

    /// A `persist batch` row array is not an entity-name slot; recursion
    /// still reaches nested `@Other` tokens inside it.
    #[test]
    fn persist_batch_position_is_not_an_entity_slot_but_recursion_still_reaches_nested_tokens() {
        let mut out = HashSet::new();
        collect_entity_slots(
            &sexpr(json!(["persist", "batch", [{"ref": "@Other.id"}]])),
            &candidates(),
            &mut out,
        );
        assert!(out.contains("Other"));
    }

    /// A batch's literal op tuples and an `array/map` lambda's tuple name entities that
    /// rename with the rest of the program; entity-less shapes are left alone.
    #[test]
    fn persist_batch_op_tuples_are_entity_slots() {
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("Other".to_string(), "Renamed".to_string());
        let mut literal = sexpr(json!([
            "persist",
            "batch",
            [["create", "Other", {}], ["delete", "Other", "i"]]
        ]));
        rewrite_identifiers(&mut literal, &subs, &orbital_core_base::collections::IndexMap::new());
        assert_eq!(
            literal,
            sexpr(json!([
                "persist",
                "batch",
                [["create", "Renamed", {}], ["delete", "Renamed", "i"]]
            ]))
        );
        let mut mapped = sexpr(json!([
            "persist",
            "batch",
            [
                "array/map",
                "@payload.rows",
                ["fn", "r", ["update", "Other", "@r", {}]]
            ]
        ]));
        rewrite_identifiers(&mut mapped, &subs, &orbital_core_base::collections::IndexMap::new());
        assert_eq!(
            mapped,
            sexpr(json!([
                "persist",
                "batch",
                [
                    "array/map",
                    "@payload.rows",
                    ["fn", "r", ["update", "Renamed", "@r", {}]]
                ]
            ]))
        );
        let mut out = HashSet::new();
        collect_entity_slots(
            &sexpr(json!(["persist", "batch", [["create", "Other", {}]]])),
            &candidates(),
            &mut out,
        );
        assert!(out.contains("Other"));
    }

    /// The rename walker and the collector agree: renaming then re-collecting
    /// with the ORIGINAL candidate finds nothing left (it was renamed away).
    #[test]
    fn visit_and_collect_agree_on_position() {
        let mut expr = sexpr(json!(["fetch", "Other", {}]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("Other".to_string(), "Renamed".to_string());
        rewrite_identifiers(&mut expr, &subs, &orbital_core_base::collections::IndexMap::new());
        let mut out = HashSet::new();
        collect_entity_slots(&expr, &candidates(), &mut out);
        assert!(out.is_empty(), "the name was renamed away: {expr:?}");
    }
}

/// `rewrite_payload_entity_refs` — the ONE owner for both `PayloadField`
/// entity markers (`field_type` type-strings AND the `entity` marker itself,
/// B4-C's "the declared entity this field's value IS an instance of").
/// Called from all three entity-rename sites (`apply_overrides_to_trait`'s
/// linked-entity rebind — which also covers the same-alias pulled-sibling
/// path, since it calls the same function — `identity_normalize`, and the
/// orbital-import splice), so a white-box test of the function itself covers
/// every call site's actual substitution shape (a straight rebind map, a
/// prefix-rename map, an out-of-orbital retarget map — all just
/// `orbital_core_base::collections::IndexMap<String, String>` to this function).
#[cfg(test)]
mod payload_entity_marker_tests {
    use super::*;
    use serde_json::json;

    fn trait_with_payload_fields(fields: serde_json::Value) -> TraitDefinition {
        serde_json::from_value(json!({
            "name": "T",
            "linkedEntity": "ConfirmAction",
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "events": [ { "key": "DELETE", "payloadSchema": fields } ],
                "transitions": []
            }
        }))
        .expect("trait fixture deserializes")
    }

    fn payload_fields(td: &TraitDefinition) -> &[PayloadField] {
        td.state_machine.events.as_ref().expect("events carried")[0]
            .payload_schema
            .as_ref()
            .expect("payloadSchema carried")
    }

    /// Scalar entity-typed field (`row: ConfirmAction`, flattened to
    /// `field_type: "object"` + `entity: "ConfirmAction"`) follows a rebind —
    /// the exact std-notes `NoteDelete` shape (`ConfirmAction -> Note`).
    #[test]
    fn scalar_entity_marker_follows_rebind() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "id", "type": "string" },
            { "name": "row", "type": "object", "entity": "ConfirmAction" }
        ]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("ConfirmAction".to_string(), "Note".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        let fields = payload_fields(&td);
        assert_eq!(fields[0].entity, None, "non-entity field untouched");
        assert_eq!(
            fields[1].entity.as_deref(),
            Some("Note"),
            "scalar entity marker must follow the rebind"
        );
    }

    /// Array-form entity-typed field (`rows: [ConfirmAction]`, flattened to
    /// `field_type: "[object]"` — `entity` still names the ELEMENT type, not
    /// an array spelling) follows the same rebind.
    #[test]
    fn array_form_entity_marker_follows_rebind() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "rows", "type": "[object]", "entity": "ConfirmAction" }
        ]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("ConfirmAction".to_string(), "Note".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        assert_eq!(payload_fields(&td)[0].entity.as_deref(), Some("Note"));
    }

    /// A `T.f` payload field (`tagId : Tag.id`) names `T` in `projectedFrom`;
    /// it follows the rebind like the entity marker does. Control: a
    /// projection of an entity the rebind does not touch stays put.
    #[test]
    fn projected_from_follows_rebind() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "tagId", "type": "string", "projectedFrom": { "type": "ConfirmAction", "field": "id" } },
            { "name": "other", "type": "string", "projectedFrom": { "type": "Elsewhere", "field": "id" } },
            { "name": "named", "type": "object", "projectedFrom": { "type": "ConfirmAction" } }
        ]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("ConfirmAction".to_string(), "Note".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        let fields = payload_fields(&td);
        assert_eq!(
            fields[0]
                .projected_from
                .as_ref()
                .map(|p| p.type_name.as_str()),
            Some("Note")
        );
        assert_eq!(
            fields[0]
                .projected_from
                .as_ref()
                .and_then(|p| p.field.as_deref()),
            Some("id")
        );
        assert_eq!(
            fields[1]
                .projected_from
                .as_ref()
                .map(|p| p.type_name.as_str()),
            Some("Elsewhere")
        );
        assert_eq!(
            fields[2]
                .projected_from
                .as_ref()
                .map(|projection| projection.type_name.as_str()),
            Some("Note")
        );
        assert!(fields[2].projected_from.as_ref().unwrap().field.is_none());
    }

    /// A nested `properties` entity marker (an entity-typed field several
    /// levels inside an anonymous struct payload) is reached by recursion.
    #[test]
    fn nested_properties_entity_marker_follows_rebind() {
        let mut td = trait_with_payload_fields(json!([
            {
                "name": "wrapper",
                "type": "object",
                "properties": [
                    { "name": "inner", "type": "object", "entity": "ConfirmAction" }
                ]
            }
        ]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("ConfirmAction".to_string(), "Note".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        let outer = &payload_fields(&td)[0];
        let inner = &outer.properties.as_ref().expect("properties carried")[0];
        assert_eq!(inner.entity.as_deref(), Some("Note"));
    }

    /// The orbital-import splice's prefix-rename shape (`Widget ->
    /// LocalWidget`) — same function, different substitution map, same
    /// contract.
    #[test]
    fn entity_marker_follows_a_splice_prefix_rename() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "row", "type": "object", "entity": "Widget" }
        ]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("Widget".to_string(), "LocalWidget".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        assert_eq!(
            payload_fields(&td)[0].entity.as_deref(),
            Some("LocalWidget")
        );
    }

    /// The Stage B `entities {}` out-of-orbital retarget shape (`Other ->
    /// ConsumerOther`) — same function, same contract.
    #[test]
    fn entity_marker_follows_an_entities_block_retarget() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "row", "type": "object", "entity": "Other" }
        ]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("Other".to_string(), "ConsumerOther".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        assert_eq!(
            payload_fields(&td)[0].entity.as_deref(),
            Some("ConsumerOther")
        );
    }

    /// A field naming an entity NOT in the substitution map is left alone —
    /// only declared renames apply, never a heuristic match.
    #[test]
    fn entity_marker_naming_an_unrelated_entity_is_untouched() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "row", "type": "object", "entity": "SomeoneElse" }
        ]));
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("ConfirmAction".to_string(), "Note".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        assert_eq!(
            payload_fields(&td)[0].entity.as_deref(),
            Some("SomeoneElse")
        );
    }

    fn entity_fixture(name: &str) -> orbital_core_base::schema::EntityDefinition {
        serde_json::from_value(json!({
            "name": name,
            "persistence": "persistent",
            "fields": [ { "name": "id", "type": "string", "required": true } ]
        }))
        .expect("entity fixture deserializes")
    }

    /// B4-V4: `resolve_type_param_sentinels`' `@entity` self-ref arm
    /// (`resolve_sentinel_fields`) must stamp `PayloadField.entity`, not
    /// just `field_type`/`properties` — the same marker the LITERAL `row :
    /// Note` lowering-time annotation carries (B4-C). Before the fix,
    /// std-notes' `NotePersistor.DO_UPDATE.data` (declared `data :
    /// @entity!`) had `properties` but no `entity` key even though its
    /// source was the self-ref sentinel. Uses the `state_machine.events[]`
    /// shape a `listens { EVENT { data: @entity! } }` declaration lowers
    /// to.
    #[test]
    fn entity_sentinel_stamps_the_entity_marker() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "id", "type": "string" },
            { "name": "data", "type": "@entity" }
        ]));
        let note = entity_fixture("Note");
        let changed = resolve_type_param_sentinels(
            &mut td,
            Some(&note),
            &HashMap::new(),
            &BTreeMap::new(),
        );
        assert!(changed);
        let fields = payload_fields(&td);
        assert_eq!(fields[1].field_type, "object");
        assert!(fields[1].properties.is_some(), "entity fields spliced in");
        assert_eq!(
            fields[1].entity.as_deref(),
            Some("Note"),
            "the @entity self-ref sentinel must stamp the SAME entity marker the literal annotation does"
        );
    }

    /// The array form (`data : [@entity!]`) gets the same marker.
    #[test]
    fn array_entity_sentinel_stamps_the_entity_marker() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "rows", "type": "[@entity]" }
        ]));
        let note = entity_fixture("Note");
        resolve_type_param_sentinels(
            &mut td,
            Some(&note),
            &HashMap::new(),
            &BTreeMap::new(),
        );
        let fields = payload_fields(&td);
        assert_eq!(fields[0].field_type, "[object]");
        assert_eq!(fields[0].entity.as_deref(), Some("Note"));
    }

    /// Stage C: the "legacy alias sentinel" arm — a bare `$Name` sentinel
    /// with NO declared trait type param of that name (so it never reaches
    /// `subs`) — is the one `resolve_sentinel_fields` arm that never
    /// stamped `PayloadField.entity` while arms 1 (`@entity` self-ref) and
    /// 2 (`ParamSub::Entity`) above already do (asymmetry found during the
    /// orbital-import Stage C span/extend program). Fixed to stamp the
    /// same marker.
    #[test]
    fn legacy_alias_sentinel_stamps_the_entity_marker() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "row", "type": "$SomeUndeclaredAlias" }
        ]));
        let note = entity_fixture("Note");
        let changed = resolve_type_param_sentinels(
            &mut td,
            Some(&note),
            &HashMap::new(),
            &BTreeMap::new(),
        );
        assert!(changed);
        let fields = payload_fields(&td);
        assert_eq!(fields[0].field_type, "object");
        assert!(fields[0].properties.is_some(), "entity fields spliced in");
        assert_eq!(
            fields[0].entity.as_deref(),
            Some("Note"),
            "the legacy alias sentinel (arm 4) must stamp the SAME entity marker arms 1-2 do"
        );
    }

    /// End-to-end: the marker `resolve_type_param_sentinels` just stamped
    /// on a trait bound to `Note` must ITSELF follow a LATER entity rename
    /// (an outer `-> Entity` rebind, or the orbital-import splice) through
    /// the SAME `rewrite_payload_entity_refs` owner every other entity
    /// marker already flows through — proving "it must follow rename/
    /// rebind like the literal marker does" end-to-end, not just that the
    /// initial stamp lands.
    #[test]
    fn entity_sentinel_marker_then_follows_a_later_rebind() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "data", "type": "@entity" }
        ]));
        let note = entity_fixture("Note");
        resolve_type_param_sentinels(
            &mut td,
            Some(&note),
            &HashMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(payload_fields(&td)[0].entity.as_deref(), Some("Note"));

        // A later rename (e.g. this trait rides an orbital-import splice
        // that prefixes every closure entity) must retarget the marker
        // through the SAME general walker, not a parallel one.
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("Note".to_string(), "LocalNote".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        assert_eq!(payload_fields(&td)[0].entity.as_deref(), Some("LocalNote"));
    }

    /// A trait `(e : Type = string)` whose listened `accepted : [e]` and emitted `entries : [e]` and `entry : e` all
    /// name the parameter; `type_args` is the call site's binding, if any.
    fn generic_trait(type_args: serde_json::Value) -> TraitDefinition {
        let list = json!({
            "name": "accepted", "type": "[$e]",
            "items": { "name": "", "type": "$e", "projectedFrom": { "type": "e" } }
        });
        let mut td: TraitDefinition = serde_json::from_value(json!({
            "name": "LookupRun",
            "typeParams": [ { "name": "e", "kind": "Type", "default": "string" } ],
            "typeArgs": type_args,
            "emits": [ { "event": "OUT", "payloadSchema": [
                { "name": "entries", "type": "[$e]", "items": { "name": "", "type": "$e" } },
                { "name": "entry", "type": "$e" }
            ] } ],
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "events": [ { "key": "LOOKUP", "payloadSchema": [ list ] } ],
                "transitions": []
            }
        }))
        .expect("generic trait fixture deserializes");
        if td.type_args.as_ref().is_some_and(|args| args.is_empty()) {
            td.type_args = None;
        }
        td
    }

    fn shape_of(
        td: &TraitDefinition,
        listened: bool,
        name: &str,
    ) -> orbital_core_base::ir::OirType {
        let fields = if listened {
            payload_fields(td)
        } else {
            td.emits.as_ref().expect("emits carried")[0]
                .payload_schema
                .as_ref()
                .expect("payloadSchema carried")
        };
        let field = fields
            .iter()
            .find(|f| f.name == name)
            .expect("field carried");
        orbital_core_base::ir::types::payload_field_shape(field).unwrap()
    }

    /// A type parameter that falls back to its declared default is substituted in a listened list's element too, so the
    /// listen side and the emit side read the same type (the stale `items` kept `$e` and the emit side read `string`).
    #[test]
    fn a_defaulted_type_parameter_resolves_a_listened_list_element_like_the_emitted_field() {
        use orbital_core_base::ir::OirType;
        let mut td = generic_trait(json!({}));
        assert!(resolve_type_param_sentinels(
            &mut td,
            None,
            &HashMap::new(),
            &BTreeMap::new()
        ));
        assert_eq!(
            shape_of(&td, true, "accepted"),
            OirType::array(OirType::String)
        );
        assert_eq!(
            shape_of(&td, false, "entries"),
            OirType::array(OirType::String)
        );
        assert_eq!(shape_of(&td, false, "entry"), OirType::String);
        assert!(
            payload_fields(&td)[0].items.is_none(),
            "the sentinel element is gone with the sentinel"
        );
    }

    /// EDGE: a call-site binding that differs from the parameter's default wins on both sides.
    #[test]
    fn a_bound_type_parameter_differing_from_its_default_resolves_both_sides_to_the_binding() {
        use orbital_core_base::ir::OirType;
        let mut td = generic_trait(json!({ "e": "number" }));
        resolve_type_param_sentinels(
            &mut td,
            None,
            &HashMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            shape_of(&td, true, "accepted"),
            OirType::array(OirType::Number)
        );
        assert_eq!(
            shape_of(&td, false, "entries"),
            OirType::array(OirType::Number)
        );
        assert_eq!(shape_of(&td, false, "entry"), OirType::Number);
    }

    /// CONTROL: an unresolvable parameter (no binding, no default) keeps its sentinel on both sides, element included.
    #[test]
    fn an_unbound_type_parameter_without_a_default_stays_the_same_variable_on_both_sides() {
        use orbital_core_base::ir::OirType;
        let mut td = generic_trait(json!({}));
        td.type_params = Some(vec![orbital_core_base::schema::TraitTypeParamDef {
            name: "e".to_string(),
            kind: "Type".to_string(),
            default: None,
        }]);
        resolve_type_param_sentinels(
            &mut td,
            None,
            &HashMap::new(),
            &BTreeMap::new(),
        );
        let var = OirType::Var {
            name: "e".into(),
        };
        assert_eq!(shape_of(&td, true, "accepted"), OirType::array(var.clone()));
        assert_eq!(shape_of(&td, false, "entries"), OirType::array(var.clone()));
        assert_eq!(shape_of(&td, false, "entry"), var);
    }

    /// An Entity-kind parameter bound to an entity replaces a listened list's element with the row, dropping the
    /// stale sentinel element.
    #[test]
    fn an_entity_bound_type_parameter_drops_the_stale_list_element() {
        let mut td = trait_with_payload_fields(json!([
            { "name": "rows", "type": "[$r]", "items": { "name": "", "type": "$r" } }
        ]));
        td.type_params = Some(vec![orbital_core_base::schema::TraitTypeParamDef {
            name: "r".to_string(),
            kind: "Entity".to_string(),
            default: None,
        }]);
        let note = entity_fixture("Note");
        resolve_type_param_sentinels(
            &mut td,
            Some(&note),
            &HashMap::new(),
            &BTreeMap::new(),
        );
        let field = &payload_fields(&td)[0];
        assert_eq!(field.field_type, "[object]");
        assert_eq!(field.entity.as_deref(), Some("Note"));
        assert!(field.items.is_none());
    }

    fn trait_with_emit(
        event: &str,
        payload_entity: Option<&str>,
        schema: serde_json::Value,
    ) -> TraitDefinition {
        serde_json::from_value(json!({
            "name": "T",
            "linkedEntity": "Item",
            "emits": [ { "event": event, "payloadEntity": payload_entity, "payloadSchema": schema } ],
            "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        }))
        .expect("trait-with-emit fixture deserializes")
    }

    fn entity_with_fields(
        name: &str,
        fields: &[(&str, &str)],
    ) -> orbital_core_base::schema::EntityDefinition {
        serde_json::from_value(json!({
            "name": name,
            "persistence": "persistent",
            "fields": fields.iter().map(|(n, t)| json!({ "name": n, "type": t })).collect::<Vec<_>>()
        }))
        .expect("entity_with_fields fixture deserializes")
    }

    /// G4 (`Almadar_Compiler_Gaps.md` §82): the core of the fix. An emit
    /// stamped `payloadEntity: "Item"` (the bare `type X = Event Item`
    /// alias form) carries a STALE flattened `payloadSchema` — frozen at
    /// the atom's own compile time, before a downstream orbital-import's
    /// `fields{}` rename (`hourlyRate` -> `billRate`) and `extend{}`
    /// addition (`workspaceId`) touched the entity. `resolve_type_param_
    /// sentinels` must re-derive the schema from `entities_by_name["Item"]`
    /// — the entity as it stands AFTER those overrides — not leave the
    /// frozen list in place.
    /// §97: a persist-success row payload re-derived on a rebound host keeps
    /// the ATOM's required set — the host's merged `name!` is not required
    /// (the atom's create never supplies it) — and drops `@intrinsic` fields.
    #[test]
    fn payload_entity_marker_keeps_atom_required_flags_and_drops_intrinsic_fields() {
        let mut td = trait_with_emit(
            "PostCreated",
            Some("Message"),
            json!([
                { "name": "id", "type": "string", "required": true },
                { "name": "content", "type": "string", "required": true }
            ]),
        );
        let message: orbital_core_base::schema::EntityDefinition = serde_json::from_value(json!({
            "name": "Message",
            "persistence": "persistent",
            "fields": [
                { "name": "id", "type": "string", "required": true },
                { "name": "content", "type": "string", "required": true },
                { "name": "name", "type": "string", "required": true },
                { "name": "draft", "type": "string", "intrinsic": true }
            ]
        }))
        .expect("entity fixture deserializes");
        let mut entities = HashMap::new();
        entities.insert("Message".to_string(), message.clone());
        resolve_type_param_sentinels(&mut td, None, &entities, &BTreeMap::new());
        let fields: Vec<PayloadField> = td.emits.as_ref().expect("emits carried")[0]
            .payload_schema
            .clone()
            .expect("payloadSchema carried");
        let by_name: orbital_core_base::collections::IndexMap<&str, &PayloadField> =
            fields.iter().map(|f| (f.name.as_str(), f)).collect();
        assert_eq!(by_name["id"].required, Some(true));
        assert_eq!(by_name["content"].required, Some(true));
        assert_eq!(
            by_name["name"].required, None,
            "host-merged required must not leak into the row payload"
        );
        assert!(
            !by_name.contains_key("draft"),
            "@intrinsic fields never appear on a row payload"
        );
    }

    #[test]
    fn a_trait_parameter_binds_the_type_variables_of_its_config_shapes() {
        let trait_with = |args: serde_json::Value| -> TraitDefinition {
            serde_json::from_value(json!({
                "name": "T",
                "linkedEntity": "Item",
                "typeParams": [ { "name": "v", "kind": "Type" }, { "name": "e", "kind": "Entity" }, { "name": "w", "kind": "Type" } ],
                "typeArgs": args,
                "config": {
                    "rows": { "type": "[Row]", "items": { "type": "object", "properties": {
                        "value": { "name": "value", "type": "$v", "required": true },
                        "owner": { "name": "owner", "type": "$e" },
                        "other": { "name": "other", "type": "$w" }
                    } } }
                },
                "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
            }))
            .expect("fixture deserializes")
        };
        let item = entity_with_fields("Item", &[("id", "string"), ("title", "string")]);
        let entities = HashMap::from([("Item".to_string(), item.clone())]);
        let mut td = trait_with(json!({ "v": "number" }));
        resolve_type_param_sentinels(&mut td, Some(&item), &entities, &BTreeMap::new());
        let members = td.config.as_ref().expect("config")["rows"]
            .items
            .as_ref()
            .and_then(|i| i.properties.clone())
            .expect("row members");
        assert_eq!(members["value"].field_type, FieldType::Number);
        assert_eq!(
            members["value"].required,
            Some(true),
            "the occurrence keeps its own flags"
        );
        assert_eq!(
            members["owner"].field_type,
            FieldType::Object,
            "Entity kind defaults to the bound row"
        );
        assert!(members["owner"]
            .properties
            .as_ref()
            .expect("row")
            .contains_key("title"));
        assert_eq!(
            members["other"].field_type,
            FieldType::type_var("w"),
            "control: no argument, no binding"
        );
    }

    #[test]
    fn a_config_knob_typed_by_a_trait_parameter_takes_its_binding() {
        let mut td: TraitDefinition = serde_json::from_value(json!({
            "name": "T",
            "linkedEntity": "Item",
            "typeParams": [ { "name": "v", "kind": "Type" }, { "name": "e", "kind": "Entity" }, { "name": "w", "kind": "Type" } ],
            "typeArgs": { "v": "number" },
            "config": {
                "result": { "type": "$v" },
                "results": { "type": "[$v]" },
                "owner": { "type": "$e" },
                "owners": { "type": "[$e]" },
                "other": { "type": "$w" },
                "plain": { "type": "string" }
            },
            "stateMachine": { "states": [ { "name": "idle", "isInitial": true } ], "transitions": [] }
        }))
        .expect("fixture deserializes");
        let item = entity_with_fields("Item", &[("id", "string"), ("title", "string")]);
        let entities = HashMap::from([("Item".to_string(), item.clone())]);
        resolve_type_param_sentinels(&mut td, Some(&item), &entities, &BTreeMap::new());
        let config = td.config.as_ref().expect("config");
        assert_eq!(config["result"].field_type, "number");
        assert_eq!(config["results"].field_type, "[number]");
        assert_eq!(config["owner"].field_type, "object");
        assert!(config["owner"]
            .properties
            .as_ref()
            .expect("row")
            .contains_key("title"));
        assert_eq!(config["owners"].field_type, "[object]");
        let row = config["owners"]
            .items
            .as_ref()
            .and_then(|i| i.properties.as_ref())
            .expect("row items");
        assert!(row.contains_key("title"));
        assert_eq!(
            config["other"].field_type, "$w",
            "control: no argument, no binding"
        );
        assert_eq!(
            config["plain"].field_type, "string",
            "control: a concrete knob is untouched"
        );
    }

    #[test]
    fn payload_entity_marker_recomputes_schema_from_the_entitys_current_fields() {
        let mut td = trait_with_emit(
            "ItemUpdated",
            Some("Item"),
            json!([ { "name": "hourlyRate", "type": "number" } ]),
        );
        let item = entity_with_fields(
            "Item",
            &[
                ("id", "string"),
                ("billRate", "number"),
                ("workspaceId", "string"),
            ],
        );
        let mut entities_by_name = HashMap::new();
        entities_by_name.insert("Item".to_string(), item);
        let changed =
            resolve_type_param_sentinels(&mut td, None, &entities_by_name, &BTreeMap::new());
        assert!(changed);
        let emit = &td.emits.as_ref().expect("emits carried")[0];
        let names: Vec<&str> = emit
            .payload_schema
            .as_ref()
            .expect("payloadSchema carried")
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["id", "billRate", "workspaceId"],
            "the stale 'hourlyRate' field must be gone, replaced by the entity's CURRENT field set"
        );
    }

    /// End-to-end with a rename in between: the `payload_entity` marker
    /// itself must follow an entity rename (`rewrite_payload_entity_refs`,
    /// the same owner every other entity marker flows through — orbital-
    /// import's unconditional prefix, or an outer `-> Entity` rebind)
    /// BEFORE the recompute above looks it up, or the lookup misses by
    /// name and the schema is silently left stale.
    #[test]
    fn payload_entity_marker_follows_a_rename_then_recomputes() {
        let mut td = trait_with_emit(
            "ItemUpdated",
            Some("Item"),
            json!([ { "name": "hourlyRate", "type": "number" } ]),
        );
        let mut subs = orbital_core_base::collections::IndexMap::new();
        subs.insert("Item".to_string(), "LocalItem".to_string());
        rewrite_payload_entity_refs(&mut td, &subs);
        assert_eq!(
            td.emits.as_ref().unwrap()[0].payload_entity.as_deref(),
            Some("LocalItem")
        );

        let local_item =
            entity_with_fields("LocalItem", &[("id", "string"), ("billRate", "number")]);
        let mut entities_by_name = HashMap::new();
        entities_by_name.insert("LocalItem".to_string(), local_item);
        resolve_type_param_sentinels(&mut td, None, &entities_by_name, &BTreeMap::new());
        let names: Vec<&str> = td.emits.as_ref().unwrap()[0]
            .payload_schema
            .as_ref()
            .unwrap()
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, vec!["id", "billRate"]);
    }

    /// Defensive fallback: a `payload_entity` naming an entity NOT in this
    /// orbital's set (should not happen in practice — every stamp names the
    /// trait's own closure) leaves the payload untouched rather than
    /// panicking or clearing it.
    #[test]
    fn payload_entity_marker_naming_an_unknown_entity_is_left_untouched() {
        let mut td = trait_with_emit(
            "ItemUpdated",
            Some("Ghost"),
            json!([ { "name": "hourlyRate", "type": "number" } ]),
        );
        let changed = resolve_type_param_sentinels(
            &mut td,
            None,
            &HashMap::new(),
            &BTreeMap::new(),
        );
        assert!(!changed);
        let names: Vec<&str> = td.emits.as_ref().unwrap()[0]
            .payload_schema
            .as_ref()
            .unwrap()
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, vec!["hourlyRate"]);
    }
}

#[cfg(test)]
mod native_record_generic_metadata_tests {
    use super::*;
    use serde_json::json;

    fn alias() -> PayloadField {
        serde_json::from_value(json!({
            "name": "", "type": "object", "nullable": true,
            "properties": [{ "name": "rows", "type": "[string]", "nullable": false,
                "items": { "name": "", "type": "string", "nullable": true } }],
            "additionalProperties": { "name": "", "type": "[number]", "nullable": false,
                "items": { "name": "", "type": "number", "nullable": true } }
        }))
        .unwrap()
    }

    #[test]
    fn native_record_generic_alias_retains_its_nullable_shape() {
        let shape = param_sub_field(&ParamSub::Struct(vec![alias()])).unwrap();
        assert_eq!(shape.field_type, FieldType::Object);
        assert_eq!(shape.nullable, Some(true));
        let properties = shape.properties.unwrap();
        assert!(properties.contains_key("rows"));
        assert!(!properties.contains_key(""));
        assert_eq!(
            properties["rows"].items.as_ref().unwrap().nullable,
            Some(true)
        );
        let extra = shape.additional_properties.unwrap();
        assert_eq!(extra.field_type, FieldType::Array);
        assert_eq!(extra.items.as_ref().unwrap().nullable, Some(true));
    }

    #[test]
    fn native_record_generic_carrier_conversion_keeps_list_depth_and_scalar_kind() {
        use orbital_core_base::ir::types::{field_shape, payload_field_shape, OirType};
        for tag in ["string", "[string]", "[[string]]"] {
            let payload: PayloadField = serde_json::from_value(json!({
                "name": "result", "type": tag, "required": true, "nullable": false
            }))
            .unwrap();
            let field = super::super::sigil_types::field_of(&payload);
            assert_eq!(field_shape(&field), payload_field_shape(&payload));
            let returned = super::super::sigil_types::payload_field_of(&field);
            assert_eq!(returned.name, "result");
            assert_eq!(returned.required, Some(true));
            assert_eq!(returned.nullable, Some(false));
            assert_eq!(
                payload_field_shape(&returned).unwrap(),
                payload_field_shape(&payload).unwrap()
            );
            if tag == "string" {
                assert!(matches!(field_shape(&field).unwrap(), OirType::String));
                assert!(field.items.is_none());
            }
        }
    }

    #[test]
    fn native_record_generic_config_preserves_occurrence_and_bound_contracts() {
        let sub = ParamSub::Struct(vec![alias()]);
        let subs = HashMap::from([("V".into(), sub)]);
        let bound: FieldDefinition = serde_json::from_value(json!({
            "type": "object", "nullable": true,
            "properties": { "rows": { "type": "array", "nullable": false,
                "items": { "type": "string", "nullable": true } } },
            "additionalProperties": { "type": "array", "nullable": false,
                "items": { "type": "number", "nullable": true } }
        }))
        .unwrap();
        let bindings =
            orbital_core_base::collections::IndexMap::from([("V".into(), bound.clone())]);
        for (tag, array) in [("$V", false), ("[$V]", true)] {
            let mut knob: ConfigField = serde_json::from_value(json!({
                "type": tag, "required": true, "nullable": false
            }))
            .unwrap();
            assert!(resolve_config_sentinel(&mut knob, &subs, &bindings));
            assert_eq!(knob.required, Some(true));
            assert_eq!(knob.nullable, Some(!array));
            if array {
                assert_eq!(serde_json::to_value(&knob.items).unwrap(), json!(bound));
                assert!(knob.properties.is_none());
                assert!(knob.additional_properties.is_none());
            } else {
                assert_eq!(
                    serde_json::to_value(&knob.properties).unwrap(),
                    json!(bound.properties)
                );
                assert_eq!(
                    serde_json::to_value(&knob.additional_properties).unwrap(),
                    json!(bound.additional_properties)
                );
            }
        }
    }

    #[test]
    fn native_record_generic_payload_preserves_occurrence_and_bound_contracts() {
        let bound = alias();
        let subs = HashMap::from([("V".into(), ParamSub::Struct(vec![bound.clone()]))]);
        for (tag, array) in [("$V", false), ("[$V]", true)] {
            let mut fields: Vec<PayloadField> = serde_json::from_value(json!([{
                "name": "result", "type": tag, "required": true, "nullable": false
            }]))
            .unwrap();
            assert!(resolve_sentinel_fields(
                &mut fields,
                None,
                &subs,
                &HashSet::new(),
                &HashMap::new()
            ));
            let result = &fields[0];
            assert_eq!(result.name, "result");
            assert_eq!(result.required, Some(true));
            assert_eq!(result.nullable, Some(!array));
            if array {
                assert_eq!(result.items.as_deref(), Some(&bound));
                assert!(result.properties.is_none());
                assert!(result.additional_properties.is_none());
            } else {
                assert_eq!(result.properties, bound.properties);
                assert_eq!(result.additional_properties, bound.additional_properties);
            }
        }
    }
}

#[cfg(test)]
mod cancelled_event_rename_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_rename_reaches_the_cancelled_emit_slot() {
        let mut effect: SExpression = serde_json::from_value(json!([
            "call-service", "svc", "run", {},
            { "key": "k", "emit": { "success": "DONE", "cancelled": "STOPPED" } }
        ]))
        .unwrap();
        let event_subs: orbital_core_base::collections::IndexMap<String, String> = [
            ("DONE".to_string(), "FINISHED".to_string()),
            ("STOPPED".to_string(), "ABORTED".to_string()),
        ]
        .into_iter()
        .collect();
        rewrite_identifiers(&mut effect, &orbital_core_base::collections::IndexMap::new(), &event_subs);
        let expected: SExpression = serde_json::from_value(json!([
            "call-service", "svc", "run", {},
            { "key": "k", "emit": { "success": "FINISHED", "cancelled": "ABORTED" } }
        ]))
        .unwrap();
        assert_eq!(effect, expected);
    }
}

#[cfg(test)]
mod event_typed_union_fold_tests {
    use super::*;

    fn def(json: serde_json::Value) -> orbital_core_base::schema::FieldDefinition {
        serde_json::from_value(json).expect("field definition")
    }

    fn agent_tool_items() -> orbital_core_base::schema::FieldDefinition {
        def(
            serde_json::json!({ "type": "union", "values": ["Scope", "Input", "Read"], "properties": {
            "Scope": { "name": "Scope", "type": "object", "properties": {
                "scope": { "name": "scope", "type": "string", "required": true, "values": ["declared"] } } },
            "Input": { "name": "Input", "type": "object", "properties": {
                "event": { "name": "event", "type": "EventAddress", "required": true } } },
            "Read": { "name": "Read", "type": "object", "properties": {
                "read": { "name": "read", "type": "string", "required": true } } }
        } }),
        )
    }

    fn entry(k: &str, v: &str) -> SExpression {
        let mut m = orbital_core_base::schema::SObject::new();
        m.insert(k.into(), SExpression::String(v.to_string().into()));
        SExpression::object(m)
    }

    fn get(value: &SExpression, i: usize, k: &str) -> String {
        match value {
            SExpression::Array(a) => match &a[i] {
                SExpression::Object(m) => match m.get(k) {
                    Some(SExpression::String(s)) => s.clone().into_inner(),
                    _ => String::new(),
                },
                _ => String::new(),
            },
            _ => String::new(),
        }
    }

    #[test]
    fn an_event_address_inside_a_union_element_is_folded_once() {
        let items = agent_tool_items();
        let mut value = SExpression::Array(vec![
            entry("event", "Proposal.PROPOSE"),
            entry("scope", "declared"),
            entry("read", "Task"),
        ].into());
        let mut calls = 0;
        let touched = map_event_typed_members(&mut value, Some(&items), None, &mut |cur| {
            calls += 1;
            Some(format!("ImportEN.{cur}"))
        });
        assert!(touched);
        assert_eq!(calls, 1);
        assert_eq!(get(&value, 0, "event"), "ImportEN.Proposal.PROPOSE");
        assert_eq!(get(&value, 1, "scope"), "declared");
        assert_eq!(get(&value, 2, "read"), "Task");
    }

    #[test]
    fn control_an_event_member_of_a_plain_struct_is_still_folded() {
        let items = def(serde_json::json!({ "type": "object", "properties": {
            "event": { "name": "event", "type": "event" } } }));
        let mut value = SExpression::Array(vec![entry("event", "SAVE")].into());
        map_event_typed_members(&mut value, Some(&items), None, &mut |cur| {
            Some(format!("{cur}_X"))
        });
        assert_eq!(get(&value, 0, "event"), "SAVE_X");
    }
}

#[cfg(test)]
mod unchanged_rewrite_tests {
    use super::*;
    use serde_json::json;

    fn expr(value: serde_json::Value) -> SExpression {
        serde_json::from_value(value).expect("expression")
    }

    fn map(pairs: &[(&str, &str)]) -> orbital_core_base::collections::IndexMap<String, String> {
        pairs.iter().map(|(from, to)| (from.to_string(), to.to_string())).collect()
    }

    #[test]
    fn identifiers_that_name_no_substitution_stay_the_shared_tree() {
        let original = expr(json!(["do", ["fetch", "Other"], ["set", "@Other.name", "x"], {"entity": "Other"}]));
        let mut rewritten = original.clone();
        rewrite_identifiers(&mut rewritten, &map(&[("Old", "New")]), &map(&[("GO", "GONE")]));
        assert!(rewritten.same_node(&original), "nothing renamed means nothing copied");
    }

    #[test]
    fn control_identifiers_that_name_a_substitution_are_rewritten() {
        let mut rewritten = expr(json!(["do", ["fetch", "Old"], ["set", "@Old.name", "x"], ["emit", "GO"], {"entity": "Old"}]));
        rewrite_identifiers(&mut rewritten, &map(&[("Old", "New")]), &map(&[("GO", "GONE")]));
        assert_eq!(
            rewritten,
            expr(json!(["do", ["fetch", "New"], ["set", "@New.name", "x"], ["emit", "GONE"], {"entity": "New"}]))
        );
    }

    #[test]
    fn identifiers_edge_a_name_that_only_prefixes_a_substitution_is_left_alone() {
        let original = expr(json!(["set", "@OldSuffix.name", "x"]));
        let mut rewritten = original.clone();
        rewrite_identifiers(&mut rewritten, &map(&[("Old", "New")]), &map(&[]));
        assert!(rewritten.same_node(&original));
    }

    #[test]
    fn field_paths_of_other_fields_stay_the_shared_tree() {
        let original = expr(json!(["do", ["set", "@entity.title", "@entity.other"], ["object/get", "@entity", "name"]]));
        let mut rewritten = original.clone();
        rewrite_entity_field_paths(&mut rewritten, &map(&[("heading", "title2")]));
        assert!(rewritten.same_node(&original));
    }

    #[test]
    fn control_field_paths_of_a_renamed_field_are_rewritten() {
        let mut rewritten = expr(json!(["do", ["set", "@entity.heading.x", 1], ["object/get", "@entity", "heading"], {"field": "heading"}]));
        rewrite_entity_field_paths(&mut rewritten, &map(&[("heading", "title")]));
        assert_eq!(
            rewritten,
            expr(json!(["do", ["set", "@entity.title.x", 1], ["object/get", "@entity", "title"], {"field": "title"}]))
        );
    }

    #[test]
    fn config_bindings_of_unset_knobs_stay_the_shared_tree() {
        let original = expr(json!(["render-ui", "main", {"type": "typography", "content": "@config.missing"}]));
        let mut rewritten = original.clone();
        let mut subs = orbital_core_base::schema::SObject::new();
        subs.insert("title".into(), SExpression::string("Hello"));
        rewrite_config_bindings(&mut rewritten, &subs, &orbital_core_base::schema::SObject::new());
        assert!(rewritten.same_node(&original));
    }

    #[test]
    fn control_config_bindings_of_set_knobs_are_substituted() {
        let mut rewritten = expr(json!(["render-ui", "main", {"type": "typography", "content": "@config.title"}]));
        let mut subs = orbital_core_base::schema::SObject::new();
        subs.insert("title".into(), SExpression::string("Hello"));
        rewrite_config_bindings(&mut rewritten, &subs, &orbital_core_base::schema::SObject::new());
        assert_eq!(rewritten, expr(json!(["render-ui", "main", {"type": "typography", "content": "Hello"}])));
    }

    fn shared_trait() -> TraitDefinition {
        serde_json::from_value(json!({
            "name": "T",
            "linkedEntity": "Task",
            "emits": [ { "event": "DONE", "payloadSchema": [ { "name": "row", "type": "Task" } ] } ],
            "listens": [ { "event": "GO", "triggers": "GO" } ],
            "stateMachine": {
                "states": [ { "name": "idle", "isInitial": true } ],
                "events": [ { "key": "GO", "payloadSchema": [ { "name": "row", "type": "Task" } ] } ],
                "transitions": [ {
                    "from": "idle", "to": "idle", "event": "GO",
                    "effects": [ ["render-ui", "main", {"type": "typography", "content": "@config.title"}] ]
                } ]
            }
        }))
        .expect("trait fixture deserializes")
    }

    fn shares_all(a: &TraitDefinition, b: &TraitDefinition) -> bool {
        orbital_core_base::value::Shared::ptr_eq(&a.state_machine, &b.state_machine)
            && orbital_core_base::value::Shared::ptr_eq(a.emits.as_ref().expect("emits"), b.emits.as_ref().expect("emits"))
            && orbital_core_base::value::Shared::ptr_eq(
                a.listens.as_ref().expect("listens"),
                b.listens.as_ref().expect("listens"),
            )
    }

    #[test]
    fn a_trait_whose_rewrites_find_nothing_keeps_sharing_its_machine_emits_and_listens() {
        let source = shared_trait();
        let mut copy = source.clone();
        assert!(shares_all(&source, &copy), "a clone shares before any rewrite");
        rewrite_payload_entity_refs(&mut copy, &map(&[("Other", "Renamed")]));
        let mut subs = orbital_core_base::schema::SObject::new();
        subs.insert("unrelated".into(), SExpression::string("x"));
        rewrite_trait_config_bindings(&mut copy, &subs, &orbital_core_base::schema::SObject::new());
        assert!(shares_all(&source, &copy), "rewrites that match nothing must not copy the trait body");
    }

    #[test]
    fn control_a_rewrite_that_matches_copies_only_the_part_it_changed() {
        let source = shared_trait();
        let mut copy = source.clone();
        let mut subs = orbital_core_base::schema::SObject::new();
        subs.insert("title".into(), SExpression::string("Hello"));
        rewrite_trait_config_bindings(&mut copy, &subs, &orbital_core_base::schema::SObject::new());
        assert!(!orbital_core_base::value::Shared::ptr_eq(&source.state_machine, &copy.state_machine));
        assert!(orbital_core_base::value::Shared::ptr_eq(
            source.emits.as_ref().expect("emits"),
            copy.emits.as_ref().expect("emits")
        ));
        assert_eq!(
            serde_json::to_string(&source.state_machine).expect("source").contains("@config.title"),
            true,
            "the source keeps its own body"
        );
        assert!(serde_json::to_string(&copy.state_machine).expect("copy").contains("Hello"));
    }

    #[test]
    fn payload_entity_renames_copy_the_emits_and_events_they_touch_and_leave_the_source() {
        let source = shared_trait();
        let mut copy = source.clone();
        rewrite_payload_entity_refs(&mut copy, &map(&[("Task", "Job")]));
        let payload_types = |td: &TraitDefinition| {
            let emit = td.emits.as_ref().expect("emits")[0].payload_schema.as_ref().expect("schema")[0].field_type.clone();
            let event = td.state_machine.events.as_ref().expect("events")[0].payload_schema.as_ref().expect("schema")[0]
                .field_type
                .clone();
            (emit, event)
        };
        assert_eq!(payload_types(&source), ("Task".to_string(), "Task".to_string()));
        assert_eq!(payload_types(&copy), ("Job".to_string(), "Job".to_string()));
    }

    #[test]
    fn edge_an_empty_substitution_map_and_a_trait_without_emits_or_listens_share_without_panicking() {
        let mut source = shared_trait();
        source.emits = None;
        source.listens = None;
        let mut copy = source.clone();
        rewrite_payload_entity_refs(&mut copy, &map(&[]));
        rewrite_payload_entity_refs(&mut copy, &map(&[("Task", "Job")]));
        assert!(copy.emits.is_none() && copy.listens.is_none());
        assert!(!orbital_core_base::value::Shared::ptr_eq(&source.state_machine, &copy.state_machine));
    }
}
