//! The declared model each record literal in an emit payload is built as, keyed by the node's path in the same scheme
//! as [`super::expression_types`], for readers that construct typed values (the native source renderers): `Alias`
//! for a record of a declared alias, `Union.Arm` for the arm of a declared union the literal fits. The arm is the first
//! one the literal fits: it sets no key the arm lacks, sets every key the arm requires, and every closed-string key it
//! sets to a string literal holds one of the arm's values. A string literal in a closed-string field of a model is
//! `enum:<Owner><Field>` (the model's enum for that field, `<Field>` in Pascal case), in a record and in
//! `(set @entity.<f> "<literal>")`; a whole payload (`?`) passed into a record field is that record's model.

use orbital_core_base::schema::{EntityDefinition, OrbitalSchema, PayloadField, SExpression, TraitReference};
use orbital_core_base::str_utils::split_to_pascal_case;
use orbital_core_eval::lower::recognized_call_head;
use std::collections::BTreeMap;

/// Every record literal of an inline trait's emit payloads whose declared field is a declared alias: path → model.
pub fn expected_types(schema: &OrbitalSchema) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let emits: BTreeMap<&str, &[PayloadField]> = trait_def
                .emits
                .iter()
                .flat_map(|emits| emits.iter())
                .filter_map(|e| e.payload_schema.as_deref().map(|p| (e.event.as_str(), p)))
                .collect();
            let entity = trait_def.linked_entity.as_deref().and_then(|name| {
                std::iter::once(&orbital.entity)
                    .chain(orbital.auxiliary_entities.iter().flatten())
                    .filter_map(|e| e.as_inline())
                    .find(|e| e.name == name)
            });
            let walker = Walker { schema, emits: &emits, entity };
            for (k, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                for (m, effect) in transition.effects.iter().flatten().enumerate() {
                    let at = format!("orbitals[{oi}].traits[{ti}].stateMachine.transitions[{k}].effects[{m}]");
                    walker.node(effect, &at, &mut out);
                }
            }
        }
    }
    out
}

/// Every declared record alias (an orbital's `types` or the program's) with its member names in declared order, for
/// readers that build a model from a whole payload.
pub fn expected_models(schema: &OrbitalSchema) -> BTreeMap<String, Vec<String>> {
    schema
        .orbitals
        .iter()
        .filter_map(|orbital| orbital.types.as_ref())
        .chain(schema.types.iter())
        .flat_map(|types| types.iter())
        .filter(|(_, members)| !matches!(members.as_slice(), [only] if only.name.is_empty()))
        .map(|(name, members)| (name.clone(), members.iter().map(|m| m.name.clone()).collect()))
        .collect()
}

struct Walker<'a> {
    schema: &'a OrbitalSchema,
    emits: &'a BTreeMap<&'a str, &'a [PayloadField]>,
    entity: Option<&'a EntityDefinition>,
}

fn piece(head: &str, i: usize) -> String {
    match (head, i) {
        ("do", _) => format!(".do[{}]", i - 1),
        ("if", 2) => ".then".to_string(),
        ("if", 3) => ".else".to_string(),
        ("when", 2..) => format!(".then[{}]", i - 2),
        ("let", 2..) => format!(".body[{}]", i - 2),
        _ => format!("[{i}]"),
    }
}

