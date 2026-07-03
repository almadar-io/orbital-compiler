//! Render-UI Effect Validation
//!
//! Validates render_ui effects, slots, patterns, and props.
//! Ported from `builder/packages/shared/src/orbitals/validation/render-ui-validator.ts`.

mod binding;
mod field_path;
mod item_actions;
mod pattern;

// Re-exports so submodules can reference shared validation items via
// `super::*` and so external paths (`render_ui::is_render_ui_effect`,
// `render_ui::validate_all_render_ui`) keep resolving unchanged.
pub(crate) use super::sync;
pub(crate) use super::{
    ErrorCode, ValidationContext, ValidationError, ValidationResult, VALID_SLOTS,
};

use self::binding::validate_all_render_ui_bindings;
use self::item_actions::{extract_fetch_entity, extract_render_ui_entity};
pub use self::item_actions::{extract_slot, is_render_ui_effect};
use self::pattern::validate_pattern;
use self::sync::ExtraPatterns;

use orbital_core::schema::{EntityDefinition, OrbitalSchema, SExpression, TraitDefinition};
use std::collections::{HashMap, HashSet};

// Pattern types and itemAction props are imported from sync::patterns

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
    // from the registry. Independent pass — additive to the existing
    // literal-only `RenderUIPropTypeMismatch` check.
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

    // --- Data flow analysis: build reachability of fetched entities across state graph ---
    // Step 1: Collect which entities are fetched per target state
    let mut fetched_by_state: HashMap<&str, HashSet<String>> = HashMap::new();
    for transition in &trait_def.state_machine.transitions {
        if let Some(effects) = &transition.effects {
            let fetched: HashSet<String> =
                effects.iter().filter_map(extract_fetch_entity).collect();
            if !fetched.is_empty() {
                fetched_by_state
                    .entry(transition.to.as_str())
                    .or_default()
                    .extend(fetched);
            }
        }
    }

    // Step 2: Build adjacency list (state -> successor states)
    let mut successors: HashMap<&str, HashSet<&str>> = HashMap::new();
    for transition in &trait_def.state_machine.transitions {
        let from_states = match &transition.from {
            orbital_core::schema::StringOrArray::Single(s) => vec![s.as_str()],
            orbital_core::schema::StringOrArray::Multiple(v) => {
                v.iter().map(|s| s.as_str()).collect()
            }
        };
        for from in from_states {
            successors
                .entry(from)
                .or_default()
                .insert(transition.to.as_str());
        }
    }

    // Step 3: Propagate fetched entities forward through state graph (BFS)
    // If state A fetches "Task" and A→B exists, B inherits "Task" as available data
    let mut available_entities: HashMap<&str, HashSet<String>> = fetched_by_state.clone();
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 50 {
        changed = false;
        iterations += 1;
        for transition in &trait_def.state_machine.transitions {
            let from_states = match &transition.from {
                orbital_core::schema::StringOrArray::Single(s) => vec![s.as_str()],
                orbital_core::schema::StringOrArray::Multiple(v) => {
                    v.iter().map(|s| s.as_str()).collect()
                }
            };
            for from in from_states {
                let from_entities: HashSet<String> =
                    available_entities.get(from).cloned().unwrap_or_default();
                if !from_entities.is_empty() {
                    let to_set = available_entities
                        .entry(transition.to.as_str())
                        .or_default();
                    let before = to_set.len();
                    to_set.extend(from_entities);
                    if to_set.len() > before {
                        changed = true;
                    }
                }
            }
        }
    }

    // --- Validate render-ui effects ---
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        if let Some(effects) = &transition.effects {
            // Entities fetched in THIS transition
            let fetched_entities: HashSet<String> =
                effects.iter().filter_map(extract_fetch_entity).collect();

            // Entities available at the target state from predecessor states
            let empty_set = HashSet::new();
            let inherited_entities: &HashSet<String> = available_entities
                .get(transition.to.as_str())
                .unwrap_or(&empty_set);

            let mut rendered_entities: Vec<(String, usize)> = Vec::new();

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

                    if let Some(entity_name) = extract_render_ui_entity(effect) {
                        rendered_entities.push((entity_name, j));
                    }
                }
            }

            // Warn for render-ui entity props without corresponding fetch,
            // BUT suppress if the entity was fetched in a predecessor state
            for (entity_name, j) in &rendered_entities {
                if !fetched_entities.contains(entity_name)
                    && !inherited_entities.contains(entity_name)
                {
                    let effect_path =
                        format!("{}.stateMachine.transitions[{}].effects[{}]", path, i, j);
                    result.add_error(
                        ValidationError::warning(
                            ErrorCode::EffectMissingFetchForEntityPattern,
                            &effect_path,
                            format!(
                                "render-ui references entity '{}' but no [\"fetch\", \"{}\"] effect exists in this or any predecessor transition",
                                entity_name, entity_name
                            ),
                        )
                        .with_suggestion(format!(
                            "Add [\"fetch\", \"{}\"] before the render-ui effect so the pattern has data to display",
                            entity_name
                        ))
                        .with_expected_shape(format!(
                            "[\"fetch\", \"{}\"]",
                            entity_name
                        )),
                    );
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
                    ValidationError::warning(
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

            if let Some(pattern) = arr.get(2) {
                result.merge(validate_pattern(
                    pattern,
                    path,
                    entity_definitions,
                    extra_patterns,
                    linked_entity,
                ));
                // `@trait.X` bindings must only appear in `children:` or
                // `node`-typed props. Walk the pattern tree and warn on
                // misuse elsewhere (see `validate_trait_binding_positions`).
                result.merge(validate_trait_binding_positions(
                    pattern,
                    path,
                    extra_patterns,
                ));
            }
        }
        SExpression::Object(obj) => {
            // Object form: { render_ui: { slot: "main", pattern: { ... } } }
            if let Some(SExpression::Object(rui_obj)) =
                obj.get("render_ui").or_else(|| obj.get("render-ui"))
            {
                if let Some(SExpression::String(slot)) = rui_obj.get("slot") {
                    result.merge(validate_slot(slot, path));
                }

                if let Some(pattern) = rui_obj.get("pattern") {
                    result.merge(validate_pattern(
                        pattern,
                        path,
                        entity_definitions,
                        extra_patterns,
                        linked_entity,
                    ));
                    result.merge(validate_trait_binding_positions(
                        pattern,
                        path,
                        extra_patterns,
                    ));
                }
            }
        }
        _ => {}
    }

    result
}

