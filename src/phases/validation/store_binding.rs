//! The store seam's binding checks (EXEC-STORE-4): `store:` names a driver orbital that answers every row
//! verb of `spec/infra/row-store.lolo`, binds only an entity whose rows live in a store, and a driver that
//! declares no conditional write never backs an entity the program takes with a single delete or update.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{EntityRef, OrbitalSchema, Persistence, SExpression, TraitReference};
use std::collections::HashMap;

/// The verbs a driver orbital handles: the row asks the kernel sends it.
const VERBS: [&str; 4] = ["ROW_READ", "ROW_LIST", "ROWS_QUERY", "ROWS_WRITE"];

pub fn validate_store_bindings(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    ctx.merge(store_binding_issues(schema));
}

pub(crate) fn store_binding_issues(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut checked: HashMap<&str, bool> = HashMap::new();
    let mut bound: HashMap<&str, &str> = HashMap::new();
    if let Some(driver) = schema.store.as_deref() {
        check_driver(schema, driver, "store", &mut checked, &mut result);
    }
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for entity_ref in std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten()) {
            let EntityRef::Inline(def) = entity_ref else { continue };
            let path = format!("orbitals[{i}].entity.{}", def.name);
            let in_store = def.persistence != Some(Persistence::Runtime) && !def.local;
            if let Some(driver) = def.store.as_deref() {
                if !in_store {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::StoreOnNonPersistent,
                            &path,
                            format!("entity {} binds store {driver} but its rows live in no store (runtime or browser-local)", def.name),
                        )
                        .with_suggestion("Drop `store:`, or declare the entity [persistent: <collection>]"),
                    );
                }
                check_driver(schema, driver, &path, &mut checked, &mut result);
            }
            if in_store {
                if let Some(driver) = def.store.as_deref().or(schema.store.as_deref()) {
                    bound.insert(def.name.as_str(), driver);
                }
            }
        }
    }
    if !bound.is_empty() {
        for (trait_def, path) in super::inline_traits(schema) {
            for transition in &trait_def.state_machine.transitions {
                for effect in transition.effects.iter().flatten() {
                    single_takes(effect, &mut |entity| {
                        let Some(driver) = bound.get(entity) else { return };
                        if write_conditional(schema, driver) == Some(false) {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::StoreDriverNotConditional,
                                    &path,
                                    format!("persist on {entity} is one conditional op (EXEC-STEP-8), and its store driver {driver} declares writeCapabilities.conditional false"),
                                )
                                .with_suggestion("Bind the entity to a driver that writes conditionally, or take rows through a persist batch"),
                            );
                        }
                    });
                }
            }
        }
    }
    result
}

fn check_driver<'a>(
    schema: &OrbitalSchema,
    driver: &'a str,
    path: &str,
    checked: &mut HashMap<&'a str, bool>,
    result: &mut ValidationResult,
) {
    if checked.insert(driver, true).is_some() {
        return;
    }
    let Some(orbital) = schema.orbitals.iter().find(|o| o.name == driver) else {
        result.add_error(
            ValidationError::error(ErrorCode::StoreUnknownOrbital, path, format!("store {driver} names no orbital of this program"))
                .with_valid_values(schema.orbitals.iter().map(|o| o.name.clone()).collect()),
        );
        return;
    };
    let handled = |verb: &str| {
        orbital.traits.iter().any(|t| match t {
            TraitReference::Inline(def) => def.state_machine.transitions.iter().any(|tr| tr.event == verb),
            TraitReference::Reference { .. } => false,
        })
    };
    let missing: Vec<&str> = VERBS.into_iter().filter(|v| !handled(v)).collect();
    if !missing.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::StoreDriverContract,
                path,
                format!("store driver {driver} has no trait handling {missing:?} (spec/infra/row-store.lolo RowStoreDriver)"),
            )
            .with_suggestion("Compose a std store driver atom, or give the driver a trait that answers every row verb"),
        );
    }
}

/// The driver's declared `writeCapabilities.conditional`, when it declares one.
pub(crate) fn write_conditional(schema: &OrbitalSchema, driver: &str) -> Option<bool> {
    let orbital = schema.orbitals.iter().find(|o| o.name == driver)?;
    let declared = orbital.config.as_ref()?.get("writeCapabilities")?.default.as_ref()?;
    declared.as_object()?.get("conditional")?.as_bool()
}

