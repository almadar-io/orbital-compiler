//! Type-export alias reference check: every name a `types` export's shape refers to must resolve.
//! The generic-type ban (`ORB_T_GENERIC_TYPE_DEPRECATED`) is owned by the std lint `std-orb-lint-generic-types`.

use orbital_core_base::schema::{FieldType, OrbitalSchema, PayloadField};

use super::{
    error::{ErrorCode, ValidationError, ValidationResult},
    ValidationContext,
};

/// A language type word (`EventAddress`, `behavior`, `trait`, `orbital`, …) is a value kind of the schema's own
/// field-type table, not a name a program declares.
fn is_value_kind(name: &str) -> bool {
    serde_json::from_value::<FieldType>(serde_json::Value::String(name.to_string())).is_ok()
}

/// Every name a type export's shape refers to must be a declared alias, an entity, a language program type or a
/// value kind.
fn check_alias_references(
    types: &std::collections::BTreeMap<String, Vec<PayloadField>>,
    is_entity: &dyn Fn(&str) -> bool,
    result: &mut ValidationResult,
) {
    for (alias, fields) in types {
        let shape = orbital_core_base::ir::types::declared_alias_shape(fields);
        let mut names = Vec::new();
        orbital_core_base::ir::types::names_in(&shape, &mut names);
        names.sort();
        names.dedup();
        for name in names {
            if !types.contains_key(&name)
                && !is_entity(&name)
                && !is_value_kind(&name)
                && orbital_core_base::language::program_types::program_type(&name).is_none()
            {
                result.add_error(ValidationError::error(
                    ErrorCode::TypeUnknown,
                    format!("types.{alias}"),
                    format!("Type export `{alias}` references undeclared type `{name}`"),
                ));
            }
        }
    }
}

/// Report type-export shapes that name an undeclared type.
pub fn validate_generic_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    if let Some(types) = &schema.types {
        check_alias_references(
            types,
            &|name| ctx.entity_definitions.contains_key(name),
            &mut result,
        );
    }
    ctx.merge(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(field_type: &str, properties: Option<Vec<PayloadField>>) -> PayloadField {
        PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "value".to_string(),
            field_type: field_type.to_string(),
            required: None,
            description: None,
            properties,
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        }
    }

    #[test]
    fn a_type_export_may_name_a_language_value_kind() {
        for word in ["EventAddress", "behavior", "trait", "orbital"] {
            let types: std::collections::BTreeMap<String, Vec<PayloadField>> =
                [("T".to_string(), vec![payload(word, None)])]
                    .into_iter()
                    .collect();
            let mut result = ValidationResult::new();
            check_alias_references(&types, &|_| false, &mut result);
            assert!(result.errors.is_empty(), "{word}: {:?}", result.errors);
        }
    }

    #[test]
    fn a_type_export_naming_nothing_declared_is_still_rejected() {
        let types: std::collections::BTreeMap<String, Vec<PayloadField>> =
            [("T".to_string(), vec![payload("Nonexistent", None)])]
                .into_iter()
                .collect();
        let mut result = ValidationResult::new();
        check_alias_references(&types, &|_| false, &mut result);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].code.as_str(), "ORB_T_TYPE_UNKNOWN");
    }
}
