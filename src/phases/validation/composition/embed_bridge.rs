//! The events a behavior value embedded with a declared bridge
//! (`(behavior/apply v { bridge: { listens, emits } })`, both literal event lists) delivers
//! into the embedded program. The bridge checks themselves are the std `embed-bridge` lint.

use std::collections::HashSet;

use orbital_core_base::schema::SExpression;

fn literal_events(value: Option<&SExpression>) -> Option<Vec<String>> {
    match value {
        Some(SExpression::Array(items)) => items
            .iter()
            .map(|item| match item {
                SExpression::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn collect_bridged_listens(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::Array(arr) => {
            if let [SExpression::String(op), _, SExpression::Object(overrides)] = arr.as_slice() {
                if op == "behavior/apply" {
                    if let Some(SExpression::Object(bridge)) = overrides.get("bridge") {
                        if let (Some(listens), Some(_)) =
                            (literal_events(bridge.get("listens")), literal_events(bridge.get("emits")))
                        {
                            out.extend(listens);
                        }
                    }
                }
            }
            for item in arr {
                collect_bridged_listens(item, out);
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                collect_bridged_listens(value, out);
            }
        }
        _ => {}
    }
}

/// Every event a bridged embed in these traits listens for: delivered into the embed, so a
/// control firing one is handled (`ORB_EMIT_CONTROL_UNHANDLED`).
pub(crate) fn bridged_listens<'a>(
    traits: impl IntoIterator<Item = &'a orbital_core_base::schema::TraitDefinition>,
) -> HashSet<String> {
    let mut listens = HashSet::new();
    for td in traits {
        for transition in &td.state_machine.transitions {
            for effect in transition.effects.iter().flatten() {
                collect_bridged_listens(effect, &mut listens);
            }
        }
    }
    listens
}
