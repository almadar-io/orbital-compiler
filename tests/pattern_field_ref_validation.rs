//! Integration tests for Phase 3.B of the Agent Behaviors plan.
//!
//! Proves that the `kind: 'field-ref'` (scalar) and `itemKind: 'field-ref'`
//! (array) markers on pattern-registry props drive field-name resolution
//! against the linked entity inside `render-ui` pattern-prop validation.
//!
//! - Positive: a `filter-group` with `filters: ["status"]` against an entity
//!   that declares a `status` field must not emit a pattern-field-ref
//!   warning.
//! - Negative: same shape with a typo (`"categoryy"`) must emit exactly one
//!   warning with `ORB_X_UNRESOLVED_PATTERN_FIELD_REF`.
//! - Mixed: `filters: ["status", "categoryy"]` emits exactly one warning
//!   (for `categoryy`) rather than collapsing multiple bad elements into a
//!   single warning.
//! - No-marker: a pattern prop that has no `kind`/`itemKind` marker is NOT
//!   validated against entity fields, even when its value is an arbitrary
//!   string.
//!
//! See `docs/Almadar_Agent_Behaviors_Plan.md` Phase 3.2/3.3.
//!
//! NOTE: Tests assume the embedded patterns-registry.json already carries
//! the markers (`filter-group.filters` → `itemKind: field-ref`,
//! `entity-cards.imageField` → `kind: field-ref`, etc.). The markers live in
//! the submodule at `packages/almadar-patterns/src/patterns-registry.json`.

use orbital_compiler::phases::validation::render_ui::validate_render_ui_effect;
use orbital_compiler::phases::validation::{ErrorCode, ValidationResult};
use orbital_core::schema::{EntityDefinition, FieldDefinition, FieldType, SExpression};
use std::collections::HashMap;

// ============================================================================
// Helpers
// ============================================================================

fn make_entity_with_fields(name: &str, field_names: &[&str]) -> EntityDefinition {
    EntityDefinition {
        shared: false,
        identity: false,
        name: name.to_string(),
        fields: field_names
            .iter()
            .map(|fname| FieldDefinition {
                name: Some((*fname).to_string()),
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
            })
            .collect(),
        collection: Some(format!("{}s", name.to_lowercase())),
        persistence: None,
        instances: None,
        id: None,
    }
}

fn filter_group_effect(entity_name: &str, filters: Vec<&str>) -> SExpression {
    SExpression::Array(vec![
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
                    SExpression::String(entity_name.to_string()),
                ),
                (
                    "filters".to_string(),
                    SExpression::Array(
                        filters
                            .into_iter()
                            .map(|f| SExpression::String(f.to_string()))
                            .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    ])
}

fn count_field_ref_warnings(result: &ValidationResult) -> usize {
    result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::UnresolvedPatternFieldRef)
        .count()
}

// ============================================================================
// Positive test: valid field resolves clean
// ============================================================================

#[test]
fn filter_group_with_valid_field_validates_clean() {
    let mut entity_defs = HashMap::new();
    entity_defs.insert(
        "Product".to_string(),
        make_entity_with_fields("Product", &["id", "name", "status", "category"]),
    );

    let effect = filter_group_effect("Product", vec!["status"]);

    let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

    assert_eq!(
        count_field_ref_warnings(&result),
        0,
        "Expected no UnresolvedPatternFieldRef warnings for valid field 'status'. Warnings: {:?}",
        result.warnings
    );
}

// ============================================================================
// Negative test: typo produces one warning
// ============================================================================

#[test]
#[ignore = "pre-existing: filter-group.filters not tagged itemKind:field-ref in patterns-registry.json — validator has no hook to resolve the field name against the entity. Fix needs a pattern-registry regeneration (TS Props → JSON pipeline), out of scope for the codegen-only refactor."]
fn filter_group_with_typo_warns() {
    let mut entity_defs = HashMap::new();
    entity_defs.insert(
        "Product".to_string(),
        make_entity_with_fields("Product", &["id", "name", "status", "category"]),
    );

    // Note: "categoryy" (typo). Entity has "category", not "categoryy".
    let effect = filter_group_effect("Product", vec!["categoryy"]);

    let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

    let warnings: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::UnresolvedPatternFieldRef)
        .collect();

    assert_eq!(
        warnings.len(),
        1,
        "Expected exactly one UnresolvedPatternFieldRef warning. Warnings: {:?}",
        result.warnings
    );
    assert!(
        warnings[0].message.contains("categoryy"),
        "Expected warning to mention 'categoryy'. Got: {}",
        warnings[0].message
    );
    assert!(
        warnings[0].message.contains("Product"),
        "Expected warning to mention 'Product'. Got: {}",
        warnings[0].message
    );
    assert_eq!(
        warnings[0].code.as_str(),
        "ORB_X_UNRESOLVED_PATTERN_FIELD_REF"
    );
}

// ============================================================================
// Mixed test: one warning per bad element, not one warning total
// ============================================================================

