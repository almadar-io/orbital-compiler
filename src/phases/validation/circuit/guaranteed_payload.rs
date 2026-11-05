//! The payload fields every producer of an event guarantees.
//!
//! An event delivered only by effect-slot failures (`persist` / `fetch` `emit: { failure: E }`) carries the
//! fields that slot's contract marks required, however loosely the trait restates the event. The contract
//! (`emit_contract`) is the single source; this pass only intersects it over every producer in the program and
//! gives up on any event something outside the effect slots can deliver.

use std::collections::{HashMap, HashSet};

use orbital_core_base::schema::{ListenSource, OrbitalSchema, SExpression, TraitReference};

use super::fired_events::collect_schema_fired_events;
use super::produced_shape::produced_failure_payload;

#[cfg(test)]
thread_local! {
    pub(crate) static RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
struct Producers {
    slot_fields: HashMap<String, Option<HashSet<String>>>,
    unguaranteed: HashSet<String>,
}

impl Producers {
    fn narrow(&mut self, event: &str, guaranteed: HashSet<String>) {
        self.slot_fields
            .entry(event.to_string())
            .and_modify(|held| {
                if let Some(held) = held {
                    held.retain(|f| guaranteed.contains(f));
                }
            })
            .or_insert(Some(guaranteed));
    }
}

/// Per event name: the payload fields every producer in the program sets. An event with no effect-slot producer,
/// or one anything else can deliver (a control, an orbital-level listen, a dispatch, a producer whose shape is
/// undecidable), is absent. An `external` emit only publishes the event; it is no producer. A field filled from a
/// guaranteed field (a route's mapping, an emit forwarding `@payload.x`) is guaranteed too, so the pass repeats
/// until nothing more is learned; each round only adds, so it ends.
pub(crate) fn guaranteed_payload_fields(
    schema: &OrbitalSchema,
) -> HashMap<String, HashSet<String>> {
    #[cfg(test)]
    RUNS.with(|r| r.set(r.get() + 1));
    if schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .any(|t| !matches!(t, TraitReference::Inline(_)))
    {
        return HashMap::new();
    }
    // `Trait.EVENT` -> the payload fields that trait's emit declares required (enforced at each of its emit sites).
    let declared: HashMap<String, HashSet<String>> = schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td),
            _ => None,
        })
        .flat_map(|td| {
            td.emits.iter().flatten().map(move |emit| {
                let required = emit
                    .payload_schema
                    .iter()
                    .flatten()
                    .filter(|f| f.required == Some(true))
                    .map(|f| f.name.clone())
                    .collect();
                (format!("{}.{}", td.name, emit.event), required)
            })
        })
        .collect();
    let controls = collect_schema_fired_events(schema);
    let mut known = HashMap::new();
    loop {
        let next = one_round(schema, &known, &declared, &controls);
        if next == known {
            return next;
        }
        known = next;
    }
}

struct Scope<'a> {
    event: Option<&'a str>,
    known: &'a HashMap<String, HashSet<String>>,
}

impl Scope<'_> {
    /// A payload value certainly present: a set literal or call, or `@payload.<f>` of a field this scope's
    /// triggering event guarantees.
    fn sets(&self, value: &SExpression) -> bool {
        is_set_literal(value)
            || matches!(value, SExpression::String(s) if s.strip_prefix("@payload.").is_some_and(|field| {
                !field.contains('.') && self.event.and_then(|e| self.known.get(e)).is_some_and(|f| f.contains(field))
            }))
    }
}

