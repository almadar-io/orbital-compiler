//! Render-UI Effect Validation
//!
//! Validates render_ui effects, slots, patterns, and props.
//! Ported from `builder/packages/shared/src/orbitals/validation/render-ui-validator.ts`.

use super::render_ui_binding::{resolve_binding_type, BindingContext, BindingType};
use crate::embedded::{get_pattern, EmbeddedPatternProp};
use super::sync::patterns::{
    get_pattern_props, get_pattern_valid_props, get_prop_enum_values, get_prop_types,
    is_valid_item_action_prop,
    is_valid_pattern, requires_action, VALID_ITEM_ACTION_PROPS,
};
use super::sync::ExtraPatterns;
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult, VALID_SLOTS};
use orbital_core::schema::{
    EntityDefinition, FieldType, OrbitalSchema, PayloadField, SExpression, TraitDefinition,
    TraitReference,
};
use std::collections::{HashMap, HashSet};

// Pattern types and itemAction props are imported from sync::patterns

/// Validate all render_ui effects in a schema
pub fn validate_all_render_ui(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Validate in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = format!("orbitals[{}].traits[{}]", i, j);
                let result = validate_trait_render_ui(
                    trait_def,
                    &path,
                    &ctx.entity_definitions,
                    ctx.extra_patterns.as_ref(),
                );
                ctx.merge(result);
            }
        }
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
            let fetched: HashSet<String> = effects
                .iter()
                .filter_map(|effect| extract_fetch_entity(effect))
                .collect();
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
            let fetched_entities: HashSet<String> = effects
                .iter()
                .filter_map(|effect| extract_fetch_entity(effect))
                .collect();

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
                ));
                // `@trait.X` bindings must only appear in `children:` or
                // `node`-typed props. Walk the pattern tree and warn on
                // misuse elsewhere (see `validate_trait_binding_positions`).
                result.merge(validate_trait_binding_positions(pattern, path, extra_patterns));
            }
        }
        SExpression::Object(obj) => {
            // Object form: { render_ui: { slot: "main", pattern: { ... } } }
            if let Some(render_ui) = obj.get("render_ui").or_else(|| obj.get("render-ui")) {
                if let SExpression::Object(rui_obj) = render_ui {
                    if let Some(SExpression::String(slot)) = rui_obj.get("slot") {
                        result.merge(validate_slot(slot, path));
                    }

                    if let Some(pattern) = rui_obj.get("pattern") {
                        result.merge(validate_pattern(
                            pattern,
                            path,
                            entity_definitions,
                            extra_patterns,
                        ));
                        result.merge(validate_trait_binding_positions(
                            pattern,
                            path,
                            extra_patterns,
                        ));
                    }
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
                    get_prop_types(pat, prop)
                        .map(|types| types.iter().any(|t| *t == "node"))
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
            let new_pat_type = obj
                .get("type")
                .and_then(|v| if let SExpression::String(s) = v { Some(s.as_str()) } else { None });
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

/// Validate a pattern
fn validate_pattern(
    pattern: &SExpression,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
    extra_patterns: Option<&ExtraPatterns>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match pattern {
        SExpression::Object(obj) => {
            // Check for type property
            if let Some(SExpression::String(pattern_type)) = obj.get("type") {
                // Check core patterns first, then extra patterns
                let in_core = is_valid_pattern(pattern_type);
                let in_extra = extra_patterns
                    .map(|ep| ep.contains_pattern(pattern_type))
                    .unwrap_or(false);

                if !in_core && !in_extra {
                    result.add_error(
                        ValidationError::warning(
                            ErrorCode::RenderUIInvalidPattern,
                            path,
                            format!("Unknown pattern type: '{}'", pattern_type),
                        )
                        .with_suggestion(
                            "Use a known pattern type or ensure custom pattern is registered",
                        ),
                    );
                }

                // Validate pattern props against registry schema
                // Check core props first, then fall back to extra patterns
                let core_props = get_pattern_valid_props(pattern_type);
                let extra_props = extra_patterns.and_then(|ep| ep.get_valid_props(pattern_type));

                if core_props.is_some() || extra_props.is_some() {
                    for prop_name in obj.keys() {
                        // Skip 'type' - it's the pattern identifier, not a prop
                        if prop_name == "type" {
                            continue;
                        }
                        let in_core_props = core_props
                            .map(|p| p.contains(prop_name.as_str()))
                            .unwrap_or(false);
                        let in_extra_props = extra_props
                            .map(|p| p.contains(prop_name.as_str()))
                            .unwrap_or(false);

                        if !in_core_props && !in_extra_props {
                            // Collect valid values from whichever set has them
                            let valid_values: Vec<String> = if let Some(cp) = core_props {
                                cp.iter().map(|s| s.to_string()).collect()
                            } else if let Some(ep) = extra_props {
                                ep.iter().cloned().collect()
                            } else {
                                vec![]
                            };

                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::RenderUIInvalidProp,
                                    path,
                                    format!(
                                        "Unknown prop '{}' for pattern '{}'",
                                        prop_name, pattern_type
                                    ),
                                )
                                .with_valid_values(valid_values)
                                .with_suggestion(format!(
                                    "Check pattern registry for valid props for '{}'",
                                    pattern_type
                                )),
                            );
                        }
                    }
                }

                // Check for missing required props
                if let Some(required) = get_pattern_props(pattern_type) {
                    for req_prop in required {
                        if !obj.contains_key(*req_prop) {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::RenderUIMissingRequiredProp,
                                    path,
                                    format!(
                                        "Pattern '{}' requires prop '{}' but it is not provided",
                                        pattern_type, req_prop
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Add '\"{}\"' to the render-ui config for '{}'",
                                    req_prop, pattern_type
                                )),
                            );
                        }
                    }
                }

                // Validate prop value types against registry
                for (prop_name, prop_value) in obj.iter() {
                    if prop_name == "type" || prop_name == "children" {
                        continue;
                    }
                    if let Some(expected_types) = get_prop_types(pattern_type, prop_name) {
                        let actual_type = classify_prop_type(prop_value);
                        // Skip bindings (@entity.x, @payload.y) - runtime resolves these
                        if actual_type == "binding" {
                            continue;
                        }
                        let expects_array = expected_types.iter().any(|t| *t == "array");
                        let expects_object = expected_types.iter().any(|t| *t == "object");
                        let accepts_string = expected_types.iter().any(|t| *t == "string");
                        // V-2 hard rule: an array-typed prop (e.g. `data-grid.entity`,
                        // `data-grid.fields`) MUST be either an array literal or a
                        // binding expression. A bare string literal like
                        // `entity: "Tenant"` cannot iterate at runtime and silently
                        // renders zero rows — error here so authors see it at validate
                        // time. The `expects_object` co-presence (entity is
                        // `["object", "array"]`) doesn't excuse a string value because
                        // a string can satisfy neither side. We DO exempt patterns
                        // that explicitly list `"string"` in expected_types (e.g.
                        // entity is `["string", "object", "array"]`) — those legitimately
                        // accept an entity-name reference.
                        if expects_array && actual_type == "string" && !accepts_string {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::RenderUIPropTypeMismatch,
                                    path,
                                    format!(
                                        "Prop '{}' on '{}' was a string literal but expected one of: {} — a bare string cannot iterate as an array, use `@payload.X` / `@entity.X` (binding) or an array literal",
                                        prop_name,
                                        pattern_type,
                                        expected_types.join(", ")
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Change '{}' to a binding expression like '@payload.data' or an array literal",
                                    prop_name,
                                )),
                            );
                            continue;
                        }
                        // L2.4 — Element-shape check: when the prop is an
                        // array of objects with per-element required fields,
                        // assert each element of an inline array literal
                        // carries every required field. Catches the
                        // landing-page/StepCard-missing-id class of bugs
                        // before codegen. Pure structural check; runtime
                        // bindings (`@payload.X`) are exempt because their
                        // resolved shape only exists at runtime. Lives BEFORE
                        // the `expects_object` early-out so it still runs on
                        // props whose types are `["object","array"]` (e.g.
                        // `entity` on EntityDisplayProps<T>-derived patterns).
                        if let SExpression::Array(elements) = prop_value {
                            let registry_prop = crate::embedded::get_pattern(pattern_type)
                                .and_then(|p| p.props_schema.get(prop_name));
                            if let Some(rp) = registry_prop {
                                if let Some(items) = rp.items.as_ref() {
                                    let required_from_array = items.required.as_names();
                                    let required_from_props: Vec<String> = items
                                        .properties
                                        .as_ref()
                                        .map(|p| {
                                            p.iter()
                                                .filter(|(_, v)| v.required.as_bool())
                                                .map(|(k, _)| k.clone())
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let mut required_fields: Vec<&str> =
                                        required_from_array.iter().map(|s| s.as_str()).collect();
                                    for r in &required_from_props {
                                        if !required_fields.contains(&r.as_str()) {
                                            required_fields.push(r.as_str());
                                        }
                                    }
                                    if !required_fields.is_empty() {
                                        for (idx, elem) in elements.iter().enumerate() {
                                            if let SExpression::Object(elem_obj) = elem {
                                                for field_name in &required_fields {
                                                    if !elem_obj.contains_key(*field_name) {
                                                        result.add_error(
                                                            ValidationError::error(
                                                                ErrorCode::RenderUIElementShapeMismatch,
                                                                &format!("{}.{}[{}]", path, prop_name, idx),
                                                                format!(
                                                                    "Element [{}] of prop '{}' on '{}' is missing required field '{}' — the pattern's element shape requires it",
                                                                    idx, prop_name, pattern_type, field_name
                                                                ),
                                                            )
                                                            .with_suggestion(format!(
                                                                "Add '{}' to every element of '{}' (e.g. `{{ {}: \"...\", ... }}`)",
                                                                field_name, prop_name, field_name
                                                            )),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Skip if expected is "object" - too broad, custom types resolve
                        // to "object" but often accept strings (e.g., StackDirection).
                        // The array-vs-string mismatch above runs first so this skip
                        // doesn't paper over the V-2 case where `entity` is `["object",
                        // "array"]` and the value is a bare string.
                        if expects_object {
                            continue;
                        }
                        let type_matches = expected_types.iter().any(|t| {
                            *t == actual_type
                                || *t == "node"  // ReactNode accepts any rendered value
                                || *t == "function" // functions can't appear in .orb
                                || *t == "icon" // icon names are strings
                                    && actual_type == "string"
                        });
                        if !type_matches {
                            result.add_error(
                                ValidationError::warning(
                                    ErrorCode::RenderUIPropTypeMismatch,
                                    path,
                                    format!(
                                        "Prop '{}' on '{}' has type '{}' but expected: {}",
                                        prop_name,
                                        pattern_type,
                                        actual_type,
                                        expected_types.join(", ")
                                    ),
                                )
                                .with_suggestion(format!(
                                    "Change '{}' value to match type: {}",
                                    prop_name,
                                    expected_types.join(" or ")
                                )),
                            );
                        }

                    }

                    // Enum-value check: when the prop's TS type was a bounded
                    // string-literal union (e.g. `StackJustify`, `AlertVariant`),
                    // the registry carries the allowed values. Reject any string
                    // literal that isn't in the set. Bindings (`@entity.x`)
                    // resolve at runtime and are skipped.
                    if let Some(allowed) = get_prop_enum_values(pattern_type, prop_name) {
                        if let SExpression::String(s) = prop_value {
                            if !s.starts_with('@') && !allowed.contains(&s.as_str()) {
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::RenderUIPropTypeMismatch,
                                        path,
                                        format!(
                                            "Prop '{}' on '{}' has value '{}' but must be one of: {}",
                                            prop_name,
                                            pattern_type,
                                            s,
                                            allowed.join(", ")
                                        ),
                                    )
                                    .with_suggestion(format!(
                                        "Use one of the allowed values for '{}': {}",
                                        prop_name,
                                        allowed.join(", ")
                                    )),
                                );
                            }
                        }
                    }
                }

                // Validate itemActions if present
                if let Some(item_actions) = obj.get("itemActions") {
                    result.merge(validate_item_actions(item_actions, path));
                }

                // Validate nested field paths in 'fields' prop
                if let Some(fields) = obj.get("fields") {
                    if let Some(SExpression::String(entity_name)) = obj.get("entity") {
                        result.merge(validate_field_paths(
                            fields,
                            entity_name,
                            path,
                            entity_definitions,
                        ));
                    }
                }

                // Validate field-ref markers on pattern props.
                // Scalar props (`kind: 'field-ref'`) and array element props
                // (`itemKind: 'field-ref'`) are resolved against the linked
                // entity's fields. See Phase 3.2/3.3 in
                // docs/Almadar_Agent_Behaviors_Plan.md.
                if let Some(SExpression::String(entity_name)) = obj.get("entity") {
                    if let Some(embedded_pattern) = crate::embedded::get_pattern(pattern_type) {
                        for (prop_name, prop_value) in obj.iter() {
                            if prop_name == "type" || prop_name == "entity" {
                                continue;
                            }
                            let Some(prop_schema) =
                                embedded_pattern.props_schema.get(prop_name)
                            else {
                                continue;
                            };

                            let scalar_is_field_ref = prop_schema
                                .kind
                                .as_deref()
                                .map(|k| k == "field-ref")
                                .unwrap_or(false);
                            let array_is_field_ref = prop_schema
                                .item_kind
                                .as_deref()
                                .map(|k| k == "field-ref")
                                .unwrap_or(false);

                            if scalar_is_field_ref {
                                if let SExpression::String(value) = prop_value {
                                    // Skip bindings (@entity.x, @payload.y,
                                    // @config.x) - the runtime/inline phase
                                    // resolves these.
                                    if value.starts_with('@') {
                                        continue;
                                    }
                                    result.merge(validate_pattern_field_ref(
                                        value,
                                        entity_name,
                                        pattern_type,
                                        prop_name,
                                        path,
                                        entity_definitions,
                                    ));
                                }
                            }

                            if array_is_field_ref {
                                if let SExpression::Array(elements) = prop_value {
                                    for element in elements {
                                        // Only validate string elements; skip
                                        // Column-object forms, objects,
                                        // numbers, and bindings.
                                        let SExpression::String(value) = element else {
                                            continue;
                                        };
                                        if value.starts_with('@') {
                                            continue;
                                        }
                                        result.merge(validate_pattern_field_ref(
                                            value,
                                            entity_name,
                                            pattern_type,
                                            prop_name,
                                            path,
                                            entity_definitions,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }

                // Check for action on button/link patterns using synced definition
                if requires_action(pattern_type)
                    && !obj.contains_key("action")
                    && !obj.contains_key("onClick")
                    && !obj.contains_key("href")
                    && !obj.contains_key("event")
                {
                    result.add_error(
                        ValidationError::warning(
                            ErrorCode::RenderUIMissingAction,
                            path,
                            format!(
                                "{} pattern should have 'action', 'onClick', 'event', or 'href'",
                                pattern_type
                            ),
                        )
                        .with_suggestion("Add action: 'EVENT_NAME' to emit an event on click"),
                    );
                }

                // Recurse into `children` so nested pattern configs
                // (stack-in-dashboard-in-modal chains) get their own prop
                // type + enum-value checks. Children can be a single pattern
                // object or an array of patterns; handle both.
                if let Some(children) = obj.get("children") {
                    match children {
                        SExpression::Array(items) => {
                            for (i, child) in items.iter().enumerate() {
                                let child_path = format!("{}.children[{}]", path, i);
                                result.merge(validate_pattern(
                                    child,
                                    &child_path,
                                    entity_definitions,
                                    extra_patterns,
                                ));
                            }
                        }
                        SExpression::Object(_) => {
                            let child_path = format!("{}.children", path);
                            result.merge(validate_pattern(
                                children,
                                &child_path,
                                entity_definitions,
                                extra_patterns,
                            ));
                        }
                        _ => {}
                    }
                }
            } else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::RenderUIMissingPatternType,
                        path,
                        "Pattern must have a 'type' property",
                    )
                    .with_suggestion("Add type: 'pattern-name' to define the pattern type"),
                );
            }
        }
        SExpression::String(pattern_type) => {
            // `@trait.X` / `@payload.X` / `@entity.X` etc. are binding tokens,
            // not pattern-type names — a separate positional validator
            // (`walk_pattern_for_trait_positions`) enforces where they may
            // appear. Skip the pattern-type check for any binding literal
            // so it does not masquerade as an unknown pattern.
            if pattern_type.starts_with('@') {
                return result;
            }
            // Simple string form for pattern type
            let in_core = is_valid_pattern(pattern_type);
            let in_extra = extra_patterns
                .map(|ep| ep.contains_pattern(pattern_type))
                .unwrap_or(false);
            if !in_core && !in_extra {
                result.add_error(ValidationError::warning(
                    ErrorCode::RenderUIInvalidPattern,
                    path,
                    format!("Unknown pattern type: '{}'", pattern_type),
                ));
            }
        }
        SExpression::Null => {
            // null is valid - it clears/closes the slot
            // e.g., ["render-ui", "modal", null] closes the modal
        }
        _ => {
            result.add_error(ValidationError::error(
                ErrorCode::RenderUIInvalidPattern,
                path,
                "Pattern must be an object, string, or null (to clear slot)",
            ));
        }
    }

    result
}

/// Validate nested field paths in a pattern's `fields` prop
///
/// For field paths like `["company.name", "inspector.fullName"]`:
/// - Validates that the first segment is a relation field on the entity
/// - Validates that subsequent segments exist on the related entity
fn validate_field_paths(
    fields: &SExpression,
    entity_name: &str,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Get the base entity definition
    let Some(base_entity) = entity_definitions.get(entity_name) else {
        // Entity not found - this will be caught by entity validation
        return result;
    };

    // Extract field strings from array
    let field_paths = match fields {
        SExpression::Array(arr) => arr
            .iter()
            .filter_map(|f| {
                if let SExpression::String(s) = f {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>(),
        _ => return result,
    };

    for field_path in field_paths {
        // Skip non-nested paths (no validation needed for simple fields)
        if !field_path.contains('.') {
            continue;
        }

        let segments: Vec<&str> = field_path.split('.').collect();
        if segments.is_empty() {
            continue;
        }

        // First segment should be a relation field on the base entity
        let first_segment = segments[0];

        // Find the relation field (check both "fieldName" and "fieldNameId" forms)
        let relation_field = base_entity.fields.iter().find(|f| {
            if f.field_type != FieldType::Relation {
                return false;
            }
            if let Some(name) = &f.name {
                // Match "company" against "company" or "companyId"
                name == first_segment
                    || name.strip_suffix("Id") == Some(first_segment)
                    || format!("{}Id", first_segment) == *name
            } else {
                false
            }
        });

        let Some(relation_field) = relation_field else {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::RenderUIInvalidFieldPath,
                    path,
                    format!(
                        "Field path '{}' - '{}' has no relation field '{}' to traverse",
                        field_path, entity_name, first_segment
                    ),
                )
                .with_suggestion(format!(
                    "Add a relation field for '{}' or use a direct field reference",
                    first_segment
                )),
            );
            continue;
        };

        // Get the target entity of the relation
        let target_entity_name = relation_field.relation.as_ref().map(|r| r.entity.as_str());

        let Some(target_entity_name) = target_entity_name else {
            // Relation doesn't specify target entity - skip further validation
            continue;
        };

        // For remaining segments (e.g., "name" in "company.name"), validate against target entity
        if segments.len() > 1 {
            let Some(target_entity) = entity_definitions.get(target_entity_name) else {
                // Target entity not found - will be caught by entity validation
                continue;
            };

            // Validate the second segment exists on the target entity
            let second_segment = segments[1];
            let field_exists = target_entity.fields.iter().any(|f| {
                f.name
                    .as_ref()
                    .map(|n| n == second_segment)
                    .unwrap_or(false)
            });

            if !field_exists {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::RenderUIInvalidFieldPath,
                        path,
                        format!(
                            "Field path '{}' - '{}' has no field '{}' on related entity '{}'",
                            field_path, entity_name, second_segment, target_entity_name
                        ),
                    )
                    .with_suggestion(format!(
                        "Check that '{}' entity has a '{}' field",
                        target_entity_name, second_segment
                    )),
                );
            }

            // Note: We only validate 2 levels deep. Deeper nesting (e.g., "company.address.city")
            // would require recursive validation, which is intentionally not implemented
            // to encourage simpler data structures.
            if segments.len() > 2 {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::RenderUIInvalidFieldPath,
                        path,
                        format!(
                            "Field path '{}' has more than 2 levels of nesting (max recommended: 2)",
                            field_path
                        ),
                    )
                    .with_suggestion(
                        "Consider flattening the data structure or using a custom pattern",
                    ),
                );
            }
        }
    }

    result
}

/// Resolve a single field-ref value against the linked entity's fields.
///
/// Emits a `UnresolvedPatternFieldRef` warning if `field_name` does not match
/// any field on `entity_name`. Dotted paths (e.g. `"company.name"`) are left
/// for the existing `validate_field_paths` helper — this function only checks
/// flat field names used by `kind: 'field-ref'` / `itemKind: 'field-ref'`
/// markers.
///
/// Severity is `Warning` per the Phase 3 staging guidance; will graduate to
/// `Error` once scope declarations in Phase 4 land.
fn validate_pattern_field_ref(
    field_name: &str,
    entity_name: &str,
    pattern_type: &str,
    prop_name: &str,
    path: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Dotted paths fall through to the existing nested-field-path validator
    // (invoked separately on the `fields` prop); skip them here.
    if field_name.contains('.') {
        return result;
    }

    // Entity not found -> entity validation will report it; no further work.
    let Some(entity) = entity_definitions.get(entity_name) else {
        return result;
    };

    // Matches when the entity declares a field with the exact name, OR when
    // the entity declares `<name>Id` for a relation (caller can refer to the
    // relation by its logical name without the `Id` suffix, matching the
    // existing convention used in `validate_field_paths`).
    let exists = entity.fields.iter().any(|f| {
        let Some(name) = &f.name else {
            return false;
        };
        name == field_name
            || name.strip_suffix("Id") == Some(field_name)
            || format!("{}Id", field_name) == *name
    });

    if !exists {
        let valid: Vec<String> = entity
            .fields
            .iter()
            .filter_map(|f| f.name.clone())
            .collect();

        result.add_error(
            ValidationError::warning(
                ErrorCode::UnresolvedPatternFieldRef,
                path,
                format!(
                    "Pattern '{}' prop '{}' references field '{}' which does not exist on entity '{}'",
                    pattern_type, prop_name, field_name, entity_name
                ),
            )
            .with_suggestion(format!(
                "Use a field declared on '{}' or add the field to the entity schema",
                entity_name
            ))
            .with_valid_values(valid),
        );
    }

    result
}

/// Validate itemActions array
fn validate_item_actions(item_actions: &SExpression, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let SExpression::Array(actions) = item_actions {
        for (i, action) in actions.iter().enumerate() {
            let action_path = format!("{}.itemActions[{}]", path, i);

            if let SExpression::Object(obj) = action {
                // Check for unknown props (hallucinated by LLM) using synced definition
                for key in obj.keys() {
                    if !is_valid_item_action_prop(key) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::RenderUIUnknownItemActionProp,
                                &action_path,
                                format!("Unknown itemAction prop: '{}'", key),
                            )
                            .with_suggestion("Remove the unknown prop")
                            .with_valid_values(
                                VALID_ITEM_ACTION_PROPS
                                    .iter()
                                    .map(|s| s.to_string())
                                    .collect(),
                            ),
                        );
                    }
                }

                // Check for required event prop
                if !obj.contains_key("event") && !obj.contains_key("href") {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::RenderUIInvalidItemAction,
                            &action_path,
                            "itemAction must have 'event' or 'href' property",
                        )
                        .with_suggestion("Add event: 'EVENT_NAME' to emit an event"),
                    );
                }
            }
        }
    }

    result
}

/// Extract entity name from a fetch effect: ["fetch", "EntityName", ...]
fn extract_fetch_entity(effect: &SExpression) -> Option<String> {
    match effect {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let SExpression::String(op) = &arr[0] {
                if op == "fetch" {
                    if let SExpression::String(entity) = &arr[1] {
                        return Some(entity.clone());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract entity name from a render-ui pattern's `entity` prop
fn extract_render_ui_entity(effect: &SExpression) -> Option<String> {
    // The entity prop accepts EITHER an entity name literal (which needs a
    // sibling fetch) OR a binding ref like `@entity.cards` / `@payload.data`
    // / `@config.foo` (populated by `(set ...)` or carried in a payload).
    // Only the literal form needs the fetch warning; binding refs skip.
    fn is_binding_ref(s: &str) -> bool {
        s.starts_with('@')
    }
    match effect {
        SExpression::Array(arr) if arr.len() >= 3 => {
            if let SExpression::String(op) = &arr[0] {
                if op != "render-ui" && op != "render_ui" {
                    return None;
                }
            }
            // Pattern is the third element
            if let SExpression::Object(obj) = &arr[2] {
                if let Some(SExpression::String(entity)) = obj.get("entity") {
                    if is_binding_ref(entity) {
                        return None;
                    }
                    return Some(entity.clone());
                }
            }
            None
        }
        SExpression::Object(obj) => {
            let render_ui = obj.get("render_ui").or_else(|| obj.get("render-ui"))?;
            if let SExpression::Object(rui_obj) = render_ui {
                if let Some(SExpression::Object(pattern)) = rui_obj.get("pattern") {
                    if let Some(SExpression::String(entity)) = pattern.get("entity") {
                        if is_binding_ref(entity) {
                            return None;
                        }
                        return Some(entity.clone());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Check if an S-expression is a render_ui effect
pub fn is_render_ui_effect(expr: &SExpression) -> bool {
    match expr {
        SExpression::Array(arr) if !arr.is_empty() => {
            if let SExpression::String(op) = &arr[0] {
                op == "render-ui" || op == "render_ui"
            } else {
                false
            }
        }
        SExpression::Object(obj) => obj.contains_key("render_ui") || obj.contains_key("render-ui"),
        _ => false,
    }
}

/// Extract slot from a render_ui effect
fn extract_slot(expr: &SExpression) -> Option<String> {
    match expr {
        SExpression::Array(arr) if arr.len() >= 2 => {
            if let SExpression::String(slot) = &arr[1] {
                Some(slot.clone())
            } else {
                None
            }
        }
        SExpression::Object(obj) => {
            let render_ui = obj.get("render_ui").or_else(|| obj.get("render-ui"))?;
            if let SExpression::Object(rui_obj) = render_ui {
                if let Some(SExpression::String(slot)) = rui_obj.get("slot") {
                    return Some(slot.clone());
                }
            }
            None
        }
        _ => None,
    }
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

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

        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        use orbital_core::schema::FieldDefinition;

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

        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);
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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);

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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);

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
        let result = validate_render_ui_effect(&effect, "test", &entity_defs, None);

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

// =============================================================================
// Prop Type Classification
// =============================================================================

/// Classify an S-expression value into a simple type string for prop type validation.
fn classify_prop_type(value: &SExpression) -> &'static str {
    match value {
        SExpression::String(s) if s.starts_with('@') => "binding",
        SExpression::String(_) => "string",
        SExpression::Number(_) => "number",
        SExpression::Bool(_) => "boolean",
        SExpression::Array(_) => "array",
        SExpression::Object(_) => "object",
        SExpression::Null => "null",
    }
}

// =============================================================================
// L2.1 keystone: render-ui binding type-check
// =============================================================================

/// Top-level entry: walk every trait, every transition's render-ui
/// patterns, and check that each prop binding resolves to a type
/// compatible with the registry's declared prop types.
///
/// Architecture: independent pass run AFTER the existing
/// `validate_all_render_ui` literal check. Adds the binding-resolution
/// dimension that was previously deferred to runtime / TS typecheck —
/// gap #6's primary lift in `docs/Almadar_Std_Verification.md`.
fn validate_all_render_ui_bindings(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Snapshot the entity_definitions reference so the BindingContext
    // borrow doesn't conflict with the &mut ctx used to add errors.
    // Both reads end at the function boundary; cheap reference, no clone.
    let entity_definitions_snapshot = ctx.entity_definitions.clone();

    // Collect diagnostics into a local Vec, then drain into ctx after
    // the inner loops — keeps the borrow checker happy without
    // requiring threading a separate accumulator through.
    let mut diagnostics: Vec<ValidationError> = Vec::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        // Sibling trait names in the orbital, for `@trait.Y` validation.
        let sibling_trait_names: Vec<String> = orbital
            .traits
            .iter()
            .filter_map(|tr| match tr {
                TraitReference::Inline(td) => Some(td.name.clone()),
                _ => None,
            })
            .collect();

        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };

            // Per-trait event → payload schema lookup. Used to resolve
            // `@payload.X` bindings against the listened event's
            // declared payload. Two sources:
            //
            // 1. The trait's own `listens` block: each entry's
            //    `payload_schema` (Almadar trait IR) is the contract for
            //    `@payload.X` reads inside that listen's triggered
            //    transition.
            // 2. The trait's `state_machine.events[]` block: declared
            //    event payloads visible to the transitions that fire on
            //    those events (typed-payload v2 metadata).
            //
            // For a transition firing on event E, the resolver sees the
            // payload from whichever source declares E. If neither
            // declares it, `event_payload` stays None and `@payload.X`
            // resolutions return Unknown — the L2 rule then skips
            // rather than emit a false positive.
            let event_payloads: HashMap<String, Vec<PayloadField>> =
                collect_trait_event_payloads(trait_def);

            for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
                let event_payload = event_payloads.get(&transition.event).map(|v| v.as_slice());
                let bctx = BindingContext {
                    trait_def,
                    entity_definitions: &entity_definitions_snapshot,
                    event_payload,
                    sibling_trait_names: &sibling_trait_names,
                };

                if let Some(effects) = &transition.effects {
                    for (j, effect) in effects.iter().enumerate() {
                        let path = format!(
                            "orbitals[{}].traits[{}].stateMachine.transitions[{}].effects[{}]",
                            oi, ti, i, j
                        );
                        check_render_ui_bindings_in_effect(effect, &bctx, &path, &mut diagnostics);
                    }
                }
            }
        }
    }

    for d in diagnostics {
        ctx.add_error(d);
    }
}

/// Build the per-event payload-schema map for a trait.
///
/// Pulls from `state_machine.events[]` — typed-payload v2 metadata that
/// declares payloads for the events the trait fires on its transitions.
/// Listen entries don't carry a separate `payload_schema` field on the
/// IR type (they have `payload_mapping` for cross-event field rewires
/// only); the canonical payload contract lives on the matching events
/// declaration.
fn collect_trait_event_payloads(
    trait_def: &TraitDefinition,
) -> HashMap<String, Vec<PayloadField>> {
    let mut map: HashMap<String, Vec<PayloadField>> = HashMap::new();
    if let Some(events) = &trait_def.state_machine.events {
        for evt in events {
            if let Some(payload) = &evt.payload_schema {
                map.insert(evt.key.clone(), payload.clone());
            }
        }
    }
    map
}

/// Recursively descend into an effect SExpression, looking for
/// `["render-ui", "<slot>", <pattern>]` calls. When found, walk the
/// pattern tree and check each prop binding.
fn check_render_ui_bindings_in_effect(
    effect: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    if let SExpression::Array(arr) = effect {
        if let Some(SExpression::String(op)) = arr.first() {
            if op == "render-ui" {
                if let Some(pattern) = arr.get(2) {
                    walk_pattern_props(pattern, bctx, path, diagnostics);
                }
            }
        }
        for item in arr {
            check_render_ui_bindings_in_effect(item, bctx, path, diagnostics);
        }
    } else if let SExpression::Object(obj) = effect {
        for value in obj.values() {
            check_render_ui_bindings_in_effect(value, bctx, path, diagnostics);
        }
    }
}

/// Walk a render-ui pattern's prop tree. For every prop value that's a
/// binding (`@`-prefixed string) or an SExpression operator call, ask
/// F3's resolver for the type and compare against the prop's declared
/// types in the registry.
///
/// Recurses into nested patterns (children arrays of stack/dashboard-
/// layout, etc.) so deep render-ui trees are covered without needing
/// per-pattern dispatch.
fn walk_pattern_props(
    pattern: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let SExpression::Object(obj) = pattern else {
        return;
    };
    let pattern_type = match obj.get("type") {
        Some(SExpression::String(t)) => Some(t.as_str()),
        _ => None,
    };

    for (prop_name, prop_value) in obj.iter() {
        if prop_name == "type" {
            continue;
        }

        // Recurse first so nested patterns get checked regardless of
        // whether the parent prop has a registry type.
        recurse_into_value(prop_value, bctx, path, diagnostics);

        // Only check props with declared types in the registry.
        let Some(pat) = pattern_type else { continue };

        // L2.2 first: when the prop is a `kind: "event-ref"` /
        // `kind: "callback"` event-name reference and the value is a
        // literal string event name, compare the registry's documented
        // bus payload (emitPayloadSchema or callbackArgs) against the
        // trait's declared `emits { EVENT { ... } }` payload. Done
        // BEFORE the L2.1 type-check because event-ref props are
        // typed as `string` in the registry, which the literal-pass
        // already accepts — but the structural payload-shape compare
        // is the deeper contract.
        if let SExpression::String(event_name) = prop_value {
            if !event_name.starts_with('@') {
                check_event_binding_payload(
                    pat,
                    prop_name,
                    event_name,
                    bctx,
                    path,
                    diagnostics,
                );
                // L2.3: when the literal matches a known event-bearing
                // prop in the registry but the event isn't declared on
                // the trait, the literal is stale — likely a leftover
                // from an events-override rename that the compiler's
                // C1 inline rewrite missed. Gap #30.
                check_event_literal_stale(
                    pat,
                    prop_name,
                    event_name,
                    bctx,
                    path,
                    diagnostics,
                );
            }
        }

        let Some(expected_types) = get_prop_types(pat, prop_name) else {
            continue;
        };

        // Skip the literal-only check that's already performed by the
        // existing `validate_pattern` pass.
        let actual_kind = classify_prop_type(prop_value);
        if actual_kind != "binding" && !matches!(prop_value, SExpression::Array(_)) {
            continue;
        }

        // Skip when expected is "object" — too broad; the existing rule
        // already documents this as deferred.
        if expected_types.iter().any(|t| *t == "object") {
            continue;
        }
        // Skip event-name bindings (kind: event-ref / callback / etc.) —
        // L2.2 covers those via emit-payload schema compare. Here we
        // only care about value-typed bindings.
        if expected_types.iter().any(|t| *t == "function") {
            continue;
        }

        // Literal array short-circuit: when the expected type is `array`
        // and the value is a literal `SExpression::Array(...)`, accept
        // it as an array regardless of what `resolve_binding_type` would
        // return. Without this, an array of bare-string field names
        // like `["path", "method", "backend"]` gets parsed as the
        // `path` canonical operator (returnType "string") and trips a
        // false positive. Field names commonly collide with operator
        // names; the surrounding prop's declared type is the
        // disambiguator.
        if expected_types.iter().any(|t| *t == "array")
            && matches!(prop_value, SExpression::Array(_))
        {
            continue;
        }

        let resolved = resolve_binding_type(prop_value, bctx);
        if matches!(
            resolved,
            BindingType::Unknown | BindingType::TraitRef
        ) {
            // Resolver couldn't determine the type (cross-orbital
            // payload, missing alias, dynamic field name, or sentinel
            // trait ref). Skip — emitting a false-positive here is
            // costly when ~half of bindings still resolve to Unknown
            // pending fuller IR coverage.
            continue;
        }
        let resolved_str = resolved.registry_type();
        // `entity` resolves to "object" — the prop's declared types
        // accept that via the existing skip-on-object rule.

        let compatible = expected_types.iter().any(|t| {
            *t == resolved_str
                || *t == "node"   // any rendered value is a ReactNode
                || *t == "string" && resolved_str == "trait"  // @trait.X used as label fallback
                || (*t == "icon" && resolved_str == "string")
        });
        if !compatible {
            diagnostics.push(
                ValidationError::error(
                    ErrorCode::RenderUIBindingTypeMismatch,
                    path,
                    format!(
                        "Prop '{}' on '{}' is bound to a value of resolved type '{}' but the registry expects: {}",
                        prop_name,
                        pat,
                        resolved_str,
                        expected_types.join(", ")
                    ),
                )
                .with_suggestion(format!(
                    "Bind '{}' to a value whose resolved type matches: {}",
                    prop_name,
                    expected_types.join(" or ")
                )),
            );
        }
    }
}

/// L2.2: when a render-ui prop with `kind: "event-ref"` (declarative
/// bus emit) or `kind: "callback"` (React callback wrapped by codegen)
/// is bound to an event-name string, compare the registry's documented
/// bus payload shape against the trait's declared
/// `emits { EVENT { ... } }` payload. Mismatch → diagnostic with
/// pattern + trait + event side-by-side.
///
/// Two flavours:
///
/// - **`kind: "event-ref"`** (e.g. `tabChangeEvent: "TAB_CHANGED"`):
///   the molecule emits `eventBus.emit('UI:{prop}', emitPayloadSchema)`.
///   The trait's declared payload for `TAB_CHANGED` must include every
///   required field in `emitPayloadSchema`.
///
/// - **`kind: "callback"`** (e.g. `onTabChange: "TAB_CHANGED"`):
///   React invokes the callback with positional args; codegen
///   synthesises `dispatch('TAB_CHANGED', { name1: arg1, name2: arg2 })`
///   from the registry's `callbackArgs` (per C2 plan). The trait's
///   declared payload must contain a field for every callbackArg by
///   name and compatible type.
///
/// Bindings (`@payload.X` / etc.) skip — the prop value isn't a static
/// event name. Empty `emits[]` or no payload contract on the trait
/// also skip (no constraint to compare against).
fn check_event_binding_payload(
    pattern_type: &str,
    prop_name: &str,
    event_name: &str,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let Some(pattern) = get_pattern(pattern_type) else {
        return;
    };
    let Some(prop_meta): Option<&EmbeddedPatternProp> = pattern.props_schema.get(prop_name) else {
        return;
    };

    // Only event-ref / callback props carry the metadata we need. The
    // legacy `event` kind is treated as `event-ref` for back-compat.
    let kind = prop_meta.kind.as_deref().unwrap_or("");
    let is_event_ref = matches!(kind, "event" | "event-ref" | "event-listen");
    let is_callback = kind == "callback";
    if !is_event_ref && !is_callback {
        return;
    }

    // Find the trait's declared payload for this event. Walk emits[]
    // first (the canonical declaration site for produced events); the
    // event must be declared somewhere on the trait or its parent for
    // the comparison to make sense.
    let declared_payload: Option<Vec<orbital_core::schema::PayloadField>> = bctx
        .trait_def
        .emits
        .as_deref()
        .and_then(|emits| emits.iter().find(|e| e.event == event_name))
        .and_then(|e| e.payload_schema.clone())
        .or_else(|| {
            bctx.trait_def
                .state_machine
                .events
                .as_ref()
                .and_then(|evts| evts.iter().find(|e| e.key == event_name))
                .and_then(|e| e.payload_schema.clone())
        });
    let Some(declared) = declared_payload else {
        // Event isn't declared on this trait. The L1.1 reachability
        // check already catches typos at parse; here we skip rather
        // than re-emit. A fully-undeclared event surfacing only at L2
        // is rare in practice.
        return;
    };

    if is_event_ref {
        // Compare emitPayloadSchema (registry) vs declared payload.
        let Some(emit_schema) = &prop_meta.emit_payload_schema else {
            return; // legacy `event` kind without emitPayloadSchema
        };
        for required_field in emit_schema.iter().filter(|f| f.required) {
            let trait_field = declared.iter().find(|p| p.name == required_field.name);
            match trait_field {
                None => {
                    diagnostics.push(
                        ValidationError::error(
                            ErrorCode::RenderUIEventBindingPayloadMismatch,
                            path,
                            format!(
                                "Prop '{}' on '{}' emits 'UI:{}' with payload '{}: {}' but trait's `emits {{ {} {{ ... }} }}` doesn't declare '{}'",
                                prop_name, pattern_type, event_name,
                                required_field.name, required_field.r#type,
                                event_name, required_field.name,
                            ),
                        )
                        .with_suggestion(format!(
                            "Add `{} : {}` to the trait's `emits {{ {} {{ ... }} }}` payload",
                            required_field.name, required_field.r#type, event_name
                        )),
                    );
                }
                Some(declared_field) => {
                    if !primitive_event_compat(
                        &declared_field.field_type,
                        &required_field.r#type,
                    ) {
                        diagnostics.push(
                            ValidationError::warning(
                                ErrorCode::RenderUIEventBindingPayloadMismatch,
                                path,
                                format!(
                                    "Prop '{}' on '{}' emits 'UI:{}' with '{}: {}' but trait declares '{} : {}'",
                                    prop_name, pattern_type, event_name,
                                    required_field.name, required_field.r#type,
                                    declared_field.name, declared_field.field_type,
                                ),
                            )
                            .with_suggestion(format!(
                                "Reconcile the type — the component emits '{}' as {} but the trait declares it as {}",
                                required_field.name, required_field.r#type, declared_field.field_type,
                            )),
                        );
                    }
                }
            }
        }
    } else if is_callback {
        // Compare callbackArgs (registry) vs declared payload by name.
        let Some(args) = &prop_meta.callback_args else {
            return;
        };
        for arg in args.iter() {
            let trait_field = declared.iter().find(|p| p.name == arg.name);
            match trait_field {
                None => {
                    diagnostics.push(
                        ValidationError::error(
                            ErrorCode::RenderUIEventBindingPayloadMismatch,
                            path,
                            format!(
                                "Callback prop '{}' on '{}' will dispatch '{}' with positional arg '{}: {}' (codegen wraps to {{ {} }}) but trait's payload for '{}' has no '{}' field",
                                prop_name, pattern_type, event_name,
                                arg.name, arg.r#type, arg.name,
                                event_name, arg.name,
                            ),
                        )
                        .with_suggestion(format!(
                            "Either rename '{}' to match a trait payload field, or add `{} : {}` to the trait's `emits {{ {} {{ ... }} }}` payload",
                            arg.name, arg.name, arg.r#type, event_name,
                        )),
                    );
                }
                Some(declared_field) => {
                    if !primitive_event_compat(&declared_field.field_type, &arg.r#type) {
                        diagnostics.push(
                            ValidationError::warning(
                                ErrorCode::RenderUIEventBindingPayloadMismatch,
                                path,
                                format!(
                                    "Callback prop '{}' on '{}' arg '{}' is type '{}' but trait declares payload field '{}' as '{}'",
                                    prop_name, pattern_type, arg.name, arg.r#type,
                                    declared_field.name, declared_field.field_type,
                                ),
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// L2.3: when a render-ui pattern carries a string literal at an
/// event-bearing prop key, but that string isn't declared anywhere in
/// the trait's event set (`emits[]` + `state_machine.events[]` +
/// `listens[]`), the literal is stale. Most common cause: a trait-ref
/// used `events: { OLD: NEW }` to rename an imported atom's events, but
/// the compiler's inline phase didn't rewrite the literal inside the
/// atom's render-ui pattern (gap #30 / C1 bug).
///
/// Skips when the prop isn't an event-bearing kind (event / event-ref /
/// event-listen / callback / event-list-eventField). Skips when the
/// trait's event set is empty (no constraint to compare).
///
/// Also surfaces non-events-override stale literals: an author rename
/// from `OPEN` → `START` on a trait but missing the rewrite at one
/// render-ui call site lands the same diagnostic.
fn check_event_literal_stale(
    pattern_type: &str,
    prop_name: &str,
    event_name: &str,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    let Some(pattern) = get_pattern(pattern_type) else {
        return;
    };
    let Some(prop_meta) = pattern.props_schema.get(prop_name) else {
        return;
    };
    let kind = prop_meta.kind.as_deref().unwrap_or("");
    let is_event_bearing =
        matches!(kind, "event" | "event-ref" | "event-listen" | "callback");
    if !is_event_bearing {
        return;
    }

    // Build the trait's full event set: every event the trait declares
    // as emit, listens for, or transitions on. A literal that matches
    // none of these is stale.
    let mut declared_events: std::collections::HashSet<&str> =
        std::collections::HashSet::new();
    if let Some(emits) = &bctx.trait_def.emits {
        for emit in emits {
            declared_events.insert(emit.event.as_str());
        }
    }
    if let Some(listens) = &bctx.trait_def.listens {
        for listen in listens {
            declared_events.insert(listen.event.as_str());
            if let Some(trigger) = &listen.triggers {
                declared_events.insert(trigger.as_str());
            }
        }
    }
    if let Some(events) = &bctx.trait_def.state_machine.events {
        for evt in events {
            declared_events.insert(evt.key.as_str());
        }
    }
    for tr in &bctx.trait_def.state_machine.transitions {
        declared_events.insert(tr.event.as_str());
    }

    if declared_events.is_empty() {
        // Vacuous compare — skip.
        return;
    }
    if !declared_events.contains(event_name) {
        diagnostics.push(
            ValidationError::error(
                ErrorCode::RenderUIEventLiteralStale,
                path,
                format!(
                    "Prop '{}' on '{}' references event '{}' but the trait's event set has no such key — literal is stale (likely missed by an `events: {{ OLD: NEW }}` override rewrite, or an authoring rename that didn't reach this call site)",
                    prop_name, pattern_type, event_name,
                ),
            )
            .with_suggestion(format!(
                "Either add '{}' to the trait's `emits` / `listens` / transition events, or rewrite the literal to match an existing event key",
                event_name
            )),
        );
    }
}

/// Lenient primitive compatibility for event-payload comparisons. Same
/// vocabulary as the L1.3/L2 emit_payload checks: `any` matches; date /
/// datetime / timestamp accept string|number; entity / relation accept
/// object|string; named entity types accept object.
fn primitive_event_compat(declared: &str, actual: &str) -> bool {
    let d = declared.trim_end_matches('!');
    let a = actual.trim_end_matches('!');
    if d == a {
        return true;
    }
    if (d == "array" || d.starts_with('[')) && (a == "array" || a.starts_with('[')) {
        return true;
    }
    match d {
        "any" => true,
        "date" | "datetime" | "timestamp" => a == "string" || a == "number",
        "entity" | "relation" => a == "object" || a == "string",
        s if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
            a == "object" || a == "string"
        }
        _ => false,
    }
}

/// Recurse into prop values that may carry nested render-ui patterns
/// (children arrays, content slots, items, etc.).
fn recurse_into_value(
    value: &SExpression,
    bctx: &BindingContext,
    path: &str,
    diagnostics: &mut Vec<ValidationError>,
) {
    match value {
        SExpression::Object(_) => walk_pattern_props(value, bctx, path, diagnostics),
        SExpression::Array(arr) => {
            for item in arr {
                recurse_into_value(item, bctx, path, diagnostics);
            }
        }
        _ => {}
    }
}
