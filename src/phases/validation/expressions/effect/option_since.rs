//! `ORB_X_OPERATOR_OPTION_NEWER` (E1): a call that states an operator option the installed operator table introduces
//! in an orb release newer than the one running (`optionSince`). The release is the one the table was admitted
//! against, the version the host passed into discovery.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::language::registry::{get_operator, running_release, EmbeddedOperatorParam};
use orbital_core_base::language_data::release_newer;
use orbital_core_base::schema::SExpression;
use std::collections::BTreeMap;

/// Checks one call form against the installed table; a no-op for an operator that lists no `optionSince`, a call
/// whose argument is not a literal object, or before a table is loaded.
pub(crate) fn check_option_release(items: &[SExpression], path: &str) -> ValidationResult {
    let (Some(SExpression::String(head)), Some(running)) = (items.first(), running_release())
    else {
        return ValidationResult::new();
    };
    let Some(operator) = get_operator(head).filter(|op| !op.option_since.is_empty()) else {
        return ValidationResult::new();
    };
    check_call(
        head,
        &operator.option_since,
        &operator.params,
        items,
        path,
        running,
    )
}

fn check_call(
    head: &str,
    option_since: &BTreeMap<String, String>,
    params: &[EmbeddedOperatorParam],
    items: &[SExpression],
    path: &str,
    running: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (option, since) in option_since {
        if !release_newer(since, running).unwrap_or(false) {
            continue;
        }
        let mut segments = option.split('/');
        let Some(position) = segments
            .next()
            .and_then(|param| params.iter().position(|p| p.name == param))
        else {
            continue;
        };
        let Some(mut value) = items.get(position + 1) else {
            continue;
        };
        let mut stated = true;
        for key in segments {
            match value {
                SExpression::Object(fields) => match fields.get(key) {
                    Some(next) => value = next,
                    None => {
                        stated = false;
                        break;
                    }
                },
                _ => {
                    stated = false;
                    break;
                }
            }
        }
        if stated {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OperatorOptionNewer,
                    path,
                    format!("`({head} …)` states option `{option}`, which exists from orb {since}; this orb is {running}"),
                )
                .with_suggestion(format!("Upgrade orb to {since} or later, or drop the option")),
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::language::registry::EmbeddedOperator;

    fn operator() -> EmbeddedOperator {
        serde_json::from_value(serde_json::json!({
            "category": "std-os", "minArity": 1, "maxArity": 3,
            "params": [
                { "name": "port", "type": { "kind": "object", "fields": { "anchor": "string" }, "open": true } },
                { "name": "options", "type": { "kind": "object", "fields": { "key": "string", "emit": { "kind": "object", "fields": { "onMessage": "string" } }, "old": "boolean" } } }
            ],
            "returnType": "void",
            "optionSince": { "options/key": "19.0.0", "options/emit/onMessage": "19.1.0", "port/anchor": "19.0.0" }
        }))
        .unwrap()
    }

    fn call(json: &str) -> Vec<SExpression> {
        serde_json::from_str(json).unwrap()
    }

    fn findings(json: &str, running: &str) -> Vec<String> {
        let op = operator();
        check_call(
            "os/x",
            &op.option_since,
            &op.params,
            &call(json),
            "t",
            running,
        )
        .errors
        .iter()
        .map(|e| e.message.clone())
        .collect()
    }

    #[test]
    fn an_option_from_a_newer_release_is_refused_naming_both_releases() {
        let found = findings(r#"["os/x", {}, {"key": "k"}]"#, "18.0.5");
        assert_eq!(found.len(), 1);
        assert!(
            found[0].contains("`options/key`")
                && found[0].contains("from orb 19.0.0")
                && found[0].contains("this orb is 18.0.5"),
            "{found:?}"
        );
    }

    #[test]
    fn a_nested_option_and_the_first_parameter_are_found_by_path() {
        assert_eq!(
            findings(
                r#"["os/x", {"anchor": "env"}, {"emit": {"onMessage": "E"}}]"#,
                "19.0.0"
            )
            .len(),
            1
        );
        assert_eq!(
            findings(r#"["os/x", {"anchor": "env"}]"#, "18.0.5").len(),
            1
        );
    }

    #[test]
    fn control_the_same_call_passes_on_the_release_that_introduced_the_option_or_a_later_one() {
        assert!(findings(r#"["os/x", {}, {"key": "k"}]"#, "19.0.0").is_empty());
        assert!(findings(r#"["os/x", {}, {"key": "k"}]"#, "19.4.2").is_empty());
    }

    #[test]
    fn control_an_option_not_stated_or_not_listed_is_not_refused() {
        assert!(findings(r#"["os/x", {}, {"old": true}]"#, "18.0.5").is_empty());
        assert!(findings(r#"["os/x", {}]"#, "18.0.5").is_empty());
        assert!(findings(r#"["os/x", {}, {"emit": {}}]"#, "18.0.5").is_empty());
    }

    #[test]
    fn a_dynamic_options_argument_is_not_a_literal_object_so_it_is_not_judged() {
        assert!(findings(r#"["os/x", {}, "@opts"]"#, "18.0.5").is_empty());
    }
}
