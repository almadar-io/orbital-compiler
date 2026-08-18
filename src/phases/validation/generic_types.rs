//! Gate C of the type-integrity campaign: deprecation diagnostics for the
//! generic types `json`, `object` (shapeless), and `any` in declaration
//! positions — entity fields, config knobs, and event payload fields
//! (type-alias members surface through whichever of those positions they
//! lower into).
//!
//! Ships as a non-blocking phase-F warning (`ORB_T_GENERIC_TYPE_DEPRECATED`)
//! so the ~1,900-site corpus can migrate in waves; flips to a hard error once
//! both behavior packages report zero occurrences.
//!
//! Coverage note: the L1 lowerer erases a *scalar* entity field authored as
//! `json`/`any` to `string` before this validator runs, so those residual
//! authoring sites are invisible here and are swept by source grep during
//! migration (ledgered in `Almadar_Compiler_Gaps.md`). Every other position —
//! config knobs and payload fields (raw type strings), `[json]`/`object`
//! entity fields (lowered to shapeless `object`) — is visible and checked.

use orbital_core::schema::{FieldDefinition, FieldType, OrbitalSchema, PayloadField};

use super::{
    error::{ErrorCode, ValidationError, ValidationResult},
    inline_traits, ValidationContext,
};

const GUIDANCE: &str = "Replace the generic type: renderable content → `node`; structured data → an inline `{ k: T }` shape, a named `type` alias, or `Map K V`; genuinely polymorphic atoms → a declared type parameter.";

/// Strip one level of array brackets: `[json]` → `json`.
fn base_type(ft: &str) -> &str {
    ft.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(ft)
}

fn warn(path: &str, position: &str, declared: &str) -> ValidationError {
    ValidationError::phase_f_warning(
        ErrorCode::GenericTypeDeprecated,
        path,
        format!(
            "{position} declares the generic type '{declared}' — generic types (`json`, shapeless `object`, `any`) are deprecated and will become a hard error once the corpus migrates."
        ),
    )
    .with_suggestion(GUIDANCE.to_string())
}

fn check_payload_field(field: &PayloadField, path: &str, result: &mut ValidationResult) {
    let field_path = format!("{}.{}", path, field.name);
    let base = base_type(&field.field_type);
    match base {
        "json" | "any" => {
            result.add_error(warn(&field_path, "Payload field", &field.field_type));
        }
        "object" if field.properties.as_ref().map_or(true, |p| p.is_empty()) => {
            result.add_error(warn(
                &field_path,
                "Payload field",
                "object (no declared properties)",
            ));
        }
        _ => {}
    }
    if let Some(props) = &field.properties {
        for nested in props {
            check_payload_field(nested, &field_path, result);
        }
    }
}

fn check_entity_field(field: &FieldDefinition, path: &str, result: &mut ValidationResult) {
    match field.field_type {
        FieldType::Object => {
            if field.properties.as_ref().map_or(true, |p| p.is_empty()) {
                result.add_error(warn(
                    path,
                    "Entity field",
                    "object (no declared properties — authored `object`/`json`)",
                ));
            } else if let Some(props) = &field.properties {
                for (name, nested) in props {
                    check_entity_field(nested, &format!("{path}.{name}"), result);
                }
            }
        }
        FieldType::Array => {
            if let Some(items) = &field.items {
                check_entity_field(items, &format!("{path}[]"), result);
            }
        }
        _ => {}
    }
}

fn check_config_type(field_type: &str, path: &str, result: &mut ValidationResult) {
    // Bare/array `object` is already a hard error (`TraitConfigObjectArrayForbidden`);
    // only warn on the survivors so a single site is not double-reported.
    if matches!(base_type(field_type), "json" | "any") {
        result.add_error(warn(path, "Config field", field_type));
    }
}

