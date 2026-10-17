//! Inbound deliveries: the app's `inbound: [...]` names and the traits that handle them
//! (`listens { EVENT -> inbound "name" answers ANSWER { … } }`). A handler reachable in a program
//! whose app does not declare its name is `ORB_INBOUND_UNDECLARED`, as an `http` origin outside
//! `egress` is `ORB_HTTP_EGRESS_UNDECLARED`.

use super::super::{inline_traits, ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::OrbitalSchema;
use std::collections::HashSet;

fn is_hook_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_credential_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn validate_inbound(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let declared: HashSet<&str> = schema.inbound.iter().flatten().map(String::as_str).collect();
    let mut seen_entries: HashSet<&str> = HashSet::new();
    for (i, entry) in schema.inbound.iter().flatten().enumerate() {
        if !is_hook_name(entry) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::InboundEntryInvalid,
                    format!("inbound[{i}]"),
                    format!("inbound entry '{entry}' is not a hook name"),
                )
                .with_suggestion("Write the one path segment after /api/hooks/: lower-case letters, digits and '-'"),
            );
        } else if !seen_entries.insert(entry.as_str()) {
            result.add_error(ValidationError::error(
                ErrorCode::InboundEntryInvalid,
                format!("inbound[{i}]"),
                format!("inbound entry '{entry}' is listed twice"),
            ));
        }
    }
    let mut handled: HashSet<&str> = HashSet::new();
    for (trait_def, path) in inline_traits(schema) {
        let emitted: HashSet<&str> = trait_def.emits.iter().flatten().map(|e| e.event.as_str()).collect();
        for (i, event) in trait_def.state_machine.events.iter().flatten().enumerate() {
            let Some(inbound) = &event.inbound else { continue };
            let at = format!("{path}.stateMachine.events[{i}]");
            if !declared.contains(inbound.name.as_str()) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::InboundUndeclared,
                        &at,
                        format!(
                            "event '{}' handles the inbound hook '{}', which the program's `inbound` does not declare",
                            event.key, inbound.name
                        ),
                    )
                    .with_suggestion("Add the hook name to the app's `inbound: [...]`"),
                );
            }
            if !handled.insert(inbound.name.as_str()) {
                result.add_error(ValidationError::error(
                    ErrorCode::InboundDuplicate,
                    &at,
                    format!("the inbound hook '{}' has more than one handler", inbound.name),
                ));
            }
            if let Some(v) = &inbound.verifies {
                if !is_hook_name(&v.header) || !is_credential_name(&v.credential) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::InboundVerifyInvalid,
                            &at,
                            format!(
                                "inbound hook '{}' verifies header '{}' against credential '{}', which are not a lower-case header name and a credential name",
                                inbound.name, v.header, v.credential
                            ),
                        )
                        .with_suggestion("Write `verifies \"x-signature-token\" \"PROVIDER_TOKEN\"`: a lower-case header, then a credential name of letters, digits and '_'"),
                    );
                }
            }
            if !emitted.contains(inbound.answers.as_str()) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::InboundAnswerUndeclared,
                        &at,
                        format!(
                            "event '{}' answers with '{}', which the trait does not declare in `emits`",
                            event.key, inbound.answers
                        ),
                    )
                    .with_suggestion("Declare the answer event in the trait's `emits { … }` with a `status : number!` and a `body : string!` payload"),
                );
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(inbound: serde_json::Value, handlers: &[(&str, &str)], emits: &[&str]) -> OrbitalSchema {
        let events: Vec<_> = handlers
            .iter()
            .enumerate()
            .map(|(i, (name, answers))| json!({ "key": format!("PUSH_{i}"), "inbound": { "name": name, "answers": answers } }))
            .collect();
        let emits: Vec<_> = emits.iter().map(|e| json!({ "event": e })).collect();
        serde_json::from_value(json!({
            "name": "App",
            "inbound": inbound,
            "orbitals": [{
                "name": "CalOrbital",
                "entity": { "name": "Req", "collection": "reqs", "persistence": "runtime",
                            "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{
                    "name": "Inbound",
                    "linkedEntity": "Req",
                    "emits": emits,
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": events,
                        "transitions": []
                    }
                }],
                "pages": []
            }]
        }))
        .expect("fixture deserializes")
    }

    fn codes(schema: &OrbitalSchema) -> Vec<ErrorCode> {
        validate_inbound(schema).errors.iter().map(|e| e.code).collect()
    }

    #[test]
    fn a_declared_handler_with_a_declared_answer_is_clean() {
        assert!(codes(&schema(
            json!(["google-calendar"]),
            &[("google-calendar", "DONE")],
            &["DONE"]
        ))
        .is_empty());
    }

    #[test]
    fn a_handler_the_app_does_not_declare_is_undeclared() {
        assert_eq!(
            codes(&schema(json!([]), &[("google-calendar", "DONE")], &["DONE"])),
            vec![ErrorCode::InboundUndeclared]
        );
        let absent: OrbitalSchema = {
            let mut s = schema(json!([]), &[("google-calendar", "DONE")], &["DONE"]);
            s.inbound = None;
            s
        };
        assert_eq!(codes(&absent), vec![ErrorCode::InboundUndeclared]);
    }

    #[test]
    fn an_app_that_declares_the_name_but_has_no_handler_is_clean() {
        assert!(codes(&schema(json!(["google-calendar"]), &[], &[])).is_empty());
    }

    #[test]
    fn two_handlers_of_one_name_are_a_duplicate() {
        let c = codes(&schema(json!(["a"]), &[("a", "DONE"), ("a", "DONE")], &["DONE"]));
        assert_eq!(c, vec![ErrorCode::InboundDuplicate]);
    }

    #[test]
    fn an_answer_the_trait_does_not_emit_is_undeclared() {
        assert_eq!(
            codes(&schema(json!(["a"]), &[("a", "DONE")], &["OTHER"])),
            vec![ErrorCode::InboundAnswerUndeclared]
        );
    }

    #[test]
    fn entries_must_be_one_lower_case_path_segment_and_unique() {
        for bad in ["", "Google", "a/b", "-a", "a b", "a_b"] {
            assert_eq!(
                codes(&schema(json!([bad]), &[], &[])),
                vec![ErrorCode::InboundEntryInvalid],
                "{bad:?}"
            );
        }
        assert_eq!(
            codes(&schema(json!(["a", "a"]), &[], &[])),
            vec![ErrorCode::InboundEntryInvalid]
        );
        assert!(codes(&schema(json!(["a-1", "0b"]), &[], &[])).is_empty());
    }

    fn verified_fixture(header: &str, credential: &str) -> OrbitalSchema {
        let mut s = schema(json!(["a"]), &[("a", "DONE")], &["DONE"]);
        let orbital_core_base::schema::TraitReference::Inline(td) = &mut s.orbitals[0].traits[0] else {
            unreachable!("fixture trait is inline")
        };
        let ev = td
            .state_machine
            .events
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e.inbound.is_some())
            .unwrap();
        ev.inbound.as_mut().unwrap().verifies = Some(orbital_core_base::schema::InboundVerification {
            header: header.to_string(),
            credential: credential.to_string(),
        });
        s
    }

    #[test]
    fn a_verifies_clause_needs_a_lower_case_header_and_a_credential_name() {
        assert!(codes(&verified_fixture(
            "x-goog-channel-token",
            "GOOGLE_CALENDAR_CHANNEL_TOKEN"
        ))
        .is_empty());
        for (header, credential) in [
            ("X-Token", "A"),
            ("", "A"),
            ("x token", "A"),
            ("x-token", ""),
            ("x-token", "A-B"),
            ("x-token", "A B"),
        ] {
            assert_eq!(
                codes(&verified_fixture(header, credential)),
                vec![ErrorCode::InboundVerifyInvalid],
                "{header:?} {credential:?}"
            );
        }
    }
}