fn one_round(
    schema: &OrbitalSchema,
    known: &HashMap<String, HashSet<String>>,
    declared: &HashMap<String, HashSet<String>>,
    controls: &super::fired_events::FiredEvents,
) -> HashMap<String, HashSet<String>> {
    let mut producers = Producers::default();
    for orbital in &schema.orbitals {
        for listen in orbital.listens.iter().flatten() {
            producers.unguaranteed.insert(
                listen
                    .triggers
                    .clone()
                    .unwrap_or_else(|| listen.event.clone()),
            );
        }
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            for listen in td.listens.iter().flatten() {
                let trigger = listen
                    .triggers
                    .clone()
                    .unwrap_or_else(|| listen.event.clone());
                if trigger == listen.event && listen.payload_mapping.is_none() {
                    continue;
                }
                // A route delivers what its emitter declares; an emitter this program does not hold guarantees nothing.
                let emitter = match &listen.source {
                    Some(
                        ListenSource::Trait { trait_name, .. }
                        | ListenSource::Orbital { trait_name, .. },
                    ) => declared.get(&format!("{trait_name}.{}", listen.event)),
                    Some(ListenSource::Any) => None,
                    None => declared.get(&listen.event),
                };
                let Some(required) = emitter else {
                    producers.unguaranteed.insert(trigger);
                    continue;
                };
                match &listen.payload_mapping {
                    None => producers.narrow(&trigger, required.clone()),
                    Some(mapping) => {
                        let source = HashMap::from([(listen.event.clone(), required.clone())]);
                        let scope = Scope {
                            event: Some(listen.event.as_str()),
                            known: &source,
                        };
                        let set = mapping
                            .iter()
                            .filter(|(_, v)| scope.sets(v))
                            .map(|(k, _)| k.as_str().to_string())
                            .collect();
                        producers.narrow(&trigger, set);
                    }
                }
            }
            for transition in &td.state_machine.transitions {
                let scope = Scope {
                    event: Some(transition.event.as_str()),
                    known,
                };
                transition
                    .effects
                    .iter()
                    .flatten()
                    .for_each(|e| walk(e, &mut producers, &scope));
            }
            let none = Scope { event: None, known };
            for tick in td.ticks.iter().flatten() {
                tick.effects
                    .iter()
                    .for_each(|e| walk(e, &mut producers, &none));
            }
            for field in td.config.iter().flat_map(|c| c.values()) {
                if let Some(default) = &field.default {
                    walk(default, &mut producers, &none);
                }
            }
        }
    }
    producers
        .slot_fields
        .into_iter()
        .filter(|(event, _)| {
            !producers.unguaranteed.contains(event)
                && !controls.by_control.iter().any(|(_, e)| e == event)
        })
        .filter_map(|(event, fields)| fields.filter(|f| !f.is_empty()).map(|f| (event, f)))
        .collect()
}

fn walk(expr: &SExpression, producers: &mut Producers, scope: &Scope<'_>) {
    match expr {
        SExpression::Object(fields) => fields.values().for_each(|v| walk(v, producers, scope)),
        SExpression::Array(items) => {
            let op = items.first().and_then(SExpression::as_str);
            match (op, items.get(1), items.get(2)) {
                (Some("emit"), Some(SExpression::String(event)), payload) => {
                    let set = match payload {
                        Some(SExpression::Object(literal)) => literal
                            .iter()
                            .filter(|(_, v)| scope.sets(v))
                            .map(|(k, _)| k.as_str().to_string())
                            .collect(),
                        _ => HashSet::new(),
                    };
                    producers.narrow(event.as_str(), set);
                }
                (Some("program/dispatch"), Some(SExpression::String(address)), _) => {
                    if let Some(SExpression::Object(options)) = items.get(3) {
                        if let Some(event) = orbital_core_base::agent_ports::calling_trait_event(
                            address,
                            options.keys().map(|k| k.as_str()),
                        ) {
                            producers.unguaranteed.insert(event.to_string());
                        }
                    }
                }
                _ => {}
            }
            if let Some(op) = op {
                for item in items {
                    let SExpression::Object(options) = item else {
                        continue;
                    };
                    let Some(SExpression::Object(routes)) = options.get("emit") else {
                        continue;
                    };
                    for (key, route) in routes.iter() {
                        let SExpression::String(event) = route else {
                            continue;
                        };
                        match (key.as_str(), produced_failure_payload(op, items.as_slice())) {
                            ("failure", Some(shape)) => {
                                producers.narrow(
                                    event.as_str(),
                                    shape
                                        .into_iter()
                                        .filter(|f| f.required == Some(true))
                                        .map(|f| f.name)
                                        .collect(),
                                );
                            }
                            _ => {
                                producers.unguaranteed.insert(event.to_string());
                            }
                        }
                    }
                }
            }
            items.iter().for_each(|item| walk(item, producers, scope));
        }
        _ => {}
    }
}

/// A payload value that is certainly not null: a non-binding string, a number, a boolean, or a call to an operator
/// that answers a value or raises (neither failable nor declared to return null), so an emit that runs carries it.
fn is_set_literal(value: &SExpression) -> bool {
    match value {
        SExpression::String(s) => !s.starts_with('@'),
        SExpression::Number(_) | SExpression::Bool(_) => true,
        SExpression::Array(items) => items
            .first()
            .and_then(SExpression::as_str)
            .and_then(orbital_core_base::language::registry::get_operator)
            .is_some_and(|op| {
                op.failable.is_none() && !op.has_side_effects && !answers_null(&op.return_type)
            }),
        _ => false,
    }
}

