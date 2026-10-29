// `orbital_core::effects::runs_on` must agree with the baked registry's `runsOn` for every operator; twin of `emit_contract_drift.rs`.

use orbital_compiler::embedded::{operators, EmbeddedOperator};
use orbital_core_base::runs_on::{entity_arg, runs_on, EFFECT_SITES, ENTITY_ARGS};
use orbital_core_base::os::OsOp;
use std::collections::HashMap;

/// Every disagreement between the core table and `registry`, as readable lines.
fn drift(registry: &HashMap<String, EmbeddedOperator>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, op) in registry {
        match (op.has_side_effects, op.runs_on.as_deref()) {
            (true, None) => out.push(format!("{name}: effect with no runsOn")),
            (false, Some(site)) => {
                out.push(format!("{name}: pure operator declares runsOn {site}"))
            }
            (true, Some(site)) if site != runs_on(name).as_str() => out.push(format!(
                "{name}: registry says {site}, orbital-core says {}",
                runs_on(name).as_str()
            )),
            _ => {}
        }
    }
    for (name, op) in registry {
        if op.entity_arg_position != entity_arg(name) {
            out.push(format!(
                "{name}: registry entityArgPosition {:?}, orbital-core {:?}",
                op.entity_arg_position,
                entity_arg(name)
            ));
        }
    }
    for (name, _) in ENTITY_ARGS {
        if !registry.contains_key(*name) {
            out.push(format!("{name}: in orbital-core's entity-argument table but not in the registry"));
        }
    }
    for (name, _) in EFFECT_SITES {
        if !registry.get(*name).is_some_and(|op| op.has_side_effects) {
            out.push(format!(
                "{name}: in orbital-core's table but not a registry effect"
            ));
        }
    }
    out.sort();
    out
}

#[test]
fn the_core_table_agrees_with_the_registry() {
    assert_eq!(drift(operators()), Vec::<String>::new());
}

#[test]
fn a_registry_site_change_is_reported() {
    let mut registry = operators().clone();
    registry.get_mut("persist").unwrap().runs_on = Some("any".into());
    assert_eq!(
        drift(&registry),
        vec!["persist: registry says any, orbital-core says server".to_string()]
    );
}

#[test]
fn a_missing_site_and_a_stray_site_are_both_reported() {
    let mut registry = operators().clone();
    registry.get_mut("emit").unwrap().runs_on = None;
    registry.get_mut("math/ln").unwrap().runs_on = Some("any".into());
    registry.remove("random-bytes");
    assert_eq!(
        drift(&registry),
        vec![
            "emit: effect with no runsOn".to_string(),
            "math/ln: pure operator declares runsOn any".to_string(),
            "random-bytes: in orbital-core's table but not a registry effect".to_string(),
        ]
    );
}

#[test]
fn an_entity_argument_change_is_reported() {
    let mut registry = operators().clone();
    registry.get_mut("persist").unwrap().entity_arg_position = Some(2);
    registry.get_mut("call-service").unwrap().entity_arg_position = Some(0);
    assert_eq!(
        drift(&registry),
        vec![
            "call-service: registry entityArgPosition Some(0), orbital-core None".to_string(),
            "persist: registry entityArgPosition Some(2), orbital-core Some(1)".to_string(),
        ]
    );
}

/// Every `os/` and `kv/` operator whose registry arity disagrees with core's `OsOp` table; the registry's max counts the
/// trailing options object.
fn os_arity_drift(registry: &HashMap<String, EmbeddedOperator>) -> Vec<String> {
    let mut out = Vec::new();
    for op in OsOp::ALL {
        let Some(entry) = registry.get(op.name()) else {
            out.push(format!("{}: in orbital-core's OsOp table but not in the registry", op.name()));
            continue;
        };
        let (min, max) = (op.min_args() as u32, op.max_args() as u32 + 1);
        if entry.min_arity != min || entry.max_arity != Some(max) {
            out.push(format!(
                "{}: registry arity {}..{:?}, orbital-core {min}..{max}",
                op.name(),
                entry.min_arity,
                entry.max_arity
            ));
        }
    }
    out
}

#[test]
fn the_os_operator_arity_agrees_with_the_registry() {
    assert_eq!(os_arity_drift(operators()), Vec::<String>::new());
}

#[test]
fn an_os_arity_change_is_reported() {
    let mut registry = operators().clone();
    registry.get_mut("os/mmio-write").unwrap().min_arity = 3;
    registry.remove("os/irq-ack");
    let mut expected = Vec::new();
    expected.push("os/mmio-write: registry arity 3..Some(5), orbital-core 4..5".to_string());
    expected.push("os/irq-ack: in orbital-core's OsOp table but not in the registry".to_string());
    assert_eq!(os_arity_drift(&registry), expected);
}
