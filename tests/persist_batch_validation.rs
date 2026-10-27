//! `persist batch`: `["persist", "batch", <ops>, { emit }?]`. The validator reads
//! `batch` as the action (no entity argument), checks each statically-readable op
//! tuple with the single-op persist validator, and requires non-literal ops to be
//! array-typed.

use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode, ValidationResult};
use orbital_core_base::schema::OrbitalSchema;
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
        "name": "Draft", "persistence": "persistent", "collection": "drafts", "browser": true,
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
    validate_with(effects, saved, json!([{ "name": "error", "type": "string", "required": true }]))
}

fn validate_with(effects: Value, saved: Value, failed: Value) -> ValidationResult {
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
                    { "event": "FAILED", "payloadSchema": failed }
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

fn failed_with(fields: &[&str]) -> Value {
    Value::Array(fields.iter().map(|f| json!({ "name": f, "type": if matches!(*f, "index" | "completedCount") { "number" } else { "string" } })).collect())
}

#[test]
fn a_batch_failure_event_may_declare_the_failing_index_and_the_completed_count() {
    let ops = json!([["create", "Row", { "id": "a" }]]);
    let r = validate_with(batch(ops), saved_payload(), failed_with(&["error", "code", "index", "completedCount"]));
    assert!(!has(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", codes(&r));
}

#[test]
fn control_a_batch_failure_event_cannot_declare_a_field_the_kernel_does_not_send() {
    let ops = json!([["create", "Row", { "id": "a" }]]);
    let r = validate_with(batch(ops), saved_payload(), failed_with(&["error", "totalCount"]));
    assert!(has(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", codes(&r));
}

#[test]
fn control_a_single_persist_failure_has_no_index() {
    let single = json!([["persist", "create", "Row", { "id": "a" }, emit()]]);
    let r = validate_with(single, saved_payload(), failed_with(&["error", "index"]));
    assert!(has(&r, ErrorCode::EmitPayloadNotProduced), "{:?}", codes(&r));
}
