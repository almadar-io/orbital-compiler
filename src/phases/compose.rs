//! Compose Phase
//!
//! Resolves `behavior/compose`, `behavior/wire`, `behavior/detect-layout`,
//! and `behavior/pipe` calls embedded in `.orb` programs.
//!
//! This phase runs between `Inline` and `Resolve`. It walks every
//! `SExpression` location in the schema (transition effects, tick effects,
//! and the orbital body itself when the body was produced by a top-level
//! `behavior/compose` call) and replaces unresolved composition calls with
//! their resolved value.
//!
//! The actual composition algorithms live in
//! `crate::composition::{compose_behaviors, event_wiring, layout_strategy, pipe}`,
//! which are faithful Rust ports of `packages/almadar-core/src/builders/`.
//! This phase is a thin orchestrator that walks the schema and dispatches.

use crate::composition::{
    compose_behaviors, ComposeBehaviorsInput, ComposeError, EventWiringEntry,
};
use crate::phases::sexpr_json::sexpr_to_json;
use orbital_core_base::schema::{
    Orbital, OrbitalSchema, SExpression, StateMachine, TickDefinition, TraitDefinition,
    TraitReference, TransitionDefinition,
};

// ============================================================================
// Config
// ============================================================================

/// Compose phase configuration.
#[derive(Debug, Clone, Default)]
pub struct ComposeConfig {
    /// Skip the compose phase entirely (used by tests).
    pub skip_compose: bool,
}

// ============================================================================
// Public API
// ============================================================================

/// Resolve all `behavior/*` composition calls in the schema. Mutates the
/// schema in place.
pub fn compose_schema(
    schema: &mut OrbitalSchema,
    config: &ComposeConfig,
) -> Result<(), ComposeError> {
    if config.skip_compose {
        return Ok(());
    }

    // Walk every effect location in the schema.
    for orbital in schema.orbitals.iter_mut() {
        compose_orbital(orbital)?;
    }

    Ok(())
}

// ============================================================================
// Walkers
// ============================================================================

fn compose_orbital(orbital: &mut Orbital) -> Result<(), ComposeError> {
    for trait_ref in orbital.traits.iter_mut() {
        if let TraitReference::Inline(trait_def) = trait_ref {
            compose_trait(trait_def)?;
        }
    }
    Ok(())
}

fn compose_trait(trait_def: &mut TraitDefinition) -> Result<(), ComposeError> {
    compose_state_machine(&mut trait_def.state_machine)?;
    if let Some(ticks) = trait_def.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            compose_tick(tick)?;
        }
    }
    Ok(())
}

fn compose_state_machine(sm: &mut StateMachine) -> Result<(), ComposeError> {
    for transition in sm.transitions.iter_mut() {
        compose_transition(transition)?;
    }
    Ok(())
}

fn compose_transition(transition: &mut TransitionDefinition) -> Result<(), ComposeError> {
    if let Some(effects) = transition.effects.as_mut() {
        compose_effects(effects)?;
    }
    Ok(())
}

fn compose_tick(tick: &mut TickDefinition) -> Result<(), ComposeError> {
    compose_effects(&mut tick.effects)
}

/// Walk a list of effect s-expressions, recursively replacing any embedded
/// `behavior/*` calls with their resolved value.
fn compose_effects(effects: &mut [SExpression]) -> Result<(), ComposeError> {
    for effect in effects.iter_mut() {
        resolve_in_place(effect)?;
    }
    Ok(())
}

/// Recursively walk an s-expression and resolve any `behavior/*` calls in
/// place. The traversal is depth-first so nested compositions resolve
/// inside-out.
fn resolve_in_place(expr: &mut SExpression) -> Result<(), ComposeError> {
    // Recurse first.
    match expr {
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                resolve_in_place(item)?;
            }
        }
        SExpression::Object(map) => {
            for (_, v) in map.iter_mut() {
                resolve_in_place(v)?;
            }
        }
        _ => {}
    }

    // Now check whether this expression itself is a behavior call.
    if let Some(replacement) = try_resolve_behavior_call(expr)? {
        *expr = replacement;
    }

    Ok(())
}

/// If `expr` is a `behavior/*` call, evaluate it and return the resolved
/// value. Otherwise return `None`.
fn try_resolve_behavior_call(expr: &SExpression) -> Result<Option<SExpression>, ComposeError> {
    let arr = match expr {
        SExpression::Array(a) if !a.is_empty() => a,
        _ => return Ok(None),
    };
    let op = match arr.first().and_then(|e| e.as_str()) {
        Some(s) => s,
        None => return Ok(None),
    };

    match op {
        "behavior/compose" => Ok(Some(resolve_compose(&arr[1..])?)),
        "behavior/wire" => Ok(Some(resolve_wire(&arr[1..])?)),
        "behavior/detect-layout" => Ok(Some(resolve_detect_layout(&arr[1..])?)),
        "behavior/pipe" => Ok(Some(crate::composition::pipe_behaviors(&arr[1..]))),
        _ => Ok(None),
    }
}

// ============================================================================
// Behavior call evaluators
// ============================================================================

fn resolve_compose(args: &[SExpression]) -> Result<SExpression, ComposeError> {
    let arg = args
        .first()
        .ok_or_else(|| ComposeError::MissingArgument("behavior/compose".to_string()))?;
    let json = sexpr_to_json(arg);
    let input: ComposeBehaviorsInput = serde_json::from_value(json)?;
    let result = compose_behaviors(input)?;
    Ok(json_to_sexpr(&result.schema))
}

