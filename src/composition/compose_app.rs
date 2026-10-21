//! An app composed from its per-orbital files: the rules no single orbital can see. Global page routes and
//! inline trait names stay unique (`dedupe_composed_surface`), exactly one `[identity]` entity survives
//! (`dedupe_composed_identity`, FIX-K/L/N), organisms are namespaces (a later organism's colliding entity,
//! collection or route moves under its prefix), the organisms' config is unioned, and every `navItems` list is
//! narrowed to pages the app owns. `program/compose` is this, then `compose_behaviors` for pages and layout.
//!
//! The one implementation (the `@almadar/core` `compose-app.ts` it replaces is deleted). It works on JSON values
//! so field order and optional-field presence are the program's own; golden cases recorded from the original
//! live in `tests/fixtures/compose-app/`.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::compose_behaviors::{compose_behaviors, ComposeBehaviorsInput};
use super::event_wiring::EventWiringEntry;

/// The registry answers composition reads. Each is optional, and its absence changes what composition does
/// (no `organism_of`: no namespacing and no landing nav).
#[derive(Default)]
pub struct ComposeLookups<'a> {
    /// The organism an orbital came from.
    pub organism_of: Option<&'a dyn Fn(&str) -> Option<String>>,
    /// An orbital's position among its organism's declared orbitals.
    pub organism_order_of: Option<&'a dyn Fn(&str) -> Option<f64>>,
    /// The atom's own declaration of a trait a reference names (`Alias.traits.T` through the orbital's `uses`).
    pub atom_trait_of: Option<&'a dyn Fn(&Value, &str) -> Option<Value>>,
    /// An orbital import's source orbital.
    pub imported_orbital_of: Option<&'a dyn Fn(&Value) -> Option<Value>>,
    /// The program an orbital import's source orbital belongs to.
    pub imported_program_of: Option<&'a dyn Fn(&Value) -> Option<Value>>,
}

/// `compose_app_from_files`'s options beyond the lookups.
#[derive(Default)]
pub struct ComposeAppOptions {
    pub app_name: String,
    pub layout_strategy: Option<String>,
    pub event_wiring: Option<Vec<EventWiringEntry>>,
    /// The timestamp ledger renames record.
    pub now: String,
}

// ============================================================================
// JSON helpers
// ============================================================================

fn str_of<'v>(v: &'v Value, key: &str) -> Option<&'v str> {
    v.get(key).and_then(Value::as_str)
}

fn arr_of<'v>(v: &'v Value, key: &str) -> &'v [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn is_schema(v: &Value) -> bool {
    v.get("orbitals").is_some_and(Value::is_array)
}

/// `asDefinitions`: a schema's orbitals, or the definition itself.
pub(crate) fn as_definitions(files: &[Value]) -> Vec<Value> {
    files
        .iter()
        .flat_map(|f| {
            if is_schema(f) {
                arr_of(f, "orbitals").to_vec()
            } else {
                vec![f.clone()]
            }
        })
        .collect()
}

/// `isEntityReferenceAny`: a string reference or an entity call (an object carrying `extends`).
fn is_entity_reference_any(entity: &Value) -> bool {
    match entity {
        Value::String(_) => true,
        Value::Object(o) => o.contains_key("extends"),
        _ => false,
    }
}

/// The object-form entity an orbital declares.
fn inline_entity(def: &Value) -> Option<&Value> {
    def.get("entity")
        .filter(|e| e.is_object() && !is_entity_reference_any(e))
}

fn inline_entities_of(def: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = inline_entity(def).cloned().into_iter().collect();
    for e in arr_of(def, "auxiliaryEntities") {
        if !is_entity_reference_any(e) {
            out.push(e.clone());
        }
    }
    out
}

/// `orbitalEntityName`: the name of the entity an orbital declares (object form only).
pub fn orbital_entity_name(def: &Value) -> Option<String> {
    def.get("entity")
        .filter(|e| e.is_object())
        .and_then(|e| str_of(e, "name"))
        .map(str::to_string)
}

/// `isPageReference`: a string, or an object with `ref` and no `name`.
fn is_page_reference(page: &Value) -> bool {
    match page {
        Value::String(_) => true,
        Value::Object(o) => o.contains_key("ref") && !o.contains_key("name"),
        _ => false,
    }
}

fn strip_orbital_suffix(name: &str) -> &str {
    name.strip_suffix("Orbital").unwrap_or(name)
}

/// `orbitalRouteSlug`: `ExpenseDashboardOrbital` → `expense-dashboard`.
pub fn orbital_route_slug(orbital_name: &str) -> String {
    let base = strip_orbital_suffix(orbital_name);
    let chars: Vec<char> = base.chars().collect();
    let mut kebab = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0
            && c.is_ascii_uppercase()
            && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit())
        {
            kebab.push('-');
        }
        kebab.push(*c);
    }
    let mut out = String::new();
    let mut in_run = false;
    for c in kebab.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out.to_lowercase().trim_matches('-').to_string()
}

/// `humanizeKey`: `team-members` / `teamMembers` → `Team Members`.
pub fn humanize_key(key: &str) -> String {
    let mut spaced = String::new();
    let mut prev_sep = false;
    for c in key.chars() {
        if c == '-' || c == '_' {
            if !prev_sep {
                spaced.push(' ');
            }
            prev_sep = true;
        } else {
            spaced.push(c);
            prev_sep = false;
        }
    }
    let chars: Vec<char> = spaced.chars().collect();
    let mut step1 = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0
            && c.is_ascii_uppercase()
            && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit())
        {
            step1.push(' ');
        }
        step1.push(*c);
    }
    let chars: Vec<char> = step1.chars().collect();
    let mut step2 = String::new();
    for (i, c) in chars.iter().enumerate() {
        let splits = i > 0
            && chars[i - 1].is_ascii_uppercase()
            && c.is_ascii_uppercase()
            && chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
        if splits {
            step2.push(' ');
        }
        step2.push(*c);
    }
    step2
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(|p| {
            if p.chars().all(|c| c.is_ascii_uppercase()) {
                p.to_string()
            } else {
                let mut cs = p.chars();
                match cs.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_free_name(base: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|c| !taken.contains(c))
        .expect("an unused name")
}

fn first_free_route(slug: &str, taken: &HashSet<String>) -> String {
    let base = format!("/{slug}");
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .expect("an unused route")
}

// ============================================================================
// Structural rewrites
// ============================================================================

fn rewrite_inline_trait_name(node: &mut Value, old: &str, new: &str) {
    match node {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|i| rewrite_inline_trait_name(i, old, new)),
        Value::Object(o) => {
            for key in ["name", "ref", "trait"] {
                if o.get(key).and_then(Value::as_str) == Some(old) {
                    o.insert(key.to_string(), Value::String(new.to_string()));
                }
            }
            if let Some(Value::Array(traits)) = o.get_mut("traits") {
                for t in traits.iter_mut() {
                    if t.as_str() == Some(old) {
                        *t = Value::String(new.to_string());
                    }
                }
            }
            for v in o.values_mut() {
                rewrite_inline_trait_name(v, old, new);
            }
        }
        _ => {}
    }
}

fn rewrite_own_route(node: &mut Value, old: &str, new: &str) {
    match node {
        Value::Array(items) => {
            if items.first().and_then(Value::as_str) == Some("navigate")
                && items.get(1).and_then(Value::as_str) == Some(old)
            {
                items[1] = Value::String(new.to_string());
            }
            items
                .iter_mut()
                .for_each(|i| rewrite_own_route(i, old, new));
        }
        Value::Object(o) => {
            if o.get("path").and_then(Value::as_str) == Some(old) {
                o.insert("path".to_string(), Value::String(new.to_string()));
            }
            o.values_mut().for_each(|v| rewrite_own_route(v, old, new));
        }
        _ => {}
    }
}

/// `rewriteTraitRefsInTree`: every `@trait.<from>` token (with its `.`/`[` tail kept) renamed.
fn rewrite_trait_refs_in_tree(node: &Value, renames: &HashMap<String, String>) -> Value {
    match node {
        Value::String(s) => Value::String(rewrite_trait_ref_token(s, renames)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|i| rewrite_trait_refs_in_tree(i, renames))
                .collect(),
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), rewrite_trait_refs_in_tree(v, renames)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn rewrite_trait_ref_token(value: &str, renames: &HashMap<String, String>) -> String {
    let Some(rest) = value.strip_prefix("@trait.") else {
        return value.to_string();
    };
    let split_at = rest.find(['.', '[']);
    let name = split_at.map_or(rest, |i| &rest[..i]);
    match renames.get(name) {
        Some(to) => format!("@trait.{to}{}", split_at.map_or("", |i| &rest[i..])),
        None => value.to_string(),
    }
}

// ============================================================================
// Surface dedupe
// ============================================================================

/// Compose-time backstop for the global-uniqueness collisions a per-orbital author cannot see: duplicate page
/// routes, page names and inline trait names. The first orbital (roster order) to claim one keeps it; later
/// claimants are re-slugged or orbital-prefixed with their references rewritten. Mutates `orbitals`; returns the
/// renames `{ routes, traits, pageNames }`.
pub fn dedupe_composed_surface(orbitals: &mut [Value]) -> Value {
    let mut routes: Vec<Value> = Vec::new();
    let mut traits: Vec<Value> = Vec::new();
    let mut page_names: Vec<Value> = Vec::new();
    let mut seen_routes: HashSet<String> = HashSet::new();
    let mut seen_traits: HashSet<String> = HashSet::new();
    let mut seen_page_names: HashSet<String> = HashSet::new();

    for (i, orbital) in orbitals.iter_mut().enumerate() {
        let mut node = orbital.clone();
        let orbital_name = str_of(&node, "name")
            .map(str::to_string)
            .unwrap_or_else(|| format!("orbital-{i}"));
        let mut mutated = false;

        let own_routes: Vec<String> = arr_of(&node, "pages")
            .iter()
            .filter_map(|p| {
                str_of(p, "path")
                    .filter(|_| p.is_object())
                    .map(str::to_string)
            })
            .collect();
        let mut reserved_routes: HashSet<String> = seen_routes
            .iter()
            .cloned()
            .chain(own_routes.iter().cloned())
            .collect();
        for route in &own_routes {
            if !seen_routes.contains(route) {
                seen_routes.insert(route.clone());
                continue;
            }
            let next = first_free_route(&orbital_route_slug(&orbital_name), &reserved_routes);
            rewrite_own_route(&mut node, route, &next);
            seen_routes.insert(next.clone());
            reserved_routes.insert(next.clone());
            routes.push(json!({ "orbitalName": orbital_name, "from": route, "to": next }));
            mutated = true;
        }

        let own_page_names: Vec<String> = arr_of(&node, "pages")
            .iter()
            .filter(|p| p.is_object() && str_of(p, "path").is_some())
            .filter_map(|p| str_of(p, "name").map(str::to_string))
            .collect();
        let mut reserved_page_names: HashSet<String> = seen_page_names
            .iter()
            .cloned()
            .chain(own_page_names.iter().cloned())
            .collect();
        if let Some(Value::Array(pages)) = node.get_mut("pages") {
            for p in pages.iter_mut() {
                let (Some(_), Some(page_name)) =
                    (str_of(p, "path"), str_of(p, "name").map(str::to_string))
                else {
                    continue;
                };
                if !seen_page_names.contains(&page_name) {
                    seen_page_names.insert(page_name);
                    continue;
                }
                let next = first_free_name(
                    &format!("{}{}", strip_orbital_suffix(&orbital_name), page_name),
                    &reserved_page_names,
                );
                p["name"] = Value::String(next.clone());
                seen_page_names.insert(next.clone());
                reserved_page_names.insert(next.clone());
                page_names
                    .push(json!({ "orbitalName": orbital_name, "from": page_name, "to": next }));
                mutated = true;
            }
        }

        let own_trait_names: Vec<String> = arr_of(&node, "traits")
            .iter()
            .filter_map(|t| {
                str_of(t, "name")
                    .filter(|_| t.is_object())
                    .map(str::to_string)
            })
            .collect();
        let prefix = strip_orbital_suffix(&orbital_name).to_string();
        let mut reserved_traits: HashSet<String> = seen_traits
            .iter()
            .cloned()
            .chain(own_trait_names.iter().cloned())
            .collect();
        let mut token_renames: HashMap<String, String> = HashMap::new();
        for trait_name in &own_trait_names {
            if !seen_traits.contains(trait_name) {
                seen_traits.insert(trait_name.clone());
                continue;
            }
            let next = first_free_name(&format!("{prefix}{trait_name}"), &reserved_traits);
            let trait_id = arr_of(&node, "traits")
                .iter()
                .find(|t| {
                    str_of(t, "name") == Some(trait_name.as_str())
                        && t.get("id").is_some_and(Value::is_string)
                })
                .and_then(|t| str_of(t, "id"))
                .map(str::to_string);
            rewrite_inline_trait_name(&mut node, trait_name, &next);
            token_renames.insert(trait_name.clone(), next.clone());
            seen_traits.insert(next.clone());
            reserved_traits.insert(next.clone());
            let mut rename = json!({ "orbitalName": orbital_name, "from": trait_name, "to": next });
            if let Some(id) = trait_id {
                rename["id"] = Value::String(id);
            }
            traits.push(rename);
            mutated = true;
        }
        if !token_renames.is_empty() {
            node = rewrite_trait_refs_in_tree(&node, &token_renames);
        }
        if mutated {
            *orbital = node;
        }
    }
    json!({ "routes": routes, "traits": traits, "pageNames": page_names })
}

// ============================================================================
// Field-comparison literal walks (the compiled path's role-literal and owner-field checks)
// ============================================================================

struct FieldBinding<'a> {
    entity_name: &'a str,
    is_identity: bool,
    scope_entity_name: Option<&'a str>,
    field: &'a str,
}

fn is_field_binding(node: &Value, ctx: &FieldBinding) -> bool {
    match node {
        Value::String(s) => {
            (ctx.is_identity && *s == format!("@user.{}", ctx.field))
                || *s == format!("@{}.{}", ctx.entity_name, ctx.field)
                || (ctx.scope_entity_name == Some(ctx.entity_name)
                    && *s == format!("@entity.{}", ctx.field))
        }
        Value::Array(a) if a.len() == 3 && a[0].as_str() == Some("object/get") => {
            let (target, key) = (a[1].as_str(), a[2].as_str());
            (ctx.is_identity && target == Some("@user") && key == Some(ctx.field))
                || (target == Some(format!("@{}", ctx.entity_name).as_str())
                    && key == Some(ctx.field))
                || (ctx.scope_entity_name == Some(ctx.entity_name)
                    && target == Some("@entity")
                    && key == Some(ctx.field))
        }
        _ => false,
    }
}

const EQUALITY_OPS: [&str; 6] = ["=", "==", "!=", "!==", "eq", "neq"];

fn collect_field_comparison_literals(node: &Value, ctx: &FieldBinding, out: &mut BTreeSet<String>) {
    match node {
        Value::Array(a) => {
            let op = a.first().and_then(Value::as_str);
            if op.is_some_and(|o| EQUALITY_OPS.contains(&o)) {
                let args = &a[1..];
                if args.iter().any(|x| is_field_binding(x, ctx)) {
                    for x in args {
                        if let Some(s) = x.as_str() {
                            if !s.starts_with('@') && !is_field_binding(x, ctx) {
                                out.insert(s.to_string());
                            }
                        }
                    }
                }
            } else if op == Some("array/includes")
                && a.get(1).is_some_and(|h| is_field_binding(h, ctx))
            {
                if let Some(needle) = a
                    .get(2)
                    .and_then(Value::as_str)
                    .filter(|n| !n.starts_with('@'))
                {
                    out.insert(needle.to_string());
                }
            }
            a.iter()
                .for_each(|x| collect_field_comparison_literals(x, ctx, out));
        }
        Value::Object(o) => o
            .values()
            .for_each(|x| collect_field_comparison_literals(x, ctx, out)),
        _ => {}
    }
}

fn entity_field_of(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("@entity.")?;
    (!rest.is_empty() && !rest.contains('.') && !rest.contains('[')).then_some(rest)
}

fn is_user_id_token(node: &Value) -> bool {
    node.as_str() == Some("@user.id")
        || node.as_array().is_some_and(|a| {
            a.first().and_then(Value::as_str) == Some("object/get")
                && a.get(1).and_then(Value::as_str) == Some("@user")
                && a.get(2).and_then(Value::as_str) == Some("id")
        })
}

fn entity_field_binding(node: &Value) -> Option<String> {
    match node {
        Value::String(s) => entity_field_of(s).map(str::to_string),
        Value::Array(a)
            if a.first().and_then(Value::as_str) == Some("object/get")
                && a.get(1).and_then(Value::as_str) == Some("@entity") =>
        {
            a.get(2).and_then(Value::as_str).map(str::to_string)
        }
        _ => None,
    }
}

fn collect_owner_compared_fields(node: &Value, out: &mut BTreeSet<String>) {
    match node {
        Value::Array(a) => {
            let op = a.first().and_then(Value::as_str);
            if matches!(op, Some("=") | Some("==")) {
                let args = &a[1..];
                if args.iter().any(is_user_id_token) {
                    for x in args {
                        if let Some(f) = entity_field_binding(x).filter(|f| f != "id") {
                            out.insert(f);
                        }
                    }
                }
            }
            a.iter().for_each(|x| collect_owner_compared_fields(x, out));
        }
        Value::Object(o) => o
            .values()
            .for_each(|x| collect_owner_compared_fields(x, out)),
        _ => {}
    }
}

fn persist_payload(a: &[Value]) -> Option<&Map<String, Value>> {
    let payload = a.get(3)?.as_object()?;
    let is_options_bag = a.len() == 4 && payload.contains_key("emit");
    (!is_options_bag).then_some(payload)
}

fn is_persist_write(a: &[Value]) -> bool {
    a.first().and_then(Value::as_str) == Some("persist")
        && matches!(
            a.get(1).and_then(Value::as_str),
            Some("create" | "upsert" | "update")
        )
}

fn collect_owner_stamped_fields(node: &Value, out: &mut BTreeMap<String, BTreeSet<String>>) {
    match node {
        Value::Array(a) => {
            if is_persist_write(a) {
                if let (Some(entity), Some(payload)) =
                    (a.get(2).and_then(Value::as_str), persist_payload(a))
                {
                    for (key, value) in payload {
                        if is_user_id_token(value) {
                            out.entry(entity.to_string())
                                .or_default()
                                .insert(key.clone());
                        }
                    }
                }
            }
            a.iter().for_each(|x| collect_owner_stamped_fields(x, out));
        }
        Value::Object(o) => o
            .values()
            .for_each(|x| collect_owner_stamped_fields(x, out)),
        _ => {}
    }
}

fn collect_persisted_fields(node: &Value, entity_name: &str, out: &mut BTreeSet<String>) {
    match node {
        Value::Array(a) => {
            if is_persist_write(a) && a.get(2).and_then(Value::as_str) == Some(entity_name) {
                if let Some(payload) = persist_payload(a) {
                    out.extend(payload.keys().cloned());
                }
            }
            a.iter()
                .for_each(|x| collect_persisted_fields(x, entity_name, out));
        }
        Value::Object(o) => o
            .values()
            .for_each(|x| collect_persisted_fields(x, entity_name, out)),
        _ => {}
    }
}

// ============================================================================
// Identity dedupe (FIX-K / FIX-L / FIX-N)
// ============================================================================

fn field_vocabulary(field: &Value) -> Option<Vec<String>> {
    if let Some(values) = field
        .get("values")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
    {
        return Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        );
    }
    if matches!(str_of(field, "type"), Some("array" | "object")) {
        if let Some(items) = field.get("items") {
            return field_vocabulary(items);
        }
    }
    None
}

