use super::collect_all_traits;
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::schema::OrbitalSchema;

/// Validate that external emits have payload definitions
pub(crate) fn validate_external_emit_payloads(schema: &OrbitalSchema) -> ValidationResult {
    use orbital_core::schema::EventScope;

    let mut result = ValidationResult::new();

    let all_traits = collect_all_traits(schema);
    for trait_def in &all_traits {
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                // Check if scope is external
                let is_external = emit
                    .scope
                    .as_ref()
                    .map(|s| *s == EventScope::External)
                    .unwrap_or(false);

                if is_external {
                    // External emits MUST have payload with at least one field
                    let has_payload = emit
                        .payload_schema
                        .as_ref()
                        .map(|p| !p.is_empty())
                        .unwrap_or(false);

                    if !has_payload {
                        result.add_error(
                            ValidationError::warning(
                                ErrorCode::TraitEventExternalMissingPayload,
                                format!("traits.{}.emits.{}", trait_def.name, emit.event),
                                format!(
                                    "External event '{}' in trait '{}' should have a payload definition with typed fields",
                                    emit.event, trait_def.name
                                ),
                            )
                            .with_suggestion(
                                "Add a payload array with fields: { \"name\": \"fieldName\", \"type\": \"string\", \"required\": true, \"description\": \"...\" }"
                            )
                            .with_expected_shape(r#"{
  "event": "EVENT_NAME",
  "scope": "external",
  "description": "What this event means",
  "payload": [
    { "name": "entityId", "type": "string", "required": true, "description": "ID of the entity" }
  ]
}"#),
                        );
                    }
                }
            }
        }
    }

    result
}