/// Walk every declaration position the `.orb` IR carries and report generic
/// types. Non-blocking (phase-F) while the corpus migrates.
pub fn validate_generic_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    for (name, entity) in &ctx.entity_definitions {
        for field in &entity.fields {
            let field_name = field.name.as_deref().unwrap_or("<unnamed>");
            let path = format!("entities.{name}.fields.{field_name}");
            check_entity_field(field, &path, &mut result);
        }
    }

    for (trait_def, trait_path) in inline_traits(schema) {
        if let Some(config) = &trait_def.config {
            for (key, field) in config {
                let path = format!("{trait_path}.config.{key}");
                check_config_type(&field.field_type, &path, &mut result);
            }
        }
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                if let Some(payload) = &emit.payload_schema {
                    let path = format!("{trait_path}.emits.{}", emit.event);
                    for field in payload {
                        check_payload_field(field, &path, &mut result);
                    }
                }
            }
        }
        if let Some(events) = &trait_def.state_machine.events {
            for event in events {
                if let Some(payload) = &event.payload_schema {
                    let path = format!("{trait_path}.events.{}", event.key);
                    for field in payload {
                        check_payload_field(field, &path, &mut result);
                    }
                }
            }
        }
    }

    ctx.merge(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity_field(field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            name: None,
            field_type,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }
    }

    fn payload(field_type: &str, properties: Option<Vec<PayloadField>>) -> PayloadField {
        PayloadField {
            name: "value".to_string(),
            field_type: field_type.to_string(),
            required: None,
            description: None,
            properties,
        }
    }

    #[test]
    fn payload_generic_types_warn_without_blocking() {
        for ft in ["object", "json", "any", "[json]", "[any]"] {
            let mut result = ValidationResult::new();
            check_payload_field(&payload(ft, None), "t.emits.E", &mut result);
            assert!(
                result.errors.is_empty(),
                "{ft} must be non-blocking during migration"
            );
            assert_eq!(result.warnings.len(), 1, "{ft} must warn");
            assert_eq!(
                result.warnings[0].code.as_str(),
                "ORB_T_GENERIC_TYPE_DEPRECATED"
            );
        }
    }

    #[test]
    fn typed_payload_object_passes() {
        let mut result = ValidationResult::new();
        let typed = payload("object", Some(vec![payload("string", None)]));
        check_payload_field(&typed, "t.emits.E", &mut result);
        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn nested_payload_generic_is_found() {
        let mut result = ValidationResult::new();
        let nested = payload("object", Some(vec![payload("any", None)]));
        check_payload_field(&nested, "t.emits.E", &mut result);
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn shapeless_object_entity_field_warns_typed_passes() {
        let shapeless = entity_field(FieldType::Object);
        let mut result = ValidationResult::new();
        check_entity_field(&shapeless, "entities.E.fields.data", &mut result);
        assert!(result.errors.is_empty());
        assert_eq!(result.warnings.len(), 1);

        let mut array_of_shapeless = entity_field(FieldType::Array);
        array_of_shapeless.items = Some(Box::new(shapeless));
        let mut result = ValidationResult::new();
        check_entity_field(&array_of_shapeless, "entities.E.fields.rows", &mut result);
        assert_eq!(result.warnings.len(), 1);

        let mut typed = entity_field(FieldType::Object);
        typed.properties = Some(
            [("x".to_string(), entity_field(FieldType::Number))]
                .into_iter()
                .collect(),
        );
        let mut result = ValidationResult::new();
        check_entity_field(&typed, "entities.E.fields.point", &mut result);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn config_json_and_any_warn_object_left_to_existing_ban() {
        let mut result = ValidationResult::new();
        check_config_type("json", "t.config.style", &mut result);
        check_config_type("[any]", "t.config.buffer", &mut result);
        check_config_type("object", "t.config.covered", &mut result);
        check_config_type("Map string string", "t.config.map", &mut result);
        assert!(result.errors.is_empty());
        assert_eq!(result.warnings.len(), 2);
    }
}