fn with_unioned_vocabulary(field: &Value, missing: &[String]) -> Value {
    let mut out = field.clone();
    if field
        .get("values")
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty())
    {
        if let Some(Value::Array(values)) = out.get_mut("values") {
            values.extend(missing.iter().map(|m| Value::String(m.clone())));
        }
        return out;
    }
    if matches!(str_of(field, "type"), Some("array" | "object")) {
        if let Some(items) = field.get("items") {
            out["items"] = with_unioned_vocabulary(items, missing);
        }
    }
    out
}

/// The literals other orbitals compare `@user.<field>` against that the identity's closed vocabulary lacks
/// (`{ fields, added }`).
pub fn identity_vocabulary_delta(identity: &Value, others: &[Value]) -> Value {
    let mut fields = Vec::new();
    let mut added = Vec::new();
    let name = str_of(identity, "name").unwrap_or("");
    for field in arr_of(identity, "fields") {
        let Some(field_name) = str_of(field, "name") else {
            continue;
        };
        let Some(vocab) = field_vocabulary(field) else {
            continue;
        };
        let mut referenced = BTreeSet::new();
        for def in others {
            let scope = orbital_entity_name(def);
            collect_field_comparison_literals(
                def,
                &FieldBinding {
                    entity_name: name,
                    is_identity: true,
                    scope_entity_name: scope.as_deref(),
                    field: field_name,
                },
                &mut referenced,
            );
        }
        let missing: Vec<String> = referenced
            .into_iter()
            .filter(|l| !vocab.contains(l))
            .collect();
        if missing.is_empty() {
            continue;
        }
        fields.push(with_unioned_vocabulary(field, &missing));
        added.push(json!({ "field": field_name, "literals": missing }));
    }
    json!({ "fields": fields, "added": added })
}

fn union_demoted_role_vocabularies(
    orbitals: &mut [Value],
    winner_index: usize,
    demoted: &[(String, Vec<Value>)],
) -> Result<Vec<Value>, String> {
    let winner_def = orbitals[winner_index].clone();
    let Some(winner_entity) = inline_entity(&winner_def).cloned() else {
        return Ok(Vec::new());
    };
    let winner_name = str_of(&winner_entity, "name").unwrap_or("").to_string();
    let winner_fields = arr_of(&winner_entity, "fields").to_vec();

    let field_names: BTreeSet<String> = demoted
        .iter()
        .flat_map(|(_, fs)| {
            fs.iter()
                .filter_map(|f| str_of(f, "name").map(str::to_string))
        })
        .collect();
    let mut unions = Vec::new();
    let mut replacements: HashMap<String, Value> = HashMap::new();
    let mut mismatches = Vec::new();
    for field_name in &field_names {
        let mut referenced = BTreeSet::new();
        for def in orbitals.iter() {
            let scope = orbital_entity_name(def);
            collect_field_comparison_literals(
                def,
                &FieldBinding {
                    entity_name: &winner_name,
                    is_identity: true,
                    scope_entity_name: scope.as_deref(),
                    field: field_name,
                },
                &mut referenced,
            );
        }
        let winner_field = winner_fields
            .iter()
            .find(|f| str_of(f, "name") == Some(field_name.as_str()));
        if winner_field.is_some_and(|w| field_vocabulary(w).is_some()) {
            for (_, fs) in demoted {
                if let Some(own) = fs
                    .iter()
                    .find(|f| str_of(f, "name") == Some(field_name.as_str()))
                {
                    referenced.extend(field_vocabulary(own).unwrap_or_default());
                }
            }
        }
        if referenced.is_empty() {
            continue;
        }
        let Some(winner_field) = winner_field else {
            let sources: Vec<&str> = demoted
                .iter()
                .filter(|(_, fs)| {
                    fs.iter()
                        .any(|f| str_of(f, "name") == Some(field_name.as_str()))
                })
                .map(|(n, _)| n.as_str())
                .collect();
            let literals: Vec<String> = referenced
                .iter()
                .map(|l| serde_json::to_string(l).unwrap_or_default())
                .collect();
            mismatches.push(format!(
                "@user.{field_name} is compared against {} (field declared by demoted {}), but the surviving [identity] entity {winner_name} declares no field \"{field_name}\"",
                literals.join(", "),
                sources.join(", ")
            ));
            continue;
        };
        let Some(vocab) = field_vocabulary(winner_field) else {
            continue;
        };
        let missing: Vec<String> = referenced
            .into_iter()
            .filter(|l| !vocab.contains(l))
            .collect();
        if missing.is_empty() {
            continue;
        }
        replacements.insert(
            field_name.clone(),
            with_unioned_vocabulary(winner_field, &missing),
        );
        unions.push(json!({ "orbitalName": str_of(&winner_def, "name"), "entityName": winner_name, "field": field_name, "addedLiterals": missing }));
    }
    if !mismatches.is_empty() {
        return Err(format!(
            "identity dedupe shape mismatch — {}. The identity shapes cannot be reconciled deterministically and silently dropping the literals ships dead policy branches (ORB_S_ROLE_LITERAL_NOT_MEMBER); compose cannot proceed.",
            mismatches.join("; ")
        ));
    }
    if !replacements.is_empty() {
        let fields: Vec<Value> = winner_fields
            .iter()
            .map(|f| {
                str_of(f, "name")
                    .and_then(|n| replacements.get(n))
                    .cloned()
                    .unwrap_or_else(|| f.clone())
            })
            .collect();
        let mut entity = winner_entity.clone();
        entity["fields"] = Value::Array(fields);
        orbitals[winner_index]["entity"] = entity;
    }
    Ok(unions)
}