/// Every entity a single `persist delete|update` in `effect` takes.
fn single_takes<'e>(effect: &'e SExpression, found: &mut impl FnMut(&'e str)) {
    let Some(items) = effect.as_array() else { return };
    if let [head, op, entity, ..] = items.as_slice() {
        if head.as_str() == Some("persist") && matches!(op.as_str(), Some("delete" | "update")) {
            if let Some(entity) = entity.as_str() {
                found(entity);
            }
        }
    }
    for item in items {
        single_takes(item, found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn driver(name: &str, verbs: &[&str], conditional: Option<bool>) -> Value {
        let transitions: Vec<Value> = verbs.iter().map(|v| json!({ "from": "ready", "to": "ready", "event": v })).collect();
        let mut orbital = json!({ "name": name, "pages": [],
            "entity": { "name": format!("{name}Row"), "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [{ "name": format!("{name}Driver"), "stateMachine": { "states": [{ "name": "ready", "isInitial": true }], "transitions": transitions } }] });
        if let Some(conditional) = conditional {
            orbital["config"] = json!({ "writeCapabilities": { "type": "WriteCapabilities", "default": { "conditional": conditional } } });
        }
        orbital
    }

    fn program(ticket: Value, take: bool, driver: Value) -> OrbitalSchema {
        let effects = if take { json!([["persist", "delete", "Ticket", "@payload.id"]]) } else { json!([["persist", "create", "Ticket", { "used": false }]]) };
        serde_json::from_value(json!({ "name": "p", "orbitals": [
            { "name": "Tickets", "pages": [], "entity": ticket,
              "traits": [{ "name": "Desk", "linkedEntity": "Ticket", "stateMachine": { "states": [{ "name": "idle", "isInitial": true }],
                "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": effects }] } }] },
            driver
        ] }))
        .unwrap()
    }

    fn ticket(extra: Value) -> Value {
        let mut t = json!({ "name": "Ticket", "persistence": "persistent", "fields": [{ "name": "id", "type": "string" }] });
        for (k, v) in extra.as_object().unwrap() {
            t[k] = v.clone();
        }
        t
    }

    fn codes(schema: &OrbitalSchema) -> Vec<&'static str> {
        store_binding_issues(schema).all_issues().map(|e| e.code.as_str()).collect()
    }

    #[test]
    fn a_store_naming_no_orbital_is_refused() {
        assert_eq!(codes(&program(ticket(json!({ "store": "Nope" })), false, driver("Rows", &VERBS, None))), ["ORB_STORE_UNKNOWN_ORBITAL"]);
    }

    #[test]
    fn a_driver_missing_a_row_verb_is_refused() {
        let found = codes(&program(ticket(json!({ "store": "Rows" })), false, driver("Rows", &["ROW_READ", "ROWS_WRITE"], None)));
        assert_eq!(found, ["ORB_STORE_DRIVER_CONTRACT"]);
    }

    #[test]
    fn a_store_on_a_runtime_or_local_entity_is_refused() {
        let runtime = ticket(json!({ "store": "Rows", "persistence": "runtime" }));
        assert_eq!(codes(&program(runtime, false, driver("Rows", &VERBS, None))), ["ORB_STORE_ON_NON_PERSISTENT"]);
        let local = ticket(json!({ "store": "Rows", "local": true }));
        assert_eq!(codes(&program(local, false, driver("Rows", &VERBS, None))), ["ORB_STORE_ON_NON_PERSISTENT"]);
    }

    #[test]
    fn a_single_take_on_an_entity_whose_driver_is_not_conditional_is_refused() {
        let unconditional = program(ticket(json!({ "store": "Rows" })), true, driver("Rows", &VERBS, Some(false)));
        assert_eq!(codes(&unconditional), ["ORB_STORE_DRIVER_NOT_CONDITIONAL"]);
        let create_only = program(ticket(json!({ "store": "Rows" })), false, driver("Rows", &VERBS, Some(false)));
        assert!(codes(&create_only).is_empty(), "a create is no conditional take");
    }

    #[test]
    fn control_a_full_conditional_driver_and_an_app_default_are_accepted() {
        assert!(codes(&program(ticket(json!({ "store": "Rows" })), true, driver("Rows", &VERBS, Some(true)))).is_empty());
        assert!(codes(&program(ticket(json!({ "store": "Rows" })), true, driver("Rows", &VERBS, None))).is_empty());
        let mut by_app = program(ticket(json!({})), true, driver("Rows", &VERBS, Some(false)));
        by_app.store = Some("Rows".to_string());
        assert_eq!(codes(&by_app), ["ORB_STORE_DRIVER_NOT_CONDITIONAL"], "the app default binds too");
        assert!(codes(&program(ticket(json!({})), true, driver("Rows", &[], None))).is_empty(), "an unbound program");
    }
}
