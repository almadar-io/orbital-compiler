//! `ORB_FETCH_DELIVER_IDS_*` and `ORB_FETCH_TOTAL_NONE_READ`: the authored `deliver: ids` and
//! `total: none` options of `fetch` (Almadar_Embedded_Kernel_Speed_Design.md section 5.2).
//!
//! `deliver: ids` makes the success event's `data` a row-set reference, not rows. v1 allows
//! exactly one use of it: passed whole as a `render-ui` prop value. Any other read in an arm that
//! handles the success event is an error naming the use. `total: none` skips the count, so the
//! success event's `totalCount` may not be read.

use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::{
    split_event_address, ListenSource, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

const DATA_REF: &str = "@payload.data";
const TOTAL_REF: &str = "@payload.totalCount";

struct FetchOptionsSeen {
    ids_events: HashSet<String>,
    no_total_events: HashSet<String>,
    ids_with_include: Vec<usize>,
    /// Transitions holding a `deliver: ids` fetch that declares no success event.
    ids_without_success: Vec<usize>,
}

/// What a received event carries: the rows by reference, no total, or both.
#[derive(Clone, Copy)]
struct Restriction {
    ids: bool,
    no_total: bool,
}

/// Every arm in the program that receives a restricted success event: the fetching trait's own
/// arms, plus the arms of any trait whose `listens` route subscribes to it (the route's
/// `triggers` is the receiving arm). A relay cannot carry the value on, since forwarding it in an
/// `emit` payload is itself a rejected use.
pub(crate) fn validate_fetch_deliver(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut traits: Vec<(String, &str, &TraitDefinition)> = Vec::new();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(td) = trait_ref {
                let path = super::trait_path(i, &orbital.name, j, trait_ref);
                traits.push((path, orbital.name.as_str(), td));
            }
        }
    }
    let mut receivers: HashMap<(usize, String), Restriction> = HashMap::new();
    for (index, (path, _, td)) in traits.iter().enumerate() {
        let mut seen = FetchOptionsSeen {
            ids_events: HashSet::new(),
            no_total_events: HashSet::new(),
            ids_with_include: Vec::new(),
            ids_without_success: Vec::new(),
        };
        for (i, transition) in td.state_machine.transitions.iter().enumerate() {
            for effect in transition.effects.iter().flatten() {
                collect_fetches(effect, i, &mut seen);
            }
        }
        for i in &seen.ids_with_include {
            result.add_error(
                ValidationError::error(
                    ErrorCode::FetchDeliverIdsEntityRefs,
                    format!("{path}.stateMachine.transitions[{i}].effects"),
                    "`deliver: ids` with `include`: included relations are row content, and `ids` delivers no rows",
                )
                .with_suggestion("Drop `include`, or drop `deliver: ids`"),
            );
        }
        for i in &seen.ids_without_success {
            result.add_error(
                ValidationError::error(
                    ErrorCode::FetchDeliverIdsNoSuccess,
                    format!("{path}.stateMachine.transitions[{i}].effects"),
                    "`deliver: ids` without `emit.success`: the reference is only usable passed whole to a `render-ui` prop, and no event carries it",
                )
                .with_suggestion("Declare `emit: { success: <Event> }` and pass `@payload.data` whole to a `render-ui` prop in that event's arm, or drop `deliver: ids`"),
            );
        }
        let events: HashSet<&String> = seen
            .ids_events
            .iter()
            .chain(&seen.no_total_events)
            .collect();
        for event in events {
            let restriction = Restriction {
                ids: seen.ids_events.contains(event),
                no_total: seen.no_total_events.contains(event),
            };
            receivers.insert((index, event.clone()), restriction);
            for (listener, (_, _, listener_td)) in traits.iter().enumerate() {
                for listen in listener_td.listens.iter().flatten() {
                    let (bare, address) = split_event_address(&listen.event);
                    if bare != *event
                        || !listen_names(&listen.source.clone().or(address), td, traits[index].1)
                    {
                        continue;
                    }
                    let triggered = listen.triggers.clone().unwrap_or(bare);
                    receivers.insert((listener, triggered), restriction);
                    for expr in listen
                        .guard
                        .iter()
                        .chain(listen.payload_mapping.iter().flat_map(|m| m.values()))
                    {
                        let mut uses = Vec::new();
                        scan(expr, "listens", &mut uses);
                        report(
                            &mut result,
                            &traits[listener].0,
                            &format!("listens `{}`", listen.event),
                            restriction,
                            uses,
                        );
                    }
                }
            }
        }
    }
    let mut ordered: Vec<(&(usize, String), &Restriction)> = receivers.iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(b.0));
    for ((index, event), restriction) in ordered {
        let (path, _, td) = &traits[*index];
        for (i, transition) in td.state_machine.transitions.iter().enumerate() {
            if &transition.event != event {
                continue;
            }
            let mut uses: Vec<(String, String, bool)> = Vec::new();
            if let Some(guard) = &transition.guard {
                scan(guard, "guard", &mut uses);
            }
            for (j, effect) in transition.effects.iter().flatten().enumerate() {
                scan(effect, &format!("effects[{j}]"), &mut uses);
            }
            let base = format!("{path}.stateMachine.transitions[{i}]");
            report(
                &mut result,
                &base,
                &format!("`{event}`"),
                *restriction,
                uses,
            );
        }
    }
    result
}

