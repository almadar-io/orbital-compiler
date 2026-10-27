//! Effect validation helpers: payload extraction + identifier checks.

use crate::phases::validation::sexpr::EFFECT_OPERATORS;
use orbital_core_base::schema::SExpression;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Operators valid in EFFECT POSITION beyond the core `EFFECT_OPERATORS`
/// (control-flow wrappers that *contain* effects, array mutation, OS / ML /
/// agent ops). The core effects are NOT relisted here — `effect_operators()`
/// derives the full set from the single `sexpr::operators::EFFECT_OPERATORS`
/// source, so a new core effect can't drift between the two lists (F3).
const EFFECT_POSITION_EXTRA: &[&str] = &[
    // Conditional and array mutation effects
    "if",            // Conditional effect execution
    "array/append",  // Append item to array field
    "array/prepend", // Prepend item to array field
    "array/remove",  // Remove item from array field
    "increment",     // Increment numeric field
    "decrement",     // Decrement numeric field
    // Functional array operators (registered in almadar-operators/operators.json)
    "array/filter", // Filter elements by predicate
    "array/map",    // Transform elements
    "array/reduce", // Reduce array to single value
    // GAP-V002: Control-flow wrappers that can contain effects
    "do",   // Sequential effect execution
    "let",  // Local variable binding in effects
    "when", // Conditional effect (like if but for effects)
    "fn",   // Lambda wrapping effects
    // Legacy trigger: not in the `os/` family (no grant, no `host: 'os'`)
    "os/watch-http",
    // Resource operators (ref/deref/swap/watch/atomic)
    "ref",    // Reactive entity subscription
    "deref",  // Snapshot read
    "swap",   // Atomic CAS read-modify-write
    "watch",  // Entity change callback
    "atomic", // Transaction group
    // Agent operators (Phase 1: agent/* namespace)
    "agent/memorize",
    "agent/forget",
    "agent/pin",
    "agent/reinforce",
    "agent/decay",
    "agent/generate",
    "agent/switch-provider",
    "agent/invoke",
    "agent/compact",
    "agent/fork",
    "agent/label",
    "agent/search-code",
    // Agent pure operators (valid in effects too, return values)
    "agent/recall",
    "agent/memories",
    "agent/memory-strength",
    "agent/is-pinned",
    "agent/provider",
    "agent/model",
    "agent/tools",
    "agent/token-count",
    "agent/context-usage",
    "agent/session-id",
    // Substrate: llm (value-position, valid in effects)
    "llm/generate",
    "llm/call-tools",
    "llm/switch",
    "llm/compact",
    "llm/embed",
    "llm/token-count",
    // Substrate: workspace (value-position, valid in effects)
    "workspace/read-orbital",
    "workspace/write-orbital",
    "workspace/read-file",
    "workspace/write-file",
    "workspace/read-plan",
    "workspace/write-plan",
    "workspace/read-schema",
    "workspace/write-schema",
    "workspace/exists",
    "workspace/list-orbitals",
    "workspace/archive-orbital",
    // Substrate: session (value-position, valid in effects)
    "session/read-history",
    "session/append-history",
    "session/read-spec",
    "session/write-spec",
    "session/read-plan",
    "session/write-plan",
    "session/read-memory",
    "session/write-memory",
    "session/read-errors",
    "session/write-errors",
    "session/read-analysis",
    // Substrate: memory (value-position, valid in effects)
    "memory/store",
    "memory/recall",
    "memory/list",
    // Substrate: trace (value-position, valid in effects)
    "trace/log",
    "trace/emit",
];

/// All operators valid in effect position: the core effects (the single
/// `sexpr::operators::EFFECT_OPERATORS` source) plus the wrappers above.
/// Replaces the former second `EFFECT_OPERATORS` const that re-listed the core
/// effects verbatim (F3 — drift-proof: add a core effect in one place).
pub fn effect_operators() -> &'static [&'static str] {
    static ALL: OnceLock<Vec<&'static str>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = EFFECT_OPERATORS.to_vec();
        v.extend_from_slice(EFFECT_POSITION_EXTRA);
        v.extend(orbital_core_base::language::registry::program_host_effects());
        v.extend(orbital_core_base::language::registry::os_host_effects());
        v
    })
}

/// Extract payload field names from a navigate effect
pub(crate) fn extract_payload_fields(arr: &[SExpression]) -> HashSet<String> {
    let mut fields = HashSet::new();

    // Payload is typically the third argument: ["navigate", "/path", { ... }]
    if arr.len() >= 3 {
        if let SExpression::Object(payload) = &arr[2] {
            for key in payload.keys() {
                fields.insert(key.clone());
            }
        }
    }

    fields
}