impl Walker<'_> {
    fn node(&self, source: &SExpression, path: &str, out: &mut BTreeMap<String, String>) {
        match source {
            SExpression::Array(items) => match recognized_call_head(source) {
                Some("emit") => {
                    let declared = items.get(1).and_then(string_of).and_then(|ev| self.emits.get(ev));
                    if let (Some(fields), Some(SExpression::Object(literal))) = (declared, items.get(2)) {
                        self.members(literal.iter().map(|(k, v)| (k.to_string(), v)), fields, None, &format!("{path}[2]"), out);
                    }
                }
                Some("set") => {
                    let field = items.get(1).and_then(string_of).and_then(|target| target.strip_prefix("@entity.")).filter(|f| !f.contains('.'));
                    let closed = field.zip(self.entity).and_then(|(f, entity)| {
                        entity.fields.iter().find(|d| d.name.as_deref() == Some(f)).filter(|d| d.values.is_some()).map(|_| (f, entity))
                    });
                    match (closed, items.get(2).and_then(string_of)) {
                        (Some((f, entity)), Some(text)) if is_literal(text) => {
                            out.insert(format!("{path}[2]"), format!("enum:{}{}", entity.name, split_to_pascal_case(f)));
                        }
                        _ => {
                            for (i, item) in items.iter().enumerate().skip(1) {
                                self.node(item, &format!("{path}{}", piece("set", i)), out);
                            }
                        }
                    }
                }
                Some("let") => {
                    if let Some(SExpression::Array(pairs)) = items.get(1) {
                        for (k, pair) in pairs.iter().enumerate() {
                            if let SExpression::Array(kv) = pair {
                                if let Some(value) = kv.get(1) {
                                    self.node(value, &format!("{path}[1][{k}][1]"), out);
                                }
                            }
                        }
                    }
                    for (i, item) in items.iter().enumerate().skip(2) {
                        self.node(item, &format!("{path}{}", piece("let", i)), out);
                    }
                }
                Some(head) => {
                    for (i, item) in items.iter().enumerate().skip(1) {
                        self.node(item, &format!("{path}{}", piece(head, i)), out);
                    }
                }
                None => {
                    for (i, item) in items.iter().enumerate() {
                        self.node(item, &format!("{path}[{i}]"), out);
                    }
                }
            },
            SExpression::Object(fields) => {
                for (key, value) in fields.iter() {
                    self.node(value, &format!("{path}.{key}"), out);
                }
            }
            _ => {}
        }
    }

    /// The keys of a record literal against the declared members of its record.
    fn members<'s>(
        &self,
        literal: impl Iterator<Item = (String, &'s SExpression)>,
        fields: &[PayloadField],
        owner: Option<&str>,
        path: &str,
        out: &mut BTreeMap<String, String>,
    ) {
        for (key, value) in literal {
            let at = format!("{path}.{key}");
            match fields.iter().find(|f| f.name == key) {
                Some(field) => match (owner, field.values.as_ref(), string_of(value)) {
                    (Some(model), Some(_), Some(text)) if is_literal(text) => {
                        out.insert(at, format!("enum:{model}{}", split_to_pascal_case(&key)));
                    }
                    _ => self.value(value, field, &at, out),
                },
                None => self.node(value, &at, out),
            }
        }
    }

    /// A value in the position of `field`: a record literal of a declared alias is recorded and walked against it, a
    /// list literal's items against the list's alias; anything else is walked for nested emits.
    fn value(&self, value: &SExpression, field: &PayloadField, path: &str, out: &mut BTreeMap<String, String>) {
        let alias = field
            .projected_from
            .as_ref()
            .filter(|p| p.field.is_none())
            .map(|p| p.type_name.as_str())
            .and_then(|name| self.alias(name).map(|members| (name, members)));
        let list = field.field_type.trim_start().starts_with('[');
        match (value, alias) {
            (SExpression::Object(literal), Some((name, members))) if !list => {
                let keys: Vec<(String, &SExpression)> = literal.iter().map(|(k, v)| (k.to_string(), v)).collect();
                match members {
                    [union] if union.name.is_empty() && union.field_type == "union" => {
                        let arms = union.properties.as_deref().unwrap_or_default();
                        if let Some(arm) = arms.iter().find(|arm| fits(&keys, arm.properties.as_deref().unwrap_or_default())) {
                            out.insert(path.to_string(), format!("{name}.{}", arm.name));
                            self.members(keys.into_iter(), arm.properties.as_deref().unwrap_or_default(), Some(&arm.name), path, out);
                        } else {
                            self.members(keys.into_iter(), &[], None, path, out);
                        }
                    }
                    record => {
                        out.insert(path.to_string(), name.to_string());
                        self.members(keys.into_iter(), record, Some(name), path, out);
                    }
                }
            }
            (SExpression::String(text), Some((name, members)))
                if !list && (text.as_str() == "?" || text.as_str() == "@payload") && !matches!(members, [only] if only.name.is_empty()) =>
            {
                out.insert(path.to_string(), name.to_string());
            }
            (SExpression::Array(items), Some(_)) if list && recognized_call_head(value).is_none() => {
                let mut item = field.clone();
                item.field_type = item.field_type.trim().trim_start_matches('[').trim_end_matches(']').to_string();
                for (i, entry) in items.iter().enumerate() {
                    self.value(entry, &item, &format!("{path}[{i}]"), out);
                }
            }
            _ => self.node(value, path, out),
        }
    }

    fn alias(&self, name: &str) -> Option<&[PayloadField]> {
        self.schema
            .orbitals
            .iter()
            .filter_map(|orbital| orbital.types.as_ref())
            .chain(self.schema.types.iter())
            .find_map(|types| types.get(name))
            .map(Vec::as_slice)
    }
}

fn string_of(s: &SExpression) -> Option<&str> {
    match s {
        SExpression::String(text) => Some(text.as_str()),
        _ => None,
    }
}

/// A string the author wrote as a value, not a binding (`@…`) or a payload read (`?…`).
fn is_literal(text: &str) -> bool {
    !text.starts_with('@') && !text.starts_with('?')
}

