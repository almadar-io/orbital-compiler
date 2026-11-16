//! `orbital-core`'s emit-contract table must agree with the baked operator
//! registry.
//!
//! The table exists because `orbital-lolo` (L1) and `orbital-compiler` (L2) are
//! siblings: L1 cannot reach the registry, so the shared answer to "what does
//! this operator emit?" lives in the crate they both depend on. That leaves two
//! statements of the same contract, and the corpus has already been burned once
//! by exactly this shape of drift — `fetch`/`ref` declared a bare entity for
//! months while every executor emitted `{ data, totalCount }`.
//!
//! So the agreement is asserted mechanically here rather than trusted: this
//! test fails the build the moment `canonical-operators.json` and
//! `orbital_core::effects::emit_contract` disagree.

use orbital_compiler::embedded::{get_operator, OperatorTypeRef, OperatorTypeRefKind};
use orbital_core_base::emit_contract::{success_payload, CallVariant, ProducedPayload};

/// The registry's `produces` branch for a call site, narrowed the way the
/// validator narrows it, reduced to the same vocabulary the core table speaks.
fn registry_shape(operator: &str, persist_action: Option<&str>) -> Option<ProducedPayload> {
    let produces = get_operator(operator)?.effect.as_ref()?.produces.as_ref()?;
    let narrowed: &OperatorTypeRef = match produces {
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Union { of }) => {
            // persist's union order: [create/update row, delete result, batch summary]
            let idx = match persist_action? {
                "create" | "update" => 0,
                "delete" => 1,
                "batch" => 2,
                _ => return None,
            };
            of.get(idx)?
        }
        other => other,
    };
    Some(match narrowed {
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Entity { .. }) => ProducedPayload::EntityRow,
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. }) => {
            let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
            names.sort_unstable();
            // Compared by NAME SET below; the leak is fine for a test.
            ProducedPayload::Fields(Box::leak(names.into_boxed_slice()))
        }
        _ => ProducedPayload::Opaque,
    })
}

fn assert_agrees(operator: &str, persist_action: Option<&str>) {
    let core = success_payload(operator, persist_action.map_or(CallVariant::Plain, CallVariant::PersistAction));
    let registry = registry_shape(operator, persist_action).unwrap_or_else(|| {
        panic!("registry has no decidable produces for {operator} {persist_action:?}")
    });
    match (&core, &registry) {
        (ProducedPayload::EntityRow, ProducedPayload::EntityRow) => {}
        (ProducedPayload::Fields(a), ProducedPayload::Fields(b)) => {
            let mut a: Vec<&str> = a.to_vec();
            let mut b: Vec<&str> = b.to_vec();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(
                a, b,
                "orbital-core's emit contract for `{operator} {persist_action:?}` lists {a:?} but \
                 canonical-operators.json declares {b:?} — the two statements of the same contract \
                 have drifted"
            );
        }
        _ => panic!(
            "shape KIND mismatch for `{operator} {persist_action:?}`: core says {core:?}, registry says {registry:?}"
        ),
    }
}

#[test]
fn persist_create_agrees_with_the_registry() {
    assert_agrees("persist", Some("create"));
}

#[test]
fn persist_update_agrees_with_the_registry() {
    assert_agrees("persist", Some("update"));
}

#[test]
fn persist_delete_agrees_with_the_registry() {
    assert_agrees("persist", Some("delete"));
}

#[test]
fn persist_batch_agrees_with_the_registry() {
    assert_agrees("persist", Some("batch"));
}

/// The one that was wrong for months, and the reason this test exists.
#[test]
fn fetch_agrees_with_the_registry() {
    assert_agrees("fetch", None);
}

/// `clear` is advertised nowhere real — `PersistOperation` has no `Clear` and
/// no executor implements it — so the core table must decline it rather than
/// invent a branch.
#[test]
fn phantom_persist_actions_stay_opaque() {
    assert_eq!(
        success_payload("persist", CallVariant::PersistAction("clear")),
        ProducedPayload::Opaque
    );
}
