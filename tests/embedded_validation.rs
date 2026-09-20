//! Integration tests for Phase 3 of Lolo v3 (embedded JSON registries) plus
//! the installed std behavior corpus (Phase 2, `docs/Almadar_Behavior_
//! Packages.md` §10: `@almadar/std` is an installed npm package, not baked).
//!
//! Proves the validator reads core language data (`operators.json`,
//! `patterns-registry.json`) from the embedded registries, the discovered
//! `@almadar/std` package's `.orb` corpus parses and carries the expected
//! atom/organism tiers, plus the hand-ported `BINDING_CONTEXT_RULES`. See
//! `docs/Lolo_Language_Spec_v3_PLAN.md` Section 8.7.

use orbital_compiler::embedded;
use orbital_compiler::phases::validation::sigil_context::validate_trait_sigil_contexts;
use orbital_core::schema::{
    OrbitalSchema, SExpression, StateDefinition, StateMachine, StringOrArray, StringOrNumber,
    TickDefinition, TraitDefinition, TransitionDefinition,
};
use std::path::{Path, PathBuf};

// ============================================================================
// Helpers
// ============================================================================

fn make_trait_with_transition(
    guard: Option<SExpression>,
    effects: Vec<SExpression>,
) -> TraitDefinition {
    TraitDefinition {
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "TestTrait".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        local: false,
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
                event_id: None,
                from: StringOrArray::Single("Browsing".to_string()),
                to: "Browsing".to_string(),
                event: "INIT".to_string(),
                guard,
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
        id: None,
        linked_entity_id: None,
        entity_ref_ids: None,
        trait_embed_ids: None,
    }
}

fn make_trait_with_tick(effects: Vec<SExpression>) -> TraitDefinition {
    let mut t = make_trait_with_transition(None, vec![]);
    t.ticks = Some(vec![TickDefinition {
        name: "test-tick".to_string(),
        interval: StringOrNumber::Number(1000.0),
        guard: None,
        effects,
        applies_to: None,
        description: None,
    }]);
    t
}

// ============================================================================
// 5 operator existence cases (one per category)
// ============================================================================

#[test]
fn op_arithmetic_plus_exists() {
    // arithmetic category
    let op = embedded::get_operator("+").expect("'+' must exist in operators.json");
    assert_eq!(op.category, "arithmetic");
    assert_eq!(op.min_arity, 2);
}

#[test]
fn op_control_let_exists() {
    // control category
    let op = embedded::get_operator("let").expect("'let' must exist in operators.json");
    assert_eq!(op.category, "control");
}

#[test]
fn op_effect_set_exists_with_side_effects() {
    // effect category
    let op = embedded::get_operator("set").expect("'set' must exist in operators.json");
    assert_eq!(op.category, "effect");
    assert!(op.has_side_effects);
}

#[test]
fn op_std_math_floor_exists() {
    // std-math category
    let op =
        embedded::get_operator("math/floor").expect("'math/floor' must exist in operators.json");
    assert_eq!(op.category, "std-math");
}

#[test]
fn op_std_composition_compose_exists() {
    // std-composition category (added in Phase 0.4)
    let op = embedded::get_operator("behavior/compose")
        .expect("'behavior/compose' must exist in operators.json");
    assert_eq!(op.category, "std-composition");
}

// ============================================================================
// 5 pattern existence cases (one per common category)
// ============================================================================

#[test]
fn pattern_display_entity_table_exists() {
    let p = embedded::get_pattern("entity-table")
        .expect("'entity-table' must exist in patterns-registry.json");
    assert_eq!(p.category, "display");
}

#[test]
fn pattern_component_button_exists() {
    let p = embedded::get_pattern("button").expect("'button' must exist in patterns-registry.json");
    // Real category in registry is "interactive"; we just check it parsed.
    assert!(!p.category.is_empty());
}

#[test]
fn pattern_template_exists() {
    // Pick any template-category pattern that ships.
    // The card template is broadly available.
    assert!(embedded::is_known_pattern("card"));
}

#[test]
fn pattern_form_exists() {
    let p = embedded::get_pattern("form").expect("'form' must exist in patterns-registry.json");
    assert_eq!(p.category, "form");
}

#[test]
fn pattern_layout_container_exists() {
    let p = embedded::get_pattern("container")
        .expect("'container' must exist in patterns-registry.json");
    assert!(!p.category.is_empty());
}

// ============================================================================
// 5 sigil context cases (one per legal context, plus violation cases)
// ============================================================================

#[test]
fn sigil_payload_in_effect_passes() {
    let trait_def = make_trait_with_transition(
        None,
        vec![SExpression::Array(vec![
            SExpression::String("set".to_string()),
            SExpression::String("@entity.amount".to_string()),
            SExpression::String("@payload.amount".to_string()),
        ])],
    );
    let result = validate_trait_sigil_contexts(&trait_def, "test");
    assert!(
        result.is_valid(),
        "Effect using @payload should pass. Errors: {:?}",
        result.errors
    );
}

#[test]
fn sigil_state_in_guard_passes() {
    let trait_def = make_trait_with_transition(
        Some(SExpression::Array(vec![
            SExpression::String("=".to_string()),
            SExpression::String("@state".to_string()),
            SExpression::String("Browsing".to_string()),
        ])),
        vec![],
    );
    let result = validate_trait_sigil_contexts(&trait_def, "test");
    assert!(
        result.is_valid(),
        "Guard using @state should pass. Errors: {:?}",
        result.errors
    );
}

