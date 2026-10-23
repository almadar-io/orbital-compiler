//! Gate C of the type-integrity campaign: deprecation diagnostics for the
//! generic types `json`, `object` (shapeless), `any` and `scalar` in declaration
//! positions — entity fields, config knobs, and event payload fields
//! (type-alias members surface through whichever of those positions they
//! lower into).
//!
//! **HARD ERROR since 2026-09-01.** It shipped as a non-blocking phase-F warning
//! while the corpus migrated in waves (4,921 → 0). Both behavior packages now
//! report zero occurrences, which is the condition this gate was always waiting
//! on, so a generic type in a declaration position is now rejected outright.
//!
//! Coverage note: the L1 lowerer erases a *scalar* entity field authored as
//! `json`/`any` to `string` before this validator runs, so those residual
//! authoring sites are invisible here and are swept by source grep during
//! migration (ledgered in `Almadar_Compiler_Gaps.md`). Every other position —
//! config knobs and payload fields (raw type strings), `[json]`/`object`
//! entity fields (lowered to shapeless `object`) — is visible and checked.

use orbital_core_base::schema::{FieldDefinition, FieldType, OrbitalSchema, PayloadField};

use super::{
    error::{ErrorCode, ValidationError, ValidationResult},
    inline_traits, ValidationContext,
};

const GUIDANCE: &str = "Replace the generic type: renderable content → `node`; structured data → an inline `{ k: T }` shape, a named `type` alias, or `Map K V`; a value of several kinds → a named tagged-union `type` alias (or the concrete primitive); the row of an entity a config knob names → `@entity @config.<knob>`; the row of the linked entity → `@entity @config.<knob>`; genuinely polymorphic atoms → a declared type parameter.";

/// Strip one level of array brackets: `[json]` → `json`.
fn base_type(ft: &str) -> &str {
    ft.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(ft)
}

fn warn(path: &str, position: &str, declared: &str) -> ValidationError {
    ValidationError::error(
        ErrorCode::GenericTypeDeprecated,
        path,
        format!(
            "{position} declares the generic type '{declared}' — generic types (`json`, shapeless `object`, `any`, `scalar`) are not allowed in a declaration position."
        ),
    )
    .with_suggestion(GUIDANCE.to_string())
}