fn fits(keys: &[(String, &SExpression)], arm: &[PayloadField]) -> bool {
    keys.iter().all(|(k, _)| arm.iter().any(|f| &f.name == k))
        && arm.iter().filter(|f| f.required == Some(true)).all(|f| keys.iter().any(|(k, _)| k == &f.name))
        && keys.iter().all(|(k, v)| {
            let closed = arm.iter().find(|f| &f.name == k).and_then(|f| f.values.as_deref());
            match (closed, string_of(v)) {
                (Some(values), Some(text)) if is_literal(text) => values.iter().any(|x| x == text),
                _ => true,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ladder(effect: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "t", "version": "1.0.0",
            "orbitals": [{
                "name": "O",
                "types": {
                    "Matched": [{ "name": "", "type": "union", "properties": [
                        { "name": "MatchedText", "type": "object", "projectedFrom": { "type": "MatchedText" }, "properties": [
                            { "name": "type", "type": "string", "values": ["text"] },
                            { "name": "text", "type": "string", "required": true } ] },
                        { "name": "MatchedIndex", "type": "object", "projectedFrom": { "type": "MatchedIndex" }, "properties": [
                            { "name": "type", "type": "string", "values": ["index"] },
                            { "name": "index", "type": "int", "required": true } ] } ] }],
                    "Tag": [{ "name": "label", "type": "string", "required": true }]
                },
                "entity": { "name": "W", "fields": [{ "name": "status", "type": "string", "values": ["idle", "busy"] }, { "name": "note", "type": "string" }] },
                "traits": [{
                    "name": "Sink",
                    "linkedEntity": "W",
                    "emits": [{ "event": "DECIDED", "payloadSchema": [
                        { "name": "matched", "type": "union", "required": true, "projectedFrom": { "type": "Matched" } },
                        { "name": "tags", "type": "[object]", "projectedFrom": { "type": "Tag" } },
                        { "name": "tag", "type": "object", "projectedFrom": { "type": "Tag" } },
                        { "name": "extra", "type": "object", "properties": [{ "name": "n", "type": "string" }] } ] }],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": [effect] }]
                    }
                }]
            }]
        }))
        .expect("fixture parses")
    }

    const AT: &str = "orbitals[0].traits[0].stateMachine.transitions[0].effects[0]";

    #[test]
    fn a_tagged_literal_picks_its_union_arm_and_a_list_literal_names_each_item() {
        let got = expected_types(&ladder(serde_json::json!(["emit", "DECIDED", {
            "matched": { "type": "index", "index": "@payload.i" },
            "tags": [{ "label": "a" }, { "label": "b" }],
            "extra": { "n": "x" }
        }])));
        assert_eq!(got.get(&format!("{AT}[2].matched")).map(String::as_str), Some("Matched.MatchedIndex"));
        assert_eq!(got.get(&format!("{AT}[2].tags[0]")).map(String::as_str), Some("Tag"));
        assert_eq!(got.get(&format!("{AT}[2].tags[1]")).map(String::as_str), Some("Tag"));
        assert_eq!(got.get(&format!("{AT}[2].extra")), None, "control: an anonymous record names no model");
    }

    #[test]
    fn a_literal_that_fits_no_arm_names_no_model_and_a_nested_emit_keeps_its_path() {
        let got = expected_types(&ladder(serde_json::json!(["emit", "DECIDED", { "matched": { "type": "tensor", "output": [] } }])));
        assert_eq!(got.get(&format!("{AT}[2].matched")), None, "no arm has a `tensor` tag or an `output` key");
        let got = expected_types(&ladder(serde_json::json!(["if", true, ["emit", "DECIDED", { "matched": { "type": "text", "text": "k" } }], null])));
        assert_eq!(got.get(&format!("{AT}.then[2].matched")).map(String::as_str), Some("Matched.MatchedText"));
    }

    #[test]
    fn a_closed_string_literal_names_its_enum_and_a_whole_payload_names_its_record() {
        let got = expected_types(&ladder(serde_json::json!(["emit", "DECIDED", {
            "matched": { "type": "text", "text": "@payload.k" },
            "tag": "@payload"
        }])));
        assert_eq!(got.get(&format!("{AT}[2].matched.type")).map(String::as_str), Some("enum:MatchedTextType"));
        assert_eq!(got.get(&format!("{AT}[2].matched.text")), None, "control: a binding into a string field names nothing");
        assert_eq!(got.get(&format!("{AT}[2].tag")).map(String::as_str), Some("Tag"));
        let got = expected_types(&ladder(serde_json::json!(["set", "@entity.status", "busy"])));
        assert_eq!(got.get(&format!("{AT}[2]")).map(String::as_str), Some("enum:WStatus"));
        let got = expected_types(&ladder(serde_json::json!(["set", "@entity.note", "x"])));
        assert_eq!(got.get(&format!("{AT}[2]")), None, "control: an open string field names no enum");
        let got = expected_types(&ladder(serde_json::json!(["set", "@entity.status", "@payload.s"])));
        assert_eq!(got.get(&format!("{AT}[2]")), None, "control: a binding into an enum field is not a literal");
    }

    #[test]
    fn expected_models_lists_each_record_alias_in_declared_order_and_skips_a_union() {
        let models = expected_models(&ladder(serde_json::json!(null)));
        assert_eq!(models.get("Tag"), Some(&vec!["label".to_string()]));
        assert_eq!(models.get("Matched"), None, "control: a union alias is not a record");
    }
}
