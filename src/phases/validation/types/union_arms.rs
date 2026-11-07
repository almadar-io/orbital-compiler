//! Every union in an entity field, an event payload or a type export has arms that name distinct model arms.

use orbital_core_base::ir::types::{field_union_arm_collision, payload_union_arm_collision, UnionArmCollision};
use orbital_core_base::schema::OrbitalSchema;

use super::{
    error::{ErrorCode, ValidationError, ValidationResult},
    ValidationContext,
};

fn refuse(result: &mut ValidationResult, path: String, collision: UnionArmCollision) {
    result.add_error(ValidationError::error(ErrorCode::TypeUnionArmCollision, path, collision.to_string()));
}

fn check(schema: &OrbitalSchema, result: &mut ValidationResult) {
    for orbital in &schema.orbitals {
        let entities = orbital
            .entity
            .as_inline()
            .into_iter()
            .chain(orbital.auxiliary_entities.iter().flatten().filter_map(|aux| aux.as_inline()));
        for entity in entities {
            for field in &entity.fields {
                if let Some(collision) = field_union_arm_collision(field) {
                    let name = field.name.as_deref().unwrap_or_default();
                    refuse(result, format!("entities.{}.{name}", entity.name), collision);
                }
            }
        }
    }
    for (trait_def, path) in super::inline_traits(schema) {
        for event in trait_def.state_machine.events.iter().flatten() {
            for field in event.payload_schema.iter().flatten() {
                if let Some(collision) = payload_union_arm_collision(field) {
                    refuse(result, format!("{path}.events.{}.{}", event.key, field.name), collision);
                }
            }
        }
    }
    for (alias, fields) in schema.types.iter().flatten() {
        for field in fields {
            if let Some(collision) = payload_union_arm_collision(field) {
                refuse(result, format!("types.{alias}"), collision);
            }
        }
    }
}

/// Report each union whose arms would collapse to one model arm.
pub fn validate_union_arms(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    check(schema, &mut result);
    ctx.merge(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(types: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({ "name": "t", "orbitals": [], "types": types })).expect("schema")
    }

    #[test]
    fn a_type_export_with_two_list_arms_is_refused() {
        let s = schema(serde_json::json!({ "Mixed": [{ "name": "", "type": "union", "properties": [
            { "name": "array", "type": "[string]" }, { "name": "array", "type": "[number]" } ] }] }));
        let mut result = ValidationResult::new();
        check(&s, &mut result);
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
        assert_eq!(result.errors[0].code.as_str(), "ORB_T_TYPE_UNION_ARM_COLLISION");
        assert_eq!(result.errors[0].path, "types.Mixed");
    }

    #[test]
    fn control_a_type_export_with_distinct_arms_passes() {
        let s = schema(serde_json::json!({ "From": [{ "name": "", "type": "union", "properties": [
            { "name": "string", "type": "string" }, { "name": "array", "type": "[string]" } ] }] }));
        let mut result = ValidationResult::new();
        check(&s, &mut result);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }
}