/// Check if a string is UPPER_SNAKE_CASE
#[allow(dead_code)] // exercised by #[cfg(test)] only
pub(crate) fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
        && !s.ends_with('_')
}

/// Accept a `.lolo`-shaped identifier: starts with a letter, contains only
/// letters / digits / underscores. Used by the emit validator post-Phase 2.5
/// since event names are now nominal identifiers in the type system (declared
/// as `type X = Event<T>`) rather than string literals tagged by convention.
/// Both UPPER_SNAKE_CASE and MixedCase shapes are valid identifiers.
pub(crate) fn is_valid_event_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::super::super::{ErrorCode, PageInitPayload};
    use super::super::navigate::path_matches_pattern;
    use super::super::{validate_effect, DeclaredEmit};
    use super::is_upper_snake_case;
    use orbital_core_base::schema::SExpression;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_valid_set_effect() {
        let effect = SExpression::Array(vec![
            SExpression::String(("set".to_string()).into()),
            SExpression::String(("@entity.title".to_string()).into()),
            SExpression::String(("value".to_string()).into()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &Default::default(),
            None,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_set_effect_payload_not_allowed() {
        let effect = SExpression::Array(vec![
            SExpression::String(("set".to_string()).into()),
            SExpression::String(("@payload.title".to_string()).into()),
            SExpression::String(("value".to_string()).into()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &Default::default(),
            None,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectSetPayloadNotAllowed));
    }

    #[test]
    fn test_fetch_effect_invalid_entity() {
        let effect = SExpression::Array(vec![
            SExpression::String(("fetch".to_string()).into()),
            SExpression::String(("NonExistent".to_string()).into()),
        ]);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_effect(
            &effect,
            "test",
            &known_entities,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &Default::default(),
            None,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectFetchInvalidEntity));
    }

    /// email.send with the real contract shape: to/subject/body required,
    /// from optional — the I-13 reproducer surface.
    fn email_registry() -> crate::embedded::ServiceRegistryData {
        use crate::embedded::{EmbeddedServiceParam, ServiceRegistryData};
        let param = |name: &str, ty: &str, required: bool| EmbeddedServiceParam {
            name: name.to_string(),
            param_type: ty.to_string(),
            required,
            items: None,
            properties: None,
            map_value: None,
        };
        ServiceRegistryData {
            runtimes: HashMap::new(),
            response_shapes: HashMap::new(),
            actions: [(
                "email".to_string(),
                ["send".to_string()].into_iter().collect(),
            )]
            .into_iter()
            .collect(),
            params: [(
                "email".to_string(),
                [(
                    "send".to_string(),
                    vec![
                        param("to", "string", true),
                        param("subject", "string", true),
                        param("body", "string", true),
                        param("from", "string", false),
                    ],
                )]
                .into_iter()
                .collect(),
            )]
            .into_iter()
            .collect(),
        }
    }

    fn run_effect(
        effect: &SExpression,
        registry: &crate::embedded::ServiceRegistryData,
    ) -> super::super::super::ValidationResult {
        validate_effect(
            effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            registry,
            None,
            &HashMap::new(),
            &HashMap::new(),
        )
    }

    fn call_service_with_emit(keys: &[(&str, &str)]) -> SExpression {
        let params = SExpression::Object(
            [
                ("to".to_string(), SExpression::String(("a@b.c".to_string()).into())),
                ("subject".to_string(), SExpression::String(("s".to_string()).into())),
                ("body".to_string(), SExpression::String(("b".to_string()).into())),
            ]
            .into_iter()
            .collect(),
        );
        let emit = SExpression::Object(
            keys.iter()
                .map(|(k, v)| (k.to_string(), SExpression::String(v.to_string().into())))
                .collect(),
        );
        let envelope = SExpression::Object([("emit".to_string(), emit)].into_iter().collect());
        SExpression::Array(vec![
            SExpression::String(("call-service".to_string()).into()),
            SExpression::String(("email".to_string()).into()),
            SExpression::String(("send".to_string()).into()),
            params,
            envelope,
        ])
    }

    #[test]
    fn test_call_service_emit_on_message_is_a_valid_envelope_key() {
        // A running call's live messages (`emit: { onMessage }`), as on fetch-stream.
        let effect = call_service_with_emit(&[("onMessage", "STEP"), ("success", "SENT")]);
        let result = run_effect(&effect, &email_registry());
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
            "onMessage must be accepted on call-service: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_call_service_emit_unknown_key_still_rejected() {
        let effect = call_service_with_emit(&[("onProgress", "STEP"), ("success", "SENT")]);
        let result = run_effect(&effect, &email_registry());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EffectEmitKeyInvalid),
            "an undeclared envelope key stays an error: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_navigate_missing_required_payload() {
        // Set up a page that requires an "id" field on INIT
        let mut page_init_payloads: HashMap<String, PageInitPayload> = HashMap::new();
        page_init_payloads.insert(
            "/inspection/:id".to_string(),
            PageInitPayload {
                required_fields: vec!["id".to_string()],
                all_fields: vec!["id".to_string()],
            },
        );

        // Navigate without payload
        let effect = SExpression::Array(vec![
            SExpression::String(("navigate".to_string()).into()),
            SExpression::String(("/inspection/123".to_string()).into()),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &page_init_payloads,
            &Default::default(),
            None,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectNavigateMissingPayload));
    }

    #[test]
    fn test_navigate_with_required_payload() {
        // Set up a page that requires an "id" field on INIT
        let mut page_init_payloads: HashMap<String, PageInitPayload> = HashMap::new();
        page_init_payloads.insert(
            "/inspection/:id".to_string(),
            PageInitPayload {
                required_fields: vec!["id".to_string()],
                all_fields: vec!["id".to_string()],
            },
        );

        // Navigate with payload
        let effect = SExpression::Array(vec![
            SExpression::String(("navigate".to_string()).into()),
            SExpression::String(("/inspection/123".to_string()).into()),
            SExpression::Object(
                vec![(
                    "id".to_string(),
                    SExpression::String(("@entity.id".to_string()).into()),
                )]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_effect(
            &effect,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &page_init_payloads,
            &Default::default(),
            None,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_path_matches_pattern() {
        assert!(path_matches_pattern("/inspection/123", "/inspection/:id"));
        assert!(path_matches_pattern(
            "/users/42/posts/7",
            "/users/:userId/posts/:postId"
        ));
        assert!(path_matches_pattern("/about", "/about"));
        assert!(!path_matches_pattern("/about", "/contact"));
        assert!(!path_matches_pattern("/inspection", "/inspection/:id"));
    }

    #[test]
    fn test_is_upper_snake_case() {
        assert!(is_upper_snake_case("TASK_CREATED"));
        assert!(is_upper_snake_case("START"));
        assert!(is_upper_snake_case("ITEM_1_SELECTED"));
        assert!(!is_upper_snake_case("taskCreated"));
        assert!(!is_upper_snake_case("task_created"));
        assert!(!is_upper_snake_case("_INVALID"));
        assert!(!is_upper_snake_case("INVALID_"));
    }

    fn nested_check(json: &str, declared: &[DeclaredEmit]) -> Vec<(ErrorCode, String)> {
        let effect: SExpression = serde_json::from_str(json).unwrap();
        let known: HashSet<String> = ["Message".to_string()].into_iter().collect();
        validate_effect(
            &effect,
            "fx",
            &known,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &Default::default(),
            Some(declared),
            &HashMap::new(),
            &HashMap::new(),
        )
        .errors
        .into_iter()
        .map(|e| (e.code, e.path))
        .collect()
    }

    #[test]
    fn effects_nested_in_control_flow_are_validated_with_their_own_path() {
        let literal = r#"["persist", "create", "Ghost", {"id": "mix"}]"#;
        let upsert = r#"["persist", "upsert", "Message", {"id": "mix"}]"#;
        let hit = |errs: &[(ErrorCode, String)], path: &str| {
            errs.iter()
                .any(|(c, p)| *c == ErrorCode::EffectFetchInvalidEntity && p == path)
        };
        let in_do = nested_check(&format!(r#"["do", {upsert}, {literal}]"#), &[]);
        assert!(hit(&in_do, "fx.do[1]"), "{in_do:?}");
        assert!(!hit(&in_do, "fx.do[0]"), "control: the upsert is clean");
        let in_if = nested_check(&format!(r#"["if", true, {upsert}, {literal}]"#), &[]);
        assert!(hit(&in_if, "fx.else"), "{in_if:?}");
        let in_when = nested_check(&format!(r#"["when", "@entity.on", {literal}]"#), &[]);
        assert!(hit(&in_when, "fx.then[0]"), "{in_when:?}");
        let in_let = nested_check(&format!(r#"["let", [["x", 1]], ["do", {literal}]]"#), &[]);
        assert!(hit(&in_let, "fx.body[0].do[0]"), "{in_let:?}");
        let in_list = nested_check(&format!(r#"["if", true, [{upsert}, {literal}]]"#), &[]);
        assert!(hit(&in_list, "fx.then[1]"), "{in_list:?}");
        let clean = nested_check(&format!(r#"["do", ["if", true, {upsert}]]"#), &[]);
        assert!(
            clean.is_empty(),
            "control: no nested defect, no error: {clean:?}"
        );
    }

}
