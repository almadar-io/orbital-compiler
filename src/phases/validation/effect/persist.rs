//! persist effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::schema::{EntityDefinition, SExpression};
use std::collections::{HashMap, HashSet};

/// Position of the entity-name literal in a `persist` call array: 2 for the
/// 4-element form (`["persist", "create"|"update"|"delete", "Entity", data]`),
/// 1 for the legacy 3-element form (`["persist", "Entity", data]`). Shared
/// with the compiler's field-usage collector (`phases/inline/entity.rs`) so
/// both readers agree on where the entity name and data literal sit — no
/// second copy of this positional rule.
pub(crate) fn persist_entity_index(arr: &[SExpression]) -> usize {
    match arr.get(1) {
        Some(SExpression::String(op)) if op == "create" || op == "update" || op == "delete" => 2,
        _ => 1,
    }
}

/// `batch` is the action word of `["persist", "batch", <ops>, { emit }?]`.
fn is_batch(arr: &[SExpression]) -> bool {
    matches!(arr.get(1), Some(SExpression::String(a)) if a == "batch")
}

/// Validate `["persist", "batch", <ops>, { emit }?]`.
///
/// Statically-readable op tuples (a literal list, or the tuple an `array/map`
/// lambda returns) go through [`validate_persist`] as the equivalent single op,
/// so the entity and per-op rules are the single forms' own. A tuple the shape
/// reader cannot place (any other expression) only has to be array-typed.
fn validate_persist_batch(
    arr: &[SExpression],
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    use orbital_core::effects::persist_batch::{
        batch_entities, batch_op_arity, classify_ops, is_op_tuple, mapped_body, BatchOps,
        BatchPlacement,
    };
    let mut result = ValidationResult::new();

    let Some(ops) = arr.get(2) else {
        result.add_error(
            ValidationError::error(
                ErrorCode::PersistBatchOpsNotArray,
                path,
                "persist batch requires an operations argument",
            )
            .with_suggestion("Use: [\"persist\", \"batch\", [[\"create\", \"Entity\", data], …]]"),
        );
        return result;
    };

    let single = |tuple: &[SExpression], op_path: &str, result: &mut ValidationResult| {
        let mut single = vec![
            SExpression::String("persist".to_string()),
            tuple[0].clone(),
            tuple[1].clone(),
        ];
        if tuple[0].as_str() != Some("delete") {
            single.extend(tuple.last().cloned());
        }
        result.merge(validate_persist(
            &single,
            op_path,
            known_entities,
            entity_definitions,
            None,
        ));
    };

    let bad_op = |op_path: &str, what: String, result: &mut ValidationResult| {
        result.add_error(
            ValidationError::error(ErrorCode::PersistBatchBadOp, op_path, what).with_suggestion(
                "Each op is [\"create\", Entity, data], [\"update\", Entity, id, data] or [\"delete\", Entity, id]",
            ),
        );
    };

    let check_tuple = |tuple: &[SExpression], op_path: &str, result: &mut ValidationResult| {
        let action = tuple.first().and_then(|a| a.as_str()).unwrap_or("");
        match batch_op_arity(action) {
            Some(arity) if tuple.len() == arity => single(tuple, op_path, result),
            Some(arity) => bad_op(
                op_path,
                format!(
                    "batch op '{action}' takes {} argument(s) after the action, found {}",
                    arity - 1,
                    tuple.len() - 1
                ),
                result,
            ),
            None => bad_op(
                op_path,
                format!("batch op action '{action}' is not one of create, update, delete"),
                result,
            ),
        }
    };

    match classify_ops(ops) {
        BatchOps::Literal(elements) => {
            for (i, element) in elements.iter().enumerate() {
                let op_path = format!("{path}.ops[{i}]");
                match element {
                    SExpression::Array(tuple) => match tuple.first().and_then(|h| h.as_str()) {
                        Some(_) if is_op_tuple(element) => {
                            check_tuple(tuple, &op_path, &mut result);
                        }
                        Some(head)
                            if crate::phases::validation::sexpr::operators::is_known_operator(
                                head,
                            ) => {}
                        Some(head) => bad_op(
                            &op_path,
                            format!(
                                "batch op action '{head}' is not one of create, update, delete"
                            ),
                            &mut result,
                        ),
                        None => bad_op(
                            &op_path,
                            "batch op must start with an action".to_string(),
                            &mut result,
                        ),
                    },
                    _ => bad_op(
                        &op_path,
                        "batch op must be a [action, Entity, …] tuple".to_string(),
                        &mut result,
                    ),
                }
            }
        }
        BatchOps::Mapped(tuple) => check_tuple(tuple, &format!("{path}.ops"), &mut result),
        BatchOps::Expression => {
            let op_path = format!("{path}.ops");
            let body_head = mapped_body(ops).and_then(|b| match b {
                SExpression::Array(items) => items.first().and_then(|h| h.as_str()),
                _ => None,
            });
            match body_head {
                Some(head)
                    if !crate::phases::validation::sexpr::operators::is_known_operator(head) =>
                {
                    bad_op(
                        &op_path,
                        format!("batch op action '{head}' is not one of create, update, delete"),
                        &mut result,
                    );
                }
                _ => {
                    let non_array = match ops {
                        SExpression::Object(_) | SExpression::Null => true,
                        other => crate::phases::resolve::resolve_sexpr_standalone(other)
                            .map(|oir| {
                                matches!(
                                    crate::type_infer::infer_oir_expr_type(
                                        &oir,
                                        &crate::type_infer::NoBindingCtx
                                    ),
                                    Some(ty) if !matches!(ty, orbital_core::ir::OirType::Array { .. })
                                )
                            })
                            .unwrap_or(false),
                    };
                    if non_array {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::PersistBatchOpsNotArray,
                                op_path,
                                "persist batch operations must evaluate to an array of [action, Entity, …] tuples",
                            )
                            .with_suggestion(
                                "Pass a list of op tuples, or (array/map rows (fn r [\"update\", \"Entity\", id, data]))",
                            ),
                        );
                    }
                }
            }
        }
    }

    let residence = orbital_core::effects::Residence::of_entities(entity_definitions.values());
    if orbital_core::effects::persist_batch::batch_placement(ops, &residence)
        == BatchPlacement::Mixed
    {
        let mut browser: Vec<&str> = Vec::new();
        let mut server: Vec<&str> = Vec::new();
        for entity in batch_entities(ops) {
            if residence.is_client(entity) {
                browser.push(entity)
            } else {
                server.push(entity)
            }
        }
        result.add_error(
            ValidationError::error(
                ErrorCode::PersistBatchMixedPlacement,
                path,
                format!(
                    "persist batch writes browser-stored {browser:?} and server-stored {server:?} in one effect — no single executor holds both"
                ),
            )
            .with_suggestion("Split into one batch per placement, on separate transitions joined by an emit and a listen."),
        );
    }

    if arr.len() > 3 {
        if let SExpression::Object(options) = &arr[arr.len() - 1] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["success", "failure"],
                    declared_emits,
                    path,
                ));
            }
        }
    }
    result
}