fn answers_null(ty: &orbital_core_base::language::registry::OperatorTypeRef) -> bool {
    use orbital_core_base::language::registry::{
        OperatorTypeRef as Ref, OperatorTypeRefKind as Kind,
    };
    match ty {
        Ref::Tagged(Kind::Union { of }) => of.iter().any(answers_null),
        Ref::Tagged(Kind::Literal { value }) => value.is_null(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::phases::validation::ErrorCode;

    fn null_operands(read: &str, other_trait: &str) -> usize {
        let program: orbital_core_base::schema::OrbitalSchema = serde_json::from_str(&format!(
            r#"{{ "name": "App", "version": "1.0.0", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "Doc", "persistence": "persistent", "collection": "docs", "fields": [
                {{ "name": "id", "type": "string" }}, {{ "name": "title", "type": "string" }} ] }},
              "traits": [{{ "name": "T", "linkedEntity": "Doc", "stateMachine": {{
                "states": [{{ "name": "idle", "isInitial": true }}],
                "events": [{{ "key": "SAVE" }}, {{ "key": "SaveFailed", "payloadSchema": [
                  {{ "name": "error", "type": "string" }}, {{ "name": "code", "type": "string" }} ] }}],
                "transitions": [
                  {{ "from": "idle", "to": "idle", "event": "SAVE",
                    "effects": [["persist", "create", "Doc", {{ "title": "x" }}, {{ "emit": {{ "failure": "SaveFailed" }} }}]] }},
                  {{ "from": "idle", "to": "idle", "event": "SaveFailed",
                    "effects": [["set", "@entity.title", ["str/concat", "failed: ", "@payload.{read}"]]] }}
                ] }} }}{other_trait}],
              "pages": [{{ "name": "P", "path": "/p", "traits": [{{ "ref": "T" }}] }}] }}] }}"#
        ))
        .expect("program");
        crate::phases::validation::validate_comprehensive_without_services(&program)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorNullOperand)
            .count()
    }

    fn emitter(payload: &str) -> String {
        format!(
            r#", {{ "name": "U", "linkedEntity": "Doc", "stateMachine": {{
              "states": [{{ "name": "idle", "isInitial": true }}],
              "events": [{{ "key": "GO" }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "GO", "effects": [["emit", "SaveFailed", {payload}]] }}] }} }}"#
        )
    }

    #[test]
    fn a_field_every_failure_producer_guarantees_is_present() {
        assert_eq!(null_operands("error", ""), 0);
    }

    #[test]
    fn control_a_field_the_failure_contract_leaves_optional_stays_optional() {
        assert_eq!(null_operands("code", ""), 1);
    }

    #[test]
    fn control_an_explicit_emit_without_the_field_keeps_it_optional() {
        assert_eq!(null_operands("error", &emitter(r#"{ "code": "x" }"#)), 1);
    }

    #[test]
    fn an_explicit_emit_that_sets_the_field_keeps_the_guarantee() {
        assert_eq!(
            null_operands("error", &emitter(r#"{ "error": "boom" }"#)),
            0
        );
    }

    #[test]
    fn a_failure_event_published_as_external_keeps_the_guarantee() {
        // std-help-article: `HelpArticleViewCountFailed -> external { error : string }` in emits.
        let published = r#", { "name": "P", "linkedEntity": "Doc",
            "emits": [{ "event": "SaveFailed", "scope": "external", "payloadSchema": [{ "name": "error", "type": "string" }] }],
            "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] } }"#;
        assert_eq!(null_operands("error", published), 0);
    }

    #[test]
    fn an_explicit_emit_of_a_call_that_cannot_answer_null_keeps_the_guarantee() {
        // std-alummaalraqamia's over-cap failure: `(emit OverviewMigrationLoadFailed { error: (str/concat …) })`.
        assert_eq!(
            null_operands(
                "error",
                &emitter(r#"{ "error": ["str/concat", "over the cap: ", "100"] }"#)
            ),
            0
        );
        assert_eq!(
            null_operands(
                "error",
                &emitter(r#"{ "error": ["str/to-number", "100"] }"#)
            ),
            1,
            "control: a failable operator can answer null"
        );
    }

    fn routed(required: bool) -> usize {
        // std-wiki-page: `WikiAttachmentStorage.STORAGE_FAILED -> WikiAttachmentStorageFailed`, std-storage declaring
        // `STORAGE_FAILED -> external { message : string! }`.
        let program: orbital_core_base::schema::OrbitalSchema = serde_json::from_str(&format!(
            r#"{{ "name": "App", "version": "1.0.0", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "Doc", "fields": [ {{ "name": "id", "type": "string" }}, {{ "name": "title", "type": "string" }} ] }},
              "traits": [
                {{ "name": "Store", "linkedEntity": "Doc",
                  "emits": [{{ "event": "STORE_FAILED", "scope": "external", "payloadSchema": [
                    {{ "name": "message", "type": "string", "required": {required} }} ] }}],
                  "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}], "events": [{{ "key": "GO" }}],
                    "transitions": [{{ "from": "idle", "to": "idle", "event": "GO",
                      "effects": [["emit", "STORE_FAILED", {{ "message": "refused" }}]] }}] }} }},
                {{ "name": "Page", "linkedEntity": "Doc",
                  "listens": [{{ "event": "STORE_FAILED", "source": {{ "kind": "trait", "trait": "Store" }}, "triggers": "LocalFailed" }}],
                  "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
                    "events": [{{ "key": "LocalFailed", "payloadSchema": [{{ "name": "message", "type": "string" }}] }}],
                    "transitions": [{{ "from": "idle", "to": "idle", "event": "LocalFailed",
                      "effects": [["set", "@entity.title", ["str/concat", "failed: ", "@payload.message"]]] }}] }} }}
              ],
              "pages": [{{ "name": "P", "path": "/p", "traits": [{{ "ref": "Page" }}, {{ "ref": "Store" }}] }}] }}] }}"#
        ))
        .expect("program");
        crate::phases::validation::validate_comprehensive_without_services(&program)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorNullOperand)
            .count()
    }

    fn forwarded(listen: &str, consumer_effects: &str, required: bool) -> usize {
        let program: orbital_core_base::schema::OrbitalSchema = serde_json::from_str(&format!(
            r#"{{ "name": "App", "version": "1.0.0", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "Doc", "fields": [ {{ "name": "id", "type": "string" }}, {{ "name": "title", "type": "string" }} ] }},
              "traits": [
                {{ "name": "Store", "linkedEntity": "Doc",
                  "emits": [{{ "event": "STORE_FAILED", "scope": "external", "payloadSchema": [
                    {{ "name": "message", "type": "string", "required": {required} }} ] }}],
                  "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}], "events": [{{ "key": "GO" }}],
                    "transitions": [{{ "from": "idle", "to": "idle", "event": "GO",
                      "effects": [["emit", "STORE_FAILED", {{ "message": "refused" }}]] }}] }} }},
                {{ "name": "Page", "linkedEntity": "Doc",
                  "listens": [{listen}],
                  "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
                    "events": [{{ "key": "LocalFailed", "payloadSchema": [{{ "name": "message", "type": "string" }}, {{ "name": "error", "type": "string" }}] }},
                               {{ "key": "LoadFailed", "payloadSchema": [{{ "name": "error", "type": "string" }}] }}],
                    "transitions": [{consumer_effects},
                      {{ "from": "idle", "to": "idle", "event": "LoadFailed",
                        "effects": [["set", "@entity.title", ["str/concat", "failed: ", "@payload.error"]]] }}] }} }}
              ],
              "pages": [{{ "name": "P", "path": "/p", "traits": [{{ "ref": "Page" }}, {{ "ref": "Store" }}] }}] }}] }}"#
        ))
        .expect("program");
        crate::phases::validation::validate_comprehensive_without_services(&program)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::OperatorNullOperand)
            .count()
    }

    #[test]
    fn a_mapped_route_carries_a_field_copied_from_a_required_source_field() {
        // std-service-research-assistant: `BEARER_API_FAILED -> ResearchCustomBearerApiFailed with { error: ?message }`.
        let listen = r#"{ "event": "STORE_FAILED", "source": { "kind": "trait", "trait": "Store" }, "triggers": "LoadFailed", "payloadMapping": { "error": "@payload.message" } }"#;
        let none = r#"{ "from": "idle", "to": "idle", "event": "LocalFailed", "effects": [] }"#;
        assert_eq!(forwarded(listen, none, true), 0);
        assert_eq!(
            forwarded(listen, none, false),
            1,
            "control: the source field may be missing"
        );
    }

    #[test]
    fn an_emit_forwarding_a_guaranteed_payload_field_keeps_the_guarantee() {
        // std-wiki-page: `WikiAttachmentStorageFailed -> (emit WikiAttachmentLoadFailed { error: ?message })`.
        let listen = r#"{ "event": "STORE_FAILED", "source": { "kind": "trait", "trait": "Store" }, "triggers": "LocalFailed" }"#;
        let forward = r#"{ "from": "idle", "to": "idle", "event": "LocalFailed", "effects": [["emit", "LoadFailed", { "error": "@payload.message" }]] }"#;
        assert_eq!(forwarded(listen, forward, true), 0);
        assert_eq!(
            forwarded(listen, forward, false),
            1,
            "control: forwarding a field that may be missing guarantees nothing"
        );
    }

    #[test]
    fn a_routed_event_carries_the_fields_its_emitter_declares_required() {
        assert_eq!(routed(true), 0);
        assert_eq!(
            routed(false),
            1,
            "control: an emitter that leaves the field optional guarantees nothing"
        );
    }

    #[test]
    fn a_route_names_its_emitter_by_source_not_by_a_dotted_event() {
        // The composed program carries `event: "STORE_FAILED"` with `source: { trait: "Store" }`; a listen with no
        // source could come from any trait, so it guarantees nothing.
        let sourced = r#"{ "event": "STORE_FAILED", "source": { "kind": "trait", "trait": "Store" }, "triggers": "LoadFailed", "payloadMapping": { "error": "@payload.message" } }"#;
        let unsourced = r#"{ "event": "STORE_FAILED", "triggers": "LoadFailed", "payloadMapping": { "error": "@payload.message" } }"#;
        let none = r#"{ "from": "idle", "to": "idle", "event": "LocalFailed", "effects": [] }"#;
        assert_eq!(forwarded(sourced, none, true), 0);
        assert_eq!(
            forwarded(unsourced, none, true),
            1,
            "control: an unsourced listen has no known emitter"
        );
    }

    fn consumers(count: usize, optional: bool) -> orbital_core_base::schema::OrbitalSchema {
        let required = if optional { "false" } else { "true" };
        let pages: Vec<String> = (0..count)
            .map(|i| {
                format!(
                    r#"{{ "name": "Page{i}", "linkedEntity": "Doc",
                  "listens": [{{ "event": "STORE_FAILED", "source": {{ "kind": "trait", "trait": "Store" }}, "triggers": "LocalFailed" }}],
                  "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
                    "events": [{{ "key": "LocalFailed", "payloadSchema": [{{ "name": "message", "type": "string", "required": {required} }}] }}],
                    "transitions": [{{ "from": "idle", "to": "idle", "event": "LocalFailed",
                      "effects": [["set", "@entity.title", ["str/concat", "failed: ", "@payload.message"]]] }}] }} }}"#
                )
            })
            .collect();
        serde_json::from_str(&format!(
            r#"{{ "name": "App", "version": "1.0.0", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "Doc", "fields": [ {{ "name": "id", "type": "string" }}, {{ "name": "title", "type": "string" }} ] }},
              "traits": [
                {{ "name": "Store", "linkedEntity": "Doc",
                  "emits": [{{ "event": "STORE_FAILED", "scope": "external", "payloadSchema": [
                    {{ "name": "message", "type": "string", "required": true }} ] }}],
                  "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}], "events": [{{ "key": "GO" }}],
                    "transitions": [{{ "from": "idle", "to": "idle", "event": "GO",
                      "effects": [["emit", "STORE_FAILED", {{ "message": "refused" }}]] }}] }} }},
                {pages}
              ],
              "pages": [{{ "name": "P", "path": "/p", "traits": [{{ "ref": "Store" }}, {{ "ref": "Page0" }}] }}] }}] }}"#,
            pages = pages.join(",\n")
        ))
        .expect("program")
    }

    fn runs_while(f: impl FnOnce()) -> usize {
        super::RUNS.with(|r| r.set(0));
        f();
        super::RUNS.with(|r| r.get())
    }

    #[test]
    fn the_program_wide_analysis_runs_once_per_pass_not_once_per_trait() {
        let shapes = |count: usize| {
            let mut program = consumers(count, true);
            runs_while(|| {
                crate::phases::validation::effect::record_declared_shapes(&mut program)
                    .expect("shapes")
            })
        };
        assert_eq!(shapes(3), 1);
        assert_eq!(shapes(6), 1, "six consuming traits still run it once");
        let validate = |count: usize| {
            let program = consumers(count, true);
            runs_while(|| {
                crate::phases::validation::validate_comprehensive_without_services(&program);
            })
        };
        assert_eq!(
            validate(3),
            validate(6),
            "validation runs it a fixed number of times, whatever the trait count"
        );
    }

    #[test]
    fn control_a_program_without_optional_payload_fields_never_runs_it() {
        let mut program = consumers(6, false);
        assert_eq!(
            runs_while(
                || crate::phases::validation::effect::record_declared_shapes(&mut program)
                    .expect("shapes")
            ),
            0
        );
    }

    #[test]
    fn control_an_explicit_emit_of_a_binding_does_not_guarantee_the_field() {
        assert_eq!(
            null_operands("error", &emitter(r#"{ "error": "@entity.title" }"#)),
            1
        );
    }
}