fn rewrite_demoted_identity_expects(
    orbitals: &mut [Value],
    demoted_names: &HashSet<String>,
    winner_name: &str,
) -> Result<Vec<Value>, String> {
    let mut entity_by_name: HashMap<String, Value> = HashMap::new();
    for def in orbitals.iter() {
        for e in inline_entities_of(def) {
            if let Some(n) = str_of(&e, "name") {
                entity_by_name.insert(n.to_string(), e.clone());
            }
        }
    }
    let mut rewrites = Vec::new();
    for def in orbitals.iter_mut() {
        let Some(expects) = def.get("expects").and_then(Value::as_array).cloned() else {
            continue;
        };
        let def_name = str_of(def, "name").unwrap_or("").to_string();
        let mut mutated = false;
        let mut viewer_shape: Option<Value> = None;
        let mut next: Vec<Value> = Vec::new();
        for e in &expects {
            let name = str_of(e, "name");
            if str_of(e, "kind") != Some("identity")
                || name.is_none_or(|n| !demoted_names.contains(n))
            {
                next.push(e.clone());
                continue;
            }
            let name = name.unwrap_or("").to_string();
            viewer_shape = orbital_core_base::expects::merge_expectation_shape(
                viewer_shape.as_ref(),
                e.get("shape"),
            )?;
            mutated = true;
            rewrites.push(json!({ "orbitalName": def_name, "entityName": name }));
            let Some(shape) = e.get("shape").and_then(Value::as_array) else {
                next.push(json!({ "kind": "entity", "name": name }));
                continue;
            };
            let mut persisted = BTreeSet::new();
            collect_persisted_fields(
                def.get("traits").unwrap_or(&Value::Array(vec![])),
                &name,
                &mut persisted,
            );
            let declared: HashSet<&str> = shape.iter().filter_map(|f| str_of(f, "name")).collect();
            let entity = entity_by_name.get(&name);
            let entity_field = |field_name: &str| {
                entity.and_then(|en| {
                    arr_of(en, "fields")
                        .iter()
                        .find(|f| str_of(f, "name") == Some(field_name))
                        .cloned()
                })
            };
            let mut new_shape: Vec<Value> = shape
                .iter()
                .map(|f| {
                    str_of(f, "name")
                        .and_then(entity_field)
                        .unwrap_or_else(|| f.clone())
                })
                .collect();
            for field in &persisted {
                if declared.contains(field.as_str()) {
                    continue;
                }
                if let Some(own) = entity_field(field) {
                    new_shape.push(own);
                }
            }
            next.push(json!({ "kind": "entity", "name": name, "shape": new_shape }));
        }
        if mutated {
            let viewer_index = next.iter().position(|e| {
                str_of(e, "kind") == Some("identity")
                    && (str_of(e, "name") == Some(winner_name) || e.get("name").is_none())
            });
            let prior = viewer_index.and_then(|i| next[i].get("shape").cloned());
            let shape = orbital_core_base::expects::merge_expectation_shape(
                prior.as_ref(),
                viewer_shape.as_ref(),
            )?;
            let mut requirement = json!({ "kind": "identity", "name": winner_name });
            if let Some(shape) = shape {
                requirement["shape"] = shape;
            }
            match viewer_index {
                Some(i) => next[i] = requirement,
                None => next.push(requirement),
            }
            def["expects"] = Value::Array(next);
        }
    }
    Ok(rewrites)
}

fn retarget_owner_relation_fields(
    entity: &Value,
    field_names: Option<&BTreeSet<String>>,
    demoted: &HashSet<String>,
    winner: &Value,
) -> Option<(Value, Vec<(String, String, String)>)> {
    let field_names = field_names?;
    let winner_name = str_of(winner, "name").unwrap_or("").to_string();
    let mut applied = Vec::new();
    let fields: Vec<Value> = arr_of(entity, "fields")
        .iter()
        .map(|f| {
            let Some(name) = str_of(f, "name") else {
                return f.clone();
            };
            if !field_names.contains(name) || str_of(f, "type") != Some("relation") {
                return f.clone();
            }
            let Some(target) = f.get("relation").and_then(|r| str_of(r, "entity")) else {
                return f.clone();
            };
            if !demoted.contains(target) {
                return f.clone();
            }
            applied.push((name.to_string(), target.to_string(), winner_name.clone()));
            let mut relation = f["relation"].clone();
            relation["entity"] = Value::String(winner_name.clone());
            if relation.get("entityId").is_some() {
                match winner.get("id") {
                    Some(id) => relation["entityId"] = id.clone(),
                    None => {
                        relation.as_object_mut().map(|r| r.shift_remove("entityId"));
                    }
                }
            }
            let mut out = f.clone();
            out["relation"] = relation;
            out
        })
        .collect();
    if applied.is_empty() {
        return None;
    }
    let mut out = entity.clone();
    out["fields"] = Value::Array(fields);
    Some((out, applied))
}

fn retarget_demoted_owner_relations(
    orbitals: &mut [Value],
    winner_index: usize,
    demoted: &HashSet<String>,
) -> Vec<Value> {
    let Some(winner_entity) = inline_entity(&orbitals[winner_index]).cloned() else {
        return Vec::new();
    };
    let mut candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for def in orbitals.iter() {
        for entity in inline_entities_of(def) {
            let mut fields = BTreeSet::new();
            for key in [
                "read_policy",
                "create_policy",
                "update_policy",
                "delete_policy",
            ] {
                if let Some(policy) = entity.get(key) {
                    collect_owner_compared_fields(policy, &mut fields);
                }
            }
            if !fields.is_empty() {
                candidates
                    .entry(str_of(&entity, "name").unwrap_or("").to_string())
                    .or_default()
                    .extend(fields);
            }
        }
        let mut stamped = BTreeMap::new();
        collect_owner_stamped_fields(
            def.get("traits").unwrap_or(&Value::Array(vec![])),
            &mut stamped,
        );
        for (name, fields) in stamped {
            if !fields.is_empty() {
                candidates.entry(name).or_default().extend(fields);
            }
        }
    }
    let mut retargets = Vec::new();
    for def in orbitals.iter_mut() {
        let def_name = str_of(def, "name").unwrap_or("").to_string();
        if let Some(entity) = inline_entity(def).cloned() {
            let name = str_of(&entity, "name").unwrap_or("").to_string();
            if let Some((replaced, applied)) = retarget_owner_relation_fields(
                &entity,
                candidates.get(&name),
                demoted,
                &winner_entity,
            ) {
                def["entity"] = replaced;
                for (field, from, to) in applied {
                    retargets.push(json!({ "orbitalName": def_name, "entityName": name, "field": field, "fromEntity": from, "toEntity": to }));
                }
            }
        }
        if let Some(Value::Array(aux)) = def.get_mut("auxiliaryEntities") {
            for er in aux.iter_mut() {
                if is_entity_reference_any(er) {
                    continue;
                }
                let name = str_of(er, "name").unwrap_or("").to_string();
                if let Some((replaced, applied)) = retarget_owner_relation_fields(
                    er,
                    candidates.get(&name),
                    demoted,
                    &winner_entity,
                ) {
                    *er = replaced;
                    for (field, from, to) in applied {
                        retargets.push(json!({ "orbitalName": def_name, "entityName": name, "field": field, "fromEntity": from, "toEntity": to }));
                    }
                }
            }
        }
    }
    retargets
}

/// Keep exactly one `[identity]` entity: roster order decides the winner, later identities are demoted in the
/// composed surface, with the FIX-K vocabulary union, FIX-L expects rewrite and FIX-N owner-relation retarget.
/// Mutates `orbitals`; returns `{ demotions, roleUnions, expectsRewrites, relationsRetargeted }`.
pub fn dedupe_composed_identity(orbitals: &mut [Value]) -> Result<Value, String> {
    let empty = |demotions: Vec<Value>| json!({ "demotions": demotions, "roleUnions": [], "expectsRewrites": [], "relationsRetargeted": [] });
    let mut demotions = Vec::new();
    let mut demoted_entities: Vec<(String, Vec<Value>)> = Vec::new();
    let mut winner_index: Option<usize> = None;
    for (i, def) in orbitals.iter_mut().enumerate() {
        let Some(entity) = inline_entity(def).cloned() else {
            continue;
        };
        if entity.get("identity") != Some(&Value::Bool(true)) {
            continue;
        }
        if winner_index.is_none() {
            winner_index = Some(i);
            continue;
        }
        let mut demoted = entity.clone();
        demoted.as_object_mut().map(|o| o.shift_remove("identity"));
        let name = str_of(&entity, "name").unwrap_or("").to_string();
        demotions.push(json!({ "orbitalName": str_of(def, "name"), "entityName": name }));
        demoted_entities.push((name, arr_of(&demoted, "fields").to_vec()));
        def["entity"] = demoted;
    }
    let Some(winner_index) = winner_index else {
        return Ok(empty(demotions));
    };
    let winner_name = orbital_entity_name(&orbitals[winner_index])
        .ok_or("identity dedupe requires a named identity entity")?;
    let demoted_names: HashSet<String> = demoted_entities.iter().map(|(n, _)| n.clone()).collect();
    let mut stray: Vec<(String, Vec<Value>)> = Vec::new();
    for def in orbitals.iter() {
        for e in arr_of(def, "expects") {
            let Some(name) = str_of(e, "name") else {
                continue;
            };
            if str_of(e, "kind") != Some("identity")
                || name == winner_name
                || demoted_names.contains(name)
            {
                continue;
            }
            stray.push((name.to_string(), arr_of(e, "shape").to_vec()));
        }
    }
    if demotions.is_empty() && stray.is_empty() {
        return Ok(empty(demotions));
    }
    let provided: HashSet<String> = orbitals
        .iter()
        .flat_map(inline_entities_of)
        .filter_map(|e| str_of(&e, "name").map(str::to_string))
        .collect();
    let losers_fields: Vec<(String, Vec<Value>)> = demoted_entities
        .iter()
        .cloned()
        .chain(stray.iter().cloned())
        .collect();
    let role_unions = union_demoted_role_vocabularies(orbitals, winner_index, &losers_fields)?;
    let mut loser_names = demoted_names.clone();
    loser_names.extend(
        stray
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| provided.contains(n)),
    );
    let expects_rewrites = rewrite_demoted_identity_expects(orbitals, &loser_names, &winner_name)?;
    let relations = retarget_demoted_owner_relations(orbitals, winner_index, &loser_names);
    Ok(
        json!({ "demotions": demotions, "roleUnions": role_unions, "expectsRewrites": expects_rewrites, "relationsRetargeted": relations }),
    )
}

// ============================================================================
// Nav narrowing and landing pages
// ============================================================================

fn nav_item_href(item: &Value) -> Option<&str> {
    item.as_object()
        .and_then(|o| o.get("href"))
        .and_then(Value::as_str)
}

fn is_nav_items_field(entry: &Value) -> bool {
    str_of(entry, "type") == Some("[NavItem]") && entry.get("default").is_some_and(Value::is_array)
}

fn collect_owned_surface(
    node: &Value,
    pages: &mut BTreeSet<String>,
    traits: &mut BTreeSet<String>,
) {
    match node {
        Value::Array(a) => a
            .iter()
            .for_each(|x| collect_owned_surface(x, pages, traits)),
        Value::Object(o) => {
            for p in o
                .get("pages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(path) = p
                    .as_object()
                    .and_then(|po| po.get("path"))
                    .and_then(Value::as_str)
                {
                    pages.insert(path.to_string());
                }
            }
            for t in o
                .get("traits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(name) = t
                    .as_object()
                    .and_then(|to| to.get("name"))
                    .and_then(Value::as_str)
                {
                    traits.insert(name.to_string());
                }
            }
            o.values()
                .for_each(|x| collect_owned_surface(x, pages, traits));
        }
        _ => {}
    }
}

fn narrow_nav_items_to_owned_pages(orbitals: &mut [Value], owned: &BTreeSet<String>) -> Vec<Value> {
    let mut drops = Vec::new();
    for (i, orbital) in orbitals.iter_mut().enumerate() {
        let orbital_name = str_of(orbital, "name")
            .map(str::to_string)
            .unwrap_or_else(|| format!("orbital-{i}"));
        let Some(Value::Array(traits)) = orbital.get_mut("traits") else {
            continue;
        };
        for t in traits.iter_mut() {
            let trait_name = str_of(t, "name")
                .or_else(|| str_of(t, "ref"))
                .unwrap_or("trait")
                .to_string();
            let Some(Value::Object(config)) = t.get_mut("config") else {
                continue;
            };
            for (_, entry) in config.iter_mut() {
                if !is_nav_items_field(entry) {
                    continue;
                }
                let mut kept = Vec::new();
                let mut dropped = Vec::new();
                for item in entry["default"].as_array().cloned().unwrap_or_default() {
                    match nav_item_href(&item) {
                        Some(href) if !owned.contains(href) => dropped.push(href.to_string()),
                        _ => kept.push(item),
                    }
                }
                if !dropped.is_empty() {
                    entry["default"] = Value::Array(kept);
                    drops.push(json!({ "orbitalName": orbital_name, "traitName": trait_name, "droppedHrefs": dropped }));
                }
            }
        }
    }
    drops
}

fn imported_page_paths(
    orbital: &Value,
    lookup: &dyn Fn(&Value) -> Option<Value>,
    seen: &mut Vec<Value>,
) -> Vec<String> {
    let Some(reference) = orbital.get("reference") else {
        return Vec::new();
    };
    if seen.contains(orbital) {
        return Vec::new();
    }
    seen.push(orbital.clone());
    let Some(upstream) = lookup(orbital) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = arr_of(&upstream, "pages")
        .iter()
        .filter(|p| !is_page_reference(p))
        .filter_map(|p| str_of(p, "path").map(str::to_string))
        .collect();
    paths.extend(imported_page_paths(&upstream, lookup, seen));
    let omitted: HashSet<&str> = arr_of(reference, "omit")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let remap = reference.get("pages").and_then(Value::as_object);
    paths
        .into_iter()
        .filter(|p| !omitted.contains(p.as_str()))
        .map(|p| {
            remap
                .and_then(|r| r.get(&p))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(p)
        })
        .collect()
}

