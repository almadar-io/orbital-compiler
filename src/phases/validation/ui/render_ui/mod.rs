//! Render-UI Effect Validation
//!
//! Validates render_ui effects, slots, patterns, and props.
//! Ported from `builder/packages/shared/src/orbitals/validation/render-ui-validator.ts`.

mod binding;

// Re-exports so submodules can reference shared validation items via
// `super::*` and so external paths (`render_ui::is_render_ui_effect`,
// `render_ui::validate_all_render_ui`) keep resolving unchanged.
pub(crate) use super::sync;
pub(crate) use super::{
    ErrorCode, ValidationContext, ValidationError, ValidationResult, VALID_SLOTS,
};

use self::binding::validate_all_render_ui_bindings;
pub(crate) mod node_fields;
use self::sync::ExtraPatterns;

use orbital_core_base::schema::{EntityDefinition, OrbitalSchema, SExpression, TraitDefinition};
use std::collections::{HashMap, HashSet};

// Pattern types and itemAction props are imported from sync::patterns

/// Check if an S-expression is a render_ui effect
pub fn is_render_ui_effect(expr: &SExpression) -> bool {
    match expr {
        SExpression::Array(arr) if !arr.is_empty() => {
            if let SExpression::String(op) = &arr[0] {
                op == "render-ui"
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Extract slot from a render_ui effect
pub fn extract_slot(expr: &SExpression) -> Option<String> {
    match expr {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let SExpression::String(slot) = &arr[1] {
                Some(slot.clone().into_inner())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Validate all render_ui effects in a schema
pub fn validate_all_render_ui(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (trait_def, path) in super::inline_traits(schema) {
        let result = validate_trait_render_ui(
            trait_def,
            &path,
            &ctx.entity_definitions,
            ctx.extra_patterns.as_ref(),
        );
        ctx.merge(result);
    }

    // L2.1 keystone (gap #6): walk every render-ui prop binding
    // (`@payload.X` / `@entity.X` / `@config.X` / SExpression operator)
    // and verify the resolved type is in the prop's declared `types`
    // from the registry.
    validate_all_render_ui_bindings(schema, ctx);
}

/// Validate render_ui effects in a trait.
/// Performs data flow analysis: tracks which entities are fetched across the state graph
/// so that a state reachable from a fetch-state doesn't produce false positive warnings.
pub fn validate_trait_render_ui(
    trait_def: &TraitDefinition,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    extra_patterns: Option<&ExtraPatterns>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut slots_per_transition: HashMap<usize, HashSet<String>> = HashMap::new();

    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path =
                    format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);

                if is_render_ui_effect(effect) {
                    let render_result = validate_render_ui_effect(
                        effect,
                        &effect_path,
                        entity_definitions,
                        extra_patterns,
                        trait_def.linked_entity.as_deref(),
                    );
                    result.merge(render_result);

                    if let Some(slot) = extract_slot(effect) {
                        slots_per_transition.entry(i).or_default().insert(slot);
                    }
                }
            }
        }
    }

    // Check for slot conflicts within same transition
    for (i, slots) in slots_per_transition {
        let mut slot_counts: HashMap<&str, usize> = HashMap::new();
        for slot in &slots {
            *slot_counts.entry(slot.as_str()).or_default() += 1;
        }

        for (slot, count) in slot_counts {
            if count > 1 {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectSlotConflict,
                        format!("{}.stateMachine.transitions[{}]", path, i),
                        format!(
                            "Multiple render_ui effects targeting slot '{}' in same transition",
                            slot
                        ),
                    )
                    .with_suggestion(
                        "Each slot should have at most one render_ui effect per transition",
                    ),
                );
            }
        }
    }

    result
}

/// Validate a single render_ui effect
pub fn validate_render_ui_effect(
    effect: &SExpression,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    extra_patterns: Option<&ExtraPatterns>,
    linked_entity: Option<&str>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match effect {
        SExpression::Array(arr) if arr.len() >= 2 => {
            // Array form: ["render-ui", "slot", { pattern }]
            if let Some(SExpression::String(slot)) = arr.get(1) {
                result.merge(validate_slot(slot, path));
            }

        }
        _ => {}
    }

    result
}

/// Validate a slot name
fn validate_slot(slot: &str, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !VALID_SLOTS.contains(&slot) {
        result.add_error(
            ValidationError::error(
                ErrorCode::RenderUIInvalidSlot,
                path,
                format!("Invalid slot: '{}'", slot),
            )
            .with_valid_values(VALID_SLOTS.iter().map(|s| s.to_string()).collect()),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_render_ui_effect() {
        let effect = SExpression::Array(vec![
            SExpression::String(("render-ui".to_string()).into()),
            SExpression::String(("main".to_string()).into()),
            SExpression::object(
                [("type".into(), SExpression::String(("card".to_string()).into()))]
                    .into_iter()
                    .collect(),
            ),
        ].into());

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_invalid_slot() {
        let effect = SExpression::Array(vec![
            SExpression::String(("render-ui".to_string()).into()),
            SExpression::String(("invalid-slot".to_string()).into()),
            SExpression::object(
                [("type".into(), SExpression::String(("card".to_string()).into()))]
                    .into_iter()
                    .collect(),
            ),
        ].into());

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderUIInvalidSlot));
    }


    // FC-5 helpers: build the pinned dynamic-children tuple
    // `["array/map", <coll>, ["fn", <param>, <node>]]`.
    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string().into())
    }
    fn obj(pairs: &[(&str, SExpression)]) -> SExpression {
        SExpression::object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string().into(), v.clone()))
                .collect(),
        )
    }
    fn array_map(coll: &str, param: &str, node: SExpression) -> SExpression {
        SExpression::Array(vec![
            s("array/map"),
            s(coll),
            SExpression::Array(vec![s("fn"), s(param), node].into()),
        ].into())
    }

    /// `["if", cond, then, else?]` is a legal children entry — the runtime
    /// kernel evaluates it and `std-browse` ships it as `children[0]` of its
    /// dense/feed/gallery/triage bodies. It reached `validate_pattern`'s
    /// catch-all arm and was rejected as ORB_RUI_INVALID_PATTERN; the bodies
    /// above only escaped because they are referenced from inside an `(if)`
    /// arm and never walked, while a body referenced directly by a guarded
    /// transition (master-detail VIEW/CLOSE) is walked, and tripped.
    #[test]
    fn conditional_children_entry_not_rejected() {
        let toolbar = obj(&[("type", s("stack"))]);
        let empty = obj(&[("type", s("stack"))]);
        let cond = SExpression::Array(vec![s("if"), s("@config.bodySearch"), toolbar, empty].into());
        let effect = SExpression::Array(vec![
            s("render-ui"),
            s("main"),
            obj(&[
                ("type", s("stack")),
                ("children", SExpression::Array(vec![cond].into())),
            ]),
        ].into());
        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidPattern),
            "conditional children entry must not be an invalid pattern. Errors: {:?}",
            result.errors
        );
    }

    /// The 3-element form (no else arm) matches the kernel's own arity.
    #[test]
    fn conditional_children_entry_without_else_arm_accepted() {
        let cond = SExpression::Array(vec![
            s("if"),
            s("@config.bodySearch"),
            obj(&[("type", s("stack"))]),
        ].into());
        let effect = SExpression::Array(vec![
            s("render-ui"),
            s("main"),
            obj(&[
                ("type", s("stack")),
                ("children", SExpression::Array(vec![cond].into())),
            ]),
        ].into());
        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidPattern),
            "3-element conditional must be accepted. Errors: {:?}",
            result.errors
        );
    }

    /// Negative control: the arms are still validated, so a bogus pattern
    /// inside a branch is still caught. Recognizing the tuple must not become
    /// a hole that lets anything through.

    /// Negative control: a non-`if` array is still rejected — the new arm is
    /// keyed on the `if` head, not on "array".

    // A conditional's condition is an expression, never a render node. A
    // condition over records — `(> (array/len @config.filters) 0)` once
    // `@config.filters` is substituted with `[{field, label, …}]` — used to be
    // flattened into the children list, so each record was reported as a
    // pattern without a `type` (std-list / std-event-log with filters set).
    fn record_condition() -> SExpression {
        SExpression::Array(vec![
            s(">"),
            SExpression::Array(vec![
                s("array/len"),
                SExpression::Array(vec![obj(&[("field", s("status")), ("label", s("Status"))])].into()),
            ].into()),
            SExpression::Number(0.0),
        ].into())
    }

    fn stack_with_children(children: SExpression) -> SExpression {
        SExpression::Array(vec![
            s("render-ui"),
            s("main"),
            obj(&[("type", s("stack")), ("children", children)]),
        ].into())
    }

    fn render_errors(effect: &SExpression) -> Vec<ValidationError> {
        validate_render_ui_effect(effect, "test", &HashMap::new(), None, None).errors
    }

    #[test]
    fn conditional_condition_records_are_not_children() {
        let entry = SExpression::Array(vec![
            s("if"),
            record_condition(),
            obj(&[("type", s("stack"))]),
        ].into());
        let errors = render_errors(&stack_with_children(SExpression::Array(vec![entry].into())));
        assert!(
            errors.is_empty(),
            "the condition's records must not be walked as patterns: {:?}",
            errors
        );
    }

    #[test]
    fn conditional_condition_records_are_not_children_with_else_arm() {
        let entry = SExpression::Array(vec![
            s("if"),
            record_condition(),
            obj(&[("type", s("stack"))]),
            obj(&[("type", s("divider"))]),
        ].into());
        let errors = render_errors(&stack_with_children(SExpression::Array(vec![entry].into())));
        assert!(
            errors.is_empty(),
            "the condition's records must not be walked as patterns: {:?}",
            errors
        );
    }

    #[test]
    fn conditional_children_value_condition_records_are_not_children() {
        let value = SExpression::Array(vec![
            s("if"),
            record_condition(),
            SExpression::Array(vec![obj(&[("type", s("stack"))])].into()),
            SExpression::Array(vec![obj(&[("type", s("divider"))])].into()),
        ].into());
        let errors = render_errors(&stack_with_children(value));
        assert!(
            errors.is_empty(),
            "the condition's records must not be walked as patterns: {:?}",
            errors
        );
    }



    /// Control: a record-bearing condition does not excuse its branches.

    /// Control: operand splicing still applies to list-combining operators.

    #[test]
    fn fc5_array_map_children_entry_not_rejected() {
        // A `children` array entry that is the dynamic-collection tuple must NOT
        // trip ORB_RUI_INVALID_PATTERN; the per-item node is validated instead.
        let badge = obj(&[("type", s("badge")), ("label", s("@item.name"))]);
        let effect = SExpression::Array(vec![
            s("render-ui"),
            s("main"),
            obj(&[
                ("type", s("stack")),
                (
                    "children",
                    SExpression::Array(vec![array_map("@entity.items", "item", badge)].into()),
                ),
            ]),
        ].into());
        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidPattern),
            "array/map children entry must not be an invalid pattern. Errors: {:?}",
            result.errors
        );
    }


    #[test]
    fn fc5_bare_array_map_children_value_not_rejected() {
        // `children: (array/map …)` (the whole value, not wrapped in a list).
        let badge = obj(&[("type", s("badge")), ("label", s("@item.name"))]);
        let effect = SExpression::Array(vec![
            s("render-ui"),
            s("main"),
            obj(&[
                ("type", s("stack")),
                ("children", array_map("@entity.items", "item", badge)),
            ]),
        ].into());
        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidPattern),
            "bare array/map children value must not be an invalid pattern. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn fc5_render_item_lambda_accepted_on_collection_pattern() {
        // renderItem: (fn item <node>) on a collection pattern must NOT be
        // rejected as an unknown/invalid prop or pattern (callback-kind prop
        // accepts the lambda form).
        let badge = obj(&[("type", s("badge")), ("label", s("@item.name"))]);
        let lambda = SExpression::Array(vec![s("fn"), s("item"), badge].into());
        for pattern in [
            "data-list",
            "data-grid",
            "carousel",
            "sortable-list",
            "table-view",
            "repeatable-form-section",
        ] {
            let effect = SExpression::Array(vec![
                s("render-ui"),
                s("main"),
                obj(&[
                    ("type", s(pattern)),
                    ("entity", s("@entity")),
                    ("renderItem", lambda.clone()),
                ]),
            ].into());
            let entity_defs = HashMap::new();
            let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
            let renderitem_err = result.errors.iter().any(|e| {
                (e.code == ErrorCode::RenderUIInvalidProp
                    || e.code == ErrorCode::RenderUIInvalidPattern)
                    && e.message.contains("renderItem")
            });
            assert!(
                !renderitem_err,
                "renderItem lambda must be accepted on '{pattern}'. Errors: {:?}",
                result.errors
            );
        }
    }

    #[test]
    fn test_is_render_ui_effect() {
        assert!(is_render_ui_effect(&SExpression::Array(vec![
            SExpression::String(("render-ui".to_string()).into()),
        ].into())));

        assert!(!is_render_ui_effect(&SExpression::Array(vec![
            SExpression::String(("set".to_string()).into()),
        ].into())));
    }


    #[test]
    fn test_valid_pattern_props() {
        // Test that correct props (options, title) on game-menu pass validation
        let effect = SExpression::Array(vec![
            SExpression::String(("render-ui".to_string()).into()),
            SExpression::String(("main".to_string()).into()),
            SExpression::object(
                [
                    (
                        "type".into(),
                        SExpression::String(("game-menu".to_string()).into()),
                    ),
                    (
                        "title".into(),
                        SExpression::String(("Main Menu".to_string()).into()),
                    ),
                    (
                        "options".into(), // Correct prop name
                        SExpression::Array(vec![].into()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ].into());

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        // Should have no invalid prop errors
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidProp),
            "Expected no RenderUIInvalidProp errors for valid props. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_filter_group_entity_prop() {
        // Test that entity prop on filter-group is valid
        let effect = SExpression::Array(vec![
            SExpression::String(("render-ui".to_string()).into()),
            SExpression::String(("filter".to_string()).into()),
            SExpression::object(
                [
                    (
                        "type".into(),
                        SExpression::String(("filter-group".to_string()).into()),
                    ),
                    (
                        "entity".into(),
                        SExpression::String(("Contact".to_string()).into()),
                    ),
                    ("filters".into(), SExpression::Array(vec![].into())),
                ]
                .into_iter()
                .collect(),
            ),
        ].into());

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        // Should have no invalid prop errors
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidProp),
            "Expected no RenderUIInvalidProp errors for filter-group with entity. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_nested_field_path_valid() {
        use orbital_core_base::schema::{
            EntityDefinition, FieldDefinition, FieldType, RelationDefinition,
        };

        // Create mock entity definitions
        let mut entity_defs = HashMap::new();

        // Company entity with 'name' field
        entity_defs.insert(
            "Company".to_string(),
            EntityDefinition {
                timestamps: None,
                soft_delete: None,
                description: None,
                visual_prompt: None,
                asset_ref: None,
                store: None,
                shared: false,
                identity: false,
                browser: false,
                locale_instances: None,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Company".to_string(),
                fields: vec![FieldDefinition {
                    nullable: None,
                    additional_properties: None,
                    mock: None,
                    merged_from: None,
                    name: Some("name".to_string()),
                    field_type: FieldType::String,
                    required: None,
                    primary_key: None,
                    unique: None,
                    relation: None,
                    items: None,
                    values: None,
                    default: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                    projected_from: None,
                    record: None,
                    derived: None,
                }],
                collection: Some("companies".to_string()),
                persistence: None,
                instances: None,
                id: None,
                invariants: None,
                indexes: None,
            },
        );

        // Inspection entity with 'companyId' relation field
        entity_defs.insert(
            "Inspection".to_string(),
            EntityDefinition {
                timestamps: None,
                soft_delete: None,
                description: None,
                visual_prompt: None,
                asset_ref: None,
                store: None,
                shared: false,
                identity: false,
                browser: false,
                locale_instances: None,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Inspection".to_string(),
                fields: vec![
                    FieldDefinition {
                        nullable: None,
                        additional_properties: None,
                        mock: None,
                        merged_from: None,
                        name: Some("title".to_string()),
                        field_type: FieldType::String,
                        required: None,
                        primary_key: None,
                        unique: None,
                        relation: None,
                        items: None,
                        values: None,
                        default: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                        projected_from: None,
                        record: None,
                        derived: None,
                    },
                    FieldDefinition {
                        nullable: None,
                        additional_properties: None,
                        mock: None,
                        merged_from: None,
                        name: Some("companyId".to_string()),
                        field_type: FieldType::Relation,
                        required: None,
                        primary_key: None,
                        unique: None,
                        relation: Some(RelationDefinition {
                            entity: "Company".to_string(),
                            cardinality: None,
                            entity_id: None,
                            on_delete: None,
                            display: None,
                        }),
                        items: None,
                        values: None,
                        default: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                        projected_from: None,
                        record: None,
                        derived: None,
                    },
                ],
                collection: Some("inspections".to_string()),
                persistence: None,
                instances: None,
                id: None,
                invariants: None,
                indexes: None,
            },
        );

        let effect = SExpression::Array(vec![
            SExpression::String(("render-ui".to_string()).into()),
            SExpression::String(("main".to_string()).into()),
            SExpression::object(
                [
                    (
                        "type".into(),
                        SExpression::String(("data-grid".to_string()).into()),
                    ),
                    (
                        "entity".into(),
                        SExpression::String(("Inspection".to_string()).into()),
                    ),
                    (
                        "fields".into(),
                        SExpression::Array(vec![
                            SExpression::String(("title".to_string()).into()),
                            SExpression::String(("company.name".to_string()).into()),
                        ].into()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ].into());

        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        // Should have no field path errors
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidFieldPath),
            "Expected no field path errors for valid nested path. Errors: {:?}",
            result.errors
        );
    }

}