fn resolve_wire(args: &[SExpression]) -> Result<SExpression, ComposeError> {
    let orbitals_arg = args
        .first()
        .ok_or_else(|| ComposeError::MissingArgument("behavior/wire orbitals".to_string()))?;
    let wiring_arg = args
        .get(1)
        .ok_or_else(|| ComposeError::MissingArgument("behavior/wire wiring".to_string()))?;

    let orbitals: Vec<serde_json::Value> = serde_json::from_value(sexpr_to_json(orbitals_arg))?;
    let wiring: Vec<EventWiringEntry> = serde_json::from_value(sexpr_to_json(wiring_arg))?;

    // Drive the typed `apply_event_wiring` indirectly: route through
    // `compose_behaviors` so the JSON-aware wiring helper runs and we get
    // back a schema. Then extract the `orbitals` array from the schema. The
    // schema-wrapper fields (name, version) are discarded — `behavior/wire`
    // returns just the array, matching the TS contract.
    let input = ComposeBehaviorsInput {
        app_name: "__wire_only__".to_string(),
        orbitals,
        layout_strategy: None,
        event_wiring: Some(wiring),
        entity_mappings: None,
    };
    let result = compose_behaviors(input)?;
    let wired = result
        .schema
        .get("orbitals")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    Ok(json_to_sexpr(&wired))
}

fn resolve_detect_layout(args: &[SExpression]) -> Result<SExpression, ComposeError> {
    let orbitals_arg = args.first().ok_or_else(|| {
        ComposeError::MissingArgument("behavior/detect-layout orbitals".to_string())
    })?;
    let orbitals: Vec<serde_json::Value> = serde_json::from_value(sexpr_to_json(orbitals_arg))?;
    let wiring: Option<Vec<EventWiringEntry>> = match args.get(1) {
        Some(w) => {
            let json = sexpr_to_json(w);
            if json.is_null() {
                None
            } else {
                Some(serde_json::from_value(json)?)
            }
        }
        None => None,
    };
    let strategy = crate::composition::layout_strategy::detect_layout_strategy_from_values(
        &orbitals,
        wiring.as_deref(),
    );
    let json = serde_json::to_value(strategy)?;
    Ok(json_to_sexpr(&json))
}

// ============================================================================
// SExpression <-> serde_json::Value bridge
// ============================================================================

fn json_to_sexpr(value: &serde_json::Value) -> SExpression {
    serde_json::from_value(value.clone()).unwrap_or(SExpression::Null)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-schemas/composition")
    }

    fn load_fixture_input(name: &str) -> serde_json::Value {
        let path = fixtures_dir().join(format!("{}.orb", name));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing fixture: {}", path.display()));
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("fixture is valid JSON");
        parsed
            .get("input")
            .cloned()
            .unwrap_or_else(|| panic!("fixture missing `input` key: {}", name))
    }

    fn load_fixture_golden(name: &str) -> serde_json::Value {
        let path = fixtures_dir().join(format!("{}.golden.json", name));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing golden: {}", path.display()));
        serde_json::from_str(&raw).expect("golden is valid JSON")
    }

    fn run_compose(input: serde_json::Value) -> serde_json::Value {
        let parsed: ComposeBehaviorsInput =
            serde_json::from_value(input).expect("valid compose input");
        let result = compose_behaviors(parsed).expect("compose succeeded");
        result.schema
    }

    #[test]
    fn fixture_01_single_orbital() {
        let input = load_fixture_input("01-single-orbital");
        let golden = load_fixture_golden("01-single-orbital");
        let actual = run_compose(input);
        assert_eq!(
            serde_json::to_string_pretty(&actual).unwrap(),
            serde_json::to_string_pretty(&golden).unwrap()
        );
    }

    #[test]
    fn fixture_02_two_orbitals() {
        let input = load_fixture_input("02-two-orbitals");
        let golden = load_fixture_golden("02-two-orbitals");
        let actual = run_compose(input);
        assert_eq!(
            serde_json::to_string_pretty(&actual).unwrap(),
            serde_json::to_string_pretty(&golden).unwrap()
        );
    }

    #[test]
    fn fixture_03_with_wiring() {
        let input = load_fixture_input("03-with-wiring");
        let golden = load_fixture_golden("03-with-wiring");
        let actual = run_compose(input);
        assert_eq!(
            serde_json::to_string_pretty(&actual).unwrap(),
            serde_json::to_string_pretty(&golden).unwrap()
        );
    }

    #[test]
    fn fixture_04_wizard_chain() {
        let input = load_fixture_input("04-wizard-chain");
        let golden = load_fixture_golden("04-wizard-chain");
        let actual = run_compose(input);
        assert_eq!(
            serde_json::to_string_pretty(&actual).unwrap(),
            serde_json::to_string_pretty(&golden).unwrap()
        );
    }

    #[test]
    fn fixture_05_five_orbitals() {
        let input = load_fixture_input("05-five-orbitals");
        let golden = load_fixture_golden("05-five-orbitals");
        let actual = run_compose(input);
        assert_eq!(
            serde_json::to_string_pretty(&actual).unwrap(),
            serde_json::to_string_pretty(&golden).unwrap()
        );
    }

    #[test]
    fn skip_compose_is_no_op() {
        let mut schema = OrbitalSchema {
            theme_definitions: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: Vec::new(),
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    os_grants: None,
        };
        let config = ComposeConfig { skip_compose: true };
        let result = compose_schema(&mut schema, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn empty_schema_is_a_no_op() {
        let mut schema = OrbitalSchema {
            theme_definitions: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "EmptyApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: Vec::new(),
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    os_grants: None,
        };
        let config = ComposeConfig::default();
        let result = compose_schema(&mut schema, &config);
        assert!(result.is_ok());
    }
}
