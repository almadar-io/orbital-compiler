// `orbital_core::effects::runs_on` must agree with the baked registry's `runsOn` for every operator; twin of `emit_contract_drift.rs`.

use orbital_compiler::embedded::{operators, EmbeddedOperator};
use orbital_core_base::runs_on::{entity_arg, forwarded_to_client, runs_on, EFFECT_SITES, ENTITY_ARGS, FORWARDED_TO_CLIENT};
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
    for (name, op) in registry {
        if op.forwarded_to_client != forwarded_to_client(name) {
            out.push(format!(
                "{name}: registry forwardedToClient {}, orbital-core {}",
                op.forwarded_to_client,
                forwarded_to_client(name)
            ));
        }
        if op.forwarded_to_client && op.runs_on.as_deref() != Some("client") {
            out.push(format!("{name}: forwardedToClient on an effect that does not run on the client"));
        }
    }
    for name in FORWARDED_TO_CLIENT {
        if !registry.contains_key(*name) {
            out.push(format!("{name}: in orbital-core's forwarded table but not in the registry"));
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

#[test]
fn a_forwarded_fact_change_is_reported() {
    let mut registry = operators().clone();
    registry.get_mut("render-ui").unwrap().forwarded_to_client = false;
    registry.get_mut("send-server").unwrap().forwarded_to_client = true;
    registry.get_mut("persist").unwrap().forwarded_to_client = true;
    assert_eq!(
        drift(&registry),
        vec![
            "persist: forwardedToClient on an effect that does not run on the client".to_string(),
            "persist: registry forwardedToClient true, orbital-core false".to_string(),
            "render-ui: registry forwardedToClient false, orbital-core true".to_string(),
            "send-server: registry forwardedToClient true, orbital-core false".to_string(),
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
        let (min, max) = (op.min_args().expect("table") as u32, op.max_args().expect("table") as u32 + 1);
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

/// Every operator whose table walk metadata disagrees with core's derivation: `valueSourceSlots` is
/// `ValueSourceSlots::of_semantics` of its `returnSemantics`, `fieldProjection` is `Signature::field_projection`, and a
/// call head is recognized exactly when the table names it.
fn walk_metadata_drift(registry: &HashMap<String, EmbeddedOperator>) -> Vec<String> {
    use orbital_core_base::schema::ValueSourceSlots;
    use orbital_core_base::signature::Signature;
    let mut out = Vec::new();
    for (name, op) in registry {
        let slots = op.return_semantics.as_deref().and_then(ValueSourceSlots::of_semantics);
        if op.value_source_slots != slots {
            out.push(format!(
                "{name}: registry valueSourceSlots {:?}, orbital-core {slots:?}",
                op.value_source_slots
            ));
        }
        let projection = Signature::of(op).field_projection();
        if op.field_projection != projection {
            out.push(format!(
                "{name}: registry fieldProjection {:?}, orbital-core {projection:?}",
                op.field_projection
            ));
        }
        let head = orbital_core_base::schema::SExpression::Array(
            vec![orbital_core_base::schema::SExpression::String(name.as_str().into())].into(),
        );
        if orbital_core_eval::lower::recognized_call_head(&head).is_none() {
            out.push(format!("{name}: in the registry but not a recognized call head"));
        }
    }
    for name in orbital_core_eval::evaluator::operators::DISPATCHED_NAMES {
        if !registry.contains_key(*name) {
            out.push(format!("{name}: dispatched by the evaluator but not in the registry"));
        }
    }
    out.sort();
    out
}

/// With `OPERATOR_WALK_METADATA_WRITE=<file>` the walk metadata core derives for every operator is written to `<file>`
/// (operator name to its `valueSourceSlots` and `fieldProjection`, only where core derives one): the one source the
/// table's fields are merged from.
fn write_walk_metadata(registry: &HashMap<String, EmbeddedOperator>, file: &str) {
    use orbital_core_base::schema::ValueSourceSlots;
    use orbital_core_base::signature::Signature;
    let mut derived = std::collections::BTreeMap::new();
    for (name, op) in registry {
        let mut entry = serde_json::Map::new();
        if let Some(slots) = op.return_semantics.as_deref().and_then(ValueSourceSlots::of_semantics) {
            entry.insert("valueSourceSlots".into(), serde_json::to_value(slots).unwrap());
        }
        if let Some(projection) = Signature::of(op).field_projection() {
            entry.insert("fieldProjection".into(), serde_json::to_value(projection).unwrap());
        }
        if !entry.is_empty() {
            derived.insert(name.clone(), entry);
        }
    }
    std::fs::write(file, serde_json::to_string_pretty(&derived).unwrap() + "\n").unwrap();
}

#[test]
fn the_table_walk_metadata_agrees_with_core() {
    if let Ok(file) = std::env::var("OPERATOR_WALK_METADATA_WRITE") {
        write_walk_metadata(operators(), &file);
        return;
    }
    assert_eq!(walk_metadata_drift(operators()), Vec::<String>::new());
}

#[test]
fn a_table_walk_metadata_change_is_reported() {
    use orbital_core_base::schema::ValueSourceSlots;
    let mut registry = operators().clone();
    registry.get_mut("array/map").unwrap().value_source_slots = Some(ValueSourceSlots::Last);
    registry.get_mut("object/get").unwrap().field_projection = None;
    registry.get_mut("array/len").unwrap().value_source_slots = Some(ValueSourceSlots::Args);
    registry.remove("array/filter");
    let drift = walk_metadata_drift(&registry);
    assert_eq!(drift.len(), 4, "{drift:?}");
    assert!(drift.iter().any(|d| d.starts_with("array/map: registry valueSourceSlots Some(Last)")));
    assert!(drift.iter().any(|d| d.starts_with("object/get: registry fieldProjection None")));
    assert!(drift.iter().any(|d| d.starts_with("array/len: registry valueSourceSlots Some(Args), orbital-core None")));
    assert!(drift.contains(&"array/filter: dispatched by the evaluator but not in the registry".to_string()));
}
