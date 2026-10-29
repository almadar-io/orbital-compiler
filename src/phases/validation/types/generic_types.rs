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
        let shape = match orbital_core_base::ir::types::declared_alias_shape(fields) {
            Ok(shape) => shape,
            Err(error) => {
                result.add_error(ValidationError::error(
                    ErrorCode::GenericTypeDeprecated,
                    format!("types.{alias}"),
                    format!("Type export `{alias}` {error}"),
                ));
                continue;
            }
        };
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

fn shape_gap(path: String, what: &str, error: orbital_core_base::ir::types::ShapeError, result: &mut ValidationResult) {
    result.add_error(ValidationError::error(
        ErrorCode::GenericTypeDeprecated,
        path,
        format!("{what} {error}"),
    ));
}

/// Every declared type position that has a shape: an entity field, an event or emit payload field, a trait config
/// knob. A position with none (an `object` without members, an `array` without an element type, a `relation`
/// naming no entity) is a generic type, which the language does not have.
fn check_type_shapes(schema: &OrbitalSchema, result: &mut ValidationResult) {
    use orbital_core_base::schema::{EntityRef, TraitReference};
    for (o, orbital) in schema.orbitals.iter().enumerate() {
        for (alias, members) in orbital.types.iter().flatten() {
            if let Err(error) = orbital_core_base::ir::types::declared_alias_shape(members) {
                shape_gap(format!("orbitals[{o}].types.{alias}"), &format!("Type `{alias}`"), error, result);
            }
        }
        let entities = std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten());
        for entity in entities.filter_map(EntityRef::as_inline) {
            for field in &entity.fields {
                if let Err(error) = orbital_core_base::ir::types::field_shape(field) {
                    let name = field.name.as_deref().unwrap_or("");
                    shape_gap(
                        format!("orbitals[{o}].entity.{}.fields.{name}", entity.name),
                        &format!("Field `{}.{name}`", entity.name),
                        error,
                        result,
                    );
                }
            }
        }
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(trait_def) = trait_ref else { continue };
            let base = format!("orbitals[{o}].traits.{}", trait_def.name);
            let declared = trait_def.state_machine.events.iter().flatten().filter_map(|e| Some((e.key.as_str(), e.payload_schema.as_ref()?)));
            let emitted = trait_def.emits.iter().flatten().filter_map(|e| Some((e.event.as_str(), e.payload_schema.as_ref()?)));
            for (event, payload) in declared.chain(emitted) {
                for field in payload {
                    if let Err(error) = orbital_core_base::ir::types::payload_field_shape(field) {
                        shape_gap(
                            format!("{base}.events.{event}.{}", field.name),
                            &format!("Payload field `{event}.{}`", field.name),
                            error,
                            result,
                        );
                    }
                }
            }
            for (knob, field) in trait_def.config.iter().flatten() {
                if let Err(error) = crate::phases::resolve::config_field_oir_type(field, schema) {
                    result.add_error(ValidationError::error(
                        ErrorCode::GenericTypeDeprecated,
                        format!("{base}.config.{knob}"),
                        format!("Config knob `{knob}`: {error}"),
                    ));
                }
            }
        }
    }
}

/// Report type-export shapes that name an undeclared type, and declared types that have no shape.
pub fn validate_generic_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    if let Some(types) = &schema.types {
        check_alias_references(
            types,
            &|name| ctx.entity_definitions.contains_key(name),
            &mut result,
        );
    }
    check_type_shapes(schema, &mut result);
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

    fn program(entity_field: serde_json::Value, payload_type: &str, knob_type: &str) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "t", "version": "1.0.0",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Row", "fields": [{ "name": "id", "type": "string" }, entity_field] },
                "traits": [{
                    "name": "Holder",
                    "linkedEntity": "Row",
                    "emits": [{ "event": "DONE", "payloadSchema": [{ "name": "rows", "type": payload_type }] }],
                    "config": { "p": { "type": knob_type } },
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": [] }]
                    }
                }]
            }]
        }))
        .expect("fixture parses")
    }

    fn shape_findings(schema: &OrbitalSchema) -> Vec<(String, String)> {
        let mut ctx = ValidationContext::new();
        validate_generic_types(schema, &mut ctx);
        ctx.result
            .errors
            .iter()
            .filter(|e| e.code.as_str() == "ORB_T_GENERIC_TYPE_DEPRECATED")
            .map(|e| (e.path.clone(), e.message.clone()))
            .collect()
    }

    #[test]
    fn a_declared_type_with_no_shape_is_reported_at_its_position() {
        let good = program(serde_json::json!({ "name": "meta", "type": "string" }), "[string]", "string");
        assert!(shape_findings(&good).is_empty(), "control: every position has a shape");

        let entity = program(serde_json::json!({ "name": "meta", "type": "object" }), "[string]", "string");
        let found = shape_findings(&entity);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0.contains("Row.fields.meta"), "{found:?}");
        assert!(found[0].1.contains("an object with no properties"), "{found:?}");

        let payload = program(serde_json::json!({ "name": "meta", "type": "string" }), "array", "string");
        let found = shape_findings(&payload);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].1.contains("rows") && found[0].1.contains("array type with no element type"), "{found:?}");

        let knob = program(serde_json::json!({ "name": "meta", "type": "string" }), "[string]", "array");
        let found = shape_findings(&knob);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0.ends_with("config.p"), "{found:?}");
    }
}