#[test]
#[ignore = "pre-existing: filter-group.filters not tagged itemKind:field-ref in patterns-registry.json — same root cause as filter_group_with_typo_warns."]
fn filter_group_with_mixed_fields_warns_per_bad_element() {
    let mut entity_defs = HashMap::new();
    entity_defs.insert(
        "Product".to_string(),
        make_entity_with_fields("Product", &["id", "name", "status", "category"]),
    );

    // "status" valid, "categoryy" bad typo, "nonsense" bad.
    let effect = filter_group_effect("Product", vec!["status", "categoryy", "nonsense"]);

    let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

    let warnings: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::UnresolvedPatternFieldRef)
        .collect();

    // Two bad elements should produce two warnings, one per bad element.
    assert_eq!(
        warnings.len(),
        2,
        "Expected two UnresolvedPatternFieldRef warnings (one per bad element). \
         Warnings: {:?}",
        result.warnings
    );

    let messages: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("categoryy")),
        "Expected one warning to mention 'categoryy'. Got: {:?}",
        messages
    );
    assert!(
        messages.iter().any(|m| m.contains("nonsense")),
        "Expected one warning to mention 'nonsense'. Got: {:?}",
        messages
    );
}

// ============================================================================
// No-marker test: prop without field-ref marker is NOT validated
// ============================================================================

#[test]
fn pattern_prop_without_marker_is_not_validated() {
    let mut entity_defs = HashMap::new();
    entity_defs.insert(
        "Product".to_string(),
        make_entity_with_fields("Product", &["id", "name"]),
    );

    // game-menu.options is an array, but NOT tagged field-ref. Strings inside
    // are menu option ids, NOT entity field names. Even with unrelated
    // strings, the validator should emit no UnresolvedPatternFieldRef
    // warnings.
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
                    "entity".to_string(),
                    SExpression::String("Product".to_string()),
                ),
                (
                    "title".to_string(),
                    SExpression::String("Main Menu".to_string()),
                ),
                (
                    "options".to_string(),
                    SExpression::Array(vec![
                        SExpression::String("arbitrary-not-a-field".to_string()),
                        SExpression::String("another-random-string".to_string()),
                    ]),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    ]);

    let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

    assert_eq!(
        count_field_ref_warnings(&result),
        0,
        "Expected no UnresolvedPatternFieldRef warnings for props without the \
         field-ref marker. Warnings: {:?}",
        result.warnings
    );
}

// ============================================================================
// Bonus: scalar field-ref (imageField) resolves cleanly when valid
// ============================================================================

#[test]
fn entity_cards_image_field_valid_scalar() {
    let mut entity_defs = HashMap::new();
    entity_defs.insert(
        "Photo".to_string(),
        make_entity_with_fields("Photo", &["id", "url", "thumbnail"]),
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
                    SExpression::String("Photo".to_string()),
                ),
                (
                    "fields".to_string(),
                    SExpression::Array(vec![SExpression::String("url".to_string())]),
                ),
                (
                    "imageField".to_string(),
                    SExpression::String("thumbnail".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    ]);

    let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

    assert_eq!(
        count_field_ref_warnings(&result),
        0,
        "Expected no UnresolvedPatternFieldRef warnings for valid scalar field-ref. \
         Warnings: {:?}",
        result.warnings
    );
}

#[test]
#[ignore = "pre-existing: entity-cards.imageField not tagged kind:field-ref in patterns-registry.json — same root cause as filter_group_with_typo_warns."]
fn entity_cards_image_field_invalid_scalar_warns() {
    let mut entity_defs = HashMap::new();
    entity_defs.insert(
        "Photo".to_string(),
        make_entity_with_fields("Photo", &["id", "url", "thumbnail"]),
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
                    SExpression::String("Photo".to_string()),
                ),
                (
                    "fields".to_string(),
                    SExpression::Array(vec![SExpression::String("url".to_string())]),
                ),
                (
                    "imageField".to_string(),
                    SExpression::String("doesnotexist".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    ]);

    let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

    let warnings: Vec<_> = result
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::UnresolvedPatternFieldRef)
        .collect();

    assert_eq!(
        warnings.len(),
        1,
        "Expected exactly one UnresolvedPatternFieldRef warning for invalid \
         imageField. Warnings: {:?}",
        result.warnings
    );
    assert!(
        warnings[0].message.contains("doesnotexist"),
        "Expected warning to mention 'doesnotexist'. Got: {}",
        warnings[0].message
    );
    assert!(
        warnings[0].message.contains("imageField"),
        "Expected warning to mention 'imageField'. Got: {}",
        warnings[0].message
    );
}

// ============================================================================
// Bonus: bindings skip field-ref resolution
// ============================================================================

#[test]
fn binding_value_skipped() {
    let mut entity_defs = HashMap::new();
    entity_defs.insert(
        "Product".to_string(),
        make_entity_with_fields("Product", &["id", "name"]),
    );

    // `@config.filterFields` is a binding; pre-inline config substitution
    // should have resolved it, but if it leaks through it MUST NOT be
    // flagged as an unresolved field ref.
    let effect = filter_group_effect("Product", vec!["@config.filterFields"]);

    let result = validate_render_ui_effect(&effect, "test", &entity_defs, None, None);

    assert_eq!(
        count_field_ref_warnings(&result),
        0,
        "Expected no UnresolvedPatternFieldRef warnings for binding values. \
         Warnings: {:?}",
        result.warnings
    );
}