#[test]
fn sigil_now_in_tick_passes() {
    let trait_def = make_trait_with_tick(vec![SExpression::Array(vec![
        SExpression::String("set".to_string()),
        SExpression::String("@entity.lastTick".to_string()),
        SExpression::String("@now".to_string()),
    ])]);
    let result = validate_trait_sigil_contexts(&trait_def, "test");
    assert!(
        result.is_valid(),
        "Tick using @now should pass. Errors: {:?}",
        result.errors
    );
}

#[test]
fn sigil_payload_in_tick_is_rejected() {
    let trait_def = make_trait_with_tick(vec![SExpression::Array(vec![
        SExpression::String("set".to_string()),
        SExpression::String("@entity.amount".to_string()),
        SExpression::String("@payload.amount".to_string()),
    ])]);
    let result = validate_trait_sigil_contexts(&trait_def, "test");
    assert!(
        !result.is_valid(),
        "Tick using @payload should fail validation"
    );
    assert!(result.errors.iter().any(|e| e.message.contains("@payload")));
}

#[test]
fn sigil_unknown_root_in_guard_is_rejected() {
    let trait_def = make_trait_with_transition(
        Some(SExpression::Array(vec![
            SExpression::String("=".to_string()),
            SExpression::String("@bogus.field".to_string()),
            SExpression::String("@entity.field".to_string()),
        ])),
        vec![],
    );
    let result = validate_trait_sigil_contexts(&trait_def, "test");
    assert!(
        !result.is_valid(),
        "Guard with unknown sigil root should fail"
    );
}

// ============================================================================
// Installed std corpus validates — `@almadar/std` is an installed npm
// package (Phase 2), not baked; its `.orb` corpus is walked straight off the
// discovered package's registry directory rather than an embedded map.
// ============================================================================

/// Recursively collect every `.orb` file under `dir`.
fn collect_orb_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_orb_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("orb") {
            out.push(path);
        }
    }
}

#[test]
fn installed_std_corpus_validates() {
    let std_pkg = orbital_test_support::installed_package(
        "almadar-std",
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    let mut files = Vec::new();
    collect_orb_files(&std_pkg.registry, &mut files);
    // Corpus is ~469 files at time of writing; floor well below that so a
    // routine publish doesn't red this test, but a wholesale registry loss
    // (e.g. a bad install) still fails loudly.
    assert!(
        files.len() >= 300,
        "installed @almadar/std registry shrank below 300 .orb files — got {} under {}",
        files.len(),
        std_pkg.registry.display()
    );

    let (mut atoms, mut organisms, mut other) = (0u32, 0u32, 0u32);
    for path in &files {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let schema: OrbitalSchema = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("{} failed to parse as OrbitalSchema: {e}", path.display()));
        assert!(
            !schema.name.is_empty(),
            "{} parsed with an empty schema name",
            path.display()
        );

        let rel = path
            .strip_prefix(&std_pkg.registry)
            .unwrap_or(path)
            .to_string_lossy();
        if rel.contains("/organisms/") {
            organisms += 1;
        } else if rel.contains("/atoms/") {
            atoms += 1;
        } else {
            // The single sanctioned molecules exception (`ui/core/molecules`)
            // plus any generated `templates/` — see Almadar_Std.md's
            // atomic-design section (two tiers: atoms + organisms).
            other += 1;
        }
    }
    assert!(atoms > 0, "installed std registry has zero atoms");
    assert!(organisms > 0, "installed std registry has zero organisms");
    let _ = other;
}

// ============================================================================
// 2 cross-registry consistency cases
// ============================================================================

#[test]
fn cross_registry_unknown_operator_is_unknown() {
    assert!(!embedded::is_known_operator("totally-fake-operator-xyz"));
}

#[test]
fn cross_registry_unknown_pattern_is_unknown() {
    assert!(!embedded::is_known_pattern("totally-fake-pattern-xyz"));
}

// ============================================================================
// Bonus: parity test from spec — embedded BINDING_CONTEXT_RULES must match TS
// ============================================================================

#[test]
fn sigil_rules_match_ts_source() {
    // Read the TS source at test time and assert the embedded constant
    // hasn't drifted. If the TS file changes, this test fails until the
    // Rust constant is hand-updated to match. (Phase 3.4 deferred the
    // automated bindings.json artifact to a future phase.)
    let ts_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/baked/bindings.ts");
    let src = std::fs::read_to_string(&ts_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", ts_path.display(), e));

    // Extremely simple substring check rather than a full TS parser.
    // Looks for the canonical allowed-list arrays in BINDING_CONTEXT_RULES.
    assert!(
        src.contains("guard:")
            && src.contains("['entity', 'payload', 'state', 'now', 'config', 'user']"),
        "TS BINDING_CONTEXT_RULES.guard drifted from Rust embedded constant (expected \
         `'entity', 'payload', 'state', 'now', 'config', 'user'` — `user` was added so \
         atoms like std-row-access-control can author role/ownership guards like \
         `when (in @user.role @config.visibleRoles)`)"
    );
    assert!(
        src.contains("effect:")
            && src.contains("'entity', 'payload', 'state', 'now', 'trait', 'config', 'user'"),
        "TS BINDING_CONTEXT_RULES.effect drifted from Rust embedded constant (expected \
         `'entity', 'payload', 'state', 'now', 'trait', 'config', 'user'` — `user` was \
         added alongside the guard rule so role/ownership filters can run in effects \
         (std-row-access-control filters @payload.data by @user.id ownership)"
    );
    assert!(
        src.contains("tick:") && src.contains("['entity', 'state', 'now', 'config', 'user']"),
        "TS BINDING_CONTEXT_RULES.tick drifted from Rust embedded constant (expected \
         `'entity', 'state', 'now', 'config', 'user'` — `user` was added alongside the \
         guard and effect rules for consistent role/ownership access in ticks)"
    );
}
