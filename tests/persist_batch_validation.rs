//! `persist batch`: `["persist", "batch", <ops>, { emit }?]`. The validator reads
//! `batch` as the action (no entity argument), checks each statically-readable op
//! tuple with the single-op persist validator, requires non-literal ops to be
//! array-typed, and requires one placement for every entity the ops name.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
use orbital_core::schema::OrbitalSchema;
use serde_json::{json, Value};

fn row() -> Value {
    json!({
        "name": "Row", "persistence": "persistent", "collection": "rows",
        "fields": [
            { "name": "id", "type": "string", "required": true },
            { "name": "status", "type": "string" },
            { "name": "draftView", "type": "string", "intrinsic": true }
        ]
    })
}

fn draft() -> Value {
    json!({
        "name": "Draft", "persistence": "persistent", "collection": "drafts", "local": true,
        "fields": [{ "name": "id", "type": "string", "required": true }, { "name": "body", "type": "string" }]
    })
}

fn saved_payload() -> Value {
    json!([
        { "name": "operations", "type": "array", "items": { "type": "object" } },
        { "name": "completedCount", "type": "number", "required": true },
        { "name": "totalCount", "type": "number", "required": true }
    ])
}

fn validate(effects: Value, saved: Value) -> ValidationResult {
    let schema: OrbitalSchema = serde_json::from_value(json!({
        "name": "fixture", "version": "1.0.0",
        "orbitals": [{
            "name": "Rows",
            "entity": row(),
            "auxiliaryEntities": [draft()],
            "traits": [{
                "name": "Batcher", "linkedEntity": "Row", "category": "interaction",
                "emits": [
                    { "event": "SAVED", "payloadSchema": saved },
                    { "event": "FAILED", "payloadSchema": [{ "name": "error", "type": "string", "required": true }] }
                ],
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "events": [{ "key": "INIT" }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "INIT", "effects": effects }]
                }
            }],
            "pages": []
        }]
    }))
    .expect("schema");
    validate_comprehensive_without_services(&schema)
}

fn emit() -> Value {
    json!({ "emit": { "success": "SAVED", "failure": "FAILED" } })
}

fn batch(ops: Value) -> Value {
    json!([["persist", "batch", ops, emit()]])
}

fn codes(r: &ValidationResult) -> Vec<ErrorCode> {
    r.errors
        .iter()
        .chain(r.warnings.iter())
        .map(|e| e.code.clone())
        .collect()
}

fn has(r: &ValidationResult, code: ErrorCode) -> bool {
    codes(r).contains(&code)
}

fn batch_findings(r: &ValidationResult) -> Vec<String> {
    r.errors
        .iter()
        .chain(r.warnings.iter())
        .filter(|e| {
            matches!(
                e.code,
                ErrorCode::EffectFetchInvalidEntity
                    | ErrorCode::PersistBatchOpsNotArray
                    | ErrorCode::PersistBatchBadOp
                    | ErrorCode::PersistBatchMixedPlacement
                    | ErrorCode::EntityIntrinsicFieldPersisted
            )
        })
        .map(|e| format!("{} {}", e.code.as_str(), e.message))
        .collect()
}

fn mapped(tuple: Value) -> Value {
    json!(["array/map", "@payload.rows", ["fn", "r", tuple]])
}

fn update_tuple() -> Value {
    json!(["update", "Row", ["object/get", "@r", "id"], { "status": "done" }])
}

#[test]
fn a_literal_ops_list_of_valid_tuples_is_accepted() {
    let ops = json!([
        ["create", "Row", { "id": "a", "status": "new" }],
        ["update", "Row", "a", { "status": "done" }],
        ["delete", "Row", "a"]
    ]);
    let r = validate(batch(ops), saved_payload());
    assert!(batch_findings(&r).is_empty(), "{:?}", batch_findings(&r));
}

#[test]
fn an_array_map_whose_lambda_builds_a_tuple_is_accepted() {
    let r = validate(batch(mapped(update_tuple())), saved_payload());
    assert!(batch_findings(&r).is_empty(), "{:?}", batch_findings(&r));
}

#[test]
fn an_opaque_array_expression_is_accepted() {
    for ops in [
        json!("@payload.ops"),
        json!(["array/filter", "@payload.ops", ["fn", "o", true]]),
    ] {
        let r = validate(batch(ops), saved_payload());
        assert!(batch_findings(&r).is_empty(), "{:?}", batch_findings(&r));
    }
}

#[test]
fn batch_is_not_read_as_an_entity_name() {
    let r = validate(batch(json!([["delete", "Row", "a"]])), saved_payload());
    assert!(
        !r.errors.iter().any(|e| e.message.contains("'batch'")),
        "{:?}",
        batch_findings(&r)
    );
}

#[test]
fn an_unknown_entity_in_a_literal_op_reuses_the_single_op_code() {
    let r = validate(batch(json!([["delete", "Nope", "a"]])), saved_payload());
    assert!(
        has(&r, ErrorCode::EffectFetchInvalidEntity),
        "{:?}",
        codes(&r)
    );
}