/// Walk a pattern tree flagging `@trait.*` strings that appear outside
/// permitted positions. Permitted positions:
/// - Element of a `children: [...]` array (any pattern).
/// - Value of a prop whose registry-declared type set includes `node`.
///
/// Emits `BindingTraitInvalidPosition` as a WARNING in v1 — graduates to
/// error once existing schemas have migrated. Scope: one orbital's trait
/// render-ui at a time (the binding is orbital-scoped, §3.8 of
/// `docs/Almadar_Std_Gaps.md`).
fn validate_trait_binding_positions(
    pattern: &SExpression,
    path: &str,
    extra_patterns: Option<&ExtraPatterns>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    walk_pattern_for_trait_positions(pattern, path, None, None, extra_patterns, &mut result);
    result
}

fn walk_pattern_for_trait_positions(
    node: &SExpression,
    path: &str,
    enclosing_pattern_type: Option<&str>,
    enclosing_prop_name: Option<&str>,
    extra_patterns: Option<&ExtraPatterns>,
    result: &mut ValidationResult,
) {
    match node {
        SExpression::String(s) if s.starts_with("@trait.") => {
            // Permitted if we're inside `children:` OR the enclosing prop
            // is declared `node`-typed on the parent pattern.
            let in_children = enclosing_prop_name == Some("children");
            let in_node_prop = match (enclosing_pattern_type, enclosing_prop_name) {
                (Some(pat), Some(prop)) => {
                    // Only core patterns expose typed props today; ExtraPatterns
                    // only tracks prop-name membership, not type. If a future
                    // extension adds type metadata, thread it through here.
                    sync::patterns::get_prop_types(pat, prop)
                        .map(|types| types.contains(&"node"))
                        .unwrap_or(false)
                }
                _ => false,
            };
            // Silence unused-variable warning when `extra_patterns` isn't
            // read inside this branch — the outer walker still uses it for
            // future extensions.
            let _ = extra_patterns;

            if !in_children && !in_node_prop {
                let prop_desc = enclosing_prop_name.unwrap_or("<root>");
                let pattern_desc = enclosing_pattern_type.unwrap_or("<unknown>");
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::BindingTraitInvalidPosition,
                        path,
                        format!(
                            "@trait binding '{}' appears in prop '{}' of pattern '{}'; \
                             allowed positions are `children: [...]` and `node`-typed props",
                            s, prop_desc, pattern_desc
                        ),
                    )
                    .with_suggestion(
                        "Move the @trait reference into a `children:` array, or use \
                         a pattern prop that accepts a node (e.g. `front`, `back`)",
                    ),
                );
            }
        }
        SExpression::Object(obj) => {
            // Read the pattern type at this level so children inherit the
            // correct parent context.
            let new_pat_type = obj.get("type").and_then(|v| {
                if let SExpression::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            });
            for (k, v) in obj {
                if k == "type" {
                    continue;
                }
                walk_pattern_for_trait_positions(
                    v,
                    &format!("{}.{}", path, k),
                    new_pat_type,
                    Some(k.as_str()),
                    extra_patterns,
                    result,
                );
            }
        }
        SExpression::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                walk_pattern_for_trait_positions(
                    item,
                    &format!("{}[{}]", path, i),
                    enclosing_pattern_type,
                    enclosing_prop_name,
                    extra_patterns,
                    result,
                );
            }
        }
        _ => {}
    }
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
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [("type".to_string(), SExpression::String("card".to_string()))]
                    .into_iter()
                    .collect(),
            ),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_invalid_slot() {
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("invalid-slot".to_string()),
            SExpression::Object(
                [("type".to_string(), SExpression::String("card".to_string()))]
                    .into_iter()
                    .collect(),
            ),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderUIInvalidSlot));
    }

    #[test]
    fn test_missing_pattern_type() {
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(HashMap::new()),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderUIMissingPatternType));
    }

    #[test]
    fn test_unknown_item_action_prop() {
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("entity-table".to_string()),
                    ),
                    (
                        "itemActions".to_string(),
                        SExpression::Array(vec![SExpression::Object(
                            [
                                (
                                    "label".to_string(),
                                    SExpression::String("Delete".to_string()),
                                ),
                                (
                                    "event".to_string(),
                                    SExpression::String("DELETE".to_string()),
                                ),
                                (
                                    "condition".to_string(), // Unknown/hallucinated prop
                                    SExpression::String("@entity.canDelete".to_string()),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                        )]),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::RenderUIUnknownItemActionProp));
    }

    #[test]
    fn test_is_render_ui_effect() {
        assert!(is_render_ui_effect(&SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
        ])));

        assert!(is_render_ui_effect(&SExpression::Object(
            [("render_ui".to_string(), SExpression::Null)]
                .into_iter()
                .collect()
        )));

        assert!(!is_render_ui_effect(&SExpression::Array(vec![
            SExpression::String("set".to_string()),
        ])));
    }

    #[test]
    fn test_unknown_pattern_prop() {
        // Test that inventorySlots (non-existent prop) on game-menu triggers warning
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("game-menu".to_string()),
                    ),
                    (
                        "title".to_string(),
                        SExpression::String("Main Menu".to_string()),
                    ),
                    (
                        "inventorySlots".to_string(), // Not a valid prop for game-menu
                        SExpression::Array(vec![]),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        // Should have an error for unknown prop
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidProp),
            "Expected RenderUIInvalidProp error for 'inventorySlots' prop. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_valid_pattern_props() {
        // Test that correct props (options, title) on game-menu pass validation
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("game-menu".to_string()),
                    ),
                    (
                        "title".to_string(),
                        SExpression::String("Main Menu".to_string()),
                    ),
                    (
                        "options".to_string(), // Correct prop name
                        SExpression::Array(vec![]),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

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
            SExpression::String("render-ui".to_string()),
            SExpression::String("filter".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("filter-group".to_string()),
                    ),
                    (
                        "entity".to_string(),
                        SExpression::String("Contact".to_string()),
                    ),
                    ("filters".to_string(), SExpression::Array(vec![])),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

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
        use orbital_core::schema::{
            EntityDefinition, FieldDefinition, FieldType, RelationDefinition,
        };

        // Create mock entity definitions
        let mut entity_defs = HashMap::new();

        // Company entity with 'name' field
        entity_defs.insert(
            "Company".to_string(),
            EntityDefinition {
                name: "Company".to_string(),
                fields: vec![FieldDefinition {
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
                }],
                collection: Some("companies".to_string()),
                persistence: None,
                instances: None,
            },
        );

        // Inspection entity with 'companyId' relation field
        entity_defs.insert(
            "Inspection".to_string(),
            EntityDefinition {
                name: "Inspection".to_string(),
                fields: vec![
                    FieldDefinition {
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
                    },
                    FieldDefinition {
                        name: Some("companyId".to_string()),
                        field_type: FieldType::Relation,
                        required: None,
                        primary_key: None,
                        unique: None,
                        relation: Some(RelationDefinition {
                            entity: "Company".to_string(),
                            cardinality: None,
                        }),
                        items: None,
                        values: None,
                        default: None,
                        properties: None,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                ],
                collection: Some("inspections".to_string()),
                persistence: None,
                instances: None,
            },
        );

        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("entity-cards".to_string()),
                    ),
                    (
                        "entity".to_string(),
                        SExpression::String("Inspection".to_string()),
                    ),
                    (
                        "fields".to_string(),
                        SExpression::Array(vec![
                            SExpression::String("title".to_string()),
                            SExpression::String("company.name".to_string()),
                        ]),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

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

    #[test]
    fn test_nested_field_path_invalid_relation() {
        use orbital_core::schema::{EntityDefinition, FieldDefinition, FieldType};

        // Create mock entity definitions
        let mut entity_defs = HashMap::new();

        // Inspection entity WITHOUT relation field
        entity_defs.insert(
            "Inspection".to_string(),
            EntityDefinition {
                name: "Inspection".to_string(),
                fields: vec![FieldDefinition {
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
                }],
                collection: Some("inspections".to_string()),
                persistence: None,
                instances: None,
            },
        );

        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("entity-cards".to_string()),
                    ),
                    (
                        "entity".to_string(),
                        SExpression::String("Inspection".to_string()),
                    ),
                    (
                        "fields".to_string(),
                        SExpression::Array(vec![SExpression::String("company.name".to_string())]),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);
        // Should have field path warning for missing relation
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::RenderUIInvalidFieldPath),
            "Expected field path warning for invalid relation. Warnings: {:?}",
            result.errors
        );
    }

    /// Helper to build a minimal TraitDefinition with given effects in a single INIT transition
    fn make_trait_with_effects(effects: Vec<SExpression>) -> TraitDefinition {
        use orbital_core::schema::{
            StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };

        TraitDefinition {
            name: "TestTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            linked_entity: Some("Task".to_string()),
            description: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "Browsing".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events: Some(vec![]),
                transitions: vec![TransitionDefinition {
                    from: StringOrArray::Single("Browsing".to_string()),
                    to: "Browsing".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(effects),
                }],
            },
            emits: None,
            listens: None,
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        }
    }

    #[test]
    fn test_missing_fetch_for_entity_pattern() {
        // render-ui with entity prop but NO fetch — should warn
        let trait_def = make_trait_with_effects(vec![SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("entity-table".to_string()),
                    ),
                    (
                        "entity".to_string(),
                        SExpression::String("Task".to_string()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ])]);

        let entity_defs = HashMap::new();
        let result = validate_trait_render_ui(&trait_def, "test", &entity_defs, None);

        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EffectMissingFetchForEntityPattern),
            "Expected EffectMissingFetchForEntityPattern warning. Warnings: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_data_grid_entity_string_literal_rejected() {
        // V-2 fix: `entity: "Tenant"` (bare string) on `data-grid` must error
        // because `data-grid.entity` is typed `["object", "array"]`. A string
        // literal cannot iterate at runtime and silently renders zero rows.
        // The fix should suggest using a binding expression like `@payload.data`.
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("data-grid".to_string()),
                    ),
                    (
                        "entity".to_string(),
                        SExpression::String("Tenant".to_string()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

        // Should report a hard error (not a warning) with a message that
        // mentions binding alternatives.
        let mismatch_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| {
                e.code == ErrorCode::RenderUIPropTypeMismatch
                    && e.message.contains("string literal")
                    && e.message.contains("cannot iterate")
            })
            .collect();

        assert!(
            !mismatch_errors.is_empty(),
            "Expected RenderUIPropTypeMismatch error for bare-string `entity` on data-grid. Errors: {:?}",
            result.errors
        );
        // Must be severity error, not warning — otherwise `orbital validate`
        // exits 0 and authors don't see the problem.
        assert!(
            mismatch_errors
                .iter()
                .any(|e| e.severity == crate::phases::validation::error::Severity::Error),
            "Bare-string array-prop mismatch must be a hard error (severity=Error), not a warning. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_data_grid_entity_binding_accepted() {
        // Sibling check: `entity: "@payload.data"` (binding) must NOT trigger
        // the V-2 rule — bindings resolve at runtime to the actual array shape.
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("data-grid".to_string()),
                    ),
                    (
                        "entity".to_string(),
                        SExpression::String("@payload.data".to_string()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

        assert!(
            !result.errors.iter().any(|e| {
                e.code == ErrorCode::RenderUIPropTypeMismatch
                    && e.message.contains("string literal")
            }),
            "Binding `@payload.data` for data-grid.entity must not trigger the bare-string mismatch rule. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_table_entity_binding_accepted() {
        // `entity-table.entity` is typed `["object", "array"]` in the registry
        // (the resolved record / rows the trait passes after a fetch — see the
        // prop description). A binding expression like `@entity` carries that
        // data, so the V-2 bare-string rule MUST NOT fire on it. (A bare string
        // literal like "Task" IS correctly rejected — that's a separate case.)
        let effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(
                [
                    (
                        "type".to_string(),
                        SExpression::String("entity-table".to_string()),
                    ),
                    (
                        "entity".to_string(),
                        SExpression::String("@entity".to_string()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

        assert!(
            !result.errors.iter().any(|e| {
                e.code == ErrorCode::RenderUIPropTypeMismatch
                    && e.message.contains("string literal")
            }),
            "entity-table.entity = `@entity` is a binding to resolved records; the bare-string rule must not fire. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_fetch_present_for_entity_pattern() {
        // fetch + render-ui with same entity — should NOT warn
        let trait_def = make_trait_with_effects(vec![
            SExpression::Array(vec![
                SExpression::String("fetch".to_string()),
                SExpression::String("Task".to_string()),
            ]),
            SExpression::Array(vec![
                SExpression::String("render-ui".to_string()),
                SExpression::String("main".to_string()),
                SExpression::Object(
                    [
                        (
                            "type".to_string(),
                            SExpression::String("entity-table".to_string()),
                        ),
                        (
                            "entity".to_string(),
                            SExpression::String("Task".to_string()),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ]),
        ]);

        let entity_defs = HashMap::new();
        let result = validate_trait_render_ui(&trait_def, "test", &entity_defs, None);

        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EffectMissingFetchForEntityPattern),
            "Should NOT have warning when fetch is present. Warnings: {:?}",
            result.errors
        );
    }
}