fn collect_owned_pages(
    orbitals: &[Value],
    imported: Option<&dyn Fn(&Value) -> Option<Value>>,
) -> BTreeSet<String> {
    let mut owned = BTreeSet::new();
    let mut traits = BTreeSet::new();
    for orbital in orbitals {
        collect_owned_surface(orbital, &mut owned, &mut traits);
        if let Some(lookup) = imported {
            owned.extend(imported_page_paths(orbital, lookup, &mut Vec::new()));
        }
    }
    owned
}

fn narrow_config_nav_items(config: &Value, owned: &BTreeSet<String>) -> (Value, Vec<Value>) {
    let mut drops = Vec::new();
    let mut next = config.clone();
    for (knob, field) in config.as_object().into_iter().flatten() {
        if !is_nav_items_field(field) {
            continue;
        }
        let mut dropped = Vec::new();
        let kept: Vec<Value> = field["default"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| match nav_item_href(item) {
                Some(href) if !owned.contains(href) => {
                    dropped.push(href.to_string());
                    false
                }
                _ => true,
            })
            .collect();
        if dropped.is_empty() {
            continue;
        }
        next[knob]["default"] = Value::Array(kept);
        drops.push(json!({ "knob": knob, "droppedHrefs": dropped }));
    }
    (next, drops)
}

fn collect_linked_paths(node: &Value, out: &mut HashSet<String>) {
    match node {
        Value::Array(a) => {
            if a.first().and_then(Value::as_str) == Some("navigate") {
                if let Some(t) = a.get(1).and_then(Value::as_str) {
                    out.insert(t.to_string());
                }
            }
            a.iter().for_each(|x| collect_linked_paths(x, out));
        }
        Value::Object(o) => {
            for (k, v) in o {
                if k == "href" {
                    if let Some(s) = v.as_str() {
                        out.insert(s.to_string());
                    }
                }
                collect_linked_paths(v, out);
            }
        }
        _ => {}
    }
}

fn first_inline_page(orbital: &Value) -> Option<(String, String)> {
    arr_of(orbital, "pages")
        .iter()
        .filter(|p| !is_page_reference(p))
        .find_map(|p| {
            Some((
                str_of(p, "name")?.to_string(),
                str_of(p, "path")?.to_string(),
            ))
        })
}

fn link_landing_pages(
    orbitals: &[Value],
    config: &Value,
    organism_of: &dyn Fn(&str) -> Option<String>,
    nav_declaring: &[String],
    organism_order_of: Option<&dyn Fn(&str) -> Option<f64>>,
) -> (Value, Vec<Value>) {
    let nav_knobs: Vec<(&String, &Value)> = config
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, f)| is_nav_items_field(f))
        .collect();
    if nav_knobs.len() != 1 {
        return (config.clone(), Vec::new());
    }
    let (knob, field) = nav_knobs[0];
    let list = field["default"].as_array().cloned().unwrap_or_default();
    let mut linked = HashSet::new();
    collect_linked_paths(&Value::Array(orbitals.to_vec()), &mut linked);
    collect_linked_paths(config, &mut linked);

    let mut owner_index: HashMap<String, usize> = HashMap::new();
    for (i, orbital) in orbitals.iter().enumerate() {
        for page in arr_of(orbital, "pages") {
            if is_page_reference(page) {
                continue;
            }
            if let Some(path) = str_of(page, "path") {
                owner_index.entry(path.to_string()).or_insert(i);
            }
        }
    }
    let index_of = |entry: &Value| -> i64 {
        nav_item_href(entry)
            .and_then(|h| owner_index.get(h))
            .map(|i| *i as i64)
            .unwrap_or(-1)
    };

    let mut added = Vec::new();
    let mut next = list.clone();
    let nav_organisms: HashSet<String> = nav_declaring
        .iter()
        .filter_map(|n| organism_of(n))
        .collect();
    let mut landing_of: HashMap<String, (String, f64)> = HashMap::new();
    for (i, orbital) in orbitals.iter().enumerate() {
        let name = str_of(orbital, "name").unwrap_or("");
        let Some(organism) = organism_of(name) else {
            continue;
        };
        if first_inline_page(orbital).is_none() {
            continue;
        }
        let rank = organism_order_of
            .and_then(|f| f(name))
            .unwrap_or((orbitals.len() + i) as f64);
        if landing_of.get(&organism).is_none_or(|(_, r)| rank < *r) {
            landing_of.insert(organism, (name.to_string(), rank));
        }
    }
    let mut boot_organism: Option<String> = None;
    let mut booted = false;
    for (i, orbital) in orbitals.iter().enumerate() {
        let Some((page_name, page_path)) = first_inline_page(orbital) else {
            continue;
        };
        let name = str_of(orbital, "name").unwrap_or("");
        let organism = organism_of(name);
        if !booted {
            booted = true;
            boot_organism = organism;
            continue;
        }
        let Some(organism) = organism else { continue };
        if nav_organisms.contains(&organism)
            && (Some(&organism) == boot_organism.as_ref()
                || landing_of.get(&organism).map(|(n, _)| n.as_str()) != Some(name))
        {
            continue;
        }
        if linked.contains(&page_path) || page_path.split('/').any(|s| s.starts_with(':')) {
            continue;
        }
        let mut at = 0;
        for (p, entry) in next.iter().enumerate() {
            if index_of(entry) < i as i64 {
                at = p + 1;
            }
        }
        next.insert(
            at,
            json!({ "href": page_path, "label": humanize_key(&page_name) }),
        );
        linked.insert(page_path.clone());
        added.push(json!({ "organism": organism, "href": page_path }));
    }
    if added.is_empty() {
        return (config.clone(), added);
    }
    let mut out = config.clone();
    out[knob.as_str()]["default"] = Value::Array(next);
    (out, added)
}

// ============================================================================
// Organism namespacing
// ============================================================================

fn organism_prefix(organism: &str) -> String {
    organism
        .strip_prefix("std-")
        .unwrap_or(organism)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            cs.next()
                .map(|f| f.to_ascii_uppercase().to_string() + cs.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn snake_prefix(prefix: &str) -> String {
    let chars: Vec<char> = prefix.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0
            && c.is_ascii_uppercase()
            && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit())
        {
            out.push('_');
        }
        out.push(*c);
    }
    out.to_lowercase()
}

