//! persist effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::row_literal::{check_row_literal, RowSite, RowWrite};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core_base::schema::SExpression;
use std::collections::HashSet;

/// Position of the entity-name literal in a `persist` call array: 2 for the
/// 4-element form (`["persist", "create"|"upsert"|"update"|"delete", "Entity", data]`),
/// 1 for the legacy 3-element form (`["persist", "Entity", data]`). Shared
/// with the compiler's field-usage collector (`phases/inline/entity.rs`) so
/// both readers agree on where the entity name and data literal sit — no
/// second copy of this positional rule.
pub(crate) fn persist_entity_index(arr: &[SExpression]) -> usize {
    match arr.get(1) {
        Some(SExpression::String(op)) if op == "create" || op == "upsert" || op == "update" || op == "delete" => 2,
        _ => 1,
    }
}

fn is_emit_options(arg: &SExpression) -> bool {
    matches!(arg, SExpression::Object(options) if extract_emit_object_ref(options).is_some())
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
    declared_emits: Option<&[DeclaredEmit]>,
    rows: Option<&RowSite<'_>>,
) -> ValidationResult {
    use orbital_core_base::persist_batch::{
        batch_op_arity, classify_ops, is_op_tuple, mapped_body, BatchOps,
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
            SExpression::string("persist"),
            tuple[0].clone(),
            tuple[1].clone(),
        ];
        if tuple[0].as_str() != Some("delete") {
            single.extend(tuple.last().cloned());
        }
        result.merge(validate_persist(&single, op_path, known_entities, None, rows));
    };

    let check_tuple = |tuple: &[SExpression], op_path: &str, result: &mut ValidationResult| {
        let action = tuple.first().and_then(|a| a.as_str()).unwrap_or("");
        if batch_op_arity(action) == Some(tuple.len()) {
            single(tuple, op_path, result);
        }
    };

    match classify_ops(ops) {
        BatchOps::Literal(elements) => {
            for (i, element) in elements.iter().enumerate() {
                let op_path = format!("{path}.ops[{i}]");
                if let SExpression::Array(tuple) = element {
                    if tuple.first().and_then(|h| h.as_str()).is_some() && is_op_tuple(element) {
                        check_tuple(tuple, &op_path, &mut result);
                    }
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
                    if !crate::phases::validation::sexpr::operators::is_known_operator(head) => {}
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
                                    Some(ty) if !matches!(ty, orbital_core_base::ir::OirType::Array { .. })
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
    declared_emits: Option<&[DeclaredEmit]>,
    rows: Option<&RowSite<'_>>,
) -> ValidationResult {
    if is_batch(arr) {
        return validate_persist_batch(arr, path, known_entities, declared_emits, rows);
    }
    let mut result = ValidationResult::new();

    // Expected formats:
    // 4-element: ["persist", "create"|"upsert"|"update"|"delete", "EntityName", data]
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
            if !known_entities.is_empty() && !known_entities.contains(entity_name.as_str()) {
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

    if let (Some(rows), SExpression::String(entity_name), Some(data)) =
        (rows, &arr[entity_index], arr.get(entity_index + 1).filter(|d| !is_emit_options(d)))
    {
        if let Some(entity) = rows.entities.get(entity_name.as_str()) {
            let write = match arr[1].as_str() {
                Some("create") => Some(RowWrite::Create),
                Some("update" | "upsert") => Some(RowWrite::Partial),
                _ => None,
            };
            if let Some(write) = write {
                let what = format!("persist {} {entity_name}", arr[1].as_str().unwrap_or_default());
                check_row_literal(entity, data, write, &what, path, rows, &mut result);
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
