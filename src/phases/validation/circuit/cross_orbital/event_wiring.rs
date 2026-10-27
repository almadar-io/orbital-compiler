use std::collections::HashSet;

/// Every value of the `emit` map (not `failure` alone, and not success/failure by name) of every
/// `fetch`/`persist` effect in one transition's effect list, recursing through `if`/`do`: the
/// whole async-result contract a request can produce. `call-service` is out of scope.
pub(crate) fn collect_async_result_events(
    effects: &[orbital_core_base::schema::SExpression],
) -> HashSet<String> {
    use orbital_core_base::schema::SExpression;
    fn walk(expr: &SExpression, out: &mut HashSet<String>) {
        let SExpression::Array(items) = expr else {
            return;
        };
        if let Some(SExpression::String(op)) = items.first() {
            if op == "fetch" || op == "persist" {
                for item in items {
                    if let SExpression::Object(map) = item {
                        if let Some(SExpression::Object(emit_map)) = map.get("emit") {
                            for v in emit_map.values() {
                                if let SExpression::String(ev) = v {
                                    if !ev.is_empty() {
                                        out.insert(ev.clone().into_inner());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for item in items {
            walk(item, out);
        }
    }
    let mut out = HashSet::new();
    for effect in effects {
        walk(effect, &mut out);
    }
    out
}

#[cfg(test)]
mod async_result_collector_tests {
    use super::*;
    use orbital_core_base::schema::SExpression;

    fn effect_with_emit(op: &str, emit: &[(&str, &str)]) -> SExpression {
        let mut emit_map = orbital_core_base::schema::SObject::new();
        for (k, v) in emit {
            emit_map.insert((*k).to_string(), SExpression::String(((*v).to_string()).into()));
        }
        let mut options = orbital_core_base::schema::SObject::new();
        options.insert("emit".to_string(), SExpression::Object(emit_map));
        SExpression::Array(vec![
            SExpression::String(op.to_string().into()),
            SExpression::String(("Item".to_string()).into()),
            SExpression::Object(options),
        ])
    }

    #[test]
    fn async_result_events_credits_every_emit_key_for_fetch_and_persist() {
        let effects = vec![
            effect_with_emit("fetch", &[("success", "Loaded"), ("failure", "LoadFailed")]),
            effect_with_emit("persist", &[("success", "Saved")]),
        ];
        let got = collect_async_result_events(&effects);
        assert_eq!(
            got,
            HashSet::from([
                "Loaded".to_string(),
                "LoadFailed".to_string(),
                "Saved".to_string()
            ])
        );
    }

    /// `call-service` is deliberately OUT of scope; a `fetch` nested inside
    /// `if` is still credited.
    #[test]
    fn async_result_events_excludes_call_service_and_descends_through_if() {
        let call_service = effect_with_emit("call-service", &[("success", "ShouldNotCount")]);
        let wrapped_fetch = SExpression::Array(vec![
            SExpression::String(("if".to_string()).into()),
            SExpression::Bool(true),
            SExpression::Array(vec![effect_with_emit("fetch", &[("success", "Nested")])]),
            SExpression::Array(vec![]),
        ]);
        let got = collect_async_result_events(&[call_service, wrapped_fetch]);
        assert_eq!(got, HashSet::from(["Nested".to_string()]));
    }
}