fn route_shape(path: &str) -> String {
    path.split('/')
        .map(|s| if s.starts_with(':') { ":" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

fn route_root(path: &str) -> String {
    format!("/{}", path.split('/').nth(1).unwrap_or(""))
}

fn under_root(link: &str, root: &str) -> bool {
    link == root || link.starts_with(&format!("{root}/"))
}

fn page_paths(file: &Value) -> Vec<String> {
    as_definitions(std::slice::from_ref(file))
        .iter()
        .flat_map(|o| {
            arr_of(o, "pages")
                .iter()
                .filter(|p| !is_page_reference(p))
                .filter_map(|p| str_of(p, "path").map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn move_routes(node: &Value, roots: &BTreeSet<String>, base: &str, in_pages: bool) -> Value {
    let mv = |p: &str| {
        if p == "/" {
            base.to_string()
        } else {
            format!("{base}{p}")
        }
    };
    let hit = |v: &str| roots.iter().any(|r| under_root(v, r));
    match node {
        Value::Array(a) => {
            let mut out: Vec<Value> = a
                .iter()
                .map(|x| move_routes(x, roots, base, in_pages))
                .collect();
            if a.first().and_then(Value::as_str) == Some("navigate") {
                if let Some(t) = a.get(1).and_then(Value::as_str).filter(|t| hit(t)) {
                    out[1] = Value::String(mv(t));
                }
            }
            Value::Array(out)
        }
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    let moved = match v.as_str() {
                        Some(s) if (k == "href" || (in_pages && k == "path")) && hit(s) => {
                            Value::String(mv(s))
                        }
                        _ => move_routes(v, roots, base, k == "pages"),
                    };
                    (k.clone(), moved)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn declared_entities(file: &Value) -> Vec<Value> {
    as_definitions(std::slice::from_ref(file))
        .iter()
        .flat_map(inline_entities_of)
        .collect()
}

fn rename_collection(file: &Value, from: &str, to: &str) -> Value {
    let swap = |e: &mut Value| {
        if str_of(e, "collection") == Some(from) {
            e["collection"] = Value::String(to.to_string());
        }
    };
    let each = |o: &Value| -> Value {
        let mut o = o.clone();
        if o.get("entity")
            .is_some_and(|e| e.is_object() && !is_entity_reference_any(e))
        {
            swap(&mut o["entity"]);
        }
        if let Some(Value::Array(aux)) = o.get_mut("auxiliaryEntities") {
            for e in aux.iter_mut().filter(|e| !is_entity_reference_any(e)) {
                swap(e);
            }
        }
        o
    };
    if is_schema(file) {
        let mut f = file.clone();
        f["orbitals"] = Value::Array(arr_of(file, "orbitals").iter().map(each).collect());
        f
    } else {
        each(file)
    }
}

const ENTITY_VALUED_KEYS: [&str; 7] = [
    "entity",
    "source",
    "entityType",
    "linkedEntity",
    "primaryEntity",
    "payloadEntity",
    "extends",
];

fn rename_token(value: &str, from: &str, to: &str) -> String {
    if value == format!("@{from}") {
        return format!("@{to}");
    }
    match value.strip_prefix(&format!("@{from}.")) {
        Some(rest) => format!("@{to}.{rest}"),
        None => value.to_string(),
    }
}

fn rename_entity_walk(node: &Value, from: &str, to: &str) -> Value {
    match node {
        Value::String(s) => Value::String(rename_token(s, from, to)),
        Value::Array(a) => {
            let mut out: Vec<Value> = a.iter().map(|x| rename_entity_walk(x, from, to)).collect();
            let op = a.first().and_then(Value::as_str);
            if matches!(op, Some("fetch" | "spawn"))
                && a.get(1).and_then(Value::as_str) == Some(from)
            {
                out[1] = Value::String(to.to_string());
            }
            if op == Some("ref") && a.len() == 2 && a[1].as_str() == Some(from) {
                out[1] = Value::String(to.to_string());
            }
            if op == Some("persist")
                && matches!(
                    a.get(1).and_then(Value::as_str),
                    Some("create" | "upsert" | "update" | "delete")
                )
                && a.get(2).and_then(Value::as_str) == Some(from)
            {
                out[2] = Value::String(to.to_string());
            }
            Value::Array(out)
        }
        Value::Object(o) => {
            let mut out = Map::new();
            for (k, v) in o {
                if k == "entityRefIds" && v.is_object() {
                    out.insert(
                        k.clone(),
                        Value::Object(
                            v.as_object()
                                .into_iter()
                                .flatten()
                                .map(|(n, id)| {
                                    (
                                        if n == from { to.to_string() } else { n.clone() },
                                        id.clone(),
                                    )
                                })
                                .collect(),
                        ),
                    );
                    continue;
                }
                if let Some(s) = v.as_str() {
                    if ENTITY_VALUED_KEYS.contains(&k.as_str()) && s == from {
                        out.insert(k.clone(), Value::String(to.to_string()));
                        continue;
                    }
                    if k == "type" && (s == from || s == format!("[{from}]")) {
                        out.insert(
                            k.clone(),
                            Value::String(if s == from {
                                to.to_string()
                            } else {
                                format!("[{to}]")
                            }),
                        );
                        continue;
                    }
                }
                out.insert(k.clone(), rename_entity_walk(v, from, to));
            }
            let named_from = o.get("name").and_then(Value::as_str) == Some(from);
            if named_from && o.get("fields").is_some_and(Value::is_array) {
                out.insert("name".to_string(), Value::String(to.to_string()));
            }
            if matches!(
                o.get("kind").and_then(Value::as_str),
                Some("persist" | "fetch")
            ) && o.get("resource").and_then(Value::as_str) == Some(from)
            {
                out.insert("resource".to_string(), Value::String(to.to_string()));
            }
            if named_from
                && matches!(
                    o.get("kind").and_then(Value::as_str),
                    Some("entity" | "identity")
                )
            {
                out.insert("name".to_string(), Value::String(to.to_string()));
            }
            if o.get("type").and_then(Value::as_str) == Some("entity")
                && o.get("default").and_then(Value::as_str) == Some(from)
            {
                out.insert("default".to_string(), Value::String(to.to_string()));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn is_call_site_declaration(entry: &Value) -> bool {
    entry
        .as_object()
        .is_some_and(|o| o.get("type").is_some_and(Value::is_string) && o.contains_key("default"))
}

fn rename_atom_entity_slots(t: &Value, atom: Option<Value>, from: &str, to: &str) -> Value {
    let Some(atom) = atom else { return t.clone() };
    if !t.get("ref").is_some_and(Value::is_string) {
        return t.clone();
    }
    let mut next = t.clone();
    if t.get("linkedEntity").is_none() && str_of(&atom, "linkedEntity") == Some(from) {
        next["linkedEntity"] = Value::String(to.to_string());
    }
    if let Some(config) = t.get("config").filter(|c| c.is_object()) {
        let mut renamed = config.clone();
        for (knob, declared) in atom
            .get("config")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if is_call_site_declaration(declared) && str_of(declared, "type") == Some("entity") {
                if let Some(field) = renamed.get_mut(knob).filter(|f| {
                    f.is_object() && f.get("default").and_then(Value::as_str) == Some(from)
                }) {
                    field["default"] = Value::String(to.to_string());
                }
            }
        }
        next["config"] = renamed;
    }
    if let Some(type_args) = t.get("typeArgs").and_then(Value::as_object) {
        let entity_params: HashSet<&str> = arr_of(&atom, "typeParams")
            .iter()
            .filter(|p| str_of(p, "kind") == Some("Entity"))
            .filter_map(|p| str_of(p, "name"))
            .collect();
        next["typeArgs"] = Value::Object(
            type_args
                .iter()
                .map(|(p, v)| {
                    (
                        p.clone(),
                        if entity_params.contains(p.as_str()) && v.as_str() == Some(from) {
                            Value::String(to.to_string())
                        } else {
                            v.clone()
                        },
                    )
                })
                .collect(),
        );
    }
    next
}

fn rename_entity_in(
    orbital: &Value,
    from: &str,
    to: &str,
    atom_trait_of: Option<&dyn Fn(&Value, &str) -> Option<Value>>,
) -> Value {
    let mut tree = orbital.clone();
    if let Some(atom_of) = atom_trait_of {
        if let Some(Value::Array(traits)) = tree.get("traits").cloned().as_ref() {
            tree["traits"] = Value::Array(
                traits
                    .iter()
                    .map(|t| {
                        rename_atom_entity_slots(
                            t,
                            str_of(t, "ref").and_then(|r| atom_of(orbital, r)),
                            from,
                            to,
                        )
                    })
                    .collect(),
            );
        }
        if let Some(Value::Array(pages)) = tree.get("pages").cloned().as_ref() {
            tree["pages"] = Value::Array(
                pages
                    .iter()
                    .map(|page| match page.get("traits").and_then(Value::as_array) {
                        Some(pts) if page.is_object() => {
                            let mut p = page.clone();
                            p["traits"] = Value::Array(
                                pts.iter()
                                    .map(|t| {
                                        rename_atom_entity_slots(
                                            t,
                                            str_of(t, "ref").and_then(|r| atom_of(orbital, r)),
                                            from,
                                            to,
                                        )
                                    })
                                    .collect(),
                            );
                            p
                        }
                        _ => page.clone(),
                    })
                    .collect(),
            );
        }
    }
    rename_entity_walk(&tree, from, to)
}

fn ledger_rename(ledger: &Value, id: &str, to: &str, at: &str) -> Value {
    let Some(entry) = ledger.get("entries").and_then(|e| e.get(id)) else {
        return ledger.clone();
    };
    if str_of(entry, "curName") == Some(to) {
        return ledger.clone();
    }
    let mut updated = entry.clone();
    let from = entry.get("curName").cloned().unwrap_or(Value::Null);
    updated["curName"] = Value::String(to.to_string());
    let mut renames = arr_of(entry, "renames").to_vec();
    renames.push(json!({ "from": from, "to": to, "at": at }));
    updated["renames"] = Value::Array(renames);
    let mut out = ledger.clone();
    out["entries"][id] = updated;
    out
}

fn declaration_ids(orbitals: &[Value], name: &str) -> Vec<String> {
    fn visit(node: &Value, name: &str, out: &mut Vec<String>) {
        match node {
            Value::Array(a) => a.iter().for_each(|x| visit(x, name, out)),
            Value::Object(o) => {
                if o.get("name").and_then(Value::as_str) == Some(name)
                    && o.get("fields").is_some_and(Value::is_array)
                {
                    if let Some(id) = o.get("id").and_then(Value::as_str) {
                        out.push(id.to_string());
                    }
                }
                o.values().for_each(|x| visit(x, name, out));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    orbitals.iter().for_each(|o| visit(o, name, &mut out));
    out
}

fn rename_entity_in_file(
    file: &Value,
    from: &str,
    to: &str,
    atom_trait_of: Option<&dyn Fn(&Value, &str) -> Option<Value>>,
    now: &str,
) -> Value {
    if !is_schema(file) {
        return rename_entity_in(file, from, to, atom_trait_of);
    }
    let orbitals = arr_of(file, "orbitals");
    let mut out = file.clone();
    out["orbitals"] = Value::Array(
        orbitals
            .iter()
            .map(|o| rename_entity_in(o, from, to, atom_trait_of))
            .collect(),
    );
    if let Some(mut ledger) = file.get("ledger").cloned() {
        for id in declaration_ids(orbitals, from) {
            if ledger.get("entries").and_then(|e| e.get(&id)).is_some() {
                ledger = ledger_rename(&ledger, &id, to, now);
            }
        }
        out["ledger"] = ledger;
    }
    out
}

fn entity_rename_blockers(
    orbitals: &[Value],
    name: &str,
    atom_trait_of: &dyn Fn(&Value, &str) -> Option<Value>,
) -> Vec<Value> {
    let mut blockers = Vec::new();
    for orbital in orbitals {
        for t in arr_of(orbital, "traits") {
            let Some(r) = t
                .as_object()
                .and_then(|o| o.get("ref"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let bound = match t.get("linkedEntity").filter(|v| !v.is_null()) {
                Some(v) => v.as_str().map(str::to_string),
                None => atom_trait_of(orbital, r)
                    .and_then(|a| str_of(&a, "linkedEntity").map(str::to_string)),
            };
            if bound.as_deref() != Some(name) {
                continue;
            }
            let rebindable = atom_trait_of(orbital, r)
                .and_then(|a| a.get("entityRebindable").cloned())
                == Some(Value::Bool(true));
            if rebindable {
                continue;
            }
            blockers.push(json!({ "orbitalName": str_of(orbital, "name"), "traitName": str_of(t, "name").unwrap_or(r), "traitRef": r }));
        }
    }
    blockers
}

struct Namespaced {
    files: Vec<Value>,
    renames: Vec<Value>,
    unrenamable: Vec<Value>,
}

fn namespace_organisms(
    files: &[Value],
    organism_of: &dyn Fn(&str) -> Option<String>,
    atom_trait_of: &dyn Fn(&Value, &str) -> Option<Value>,
    now: &str,
) -> Namespaced {
    let organism_of_file = |file: &Value| -> Option<String> {
        as_definitions(std::slice::from_ref(file))
            .first()
            .and_then(|o| str_of(o, "name"))
            .and_then(organism_of)
    };
    let mut out: Vec<Value> = files.to_vec();
    let mut order: Vec<String> = Vec::new();
    for file in files {
        if let Some(o) = organism_of_file(file) {
            if !order.contains(&o) {
                order.push(o);
            }
        }
    }
    let mut entity_owner: HashMap<String, String> = HashMap::new();
    let mut collection_owner: HashMap<String, String> = HashMap::new();
    let mut route_owner: HashMap<String, String> = HashMap::new();
    let mut renames = Vec::new();
    let mut unrenamable = Vec::new();

    let mut implicit_owner: HashMap<String, String> = HashMap::new();
    let mut implicit_key: HashMap<String, String> = HashMap::new();
    for organism in &order {
        let own: Vec<Value> = files
            .iter()
            .filter(|f| organism_of_file(f).as_ref() == Some(organism))
            .cloned()
            .collect();
        let declared: HashSet<String> = own
            .iter()
            .flat_map(declared_entities)
            .filter_map(|e| str_of(&e, "name").map(str::to_string))
            .collect();
        let mut conflicts: Vec<(String, Vec<Value>)> = Vec::new();
        for orbital in as_definitions(&own) {
            for t in arr_of(&orbital, "traits") {
                let Some(r) = t
                    .as_object()
                    .and_then(|o| o.get("ref"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if t.get("linkedEntity").is_some() {
                    continue;
                }
                let Some(implicit) = atom_trait_of(&orbital, r)
                    .and_then(|a| str_of(&a, "linkedEntity").map(str::to_string))
                else {
                    continue;
                };
                if declared.contains(&implicit) {
                    continue;
                }
                let alias = r.split('.').next().unwrap_or("");
                let behavior = arr_of(&orbital, "uses")
                    .iter()
                    .find(|u| str_of(u, "as") == Some(alias))
                    .and_then(|u| str_of(u, "from"))
                    .unwrap_or(alias)
                    .to_string();
                let mut type_args: Vec<(String, Value)> = t
                    .get("typeArgs")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                type_args.sort_by(|a, b| a.0.cmp(&b.0));
                let tail: Vec<&str> = r.split('.').skip(1).collect();
                let key = serde_json::to_string(&json!([
                    behavior,
                    tail,
                    type_args
                        .iter()
                        .map(|(k, v)| json!([k, v]))
                        .collect::<Vec<_>>()
                ]))
                .unwrap_or_default();
                match implicit_owner.get(&implicit) {
                    None => {
                        implicit_owner.insert(implicit.clone(), organism.clone());
                        implicit_key.insert(implicit.clone(), key);
                    }
                    Some(owner)
                        if owner != organism && implicit_key.get(&implicit) != Some(&key) =>
                    {
                        let blocker = json!({ "orbitalName": str_of(&orbital, "name"), "traitName": str_of(t, "name").unwrap_or(r), "traitRef": r });
                        match conflicts.iter_mut().find(|(e, _)| *e == implicit) {
                            Some((_, bs)) => bs.push(blocker),
                            None => conflicts.push((implicit.clone(), vec![blocker])),
                        }
                    }
                    _ => {}
                }
            }
        }
        for (entity, blockers) in conflicts {
            unrenamable
                .push(json!({ "organism": organism, "entity": entity, "blockers": blockers }));
        }
    }

    for organism in &order {
        let indices: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, f)| organism_of_file(f).as_ref() == Some(organism))
            .map(|(i, _)| i)
            .collect();
        let prefix = organism_prefix(organism);
        let mut entity_renames = Vec::new();
        let mut collection_renames = Vec::new();
        let mut route_renames = Vec::new();

        let mut names: Vec<String> = Vec::new();
        for i in &indices {
            for e in declared_entities(&out[*i]) {
                if let Some(n) = str_of(&e, "name") {
                    if !names.iter().any(|x| x == n) {
                        names.push(n.to_string());
                    }
                }
            }
        }
        for name in names {
            let collides = entity_owner.get(&name).is_some_and(|o| o != organism)
                || implicit_owner.get(&name).is_some_and(|o| o != organism);
            if !collides {
                continue;
            }
            let own_orbitals: Vec<Value> = indices
                .iter()
                .flat_map(|i| as_definitions(std::slice::from_ref(&out[*i])))
                .collect();
            let blockers = entity_rename_blockers(&own_orbitals, &name, atom_trait_of);
            if !blockers.is_empty() {
                unrenamable
                    .push(json!({ "organism": organism, "entity": name, "blockers": blockers }));
                continue;
            }
            let to = format!("{prefix}{name}");
            for i in &indices {
                out[*i] = rename_entity_in_file(&out[*i], &name, &to, Some(atom_trait_of), now);
            }
            entity_renames.push(json!({ "from": name, "to": to }));
        }

        let mut roots: BTreeSet<String> = BTreeSet::new();
        let mut root_order: Vec<String> = Vec::new();
        for i in &indices {
            for path in page_paths(&out[*i]) {
                if route_owner
                    .get(&route_shape(&path))
                    .is_some_and(|o| o != organism)
                {
                    let root = route_root(&path);
                    if roots.insert(root.clone()) {
                        root_order.push(root);
                    }
                }
            }
        }
        if !roots.is_empty() {
            let base = format!("/{}", organism.strip_prefix("std-").unwrap_or(organism));
            for i in &indices {
                out[*i] = move_routes(&out[*i], &roots, &base, false);
            }
            for root in &root_order {
                route_renames.push(json!({ "from": root, "to": format!("{base}{}", if root == "/" { "" } else { root.as_str() }) }));
            }
        }

        let mut collections: Vec<String> = Vec::new();
        for i in &indices {
            for e in declared_entities(&out[*i]) {
                if let Some(c) = str_of(&e, "collection") {
                    if !collections.iter().any(|x| x == c) {
                        collections.push(c.to_string());
                    }
                }
            }
        }
        for collection in collections {
            if collection_owner
                .get(&collection)
                .is_none_or(|o| o == organism)
            {
                continue;
            }
            let to = format!("{}_{collection}", snake_prefix(&prefix));
            for i in &indices {
                out[*i] = rename_collection(&out[*i], &collection, &to);
            }
            collection_renames.push(json!({ "from": collection, "to": to }));
        }

        for i in &indices {
            for path in page_paths(&out[*i]) {
                route_owner
                    .entry(route_shape(&path))
                    .or_insert_with(|| organism.clone());
            }
            for e in declared_entities(&out[*i]) {
                if let Some(n) = str_of(&e, "name") {
                    entity_owner
                        .entry(n.to_string())
                        .or_insert_with(|| organism.clone());
                }
                if let Some(c) = str_of(&e, "collection") {
                    collection_owner
                        .entry(c.to_string())
                        .or_insert_with(|| organism.clone());
                }
            }
        }
        if !entity_renames.is_empty() || !collection_renames.is_empty() || !route_renames.is_empty()
        {
            renames.push(json!({ "organism": organism, "entities": entity_renames, "collections": collection_renames, "routes": route_renames }));
        }
    }
    Namespaced {
        files: out,
        renames,
        unrenamable,
    }
}

// ============================================================================
// The surface and the app
// ============================================================================

fn rename_nav_hrefs(config: &Value, from: &str, to: &str) -> Value {
    let mut next = config.clone();
    for (knob, field) in config.as_object().into_iter().flatten() {
        if !is_nav_items_field(field) {
            continue;
        }
        let items = field["default"].as_array().cloned().unwrap_or_default();
        if !items.iter().any(|i| nav_item_href(i) == Some(from)) {
            continue;
        }
        next[knob]["default"] = Value::Array(
            items
                .into_iter()
                .map(|mut i| {
                    if nav_item_href(&i) == Some(from) && i.is_object() {
                        i["href"] = Value::String(to.to_string());
                    }
                    i
                })
                .collect(),
        );
    }
    next
}

fn rename_listen_sources(node: &Value, orbital: &str, from: &str, to: &str) -> Value {
    match node {
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| rename_listen_sources(x, orbital, from, to))
                .collect(),
        ),
        Value::Object(o) => {
            let mut out: Map<String, Value> = o
                .iter()
                .map(|(k, v)| (k.clone(), rename_listen_sources(v, orbital, from, to)))
                .collect();
            if o.get("kind").and_then(Value::as_str) == Some("orbital")
                && o.get("orbital").and_then(Value::as_str) == Some(orbital)
                && o.get("trait").and_then(Value::as_str) == Some(from)
            {
                out.insert("trait".to_string(), Value::String(to.to_string()));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn rename_route_links(node: &Value, from: &str, to: &str) -> Value {
    match node {
        Value::Array(a) => {
            let mut out: Vec<Value> = a.iter().map(|x| rename_route_links(x, from, to)).collect();
            if a.first().and_then(Value::as_str) == Some("navigate")
                && a.get(1).and_then(Value::as_str) == Some(from)
            {
                out[1] = Value::String(to.to_string());
            }
            Value::Array(out)
        }
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if k == "href" && v.as_str() == Some(from) {
                            Value::String(to.to_string())
                        } else {
                            rename_route_links(v, from, to)
                        },
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `mergeLedgers`: the schema inputs' ledgers unioned by id (first wins), entries sorted by id.
pub(crate) fn merge_ledgers(files: &[Value]) -> Option<Value> {
    let mut merged: BTreeMap<String, Value> = BTreeMap::new();
    let mut saw = false;
    for f in files.iter().filter(|f| is_schema(f)) {
        let Some(entries) = f
            .get("ledger")
            .and_then(|l| l.get("entries"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        saw = true;
        for (id, entry) in entries {
            merged.entry(id.clone()).or_insert_with(|| entry.clone());
        }
    }
    saw.then(
        || json!({ "schemaVersion": 1, "entries": Value::Object(merged.into_iter().collect()) }),
    )
}

const CONFIG_FORWARD_PREFIX: &str = "@config.";

fn config_forward_knob(token: &str) -> Option<&str> {
    let knob = token.strip_prefix(CONFIG_FORWARD_PREFIX)?;
    let mut cs = knob.chars();
    let first = cs.next()?;
    ((first.is_ascii_alphabetic() || first == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(knob)
}

fn config_forward_tokens(entry: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) if s.starts_with(CONFIG_FORWARD_PREFIX) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut tokens = Vec::new();
    if let Some(d) = entry.get("default") {
        walk(d, &mut tokens);
    }
    if let Some(f) = str_of(entry, "forwardedFrom") {
        tokens.push(f.to_string());
    }
    tokens.extend(
        arr_of(entry, "nestedForwardedFrom")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string),
    );
    tokens
}

fn collect_orbital_forwarded_config_keys(orbital: &Value) -> HashSet<String> {
    let mut forwarded = HashSet::new();
    for t in arr_of(orbital, "traits") {
        for entry in t
            .get("config")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(_, v)| v)
        {
            if !is_call_site_declaration(entry) {
                continue;
            }
            for token in config_forward_tokens(entry) {
                if let Some(k) = config_forward_knob(&token) {
                    forwarded.insert(k.to_string());
                }
            }
        }
    }
    for u in arr_of(orbital, "uses") {
        for entry in u
            .get("config")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(_, v)| v)
        {
            for token in config_forward_tokens(entry) {
                if let Some(k) = config_forward_knob(&token) {
                    forwarded.insert(k.to_string());
                }
            }
        }
    }
    forwarded
}

fn union_organism_configs(sources: &[Value]) -> Option<Value> {
    let mut merged: Option<Map<String, Value>> = None;
    for s in sources {
        let Some(config) = s.get("config").and_then(Value::as_object) else {
            continue;
        };
        let m = merged.get_or_insert_with(Map::new);
        for (key, field) in config {
            let Some(held) = m.get(key).cloned() else {
                m.insert(key.clone(), field.clone());
                continue;
            };
            if !is_nav_items_field(&held) || !is_nav_items_field(field) {
                continue;
            }
            let held_list = held["default"].as_array().cloned().unwrap_or_default();
            let listed: HashSet<String> = held_list
                .iter()
                .filter_map(|i| nav_item_href(i).map(str::to_string))
                .collect();
            let added: Vec<Value> = field["default"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| nav_item_href(i).is_none_or(|h| !listed.contains(h)))
                .collect();
            if !added.is_empty() {
                let mut next = held.clone();
                next["default"] = Value::Array(held_list.into_iter().chain(added).collect());
                m.insert(key.clone(), next);
            }
        }
    }
    merged.map(Value::Object)
}

/// The app's surface from its per-orbital files, in the files' order (roster order decides every collision).
/// No pages or layout are generated; the files are not mutated.
pub fn compose_orbital_surface(
    files_in: &[Value],
    lookups: &ComposeLookups,
    now: &str,
) -> Result<Value, String> {
    let no_atom = |_: &Value, _: &str| -> Option<Value> { None };
    let namespaced = match lookups.organism_of {
        Some(organism_of) => namespace_organisms(
            files_in,
            organism_of,
            lookups.atom_trait_of.unwrap_or(&no_atom),
            now,
        ),
        None => Namespaced {
            files: files_in.to_vec(),
            renames: Vec::new(),
            unrenamable: Vec::new(),
        },
    };
    let files = namespaced.files;
    let mut orbitals = as_definitions(&files);
    let surface_renames = dedupe_composed_surface(&mut orbitals);
    for r in arr_of(&surface_renames, "traits") {
        let (o, from, to) = (
            str_of(r, "orbitalName").unwrap_or(""),
            str_of(r, "from").unwrap_or(""),
            str_of(r, "to").unwrap_or(""),
        );
        for orbital in orbitals.iter_mut() {
            let next = rename_listen_sources(orbital, o, from, to);
            if next != *orbital {
                *orbital = next;
            }
        }
    }
    if let Some(organism_of) = lookups.organism_of {
        for r in arr_of(&surface_renames, "routes") {
            let renamed = str_of(r, "orbitalName").unwrap_or("");
            let Some(organism) = organism_of(renamed) else {
                continue;
            };
            for orbital in orbitals.iter_mut() {
                let name = str_of(orbital, "name").unwrap_or("").to_string();
                if name == renamed || organism_of(&name).as_ref() != Some(&organism) {
                    continue;
                }
                let next = rename_route_links(
                    orbital,
                    str_of(r, "from").unwrap_or(""),
                    str_of(r, "to").unwrap_or(""),
                );
                if next != *orbital {
                    *orbital = next;
                }
            }
        }
    }
    let renamed_files: Vec<Value> = files
        .iter()
        .map(|file| {
            if !is_schema(file) || file.get("config").is_none() {
                return file.clone();
            }
            let own_names: HashSet<String> = arr_of(file, "orbitals")
                .iter()
                .filter_map(|o| str_of(o, "name").map(str::to_string))
                .collect();
            let organisms: HashSet<Option<String>> = as_definitions(std::slice::from_ref(file))
                .iter()
                .map(|o| {
                    lookups
                        .organism_of
                        .and_then(|f| f(str_of(o, "name").unwrap_or("")))
                })
                .collect();
            let own_paths: HashSet<String> = orbitals
                .iter()
                .filter(|o| str_of(o, "name").is_some_and(|n| own_names.contains(n)))
                .flat_map(|o| {
                    arr_of(o, "pages")
                        .iter()
                        .filter(|p| !is_page_reference(p))
                        .filter_map(|p| str_of(p, "path").map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .collect();
            let mut config = file["config"].clone();
            for r in arr_of(&surface_renames, "routes") {
                let renamed = str_of(r, "orbitalName").unwrap_or("");
                let from = str_of(r, "from").unwrap_or("");
                let organism = lookups.organism_of.and_then(|f| f(renamed));
                if !own_names.contains(renamed) && own_paths.contains(from) {
                    continue;
                }
                if own_names.contains(renamed)
                    || (organism.is_some() && organisms.contains(&organism))
                {
                    config = rename_nav_hrefs(&config, from, str_of(r, "to").unwrap_or(""));
                }
            }
            if config == file["config"] {
                return file.clone();
            }
            let mut f = file.clone();
            f["config"] = config;
            f
        })
        .collect();
    let identity = dedupe_composed_identity(&mut orbitals)?;
    let owned = collect_owned_pages(&orbitals, lookups.imported_orbital_of);
    let nav_items_narrowed = narrow_nav_items_to_owned_pages(&mut orbitals, &owned);

    let mut ledger = merge_ledgers(&renamed_files);
    if let Some(l) = ledger.as_mut() {
        for r in arr_of(&surface_renames, "traits") {
            if let Some(id) = str_of(r, "id") {
                *l = ledger_rename(l, id, str_of(r, "to").unwrap_or(""), now);
            }
        }
    }

    let mut hoisted: HashMap<String, Value> = HashMap::new();
    for orbital in orbitals.iter_mut() {
        let Some(config) = orbital.get("config").and_then(Value::as_object).cloned() else {
            continue;
        };
        let forwarded = collect_orbital_forwarded_config_keys(orbital);
        let lifted: Map<String, Value> = config
            .iter()
            .filter(|(k, f)| {
                str_of(f, "type") == Some("[NavItem]") && forwarded.contains(k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if lifted.is_empty() {
            continue;
        }
        hoisted.insert(
            str_of(orbital, "name").unwrap_or("").to_string(),
            Value::Object(lifted.clone()),
        );
        let kept: Map<String, Value> = config
            .into_iter()
            .filter(|(k, _)| !lifted.contains_key(k))
            .collect();
        let o = orbital.as_object_mut().expect("an orbital object");
        if kept.is_empty() {
            o.shift_remove("config");
        } else {
            o.insert("config".to_string(), Value::Object(kept));
        }
    }
    let mut config_sources: Vec<Value> = Vec::new();
    for f in &renamed_files {
        if is_schema(f) {
            config_sources.push(f.clone());
        }
        for o in as_definitions(std::slice::from_ref(f)) {
            if let Some(c) = str_of(&o, "name").and_then(|n| hoisted.get(n)) {
                config_sources.push(json!({ "config": c }));
            }
        }
    }
    let mut config = None;
    let mut config_nav_narrowed = Vec::new();
    if let Some(unioned) = union_organism_configs(&config_sources) {
        let (c, drops) = narrow_config_nav_items(&unioned, &owned);
        config = Some(c);
        config_nav_narrowed = drops;
    }
    let mut landing_nav_added = Vec::new();
    if let (Some(c), Some(organism_of)) = (config.clone(), lookups.organism_of) {
        let mut nav_declaring: Vec<String> = Vec::new();
        for f in &renamed_files {
            let Some(fc) = f
                .get("config")
                .and_then(Value::as_object)
                .filter(|_| is_schema(f))
            else {
                continue;
            };
            if !fc.values().any(|e| str_of(e, "type") == Some("[NavItem]")) {
                continue;
            }
            nav_declaring.extend(
                arr_of(f, "orbitals")
                    .iter()
                    .filter_map(|o| str_of(o, "name").map(str::to_string)),
            );
        }
        let (linked, added) = link_landing_pages(
            &orbitals,
            &c,
            organism_of,
            &nav_declaring,
            lookups.organism_order_of,
        );
        config = Some(linked);
        landing_nav_added = added;
    }

    let mut out = Map::new();
    out.insert("orbitals".to_string(), Value::Array(orbitals));
    if let Some(c) = config {
        out.insert("config".to_string(), c);
    }
    if let Some(l) = ledger {
        out.insert("ledger".to_string(), l);
    }
    out.insert("surfaceRenames".to_string(), surface_renames);
    out.insert("identity".to_string(), identity);
    out.insert(
        "navItemsNarrowed".to_string(),
        Value::Array(nav_items_narrowed),
    );
    out.insert(
        "configNavItemsNarrowed".to_string(),
        Value::Array(config_nav_narrowed),
    );
    out.insert(
        "landingNavAdded".to_string(),
        Value::Array(landing_nav_added),
    );
    out.insert(
        "organismRenames".to_string(),
        Value::Array(namespaced.renames),
    );
    out.insert(
        "unrenamableCollisions".to_string(),
        Value::Array(namespaced.unrenamable),
    );
    Ok(Value::Object(out))
}

/// The union of two os grant sets: lists deduplicated in first-seen order, an fs glob or net target named by both
/// with its modes or kinds unioned.
pub fn union_os_grants(a: Option<&Value>, b: Option<&Value>) -> Value {
    let list = |key: &str| -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        for v in a.into_iter().chain(b).flat_map(|g| arr_of(g, key)) {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
        out
    };
    let anchored: Vec<&Value> = a.into_iter().chain(b).filter_map(|g| g.get("anchored")).collect();
    let keyed_in = |sets: &[&Value], key: &str, ids: &[&str], items: &str| -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        for g in sets.iter().flat_map(|g| arr_of(g, key)) {
            match out.iter_mut().find(|o| ids.iter().all(|id| o.get(id) == g.get(id))) {
                Some(seen) => {
                    let mut merged = arr_of(seen, items).to_vec();
                    for x in arr_of(g, items) {
                        if !merged.contains(x) {
                            merged.push(x.clone());
                        }
                    }
                    seen[items] = Value::Array(merged);
                }
                None => out.push(g.clone()),
            }
        }
        out
    };
    let mut anchored_out = Map::new();
    let anchored_fs = keyed_in(&anchored, "fs", &["anchor", "glob"], "modes");
    let anchored_net = keyed_in(&anchored, "net", &["host", "port"], "kinds");
    let mut anchored_exec: Vec<Value> = Vec::new();
    for v in anchored.iter().flat_map(|g| arr_of(g, "exec")) {
        if !anchored_exec.contains(v) {
            anchored_exec.push(v.clone());
        }
    }
    for (key, items) in [("fs", anchored_fs), ("exec", anchored_exec), ("net", anchored_net)] {
        if !items.is_empty() {
            anchored_out.insert(key.to_string(), Value::Array(items));
        }
    }
    let mut out = Map::new();
    let sets: Vec<&Value> = a.into_iter().chain(b).collect();
    let fs = keyed_in(&sets, "fs", &["glob"], "modes");
    let net = keyed_in(&sets, "net", &["target"], "kinds");
    if !fs.is_empty() {
        out.insert("fs".to_string(), Value::Array(fs));
    }
    if !net.is_empty() {
        out.insert("net".to_string(), Value::Array(net));
    }
    for key in ["exec", "env", "stdio", "sys", "desktop"] {
        let l = list(key);
        if !l.is_empty() {
            out.insert(key.to_string(), Value::Array(l));
        }
    }
    if !anchored_out.is_empty() {
        out.insert("anchored".to_string(), Value::Object(anchored_out));
    }
    Value::Object(out)
}

fn with_upstream_os_grants(
    files: &[Value],
    imported: Option<&dyn Fn(&Value) -> Option<Value>>,
) -> Vec<Value> {
    let granted = |o: &Value| -> Value {
        if o.get("reference").is_none() {
            return o.clone();
        }
        let Some(upstream) = imported.and_then(|f| f(o)) else {
            return o.clone();
        };
        let Some(up) = upstream.get("osGrants") else {
            return o.clone();
        };
        let mut out = o.clone();
        out["osGrants"] = union_os_grants(o.get("osGrants"), Some(up));
        out
    };
    files
        .iter()
        .map(|f| {
            if is_schema(f) {
                let mut out = f.clone();
                out["orbitals"] = Value::Array(arr_of(f, "orbitals").iter().map(granted).collect());
                out
            } else {
                granted(f)
            }
        })
        .collect()
}

fn declared_reach(
    files: &[Value],
    imported_program: Option<&dyn Fn(&Value) -> Option<Value>>,
) -> (Vec<Value>, Vec<Value>) {
    let mut inbound: Vec<Value> = Vec::new();
    let mut egress: Vec<Value> = Vec::new();
    let add = |into: &mut Vec<Value>, items: &[Value]| {
        for i in items {
            if !into.contains(i) {
                into.push(i.clone());
            }
        }
    };
    for file in files.iter().filter(|f| is_schema(f)) {
        add(&mut inbound, arr_of(file, "inbound"));
        add(&mut egress, arr_of(file, "egress"));
        for orbital in arr_of(file, "orbitals") {
            if let Some(up) = imported_program.and_then(|f| f(orbital)) {
                add(&mut inbound, arr_of(&up, "inbound"));
                add(&mut egress, arr_of(&up, "egress"));
            }
        }
    }
    (inbound, egress)
}

/// A whole app from its per-orbital files: `compose_orbital_surface`, then `compose_behaviors` for pages, layout
/// and wiring, with the surface's unioned config and ledger, and the hooks and egress the files and the programs
/// their imports come from declare, on the schema. Returns `{ schema, layout, wiring, …outcomes }`.
pub fn compose_app_from_files(
    inputs: &[Value],
    options: &ComposeAppOptions,
    lookups: &ComposeLookups,
) -> Result<Value, String> {
    let files = with_upstream_os_grants(inputs, lookups.imported_orbital_of);
    let surface = compose_orbital_surface(&files, lookups, &options.now)?;
    let orbitals = arr_of(&surface, "orbitals").to_vec();
    let composed = compose_behaviors(ComposeBehaviorsInput {
        app_name: options.app_name.clone(),
        orbitals,
        layout_strategy: options.layout_strategy.clone(),
        event_wiring: options.event_wiring.clone(),
        entity_mappings: None,
    })
    .map_err(|e| e.to_string())?;
    let mut schema = composed.schema;
    if let Some(l) = surface.get("ledger") {
        schema["ledger"] = l.clone();
    }
    if let Some(c) = surface.get("config") {
        schema["config"] = c.clone();
    }
    let (inbound, egress) = declared_reach(&files, lookups.imported_program_of);
    if !inbound.is_empty() {
        schema["inbound"] = Value::Array(inbound);
    }
    if !egress.is_empty() {
        schema["egress"] = Value::Array(egress);
    }
    let program_grants = files
        .iter()
        .filter(|f| is_schema(f))
        .filter_map(|f| f.get("osGrants"))
        .fold(schema.get("osGrants").cloned(), |acc: Option<Value>, g| Some(union_os_grants(acc.as_ref(), Some(g))));
    if let Some(grants) = program_grants {
        schema["osGrants"] = grants;
    }
    let mut out = Map::new();
    out.insert("schema".to_string(), schema);
    out.insert(
        "layout".to_string(),
        serde_json::to_value(&composed.layout).map_err(|e| e.to_string())?,
    );
    out.insert(
        "wiring".to_string(),
        serde_json::to_value(&composed.wiring).map_err(|e| e.to_string())?,
    );
    for key in [
        "surfaceRenames",
        "identity",
        "navItemsNarrowed",
        "configNavItemsNarrowed",
        "landingNavAdded",
        "organismRenames",
        "unrenamableCollisions",
    ] {
        if let Some(v) = surface.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    Ok(Value::Object(out))
}

/// The behavior a specifier names: its last segment (`std/behaviors/std-x` → `std-x`).
fn behavior_name(from: &str) -> &str {
    from.rsplit('/').next().unwrap_or(from)
}

/// The behavior an orbital import takes its source from: `(from, orbital)` through the orbital's `uses`.
fn import_source(orbital: &Value) -> Option<(String, String)> {
    let r = orbital.get("reference").and_then(|r| str_of(r, "ref"))?;
    let mut parts = r.split('.');
    let (alias, kind, name) = (parts.next()?, parts.next()?, parts.next()?);
    if kind != "orbitals" {
        return None;
    }
    let from = arr_of(orbital, "uses")
        .iter()
        .find(|u| str_of(u, "as") == Some(alias))
        .and_then(|u| str_of(u, "from"))?;
    Some((from.to_string(), name.to_string()))
}

/// `program/compose`: the programs, in order, as one app program (`compose_app_from_files`), the registry reads
/// served from the source of every behavior their orbitals import, loaded once up front (`load_source` takes a
/// `uses` specifier). An orbital value at `orbitals[i]` stands for its import orbital.
pub fn compose_programs(
    programs: &[Value],
    app_name: &str,
    layout: Option<&str>,
    theme: Option<&str>,
    overrides: Option<&Value>,
    now: &str,
    load_source: &dyn Fn(&str) -> Result<Value, String>,
) -> Result<Value, String> {
    let programs: Vec<Value> = programs
        .iter()
        .map(orbital_core_base::schema::behavior_value::program_with_orbital_imports)
        .collect::<Result<_, _>>()?;
    let mut loaded: HashMap<String, Value> = HashMap::new();
    for program in &programs {
        for orbital in arr_of(program, "orbitals") {
            for u in arr_of(orbital, "uses") {
                let Some(from) = str_of(u, "from") else {
                    continue;
                };
                let name = behavior_name(from).to_string();
                if !loaded.contains_key(&name) {
                    loaded.insert(name, load_source(from)?);
                }
            }
        }
    }
    let load = |name: &str| loaded.get(name);
    let mut by_name: HashMap<String, Value> = HashMap::new();
    for program in &programs {
        for orbital in arr_of(program, "orbitals") {
            if let Some(n) = str_of(orbital, "name") {
                by_name.insert(n.to_string(), orbital.clone());
            }
        }
    }
    let catalog_orbital_of = |name: &str| -> Option<(String, String)> {
        let (from, orbital) = import_source(by_name.get(name)?)?;
        Some((behavior_name(&from).to_string(), orbital))
    };
    let organism_of = |name: &str| catalog_orbital_of(name).map(|(organism, _)| organism);
    let organism_order_of = |name: &str| -> Option<f64> {
        let (organism, orbital) = catalog_orbital_of(name)?;
        arr_of(load(&organism)?, "orbitals")
            .iter()
            .position(|o| str_of(o, "name") == Some(orbital.as_str()))
            .map(|i| i as f64)
    };
    let atom_trait_of = |orbital: &Value, trait_ref: &str| -> Option<Value> {
        let mut parts = trait_ref.split('.');
        let (alias, kind, name) = (parts.next()?, parts.next()?, parts.next()?);
        if kind != "traits" {
            return None;
        }
        let from = arr_of(orbital, "uses")
            .iter()
            .find(|u| str_of(u, "as") == Some(alias))
            .and_then(|u| str_of(u, "from"))?;
        arr_of(load(behavior_name(from))?, "orbitals")
            .iter()
            .flat_map(|o| arr_of(o, "traits"))
            .find(|t| t.is_object() && t.get("ref").is_none() && str_of(t, "name") == Some(name))
            .cloned()
    };
    let imported_orbital_of = |orbital: &Value| -> Option<Value> {
        let (from, name) = import_source(orbital)?;
        arr_of(load(behavior_name(&from))?, "orbitals")
            .iter()
            .find(|o| str_of(o, "name") == Some(name.as_str()))
            .cloned()
    };
    let imported_program_of = |orbital: &Value| -> Option<Value> {
        let (from, _) = import_source(orbital)?;
        load(behavior_name(&from)).cloned()
    };
    let lookups = ComposeLookups {
        organism_of: Some(&organism_of),
        organism_order_of: Some(&organism_order_of),
        atom_trait_of: Some(&atom_trait_of),
        imported_orbital_of: Some(&imported_orbital_of),
        imported_program_of: Some(&imported_program_of),
    };
    let options = ComposeAppOptions {
        app_name: app_name.to_string(),
        layout_strategy: layout.map(str::to_string),
        event_wiring: None,
        now: now.to_string(),
    };
    let mut schema = compose_app_from_files(&programs, &options, &lookups)?["schema"].take();
    if let Some(theme) = theme {
        schema["theme"] = Value::String(theme.to_string());
    }
    if let Some(overrides) = overrides {
        apply_compose_overrides(&mut schema, overrides)?;
    }
    refuse_ungranted_os_calls(&schema)?;
    Ok(schema)
}

/// The composing caller's explicit overrides: `"<Trait>.<knob>": value` sets that composed trait's knob default (the
/// value of the knob's own kind), and `grants` (an `osGrants` object) is unioned into the program's grants. Both land in
/// the program itself, so they are visible in it; a key naming no trait or no knob of it is refused, as is a trait name
/// more than one orbital uses.
pub fn apply_compose_overrides(schema: &mut Value, overrides: &Value) -> Result<(), String> {
    let Some(entries) = overrides.as_object() else {
        return Err("overrides must be an object".to_string());
    };
    for (key, value) in entries {
        if key == "grants" {
            if !value.is_object() {
                return Err("overrides.grants must be an osGrants object".to_string());
            }
            schema["osGrants"] = union_os_grants(schema.get("osGrants"), Some(value));
            continue;
        }
        let Some((trait_name, knob)) = key.split_once('.').filter(|(t, k)| !t.is_empty() && !k.is_empty() && !k.contains('.')) else {
            return Err(format!("override \"{key}\" is not <Trait>.<knob> or grants"));
        };
        let mut found: Vec<&mut Value> = Vec::new();
        for orbital in schema.get_mut("orbitals").and_then(Value::as_array_mut).into_iter().flatten() {
            for t in orbital.get_mut("traits").and_then(Value::as_array_mut).into_iter().flatten() {
                if str_of(t, "name") == Some(trait_name) {
                    found.push(t);
                }
            }
        }
        let t = match found.len() {
            0 => return Err(format!("override \"{key}\" names no trait {trait_name} in the composed program")),
            1 => found.pop().expect("one trait"),
            n => return Err(format!("override \"{key}\" is ambiguous: {n} traits are named {trait_name}")),
        };
        let Some(field) = t.get_mut("config").and_then(|c| c.get_mut(knob)).filter(|f| f.is_object()) else {
            return Err(format!("override \"{key}\" names no knob {knob} of trait {trait_name}"));
        };
        let kind = |v: &Value| match v {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if let Some(current) = field.get("default").filter(|d| !d.is_null()) {
            if kind(current) != kind(value) {
                return Err(format!("override \"{key}\" is a {}, the knob holds a {}", kind(value), kind(current)));
            }
        }
        field["default"] = value.clone();
    }
    Ok(())
}

/// A composed program is checked for grants as an authored one is: an `os/` call its grants do not cover is refused here,
/// not at the first ask of a booted server.
fn refuse_ungranted_os_calls(schema: &Value) -> Result<(), String> {
    let parsed: orbital_core_base::schema::OrbitalSchema =
        serde_json::from_value(schema.clone()).map_err(|e| format!("the composed program does not parse: {e}"))?;
    let result = crate::phases::validation::effect::validate_os_grants(&parsed);
    let refused: Vec<String> = result
        .errors
        .iter()
        .map(|e| format!("{} at {}: {}", e.code.as_str(), e.path, e.message))
        .collect();
    if refused.is_empty() {
        Ok(())
    } else {
        Err(format!("the composed program's os calls are not granted:\n- {}", refused.join("\n- ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> Vec<(String, Value)> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compose-app");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("the golden fixtures")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|p| {
                (
                    p.file_name().unwrap().to_string_lossy().to_string(),
                    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap(),
                )
            })
            .collect()
    }

    /// A lookup that answers exactly what the original answered for the same argument (a call it never saw is a
    /// divergence and panics).
    struct Recorded(Vec<(Value, Value)>);

    impl Recorded {
        fn of(golden: &Value, path: &str) -> Option<Recorded> {
            arr_of(golden, "lookups")
                .iter()
                .find(|l| str_of(l, "$lookup") == Some(path))
                .map(|l| {
                    Recorded(
                        arr_of(l, "calls")
                            .iter()
                            .map(|c| (c["arg"].clone(), c["result"].clone()))
                            .collect(),
                    )
                })
        }
        fn answer(&self, arg: &Value) -> Value {
            self.0
                .iter()
                .find(|(a, _)| a == arg)
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| panic!("a lookup the original never made: {arg}"))
        }
    }

    fn some(v: Value) -> Option<Value> {
        (!v.is_null()).then_some(v)
    }

    fn run(golden: &Value) -> Result<(Value, Vec<Value>), String> {
        let args = arr_of(golden, "args");
        let opts = args.get(1).cloned().unwrap_or(Value::Null);
        let organism = Recorded::of(golden, "arg1.organismOf");
        let order = Recorded::of(golden, "arg1.organismOrderOf");
        let atom = Recorded::of(golden, "arg1.atomTraitOf");
        let imported = Recorded::of(golden, "arg1.importedOrbitalOf");
        let program = Recorded::of(golden, "arg1.importedProgramOf");
        let organism_of = |n: &str| {
            organism
                .as_ref()
                .map(|r| r.answer(&json!(n)))
                .and_then(some)
                .and_then(|v| v.as_str().map(str::to_string))
        };
        let order_of = |n: &str| {
            order
                .as_ref()
                .map(|r| r.answer(&json!(n)))
                .and_then(some)
                .and_then(|v| v.as_f64())
        };
        let atom_of = |o: &Value, r: &str| {
            atom.as_ref()
                .map(|rec| rec.answer(&json!([o, r])))
                .and_then(some)
        };
        let imported_of = |o: &Value| imported.as_ref().map(|r| r.answer(o)).and_then(some);
        let program_of = |o: &Value| program.as_ref().map(|r| r.answer(o)).and_then(some);
        let has = |k: &str| opts.get(k).is_some();
        let lookups = ComposeLookups {
            organism_of: has("organismOf")
                .then_some(&organism_of as &dyn Fn(&str) -> Option<String>),
            organism_order_of: has("organismOrderOf")
                .then_some(&order_of as &dyn Fn(&str) -> Option<f64>),
            atom_trait_of: has("atomTraitOf")
                .then_some(&atom_of as &dyn Fn(&Value, &str) -> Option<Value>),
            imported_orbital_of: has("importedOrbitalOf")
                .then_some(&imported_of as &dyn Fn(&Value) -> Option<Value>),
            imported_program_of: has("importedProgramOf")
                .then_some(&program_of as &dyn Fn(&Value) -> Option<Value>),
        };
        let now = "2026-01-01T00:00:00.000Z";
        let files = args
            .first()
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match str_of(golden, "fn").unwrap_or("") {
            "dedupeComposedSurface" => {
                let mut o = files;
                let r = dedupe_composed_surface(&mut o);
                Ok((r, o))
            }
            "dedupeComposedIdentity" => {
                let mut o = files;
                let r = dedupe_composed_identity(&mut o)?;
                Ok((r, o))
            }
            "composeOrbitalSurface" => Ok((compose_orbital_surface(&files, &lookups, now)?, files)),
            "composeAppFromFiles" => {
                let options = ComposeAppOptions {
                    app_name: str_of(&opts, "appName").unwrap_or("").to_string(),
                    layout_strategy: str_of(&opts, "layoutStrategy").map(str::to_string),
                    event_wiring: opts
                        .get("eventWiring")
                        .map(|w| serde_json::from_value(w.clone()).expect("wiring")),
                    now: now.to_string(),
                };
                Ok((compose_app_from_files(&files, &options, &lookups)?, files))
            }
            other => Err(format!("no port of {other}")),
        }
    }

    #[test]
    fn every_recorded_composition_matches_the_original() {
        let mut failures = Vec::new();
        for (name, golden) in fixtures() {
            let want_error = str_of(&golden, "error");
            match (run(&golden), want_error) {
                (Ok((result, after)), None) => {
                    if result != golden["result"] {
                        failures.push(format!(
                            "{name} ({}): result differs\n want {}\n got  {}",
                            str_of(&golden, "test").unwrap_or(""),
                            golden["result"],
                            result
                        ));
                    } else if golden["argsAfter"].get(0).is_some_and(Value::is_array)
                        && Value::Array(after) != golden["argsAfter"][0]
                    {
                        failures.push(format!("{name}: the orbitals after differ"));
                    }
                }
                (Err(e), Some(want)) if e == want => {}
                (Err(e), want) => failures.push(format!("{name}: error {e:?}, want {want:?}")),
                (Ok(_), Some(want)) => failures.push(format!("{name}: no error, want {want:?}")),
            }
        }
        assert!(
            failures.is_empty(),
            "{} of the golden compositions differ:\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    #[test]
    fn route_slug_and_humanized_labels_follow_the_original_rules() {
        assert_eq!(
            orbital_route_slug("ExpenseDashboardOrbital"),
            "expense-dashboard"
        );
        assert_eq!(orbital_route_slug("HRPortal"), "hrportal");
        assert_eq!(humanize_key("TeamMembers"), "Team Members");
        assert_eq!(humanize_key("team-members_list"), "Team Members List");
        assert_eq!(humanize_key("HTTPServer"), "HTTP Server");
    }

    #[test]
    fn os_grants_union_merges_modes_on_a_shared_glob_and_control_nothing_is_no_grant() {
        let a = json!({ "fs": [{ "glob": "~/.a/**", "modes": ["read"] }], "env": ["A", "B"] });
        let b = json!({ "fs": [{ "glob": "~/.a/**", "modes": ["write", "read"] }, { "glob": "./x", "modes": ["meta"] }], "env": ["B", "C"] });
        assert_eq!(
            union_os_grants(Some(&a), Some(&b)),
            json!({ "fs": [{ "glob": "~/.a/**", "modes": ["read", "write"] }, { "glob": "./x", "modes": ["meta"] }], "env": ["A", "B", "C"] })
        );
        assert_eq!(union_os_grants(None, None), json!({}));
    }

    #[test]
    fn os_grants_union_carries_anchored_entries_and_merges_modes_on_the_same_anchor_and_glob() {
        let a = json!({ "anchored": { "fs": [{ "anchor": "cwd", "glob": "docs/**", "modes": ["read"] }], "exec": [{ "anchor": "exe-dir", "path": "bun" }] } });
        let b = json!({ "exec": ["bun"], "anchored": {
            "fs": [{ "anchor": "cwd", "glob": "docs/**", "modes": ["write"] }, { "anchor": "exe-dir", "glob": "docs/**", "modes": ["read"] }],
            "exec": [{ "anchor": "exe-dir", "path": "bun" }],
            "net": [{ "host": { "anchor": { "env": "HOST" } }, "port": { "anchor": { "env": "PORT" } }, "kinds": ["listen"] }] } });
        assert_eq!(
            union_os_grants(Some(&a), Some(&b)),
            json!({ "exec": ["bun"], "anchored": {
                "fs": [{ "anchor": "cwd", "glob": "docs/**", "modes": ["read", "write"] }, { "anchor": "exe-dir", "glob": "docs/**", "modes": ["read"] }],
                "exec": [{ "anchor": "exe-dir", "path": "bun" }],
                "net": [{ "host": { "anchor": { "env": "HOST" } }, "port": { "anchor": { "env": "PORT" } }, "kinds": ["listen"] }] } })
        );
        assert!(union_os_grants(Some(&json!({ "exec": ["git"] })), None).get("anchored").is_none(), "control: no anchored key without anchored entries");
    }

    fn composable(name: &str, orbital: &str, grants: Option<Value>, effect: Option<Value>, config: Option<Value>) -> Value {
        let mut t = json!({
            "name": format!("{orbital}Trait"), "linkedEntity": format!("{orbital}Row"),
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "events": [{ "key": "GO", "name": "GO" }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": effect.into_iter().collect::<Vec<_>>() }] }
        });
        if let Some(c) = config {
            t["config"] = c;
        }
        let mut p = json!({
            "name": name, "version": "1.0.0",
            "orbitals": [{ "name": orbital, "entity": { "name": format!("{orbital}Row"), "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [t], "pages": [{ "name": format!("{orbital}Page"), "path": format!("/{}", orbital.to_lowercase()), "traits": [{ "ref": format!("{orbital}Trait") }] }] }]
        });
        if let Some(g) = grants {
            p["osGrants"] = g;
        }
        p
    }

    fn compose(programs: &[Value], overrides: Option<&Value>) -> Result<Value, String> {
        compose_programs(programs, "served", None, None, overrides, "2026-10-21T00:00:00.000Z", &|from: &str| Err(format!("no source {from}")))
    }

    fn getenv(name: &str) -> Value {
        json!(["os/getenv", name, { "emit": { "success": "GO", "failure": "GO" } }])
    }

    #[test]
    fn composing_keeps_each_programs_top_level_grants() {
        let server = composable("server", "Http", Some(json!({ "env": ["NODE_ENV"] })), Some(getenv("NODE_ENV")), None);
        let app = composable("app", "Notes", None, None, None);
        let composed = compose(&[app, server], None).unwrap();
        assert_eq!(composed["osGrants"]["env"], json!(["NODE_ENV"]), "the server's env grant survives: {}", composed["osGrants"]);
    }

    #[test]
    fn control_programs_declaring_no_grants_compose_to_none() {
        let composed = compose(&[composable("app", "Notes", None, None, None), composable("other", "Desk", None, None, None)], None).unwrap();
        assert!(composed.get("osGrants").is_none(), "{}", composed["osGrants"]);
    }

    #[test]
    fn a_composed_os_call_no_grant_covers_is_refused_at_compose() {
        let error = compose(&[composable("app", "Notes", None, Some(getenv("SECRET")), None)], None).unwrap_err();
        assert!(error.contains("ORB_OS_GRANT_MISSING"), "{error}");
    }

    fn knobbed() -> Value {
        composable("server", "Http", None, None, Some(json!({ "port": { "type": "number", "default": 3000 } })))
    }

    #[test]
    fn an_override_reaches_the_knob_and_its_grant() {
        let overrides = json!({ "HttpTrait.port": 4100, "grants": { "net": [{ "target": "127.0.0.1:4100", "kinds": ["listen"] }] } });
        let composed = compose(&[knobbed()], Some(&overrides)).unwrap();
        let t = &composed["orbitals"][0]["traits"][0];
        assert_eq!(t["config"]["port"]["default"], json!(4100), "{t}");
        assert_eq!(composed["osGrants"]["net"], json!([{ "target": "127.0.0.1:4100", "kinds": ["listen"] }]));
        let plain = compose(&[knobbed()], None).unwrap();
        assert_eq!(plain["orbitals"][0]["traits"][0]["config"]["port"]["default"], json!(3000), "control: no overrides keeps the default");
        assert!(plain.get("osGrants").is_none(), "control: and widens no grant");
    }

    #[test]
    fn an_override_naming_an_unknown_knob_or_trait_or_kind_is_refused() {
        for (overrides, expect) in [
            (json!({ "HttpTrait.prot": 1 }), "names no knob prot"),
            (json!({ "Nope.port": 1 }), "names no trait Nope"),
            (json!({ "HttpTrait.port": "4100" }), "is a string, the knob holds a number"),
            (json!({ "port": 1 }), "is not <Trait>.<knob>"),
            (json!({ "grants": ["127.0.0.1:1"] }), "osGrants object"),
        ] {
            let error = compose(&[knobbed()], Some(&overrides)).unwrap_err();
            assert!(error.contains(expect), "{overrides}: {error}");
        }
    }
}