fn check_payload_field(field: &PayloadField, path: &str, result: &mut ValidationResult) {
    let field_path = format!("{}.{}", path, field.name);
    if let Some(additional) = &field.additional_properties {
        check_payload_field(additional, &format!("{field_path}{{}}"), result);
    }
    // A list or map whose element is itself a typed field (`[(A | B)]`, `Map string (A | B)`) is typed by that element.
    if let Some(item) = &field.items {
        check_payload_field(item, &format!("{field_path}[]"), result);
        return;
    }
    let base = base_type(&field.field_type);
    match base {
        "json" | "any" | "scalar" => {
            result.add_error(warn(&field_path, "Payload field", &field.field_type));
        }
        "object" if field.additional_properties.is_none() && field.properties.as_ref().map_or(true, |p| p.is_empty()) => {
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
        // `Map K V` on an ENTITY field lowers to the `Object` tag with the
        // VALUE shape in `items` and no `properties` (see `lower/entity.rs`'s
        // Map arm) — a map's keys are dynamic, so it HAS no properties by
        // construction. Without this arm a fully-typed `Map string number`
        // would be reported as "no declared properties", i.e. the gate would
        // reject the very fix it is asking for.
        FieldType::Object if field.items.is_some() => {
            if let Some(value_schema) = &field.items {
                check_entity_field(value_schema, &format!("{path}{{}}"), result);
            }
        }
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
    // only report the survivors so a single site is not double-reported.
    if matches!(base_type(field_type), "json" | "any" | "scalar") {
        result.add_error(warn(path, "Config field", field_type));
    }
}

fn check_types_map(
    types: Option<&std::collections::BTreeMap<String, Vec<PayloadField>>>,
    owner: &str,
    result: &mut ValidationResult,
) {
    if let Some(types) = types {
        for (alias, fields) in types {
            let path = format!("{owner}.{alias}");
            for field in fields {
                check_payload_field(field, &path, result);
            }
        }
    }
}

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

/// Walk every declaration position the `.orb` IR carries and report generic
/// types. Hard error.
pub fn validate_generic_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    for (name, entity) in &ctx.entity_definitions {
        for field in &entity.fields {
            let field_name = field.name.as_deref().unwrap_or("<unnamed>");
            let path = format!("entities.{name}.fields.{field_name}");
            check_entity_field(field, &path, &mut result);
        }
    }

    // The orbital's struct-alias map (`Orbital.types`) is a declaration
    // position in its own right: alias members reach payloads BY NAME
    // (recursive back-reference), so a generic type inside an alias shape is
    // invisible to the payload walks below — the exact hole `targetQuestionId:
    // json` sat in while the file validated 0/0.
    check_types_map(schema.types.as_ref(), "types", &mut result);
    if let Some(types) = &schema.types {
        check_alias_references(types, &|name| ctx.entity_definitions.contains_key(name), &mut result);
    }
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        check_types_map(orbital.types.as_ref(), &format!("orbitals[{oi}].types"), &mut result);
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
            nullable: None,
            additional_properties: None,
            mock: None,
            merged_from: None,
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
            projected_from: None,
        }
    }

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

    /// The struct-alias map is a declaration position: alias members reach
    /// payloads by NAME (recursive back-reference), so a generic type hiding
    /// inside an alias shape is invisible to the payload walks — the hole
    /// `targetQuestionId: json` validated 0/0 through.
    #[test]
    fn types_alias_map_generics_are_rejected() {
        let mut result = ValidationResult::new();
        let types: std::collections::BTreeMap<String, Vec<PayloadField>> = [(
            "RulesItem".to_string(),
            vec![payload("string", None), payload("json", None)],
        )]
        .into_iter()
        .collect();
        check_types_map(Some(&types), "orbitals[0].types", &mut result);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(
            result.errors[0].code.as_str(),
            "ORB_T_GENERIC_TYPE_DEPRECATED"
        );
        assert!(result.errors[0]
            .path
            .contains("orbitals[0].types.RulesItem"));
    }

    #[test]
    fn a_type_export_may_name_a_language_value_kind() {
        for word in ["EventAddress", "behavior", "trait", "orbital"] {
            let types: std::collections::BTreeMap<String, Vec<PayloadField>> =
                [("T".to_string(), vec![payload(word, None)])].into_iter().collect();
            let mut result = ValidationResult::new();
            check_alias_references(&types, &|_| false, &mut result);
            assert!(result.errors.is_empty(), "{word}: {:?}", result.errors);
        }
    }

    #[test]
    fn a_type_export_naming_nothing_declared_is_still_rejected() {
        let types: std::collections::BTreeMap<String, Vec<PayloadField>> =
            [("T".to_string(), vec![payload("Nonexistent", None)])].into_iter().collect();
        let mut result = ValidationResult::new();
        check_alias_references(&types, &|_| false, &mut result);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].code.as_str(), "ORB_T_TYPE_UNKNOWN");
    }

    #[test]
    fn payload_generic_types_are_rejected() {
        for ft in ["object", "json", "any", "scalar", "[json]", "[any]", "[scalar]"] {
            let mut result = ValidationResult::new();
            check_payload_field(&payload(ft, None), "t.emits.E", &mut result);
            assert_eq!(result.errors.len(), 1, "{ft} must be rejected");
            assert_eq!(
                result.errors[0].code.as_str(),
                "ORB_T_GENERIC_TYPE_DEPRECATED"
            );
        }
    }

    #[test]
    fn scalar_knob_is_rejected_and_a_typed_primitive_passes() {
        for ft in ["scalar", "[scalar]"] {
            let mut result = ValidationResult::new();
            check_config_type(ft, "t.config.cell", &mut result);
            assert_eq!(result.errors.len(), 1, "{ft} must be rejected");
        }
        let mut result = ValidationResult::new();
        check_config_type("string", "t.config.cell", &mut result);
        assert!(result.errors.is_empty(), "control");
    }

    #[test]
    fn typed_payload_object_passes() {
        let mut result = ValidationResult::new();
        let typed = payload("object", Some(vec![payload("string", None)]));
        check_payload_field(&typed, "t.emits.E", &mut result);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn a_payload_map_or_list_of_a_union_is_typed_by_its_items() {
        let union = payload("union", Some(vec![payload("string", None), payload("number", None)]));
        for container in ["object", "array"] {
            let mut field = payload(container, None);
            field.items = Some(Box::new(union.clone()));
            let mut result = ValidationResult::new();
            check_payload_field(&field, "t.emits.E", &mut result);
            assert!(result.errors.is_empty(), "{container} + items: {:?}", result.errors);
        }
    }

    #[test]
    fn control_a_generic_inside_payload_items_is_still_found() {
        let mut field = payload("object", None);
        field.items = Some(Box::new(payload("json", None)));
        let mut result = ValidationResult::new();
        check_payload_field(&field, "t.emits.E", &mut result);
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn nested_payload_generic_is_found() {
        let mut result = ValidationResult::new();
        let nested = payload("object", Some(vec![payload("any", None)]));
        check_payload_field(&nested, "t.emits.E", &mut result);
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn shapeless_object_entity_field_rejected_typed_passes() {
        let shapeless = entity_field(FieldType::Object);
        let mut result = ValidationResult::new();
        check_entity_field(&shapeless, "entities.E.fields.data", &mut result);
        assert_eq!(result.errors.len(), 1);

        let mut array_of_shapeless = entity_field(FieldType::Array);
        array_of_shapeless.items = Some(Box::new(shapeless));
        let mut result = ValidationResult::new();
        check_entity_field(&array_of_shapeless, "entities.E.fields.rows", &mut result);
        assert_eq!(result.errors.len(), 1);

        let mut typed = entity_field(FieldType::Object);
        typed.properties = Some(
            [("x".to_string(), entity_field(FieldType::Number))]
                .into_iter()
                .collect(),
        );
        let mut result = ValidationResult::new();
        check_entity_field(&typed, "entities.E.fields.point", &mut result);
        assert!(result.errors.is_empty());
    }

    /// Recursive-types safety (spec §3 `generic_types.rs`): the by-name
    /// back-reference form must never trip this gate. On the payload side
    /// it's a bare alias-name `field_type` (e.g. `"[MenuItem]"`/`"MenuItem"`,
    /// never `"object"`/`"json"`/`"any"`) with an EMPTY (not absent)
    /// `properties` marker; on the entity side it's `FieldType::Union` with
    /// `properties: None`, a variant this function's `match` doesn't even
    /// list (falls to `_ => {}`). Neither reads as "object with no declared
    /// properties" — that check is keyed strictly on `field_type == "object"`.
    #[test]
    fn recursive_backref_forms_produce_no_finding() {
        let mut result = ValidationResult::new();
        check_payload_field(
            &payload("[MenuItem]", Some(vec![])),
            "t.emits.ItemPicked",
            &mut result,
        );
        check_payload_field(
            &payload("MenuItem", Some(vec![])),
            "t.emits.ItemPicked",
            &mut result,
        );
        assert!(
            result.errors.is_empty(),
            "a recursive back-reference must not read as a generic type: {:?}",
            result.errors
        );

        let mut result = ValidationResult::new();
        let backref = entity_field(FieldType::Union);
        check_entity_field(&backref, "entities.E.fields.subMenu", &mut result);
        assert!(
            result.errors.is_empty(),
            "FieldType::Union isn't matched by check_entity_field at all — a properties-none Union is not \"object with no properties\": {:?}",
            result.errors
        );
    }

    #[test]
    fn config_json_and_any_rejected_object_left_to_existing_ban() {
        let mut result = ValidationResult::new();
        check_config_type("json", "t.config.style", &mut result);
        check_config_type("[any]", "t.config.buffer", &mut result);
        check_config_type("object", "t.config.covered", &mut result);
        check_config_type("Map string string", "t.config.map", &mut result);
        // `json` + `[any]` only: bare/array `object` stays with the existing
        // TraitConfigObjectArrayForbidden ban, and a typed Map is not generic.
        assert_eq!(result.errors.len(), 2);
    }
}
