//! Effect validation helpers: payload extraction + identifier checks.

use crate::phases::validation::sexpr::effect_category_operators;
use orbital_core_base::schema::SExpression;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Operators valid in EFFECT POSITION beyond the table's `effect` category
/// (control-flow wrappers that *contain* effects, array mutation, OS / ML /
/// agent ops). The core effects are NOT relisted here — `effect_operators()`
/// derives the full set from the operator table's `effect` category, so a new core effect can't drift between the two lists.
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
];

/// All operators valid in effect position: the table's `effect` category plus the wrappers above.
pub fn effect_operators() -> &'static [&'static str] {
    static ALL: OnceLock<Vec<&'static str>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v: Vec<&'static str> = effect_category_operators().collect();
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
                fields.insert(key.to_string());
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
        ].into());

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
        ].into());

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
        ].into());

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
        let params = SExpression::object(
            [
                (
                    "to".into(),
                    SExpression::String(("a@b.c".to_string()).into()),
                ),
                (
                    "subject".into(),
                    SExpression::String(("s".to_string()).into()),
                ),
                (
                    "body".into(),
                    SExpression::String(("b".to_string()).into()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let emit = SExpression::object(
            keys.iter()
                .map(|(k, v)| (k.to_string().into(), SExpression::String(v.to_string().into())))
                .collect(),
        );
        let envelope = SExpression::object([("emit".into(), emit)].into_iter().collect());
        SExpression::Array(vec![
            SExpression::String(("call-service".to_string()).into()),
            SExpression::String(("email".to_string()).into()),
            SExpression::String(("send".to_string()).into()),
            params,
            envelope,
        ].into())
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
            SExpression::object(
                vec![(
                    "id".into(),
                    SExpression::String(("@entity.id".to_string()).into()),
                )]
                .into_iter()
                .collect(),
            ),
        ].into());

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

}

/// Position of the entity-name literal in a `persist` call array: 2 for the
/// 4-element form (`["persist", "create"|"upsert"|"update"|"delete", "Entity", data]`),
/// 1 for the legacy 3-element form (`["persist", "Entity", data]`). Shared
/// with the compiler's field-usage collector (`phases/inline/entity.rs`) so
/// both readers agree on where the entity name and data literal sit — no
/// second copy of this positional rule.
pub(crate) fn persist_entity_index(arr: &[SExpression]) -> usize {
    match arr.get(1) {
        Some(SExpression::String(op)) if op == "create" || op == "upsert" || op == "update" || op == "delete" => 2,
        _ => 1,
    }
}

/// Extract the `emit:` object from a trailing options SExpression::Object.
/// Returns None if the map has no `emit` key or if `emit` is not an object.
pub(crate) fn extract_emit_object_ref(
    options: &orbital_core_base::schema::SObject,
) -> Option<&orbital_core_base::schema::SObject> {
    match options.get("emit")? {
        SExpression::Object(obj) => Some(&**obj),
        _ => None,
    }
}