/// True when `source` names the fetching trait `fetcher` (in `fetcher_orbital`).
fn listen_names(
    source: &Option<ListenSource>,
    fetcher: &TraitDefinition,
    fetcher_orbital: &str,
) -> bool {
    match source {
        Some(ListenSource::Any) => true,
        Some(ListenSource::Trait { trait_name, .. }) => *trait_name == fetcher.name,
        Some(ListenSource::Orbital {
            orbital,
            trait_name,
            ..
        }) => *trait_name == fetcher.name && orbital == fetcher_orbital,
        None => false,
    }
}

fn report(
    result: &mut ValidationResult,
    base: &str,
    arm: &str,
    restriction: Restriction,
    uses: Vec<(String, String, bool)>,
) {
    for (where_, what, is_total) in uses {
        let (code, message) = if is_total {
            if !restriction.no_total {
                continue;
            }
            (
                ErrorCode::FetchTotalNoneRead,
                format!("{arm} reads `{TOTAL_REF}` ({what}), but the fetch that produces it declares `total: none`"),
            )
        } else {
            if !restriction.ids {
                continue;
            }
            (
                ErrorCode::FetchDeliverIdsRowsRead,
                format!("{arm} reads `{DATA_REF}` ({what}), but the fetch that produces it declares `deliver: ids`; the reference may only be passed whole as a `render-ui` prop value"),
            )
        };
        result.add_error(
            ValidationError::error(code, format!("{base}.{where_}"), message).with_suggestion(
                "Pass it whole, e.g. `entity: ?data`, or drop the option from the fetch",
            ),
        );
    }
}

fn collect_fetches(expr: &SExpression, transition: usize, seen: &mut FetchOptionsSeen) {
    let SExpression::Array(items) = expr else {
        return;
    };
    if matches!(items.first(), Some(SExpression::String(op)) if op == "fetch") {
        for arg in items.iter().skip(2) {
            let SExpression::Object(options) = arg else {
                continue;
            };
            let success = match options.get("emit") {
                Some(SExpression::Object(emit)) => match emit.get("success") {
                    Some(SExpression::String(s)) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                },
                _ => None,
            };
            let is = |key: &str, value: &str| matches!(options.get(key), Some(SExpression::String(s)) if s == value);
            if is("deliver", "ids") {
                if options.contains_key("include") {
                    seen.ids_with_include.push(transition);
                }
                match &success {
                    Some(event) => {
                        seen.ids_events.insert(event.clone());
                    }
                    None => seen.ids_without_success.push(transition),
                }
            }
            if is("total", "none") {
                if let Some(event) = &success {
                    seen.no_total_events.insert(event.clone());
                }
            }
        }
    }
    for item in items {
        collect_fetches(item, transition, seen);
    }
}

/// Every reference to the delivered values, as `(path, use, is_total)`. A `render-ui` prop whose
/// whole value is `@payload.data` is the one allowed use and is skipped.
fn scan(expr: &SExpression, at: &str, uses: &mut Vec<(String, String, bool)>) {
    match expr {
        SExpression::String(s) => {
            if let Some(kind) = reference_kind(s) {
                uses.push((at.to_string(), describe(s, at), kind));
            }
        }
        SExpression::Array(items) => {
            let head = match items.first() {
                Some(SExpression::String(op)) => Some(op.as_str()),
                _ => None,
            };
            if head == Some("render-ui") {
                for (k, item) in items.iter().enumerate().skip(1) {
                    match item {
                        SExpression::Object(props) => {
                            for (name, value) in props {
                                let here = format!("{at}[{k}].{name}");
                                if matches!(value, SExpression::String(s) if s == DATA_REF) {
                                    continue;
                                }
                                scan(value, &here, uses);
                            }
                        }
                        other => scan(other, &format!("{at}[{k}]"), uses),
                    }
                }
                return;
            }
            for (k, item) in items.iter().enumerate() {
                if k == 0 && head.is_some() {
                    continue;
                }
                let here = match head {
                    Some(op) => format!("{at}.{op}[{k}]"),
                    None => format!("{at}[{k}]"),
                };
                scan(item, &here, uses);
            }
        }
        SExpression::Object(map) => {
            for (name, value) in map {
                scan(value, &format!("{at}.{name}"), uses);
            }
        }
        _ => {}
    }
}