#[test]
fn an_unknown_entity_in_a_mapped_tuple_reuses_the_single_op_code() {
    let r = validate(
        batch(mapped(json!([
            "update",
            "Nope",
            ["object/get", "@r", "id"],
            {}
        ]))),
        saved_payload(),
    );
    assert!(
        has(&r, ErrorCode::EffectFetchInvalidEntity),
        "{:?}",
        codes(&r)
    );
}

#[test]
fn an_intrinsic_field_in_an_op_literal_reuses_the_single_op_rule() {
    let r = validate(
        batch(json!([["update", "Row", "a", { "draftView": "x" }]])),
        saved_payload(),
    );
    assert!(
        has(&r, ErrorCode::EntityIntrinsicFieldPersisted),
        "{:?}",
        codes(&r)
    );
}

#[test]
fn an_unknown_action_is_a_bad_op() {
    let r = validate(
        batch(json!([["upsert", "Row", { "id": "a" }]])),
        saved_payload(),
    );
    assert!(has(&r, ErrorCode::PersistBatchBadOp), "{:?}", codes(&r));
}

#[test]
fn the_wrong_arity_is_a_bad_op() {
    for tuple in [
        json!(["update", "Row", { "status": "x" }]),
        json!(["delete", "Row"]),
        json!(["create", "Row"]),
        json!(["create", "Row", {}, "extra"]),
    ] {
        let r = validate(batch(json!([tuple])), saved_payload());
        assert!(
            has(&r, ErrorCode::PersistBatchBadOp),
            "{tuple}: {:?}",
            codes(&r)
        );
    }
}

#[test]
fn an_element_that_is_not_a_tuple_is_a_bad_op() {
    let r = validate(batch(json!([42])), saved_payload());
    assert!(has(&r, ErrorCode::PersistBatchBadOp), "{:?}", codes(&r));
}

#[test]
fn a_mapped_lambda_with_an_unknown_action_head_is_a_bad_op() {
    let r = validate(
        batch(mapped(json!([
            "upsert",
            "Row",
            ["object/get", "@r", "id"],
            {}
        ]))),
        saved_payload(),
    );
    assert!(has(&r, ErrorCode::PersistBatchBadOp), "{:?}", codes(&r));
}

#[test]
fn a_mapped_tuple_with_the_wrong_arity_is_a_bad_op() {
    let r = validate(batch(mapped(json!(["delete", "Row"]))), saved_payload());
    assert!(has(&r, ErrorCode::PersistBatchBadOp), "{:?}", codes(&r));
}

#[test]
fn ops_that_are_not_an_array_are_rejected() {
    for ops in [
        json!("plain text"),
        json!(7),
        json!(true),
        json!({ "a": 1 }),
        json!(["str/concat", "a", "b"]),
        json!(["+", 1, 2]),
    ] {
        let r = validate(batch(ops.clone()), saved_payload());
        assert!(
            has(&r, ErrorCode::PersistBatchOpsNotArray),
            "{ops}: {:?}",
            codes(&r)
        );
    }
}

#[test]
fn control_a_missing_ops_argument_is_not_array_typed() {
    let r = validate(json!([["persist", "batch"]]), saved_payload());
    assert!(
        has(&r, ErrorCode::PersistBatchOpsNotArray),
        "{:?}",
        codes(&r)
    );
}

#[test]
fn ops_naming_browser_and_server_entities_are_mixed_placement() {
    let r = validate(
        batch(json!([["create", "Draft", { "id": "d" }], ["create", "Row", { "id": "r" }]])),
        saved_payload(),
    );
    assert!(
        has(&r, ErrorCode::PersistBatchMixedPlacement),
        "{:?}",
        codes(&r)
    );
}

#[test]
fn control_ops_on_one_placement_are_not_mixed() {
    for ops in [
        json!([["create", "Draft", { "id": "d" }], ["delete", "Draft", "d"]]),
        json!([["create", "Row", { "id": "r" }], ["delete", "Row", "r"]]),
    ] {
        let r = validate(batch(ops), saved_payload());
        assert!(
            !has(&r, ErrorCode::PersistBatchMixedPlacement),
            "{:?}",
            codes(&r)
        );
    }
}

#[test]
fn the_success_payload_is_the_declared_batch_summary() {
    let r = validate(batch(mapped(update_tuple())), saved_payload());
    let not_produced: Vec<_> = r
        .errors
        .iter()
        .filter(|e| e.code == ErrorCode::EmitPayloadNotProduced)
        .map(|e| e.message.clone())
        .collect();
    assert!(not_produced.is_empty(), "{not_produced:?}");
}

#[test]
fn a_success_event_reading_a_field_the_batch_does_not_produce_is_reported() {
    let saved = json!([{ "name": "rows", "type": "number" }]);
    let r = validate(batch(mapped(update_tuple())), saved);
    assert!(
        has(&r, ErrorCode::EmitPayloadNotProduced),
        "{:?}",
        codes(&r)
    );
}

#[test]
fn control_operations_is_an_array_so_declaring_it_a_number_is_a_type_mismatch() {
    let saved = json!([{ "name": "operations", "type": "number" }]);
    let r = validate(batch(mapped(update_tuple())), saved);
    assert!(
        has(&r, ErrorCode::EmitPayloadNotProduced),
        "{:?}",
        codes(&r)
    );
}