/// Validate persist effect (array form)
pub(crate) fn validate_persist(
    arr: &[SExpression],
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    if is_batch(arr) {
        return validate_persist_batch(
            arr,
            path,
            known_entities,
            entity_definitions,
            declared_emits,
        );
    }
    let mut result = ValidationResult::new();

    // Expected formats:
    // 4-element: ["persist", "create"|"update"|"delete", "EntityName", data]
    // 3-element: ["persist", "EntityName", data] (legacy)
    if arr.len() < 3 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectFetchInvalidEntity,
                path,
                "persist requires operation and entity name",
            )
            .with_suggestion("Use: [\"persist\", \"create\", \"EntityName\", data]"),
        );
        return result;
    }

    // Determine entity position based on format
    // If arr[1] is an operation (create/update/delete), entity is at arr[2]
    // Otherwise, entity is at arr[1] (legacy format)
    let entity_index = persist_entity_index(arr);

    // Check entity name
    if arr.len() > entity_index {
        if let SExpression::String(entity_name) = &arr[entity_index] {
            if !known_entities.is_empty() && !known_entities.contains(entity_name) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectFetchInvalidEntity,
                        path,
                        format!("Entity '{}' does not exist", entity_name),
                    )
                    .with_valid_values(known_entities.iter().cloned().collect()),
                );
            }
        }
    }

    // Wave 2 rules 1 & 3 — only when the data argument is an explicit Object
    // LITERAL (a bare whole-frame `@entity`/`@payload` binding is not a hit
    // for either rule: it carries no key set to inspect).
    let op = if entity_index == 2 {
        match &arr[1] {
            SExpression::String(o) => Some(o.as_str()),
            _ => None,
        }
    } else {
        None
    };
    let entity_name = match arr.get(entity_index) {
        Some(SExpression::String(name)) => Some(name.as_str()),
        _ => None,
    };
    if let (Some(entity_name), Some(SExpression::Object(data_fields))) =
        (entity_name, arr.get(entity_index + 1))
    {
        if let Some(entity_def) = entity_definitions.get(entity_name) {
            // Rule 1 (ORB_ENTITY_INTRINSIC_FIELD_PERSISTED): the literal names
            // an `@intrinsic` field of the target entity.
            let mut intrinsic_hits: Vec<&String> = data_fields
                .keys()
                .filter(|key| {
                    entity_def.fields.iter().any(|f| {
                        f.name.as_deref() == Some(key.as_str()) && f.intrinsic == Some(true)
                    })
                })
                .collect();
            intrinsic_hits.sort();
            for field in intrinsic_hits {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityIntrinsicFieldPersisted,
                        path,
                        format!(
                            "persist {} '{}' data literal names '{}', an @intrinsic field — intrinsic fields are trait-owned view-state, never a persisted column",
                            op.unwrap_or("create"), entity_name, field
                        ),
                    )
                    .with_suggestion(format!(
                        "Remove '{}' from the persist data literal, or drop @intrinsic from the field if it is really meant to be stored",
                        field
                    )),
                );
            }

            // Rule 3 (ORB_PERSIST_CREATE_MISSING_POLICY_FIELD): only on
            // `create`, and only when `@create` compares a field the literal
            // doesn't supply.
            if op == Some("create") {
                if let Some(create_policy) = &entity_def.create_policy {
                    let mut referenced: HashMap<String, String> = HashMap::new();
                    let mut set_fields: HashSet<String> = HashSet::new();
                    create_policy.collect_entity_field_refs(
                        &mut referenced,
                        &mut set_fields,
                        "CREATE",
                    );
                    let mut missing: Vec<&String> = referenced
                        .keys()
                        .filter(|field| !data_fields.contains_key(field.as_str()))
                        .collect();
                    missing.sort();
                    for field in missing {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::PersistCreateMissingPolicyField,
                                path,
                                format!(
                                    "persist create '{}' data literal has no '{}', but '{}'.@create compares that field — a row created here will never satisfy its own access policy",
                                    entity_name, field, entity_name
                                ),
                            )
                            .with_suggestion(format!(
                                "Add '{}' to the persist data literal so the created row satisfies its own @create policy",
                                field
                            )),
                        );
                    }
                }
            }
        }
    }

    // Trailing options object containing `emit:` is detected by the resolver
    // only when it actually has an `emit` key — match that here to avoid
    // treating an inline data payload as an options bag.
    if arr.len() > entity_index + 1 {
        if let SExpression::Object(options) = &arr[arr.len() - 1] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["success", "failure"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message_entity(create_policy: Option<serde_json::Value>) -> EntityDefinition {
        serde_json::from_value(json!({
            "name": "Message",
            "collection": "messages",
            "persistence": "persistent",
            "fields": [
                { "name": "id", "type": "string" },
                { "name": "sender", "type": "string" },
                { "name": "text", "type": "string" },
                { "name": "isEditing", "type": "boolean", "intrinsic": true }
            ],
            "create_policy": create_policy,
        }))
        .expect("valid entity definition")
    }

    fn persist_create_arr(entity: &str, fields: Vec<(&str, SExpression)>) -> Vec<SExpression> {
        let mut obj = orbital_core::schema::SObject::new();
        for (k, v) in fields {
            obj.insert(k.to_string(), v);
        }
        vec![
            SExpression::String("persist".to_string()),
            SExpression::String("create".to_string()),
            SExpression::String(entity.to_string()),
            SExpression::Object(obj),
        ]
    }

    #[test]
    fn intrinsic_field_in_persist_literal_errors() {
        let arr = persist_create_arr(
            "Message",
            vec![
                ("sender", SExpression::String("@user.id".to_string())),
                ("text", SExpression::String("@payload.text".to_string())),
                ("isEditing", SExpression::Bool(true)),
            ],
        );
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), message_entity(None));
        let known = ["Message".to_string()].into_iter().collect();
        let result = validate_persist(&arr, "test", &known, &defs, None);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityIntrinsicFieldPersisted),
            "Expected EntityIntrinsicFieldPersisted error: {:?}",
            result.errors
        );
    }

    #[test]
    fn no_intrinsic_field_in_literal_passes() {
        let arr = persist_create_arr(
            "Message",
            vec![
                ("sender", SExpression::String("@user.id".to_string())),
                ("text", SExpression::String("@payload.text".to_string())),
            ],
        );
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), message_entity(None));
        let known = ["Message".to_string()].into_iter().collect();
        let result = validate_persist(&arr, "test", &known, &defs, None);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::EntityIntrinsicFieldPersisted),
            "No intrinsic field in the literal must pass: {:?}",
            result.errors
        );
    }

    #[test]
    fn bare_whole_frame_persist_is_not_a_hit_for_intrinsic_rule() {
        let arr = vec![
            SExpression::String("persist".to_string()),
            SExpression::String("create".to_string()),
            SExpression::String("Message".to_string()),
            SExpression::String("@entity".to_string()),
        ];
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), message_entity(None));
        let known = ["Message".to_string()].into_iter().collect();
        let result = validate_persist(&arr, "test", &known, &defs, None);
        assert!(
            result.errors.is_empty(),
            "A bare whole-frame persist must not trip either Wave 2 rule: {:?}",
            result.errors
        );
    }

    #[test]
    fn persist_create_missing_policy_field_errors() {
        // @create sender == @user.id
        let create_policy = json!(["==", "@entity.sender", "@user.id"]);
        let arr = persist_create_arr(
            "Message",
            vec![("text", SExpression::String("@payload.text".to_string()))],
        );
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), message_entity(Some(create_policy)));
        let known = ["Message".to_string()].into_iter().collect();
        let result = validate_persist(&arr, "test", &known, &defs, None);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::PersistCreateMissingPolicyField),
            "Expected PersistCreateMissingPolicyField error: {:?}",
            result.errors
        );
    }

    #[test]
    fn persist_create_with_policy_field_present_passes() {
        let create_policy = json!(["==", "@entity.sender", "@user.id"]);
        let arr = persist_create_arr(
            "Message",
            vec![
                ("sender", SExpression::String("@user.id".to_string())),
                ("text", SExpression::String("@payload.text".to_string())),
            ],
        );
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), message_entity(Some(create_policy)));
        let known = ["Message".to_string()].into_iter().collect();
        let result = validate_persist(&arr, "test", &known, &defs, None);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::PersistCreateMissingPolicyField),
            "Policy field present in the literal must pass: {:?}",
            result.errors
        );
    }

    #[test]
    fn bare_whole_frame_persist_create_is_not_a_hit_for_policy_rule() {
        let create_policy = json!(["==", "@entity.sender", "@user.id"]);
        let arr = vec![
            SExpression::String("persist".to_string()),
            SExpression::String("create".to_string()),
            SExpression::String("Message".to_string()),
            SExpression::String("@entity".to_string()),
        ];
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), message_entity(Some(create_policy)));
        let known = ["Message".to_string()].into_iter().collect();
        let result = validate_persist(&arr, "test", &known, &defs, None);
        assert!(
            result.errors.is_empty(),
            "A bare whole-frame persist create must not trip the policy-field rule: {:?}",
            result.errors
        );
    }

    #[test]
    fn persist_update_is_exempt_from_the_policy_field_rule() {
        let create_policy = json!(["==", "@entity.sender", "@user.id"]);
        let arr = persist_create_arr("Message", vec![]);
        let mut update_arr = arr.clone();
        update_arr[1] = SExpression::String("update".to_string());
        let mut defs = HashMap::new();
        defs.insert("Message".to_string(), message_entity(Some(create_policy)));
        let known = ["Message".to_string()].into_iter().collect();
        let result = validate_persist(&update_arr, "test", &known, &defs, None);
        assert!(
            !result
                .all_issues()
                .any(|e| e.code == ErrorCode::PersistCreateMissingPolicyField),
            "persist update must never trigger the create-only policy rule: {:?}",
            result.errors
        );
    }
}