/// `Some(false)` for a `@payload.data` reference, `Some(true)` for `@payload.totalCount`.
fn reference_kind(s: &str) -> Option<bool> {
    if mentions(s, DATA_REF) {
        Some(false)
    } else if mentions(s, TOTAL_REF) {
        Some(true)
    } else {
        None
    }
}

fn mentions(s: &str, reference: &str) -> bool {
    s.match_indices(reference).any(|(i, _)| {
        !s[i + reference.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

fn describe(s: &str, at: &str) -> String {
    if s == DATA_REF || s == TOTAL_REF {
        format!("used as a value at `{at}`")
    } else if s.starts_with(DATA_REF) || s.starts_with(TOTAL_REF) {
        format!("field or index access `{s}` at `{at}`")
    } else {
        format!("interpolated into `{s}` at `{at}`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_with(fetch_options: &str, arm_effects: &str, guard: Option<&str>) -> OrbitalSchema {
        let guard = guard.map_or(String::new(), |g| format!(r#", "guard": {g}"#));
        schema_json(&format!(
            r#"{{ "name": "T", "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [
                {{ "from": "idle", "to": "idle", "event": "INIT",
                   "effects": [["fetch", "Item", {{ {fetch_options}, "emit": {{ "success": "Loaded" }} }}]] }},
                {{ "from": "idle", "to": "idle", "event": "Loaded"{guard}, "effects": [{arm_effects}] }}
              ] }} }}"#
        ))
    }

    fn schema_json(trait_json: &str) -> OrbitalSchema {
        schema_of(&[trait_json])
    }

    fn schema_of(traits: &[&str]) -> OrbitalSchema {
        serde_json::from_str(&format!(
            r#"{{ "name": "S", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "Item", "persistence": "runtime", "fields": [{{ "name": "id", "type": "string" }}] }},
              "traits": [{}] }}] }}"#,
            traits.join(", ")
        ))
        .expect("schema json")
    }

    fn listener(source: &str, arm_effects: &str) -> String {
        format!(
            r#"{{ "name": "L", "listens": [{{ "event": "Loaded", "triggers": "GOT", "source": {source} }}],
              "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "GOT", "effects": [{arm_effects}] }}] }} }}"#
        )
    }

    fn fetcher(options: &str) -> String {
        format!(
            r#"{{ "name": "F", "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["fetch", "Item", {{ {options}, "emit": {{ "success": "Loaded" }} }}]] }}] }} }}"#
        )
    }

    fn codes(s: &OrbitalSchema) -> Vec<ErrorCode> {
        validate_fetch_deliver(s)
            .errors
            .iter()
            .map(|e| e.code.clone())
            .collect()
    }

    fn trait_with(o: &str, a: &str, g: Option<&str>) -> OrbitalSchema {
        schema_with(o, a, g)
    }

    const IDS: &str = r#""deliver": "ids""#;
    const F_SRC: &str = r#"{ "kind": "trait", "trait": "F" }"#;

    #[test]
    fn a_cross_trait_listener_reading_a_row_is_an_error() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(F_SRC, r#"["set", "@entity.n", "@payload.data.0.name"]"#),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn a_wildcard_listener_reading_a_row_is_an_error() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(
                r#"{ "kind": "any" }"#,
                r#"["set", "@entity.n", "@payload.data.0.name"]"#,
            ),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn a_relay_emitting_the_reference_is_an_error() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(F_SRC, r#"["emit", "Again", { "rows": "@payload.data" }]"#),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn a_cross_trait_listener_passing_it_whole_to_render_ui_is_accepted() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(
                F_SRC,
                r#"["render-ui", "main", { "entity": "@payload.data" }]"#,
            ),
        ]);
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn control_a_listener_of_another_trait_is_not_restricted() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(
                r#"{ "kind": "trait", "trait": "Other" }"#,
                r#"["set", "@entity.n", "@payload.data.0.name"]"#,
            ),
        ]);
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn control_a_cross_trait_listener_of_a_rows_fetch_may_read_rows() {
        let s = schema_of(&[
            &fetcher(r#""limit": 5"#),
            &listener(F_SRC, r#"["set", "@entity.n", "@payload.data.0.name"]"#),
        ]);
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn a_cross_trait_listener_reading_the_total_of_a_total_none_fetch_is_an_error() {
        let s = schema_of(&[
            &fetcher(r#""total": "none""#),
            &listener(F_SRC, r#"["set", "@entity.n", "@payload.totalCount"]"#),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchTotalNoneRead]);
    }

    fn bare_fetch(options: &str) -> OrbitalSchema {
        schema_json(&format!(
            r#"{{ "name": "T", "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["fetch", "Item", {options}]] }}] }} }}"#
        ))
    }

    #[test]
    fn ids_without_a_success_event_is_an_error() {
        assert_eq!(
            codes(&bare_fetch(r#"{ "deliver": "ids" }"#)),
            vec![ErrorCode::FetchDeliverIdsNoSuccess]
        );
        // a failure-only emit still carries no result
        assert_eq!(
            codes(&bare_fetch(r#"{ "deliver": "ids", "emit": { "failure": "Bad" } }"#)),
            vec![ErrorCode::FetchDeliverIdsNoSuccess]
        );
    }

    #[test]
    fn control_a_rows_fetch_without_a_success_event_and_an_ids_fetch_with_one_validate() {
        assert!(codes(&bare_fetch(r#"{ "limit": 5 }"#)).is_empty());
        assert!(codes(&bare_fetch(r#"{ "deliver": "rows" }"#)).is_empty());
        assert!(codes(&bare_fetch(r#"{ "deliver": "ids", "emit": { "success": "Loaded" } }"#)).is_empty());
    }

    #[test]
    fn invalid_option_values_are_errors_and_valid_ones_pass() {
        use crate::phases::validation::effect::validate_fetch;
        use std::collections::{HashMap, HashSet};
        let run = |options: serde_json::Value| {
            let arr: Vec<SExpression> =
                serde_json::from_value(serde_json::json!(["fetch", "Item", options])).unwrap();
            let known: HashSet<String> = ["Item".to_string()].into();
            validate_fetch(&arr, "p", &known, &HashMap::new(), None)
                .errors
                .iter()
                .filter(|e| e.code == ErrorCode::EffectFetchInvalidOption)
                .count()
        };
        assert_eq!(run(serde_json::json!({ "deliver": "bytes" })), 1);
        assert_eq!(run(serde_json::json!({ "total": "approx" })), 1);
        assert_eq!(run(serde_json::json!({ "deliver": 1, "total": "x" })), 2);
        assert_eq!(
            run(serde_json::json!({ "deliver": "ids", "total": "none" })),
            0
        );
        assert_eq!(
            run(serde_json::json!({ "deliver": "rows", "total": "exact" })),
            0
        );
    }

    #[test]
    fn ids_passed_whole_as_a_render_ui_prop_is_accepted() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", { "type": "entity-table", "entity": "@payload.data" }]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn ids_whole_inside_a_when_wrapped_render_ui_is_accepted() {
        let t = trait_with(
            IDS,
            r#"["when", "@config.on", ["render-ui", "main", { "entity": "@payload.data" }]]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn ids_field_access_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", { "title": "@payload.data.title" }]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_array_operator_argument_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", { "n": ["array/len", "@payload.data"] }]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_set_into_the_entity_is_an_error() {
        let t = trait_with(IDS, r#"["set", "@entity.rows", "@payload.data"]"#, None);
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_in_an_emit_payload_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["emit", "Done", { "rows": "@payload.data" }]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_in_a_guard_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", null]"#,
            Some(r#"[">", ["array/len", "@payload.data"], 0]"#),
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn control_rows_delivery_may_read_the_rows_anywhere() {
        let t = trait_with(
            r#""deliver": "rows""#,
            r#"["set", "@entity.rows", "@payload.data"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
        let t = trait_with(
            r#""limit": 20"#,
            r#"["set", "@entity.rows", "@payload.data"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn a_sibling_payload_field_is_not_the_data_reference() {
        let t = trait_with(IDS, r#"["set", "@entity.x", "@payload.dataset"]"#, None);
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn ids_with_include_is_an_error() {
        let t = trait_with(
            r#""deliver": "ids", "include": ["owner"]"#,
            r#"["render-ui", "main", null]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsEntityRefs]);
    }

    #[test]
    fn total_none_reading_the_total_is_an_error() {
        let t = trait_with(
            r#""total": "none""#,
            r#"["set", "@entity.n", "@payload.totalCount"]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchTotalNoneRead]);
    }

    #[test]
    fn control_total_none_without_reading_the_total_passes() {
        let t = trait_with(
            r#""total": "none""#,
            r#"["set", "@entity.rows", "@payload.data"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn control_exact_total_may_be_read() {
        let t = trait_with(
            r#""total": "exact""#,
            r#"["set", "@entity.n", "@payload.totalCount"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }
}
