//! set effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{EntityDefinition, FieldType, OrbitalSchema, SExpression};
use std::collections::HashMap;

/// SET-EMITS-PAYLOAD type backstop (raw-`.orb` defense-in-depth).
///
/// For `(set @entity.X ?payloadField)`, compare the entity field's declared
/// type to the payload field's declared type. The lolo crate catches this at
/// author time for `.lolo`; raw `.orb` IR (agent/factory output) bypasses that
/// path, so this validator pass is the backstop.
///
/// `entity_field_types` is the `{ fieldName -> meta }` map of the trait's
/// linked entity (from its `EntityDefinition.fields`). `payload_shapes` is the
/// `{ fieldName -> OirType }` map of the triggering event's declared payload.
/// Both sides compare as real types through the one assignability relation in
/// orbital-core, never through a kind string. A comparison only fires when
/// BOTH sides carry a known declared type.
pub(crate) fn validate_set_value_type(
    arr: &[SExpression],
    path: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_shapes: &HashMap<String, orbital_core_base::ir::OirType>,
    schema: Option<&OrbitalSchema>,
    types: Option<&BindingTypes>,
    guard: Option<&SExpression>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Need exactly the [set, target, value] head; trailing options ignored.
    if arr.len() < 3 {
        return result;
    }

    // Target must be `@entity.<field>` string form.
    let Some(entity_field) = set_entity_field(&arr[1]) else {
        return result;
    };
    // Value must be a `?payloadField` / `@payload.<field>` read.
    let Some(payload_field) = payload_field_ref(&arr[2]) else {
        return result;
    };

    let (Some(entity_meta), Some(payload_shape)) = (
        entity_field_types.get(entity_field),
        payload_shapes.get(payload_field),
    ) else {
        return result;
    };
    let entity_ty = &entity_meta.field_type;
    let declared_optional = types.is_some_and(|t| t.optional_payload.contains(payload_field));

    let resolve = |name: &str| {
        schema.and_then(|s| {
            crate::phases::resolve::declared_struct_type(s, name)
                .ok()
                .flatten()
        })
    };
    if !orbital_core_base::type_compat::oir_type_satisfies_with_named(
        payload_shape,
        &entity_meta.shape,
        &resolve,
    ) && !guard_narrows_into(
        payload_field,
        &entity_meta.shape,
        entity_field_types,
        types,
        schema,
        guard,
    ) {
        result.add_error(
            ValidationError::error(
                ErrorCode::SetValueTypeMismatch,
                path,
                format!(
                    "set @entity.{} (type '{}') is assigned payload field '{}' (type '{}'); \
                     the declared types do not match.",
                    entity_field,
                    orbital_core_base::signature::describe_type(&entity_meta.shape),
                    payload_field,
                    orbital_core_base::signature::describe_type(payload_shape)
                ),
            )
            .with_suggestion(if declared_optional {
                format!(
                    "Payload field '{payload_field}' is optional on this event, so the set can write an absent value into '{entity_field}'. Declare it `{payload_field} : <type>!` on the event where every sender provides it, or guard the set with `when (!= ?{payload_field} null)`."
                )
            } else if entity_ty == "relation" {
                // The source often looks IDENTICAL on both sides (`topics :
                // [ReviewTopic]` in the entity AND the payload) — the entity
                // side lowers to `relation` because the element type is a
                // persistent entity, which no amount of re-reading the source
                // reveals (observed: ~100 turns across 3 agents on one such
                // mismatch).
                format!(
                    "`@entity.{entity_field}` is a RELATION: a list of a persistent entity type \
                     lowers to `relation`, even though the source declaration looks like a plain \
                     list. Relations are populated by `persist` effects/queries, never `set` from \
                     a payload. For a runtime list, declare a plain shaped type at orbital level \
                     (`type Row = {{ field : type … }}`) and type the field `[Row]`."
                )
            } else {
                format!(
                    "Align the payload field '{}' type with the entity field '{}' type, or set a \
                     converted value.",
                    payload_field, entity_field
                )
            }),
        );
    }

    result
}

/// Whether the transition's `guard` makes the payload read `value` fit `declared`. Needs the full declared binding
/// types: without them a payload field reads as its bare kind and would pass for the wrong reason.
fn guard_narrows_into(
    payload_field: &str,
    declared: &orbital_core_base::ir::OirType,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    types: Option<&BindingTypes>,
    schema: Option<&OrbitalSchema>,
    guard: Option<&SExpression>,
) -> bool {
    let (Some(types), Some(guard)) = (types, guard) else {
        return false;
    };
    let Ok(condition) = orbital_core_eval::lower::lower_sexpr(guard) else {
        return false;
    };
    let lowered = orbital_core_base::ir::OirExpr::Binding {
        binding: orbital_core_base::ir::OirBinding {
            root: orbital_core_base::ir::OirBindingRoot::Payload,
            path: vec![payload_field.to_string()],
        },
    };
    let payload = HashMap::new();
    let ctx = ExprReturnCtx {
        entity: entity_field_types,
        payload: &payload,
        types: Some(types),
        schema,
    };
    crate::type_infer::admits(&lowered, declared, Some(&condition), &ctx)
}

/// REQUIRED-FIELD-FROM-OPTIONAL-KNOB backstop (rung 2, item 1 of the
/// orbital-import Stage C emitter-debt close-out).
///
/// `(set @entity.<field> @config.<knob>)` where the value is STILL the
/// literal `@config.<knob>` token post-inline — no declared default resolved
/// it and no call-site override supplied one; a substituted value would no
/// longer read as the bare token — and `<field>` declares no null (not `T?`) on the
/// trait's linked entity, where the kernel refuses the write (EXEC-STEP-3). The emitter has nothing honest to assign: it
/// lowers to `entity.field = (undefined) as E['field']`, which is wrong for
/// an OPTIONAL field's honest `undefined` but a live TS2352 lie for a
/// REQUIRED one. The post-inline exact-token test is decidable and cheap —
/// the same "does the literal string still read `@config.<key>`" test
/// `ORB_CONFIG_DANGLING_REF` / `ORB_O_CONFIG_DEAD_KNOB` run.
///
/// Called alongside `validate_effect` from the same effects walk (mirrors
/// `validate_emit_against_declaration`'s call-site shape) rather than from
/// inside `validate_effect`'s per-operator dispatch, so the owning trait's
/// name reaches the message without widening that function's already-long
/// argument list.
pub(crate) fn validate_set_required_field_from_optional_knob(
    effect: &SExpression,
    path: &str,
    trait_name: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    let SExpression::Array(arr) = effect else {
        return result;
    };
    if arr.len() < 3 {
        return result;
    }
    let Some(SExpression::String(op)) = arr.first() else {
        return result;
    };
    if op != "set" {
        return result;
    }
    let Some(entity_field) = set_entity_field(&arr[1]) else {
        return result;
    };
    let Some(meta) = entity_field_types.get(entity_field) else {
        return result;
    };
    if matches!(meta.shape, orbital_core_base::ir::OirType::Optional { .. })
        || orbital_core_base::type_compat::is_unknowable(&meta.shape)
    {
        return result;
    }
    let SExpression::String(value) = &arr[2] else {
        return result;
    };
    let Some(knob) = value
        .strip_prefix("@config.")
        .filter(|rest| !rest.is_empty() && !rest.contains(['.', '[']))
    else {
        return result;
    };

    result.add_error(
        ValidationError::error(
            ErrorCode::SetRequiredFieldFromOptionalKnob,
            path,
            format!(
                "set @entity.{} on trait '{}' is assigned config knob '{}', which declares no \
                 default and receives no call-site value here — the token is still unresolved \
                 and the kernel refuses the null it writes into a field that declares no null.",
                entity_field, trait_name, knob
            ),
        )
        .with_suggestion(format!(
            "Give knob '{}' a default value on trait '{}' (`{} : <type> = <default>`), or declare \
             field '{}' `T?` if it is honestly allowed to be unset.",
            knob, trait_name, knob, entity_field
        )),
    );

    result
}

/// SET-OP-RETURN check (Gap G-LOLO-SET-OP-RETURN).
///
/// For `(set @entity.<field> (op …))` — an operator/expression value — infer the
/// value's result type *through* the S-expression (via the canonical
/// `return_semantics` lattice, the single `infer_oir_expr_type` derivation) and
/// reject when it is incompatible with the field's declared type. The value is
/// resolved to `OirExpr` with `resolve_sexpr_standalone` (expression arms never
/// touch the schema); a shape that doesn't resolve, or a non-array-form literal
/// (covered by the type/enum checks), is skipped. An unknowable inferred type
/// (`Json` — an input-dependent op the std metadata can't pin down) never
/// rejects, so this can't false-positive.
pub(crate) fn validate_set_return_type(
    arr: &[SExpression],
    path: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    schema: Option<&orbital_core_base::schema::OrbitalSchema>,
    types: Option<&BindingTypes>,
    guard: Option<&SExpression>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if arr.len() < 3 {
        return result;
    }
    let Some(entity_field) = set_entity_field(&arr[1]) else {
        return result;
    };
    let Some(meta) = entity_field_types.get(entity_field) else {
        return result;
    };
    let reads_entity_field = matches!(&arr[2], SExpression::String(s) if s.strip_prefix("@entity.").is_some_and(|rest| !rest.is_empty()));
    let numeric_literal = matches!(&arr[2], SExpression::Number(_))
        && orbital_core_base::type_compat::is_number_kind(match &meta.shape {
            orbital_core_base::ir::OirType::Optional { inner } => inner,
            declared => declared,
        });
    if !reads_entity_field
        && !numeric_literal
        && !matches!(
            &arr[2],
            SExpression::Array(_) | SExpression::Object(_) | SExpression::Null
        )
    {
        return result;
    }
    let ctx = ExprReturnCtx {
        entity: entity_field_types,
        payload: payload_field_types,
        types,
        schema,
    };
    let condition = guard.and_then(|g| orbital_core_eval::lower::lower_sexpr(g).ok());
    if let Some(slot) =
        null_written_into_never_null_slot(&arr[2], &meta.shape, condition.as_ref(), &ctx)
    {
        result.add_error(
            ValidationError::error(
                ErrorCode::SetValueTypeMismatch,
                path,
                format!(
                    "set @entity.{} writes a null where '{}' is declared; the declared type admits no null.",
                    entity_field,
                    orbital_core_base::signature::describe_type(&slot)
                ),
            )
            .with_suggestion("Write a value of the declared type, or declare the field optional (`T?`)"),
        );
    }
    if !reads_entity_field && !numeric_literal && !matches!(&arr[2], SExpression::Array(_)) {
        return result;
    }
    if let orbital_core_base::ir::OirType::Reference { entity } = &meta.shape {
        use crate::type_infer::TypeCtx;
        let reads_a_relation = orbital_core_eval::lower::lower_sexpr(&arr[2])
            .ok()
            .and_then(|lowered| crate::type_infer::infer_oir_expr_type(&lowered, &ctx))
            .is_some_and(|inferred| {
                matches!(inferred, orbital_core_base::ir::OirType::Reference { .. })
            });
        if reads_a_relation {
            if let Some(inferred) =
                expr_return_incompatible_given(&arr[2], &meta.shape, &ctx, guard)
            {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SetReturnTypeMismatch,
                        path,
                        format!(
                            "set @entity.{entity_field} writes a relation to '{}' into a relation to '{entity}'.",
                            orbital_core_base::signature::describe_type(&inferred)
                        ),
                    )
                    .with_suggestion(format!("Write a value that refers to '{entity}'")),
                );
            }
            return result;
        }
        if let Some(orbital_core_base::ir::OirType::Struct { additional, fields }) =
            ctx.entity_type(entity)
        {
            let partial = orbital_core_base::ir::OirType::Struct {
                additional,
                fields: fields
                    .iter()
                    .cloned()
                    .map(|mut f| {
                        f.required = false;
                        f
                    })
                    .collect(),
            };
            let unknown: Vec<String> = orbital_core_eval::lower::lower_sexpr(&arr[2])
                .ok()
                .and_then(|lowered| crate::type_infer::infer_oir_expr_type(&lowered, &ctx))
                .and_then(|inferred| match inferred {
                    orbital_core_base::ir::OirType::Struct {
                        fields: written, ..
                    } => Some(
                        written
                            .iter()
                            .map(|f| f.name.clone())
                            .filter(|n| {
                                !fields.iter().any(|d| &d.name == n)
                                    && !orbital_core_base::schema::types::FRAMEWORK_STAMPED_FIELDS
                                        .contains(&n.as_str())
                            })
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            if !unknown.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SetReturnTypeMismatch,
                        path,
                        format!(
                            "set @entity.{entity_field} writes a record with {} not declared on '{entity}'.",
                            unknown.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ")
                        ),
                    )
                    .with_suggestion(format!("Name only declared fields of '{entity}'"))
                    .with_valid_values(fields.iter().map(|f| f.name.clone()).collect()),
                );
            } else if let Some(slot) =
                null_written_into_never_null_slot(&arr[2], &partial, condition.as_ref(), &ctx)
            {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SetReturnTypeMismatch,
                        path,
                        format!(
                            "set @entity.{entity_field} builds a null where '{}' is declared in '{entity}'; the declared type admits no null.",
                            orbital_core_base::signature::describe_type(&slot)
                        ),
                    )
                    .with_suggestion("Write a value of the declared type, or declare the field optional (`T?`)"),
                );
            } else if let Some(inferred) =
                expr_return_incompatible_given(&arr[2], &partial, &ctx, guard)
            {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SetReturnTypeMismatch,
                        path,
                        format!(
                            "set @entity.{entity_field} writes a record into '{entity}' that evaluates to '{}'; it does not match the fields of '{entity}'.",
                            orbital_core_base::signature::describe_type(&inferred)
                        ),
                    )
                    .with_suggestion(format!("Name only declared fields of '{entity}', each with a value of its declared type")),
                );
            }
        }
        return result;
    }
    if let Some(inferred) = expr_return_incompatible_given(&arr[2], &meta.shape, &ctx, guard) {
        result.add_error(
            ValidationError::error(
                ErrorCode::SetReturnTypeMismatch,
                path,
                format!(
                    "set @entity.{} (type '{}') is assigned a value that evaluates to \
                     '{}'; the declared types are incompatible.",
                    entity_field,
                    orbital_core_base::signature::describe_type(&meta.shape),
                    orbital_core_base::signature::describe_type(&inferred)
                ),
            )
            .with_suggestion(if meta.field_type == "relation" {
                // Same teaching as the SET_VALUE arm above — run 29:
                // `(set @entity.days [])` on `days : [ActivityDay]` spun 4+
                // sessions against the generic "wrap/convert" text because
                // nothing said WHY the plain-looking list is a relation.
                format!(
                    "`@entity.{entity_field}` is a RELATION: a list of a persistent entity type \
                     lowers to `relation`, even though the source declaration looks like a plain \
                     list. Relations are populated by `persist` effects/queries, never `set` \
                     (not even to `[]` — a relation needs no initialization). For a runtime \
                     list, declare a plain shaped type at orbital level \
                     (`type Row = {{ field : type … }}`) and type the field `[Row]`."
                )
            } else {
                format!(
                    "Return a '{}'-typed value (wrap/convert the expression), or change the \
                     field type.",
                    meta.field_type
                )
            }),
        );
    }

    result
}

/// The declared type of the never-null slot a literal `null` written inside `value` flows into, read top-down from
/// `declared`; `None` when no such null is written.
pub(crate) fn null_written_into_never_null_slot<C: crate::type_infer::TypeCtx>(
    value: &SExpression,
    declared: &orbital_core_base::ir::OirType,
    condition: Option<&orbital_core_base::ir::OirExpr>,
    ctx: &C,
) -> Option<orbital_core_base::ir::OirType> {
    let lowered = orbital_core_eval::lower::lower_sexpr(value).ok()?;
    crate::type_infer::null_at_declared_slot(&lowered, declared, condition, ctx)
}

/// Infer an array-form value expression's result type — through the canonical
/// `infer_oir_expr_type` derivation, resolving `@entity.<f>` / `@payload.<f>`
/// bindings via the field-type maps — and return `Some(inferred)` when the
/// inferred kind is INCOMPATIBLE with `declared`, else `None`. The single
/// shared derivation behind BOTH the `(set @entity.X <expr>)` and the
/// `(emit EVT { field: <expr> })` return-type backstops, so the two can't drift.
/// **Never false-positives**: a non-array value (scalar/binding — covered by the
/// literal/enum checks), an unresolvable shape, or an unknowable `Json` inferred
/// type all yield `None`.
#[cfg(test)]
pub(crate) fn expr_return_incompatible(
    value: &SExpression,
    declared: &orbital_core_base::ir::OirType,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
    schema: Option<&orbital_core_base::schema::OrbitalSchema>,
) -> Option<orbital_core_base::ir::OirType> {
    // Only an array-form value (operator call / conditional / array literal)
    // carries an inferrable result type worth checking here.
    if !matches!(value, SExpression::Array(_)) {
        return None;
    }
    let ctx = ExprReturnCtx {
        entity: entity_field_types,
        payload: payload_field_types,
        types: None,
        schema,
    };
    expr_return_incompatible_in_ctx(value, declared, &ctx)
}

pub(crate) fn expr_return_incompatible_in_ctx<C: crate::type_infer::TypeCtx>(
    value: &SExpression,
    declared: &orbital_core_base::ir::OirType,
    ctx: &C,
) -> Option<orbital_core_base::ir::OirType> {
    expr_return_incompatible_given(value, declared, ctx, None)
}

/// As [`expr_return_incompatible_in_ctx`], read where `guard` holds (the guard of the transition the value is in).
pub(crate) fn expr_return_incompatible_given<C: crate::type_infer::TypeCtx>(
    value: &SExpression,
    declared: &orbital_core_base::ir::OirType,
    ctx: &C,
    guard: Option<&SExpression>,
) -> Option<orbital_core_base::ir::OirType> {
    // A `typed` value (an annotated `let` local) is read as the type the program declared for it.
    let oir_value = orbital_core_eval::lower::lower_sexpr(value).ok()?;
    let condition = guard.and_then(|g| orbital_core_eval::lower::lower_sexpr(g).ok());
    if crate::type_infer::admits(&oir_value, declared, condition.as_ref(), ctx) {
        return None;
    }
    let inferred = match &condition {
        Some(condition) => crate::type_infer::infer_under_condition(condition, &oir_value, ctx)?,
        None => crate::type_infer::infer_oir_expr_type(&oir_value, ctx)?,
    };
    if orbital_core_base::type_compat::is_unknowable(&inferred)
        || orbital_core_base::type_compat::sub_with_named(&inferred, declared, &|name| {
            ctx.named_type(name)
        })
        .is_some_and(|c| !c.requires_cast())
    {
        None
    } else {
        Some(inferred)
    }
}

/// `ORB_X_COMPARISON_OPERAND_TYPE_MISMATCH`: an ordering comparison anywhere in
/// `expr` whose operands infer to a number against a string. The JS runtime
/// coerces, the compiled TypeScript does not typecheck, and Rust compares
/// kinds, so the paths disagree. Operand types come from the one
/// `infer_oir_expr_type` derivation; an unknowable side (`Json`, an unresolved
/// binding) never rejects.
pub(crate) fn validate_comparison_operands(
    expr: &SExpression,
    path: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let ctx = ExprReturnCtx {
        entity: entity_field_types,
        payload: payload_field_types,
        types: None,
        schema: None,
    };
    collect_comparison_mismatches(expr, path, &ctx, &mut result);
    result
}

fn collect_comparison_mismatches(
    expr: &SExpression,
    path: &str,
    ctx: &ExprReturnCtx<'_>,
    result: &mut ValidationResult,
) {
    let SExpression::Array(items) = expr else {
        return;
    };
    if let [SExpression::String(op), left, right] = items.as_slice() {
        if matches!(op.as_str(), "<" | "<=" | ">" | ">=") {
            if let (Some(l), Some(r)) = (operand_kind(left, ctx), operand_kind(right, ctx)) {
                if l != r {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::ComparisonOperandTypeMismatch,
                            path,
                            format!("`({op} …)` compares a {l} with a {r}"),
                        )
                        .with_suggestion(
                            "Compare the same kind on both sides: convert the string side to a number, or render the number as a string (e.g. zero-padded) first",
                        ),
                    );
                }
            }
        }
    }
    for item in items {
        collect_comparison_mismatches(item, path, ctx, result);
    }
}

/// `number` / `string` for an operand whose inferred type is known to be one
/// of those kinds; `None` for anything else (never rejects).
fn operand_kind(operand: &SExpression, ctx: &ExprReturnCtx<'_>) -> Option<&'static str> {
    use orbital_core_base::ir::OirType;
    let ty = match operand {
        SExpression::Number(_) => OirType::Number,
        SExpression::String(v) if !v.starts_with('@') && !v.starts_with('?') => OirType::String,
        _ => {
            let oir = crate::phases::resolve::resolve_sexpr_standalone(operand).ok()?;
            crate::type_infer::infer_oir_expr_type(&oir, ctx)?
        }
    };
    let ty = match ty {
        OirType::Optional { inner } => *inner,
        present => present,
    };
    match ty {
        OirType::Number | OirType::Integer => Some("number"),
        OirType::String
        | OirType::Email
        | OirType::Url
        | OirType::Phone
        | OirType::Uuid
        | OirType::Date
        | OirType::DateTime
        | OirType::Instant => Some("string"),
        _ => None,
    }
}

/// Binding-resolution context for expression-return inference (`set` + `emit`):
/// resolves a single-segment `@entity.<f>` / `@payload.<f>` binding to its
/// declared `OirType` from the validator's field-type maps. Deeper paths and
/// other roots resolve to `None` → the inferencer treats them as `Json`
/// (conservative).
pub(crate) struct ExprReturnCtx<'a> {
    pub(crate) entity: &'a HashMap<String, EntityFieldMeta>,
    pub(crate) payload: &'a HashMap<String, String>,
    /// The program, for the struct aliases its orbitals declare; `None` where a check has only the field maps.
    /// The full declared types of the bindings in scope. `None` for a consumer that only compares the primitive
    /// kinds (it never reports an undetermined value).
    pub(crate) types: Option<&'a BindingTypes>,
    pub(crate) schema: Option<&'a orbital_core_base::schema::OrbitalSchema>,
}

impl crate::type_infer::TypeCtx for ExprReturnCtx<'_> {
    fn binding_type(
        &self,
        binding: &orbital_core_base::ir::OirBinding,
    ) -> Option<orbital_core_base::ir::OirType> {
        use orbital_core_base::ir::{OirBindingRoot, OirField, OirType};
        // The whole `@entity` / `@payload` is a record of unknown extent: besides the declared fields a row carries
        // the system fields (id, timestamps, frame fields) and a payload carries whatever its emitter sent.
        match (&binding.root, self.types) {
            (OirBindingRoot::User, Some(types)) => {
                return orbital_core_base::signature::type_at_path(&types.user, &binding.path)
            }
            (OirBindingRoot::Config, Some(types)) => {
                return orbital_core_base::signature::type_at_path(
                    &types.config_struct(),
                    &binding.path,
                )
            }
            (OirBindingRoot::Local(name), Some(types)) => {
                return types
                    .locals
                    .get(name)
                    .and_then(|ty| orbital_core_base::signature::type_at_path(ty, &binding.path))
            }
            _ => {}
        }
        let whole = match binding.root {
            OirBindingRoot::Entity => "@entity",
            OirBindingRoot::Payload => "@payload",
            _ => return None,
        };
        // A consumer with the full declared types reads the whole record as what it is: the entity's declared fields
        // and the three system fields every row carries, or the payload's declared fields. One without them only
        // knows a record of unknown extent.
        if binding.path.is_empty() && self.types.is_none() {
            return Some(OirType::Row {
                entity: whole.to_string(),
            });
        }
        let root = match binding.root {
            OirBindingRoot::Entity => {
                let mut names: Vec<&String> = self.entity.keys().collect();
                names.sort();
                let mut fields: Vec<OirField> = names
                    .into_iter()
                    .map(|n| {
                        let meta = &self.entity[n];
                        let mut field = OirField::new(n.clone(), meta.shape.clone());
                        field.required = meta.required || meta.defaulted;
                        field
                    })
                    .collect();
                if self.types.is_some() {
                    for (system, ty) in [
                        ("id", OirType::String),
                        ("createdAt", OirType::DateTime),
                        ("updatedAt", OirType::DateTime),
                    ] {
                        if !fields.iter().any(|f| f.name == system) {
                            fields.push(OirField::new(system.to_string(), ty));
                        }
                    }
                }
                OirType::Struct {
                    additional: None,
                    fields: fields.into(),
                }
            }
            _ if self.types.is_some() => {
                self.types
                    .map(BindingTypes::payload_struct)
                    .unwrap_or(OirType::Struct {
                        additional: None,
                        fields: Default::default(),
                    })
            }
            _ => {
                // Only a payload field of a primitive kind has a type here: a struct-, list- or entity-typed one is
                // carried as a bare kind string, which says nothing of its shape.
                const PRIMITIVE_KINDS: [&str; 16] = [
                    "string",
                    "number",
                    "boolean",
                    "date",
                    "datetime",
                    "timestamp",
                    "instant",
                    "email",
                    "url",
                    "phone",
                    "uuid",
                    "image",
                    "money",
                    "bytes",
                    "int",
                    "float",
                ];
                let mut names: Vec<&String> = self
                    .payload
                    .iter()
                    .filter(|(_, kind)| {
                        PRIMITIVE_KINDS.contains(&kind.as_str())
                            || orbital_core_base::num_width::NumWidth::from_name(kind).is_some()
                    })
                    .map(|(n, _)| n)
                    .collect();
                names.sort();
                OirType::Struct {
                    additional: None,
                    fields: names
                        .into_iter()
                        .map(|n| {
                            OirField::new(
                                n.clone(),
                                orbital_core_base::type_compat::compat_kind_oir(&self.payload[n]),
                            )
                        })
                        .collect(),
                }
            }
        };
        orbital_core_base::signature::type_at_path(&root, &binding.path)
    }

    fn object_literals_are_exact(&self) -> bool {
        self.types.is_some()
    }

    fn may_be_absent(&self, binding: &orbital_core_base::ir::OirBinding) -> bool {
        use orbital_core_base::ir::OirBindingRoot;
        let [field] = binding.path.as_slice() else {
            return false;
        };
        match binding.root {
            OirBindingRoot::Entity => self
                .entity
                .get(field)
                .is_some_and(|meta| !meta.required && !meta.defaulted),
            OirBindingRoot::Payload => self
                .types
                .is_some_and(|types| types.optional_payload.contains(field)),
            _ => false,
        }
    }

    fn delivery_payload(&self) -> Option<orbital_core_base::ir::OirType> {
        self.types.map(BindingTypes::payload_struct)
    }

    fn prev_delivery_payload(&self) -> Option<orbital_core_base::ir::OirType> {
        self.types.and_then(|t| t.prev_payload.clone())
    }

    fn entity_type(&self, name: &str) -> Option<orbital_core_base::ir::OirType> {
        use orbital_core_base::ir::{OirField, OirType};
        use orbital_core_base::schema::EntityRef;
        let schema = self.schema?;
        let declared = schema.orbitals.iter().find_map(|o| {
            std::iter::once(&o.entity)
                .chain(o.auxiliary_entities.iter().flatten())
                .find_map(|e| match e {
                    EntityRef::Inline(def) if def.name == name => Some(def.fields.as_slice()),
                    _ => None,
                })
        });
        // An entity or identity the program expects from its host is a declared type too: its promised shape.
        let promised = || {
            schema.orbitals.iter().find_map(|o| {
                o.expects
                    .iter()
                    .flatten()
                    .find_map(|expectation| match expectation {
                        orbital_core_base::schema::ExpectDeclaration::Entity {
                            name: n,
                            shape: Some(shape),
                            ..
                        } if n == name => Some(shape.as_slice()),
                        orbital_core_base::schema::ExpectDeclaration::Identity {
                            name: Some(n),
                            shape: Some(shape),
                            ..
                        } if n == name => Some(shape.as_slice()),
                        _ => None,
                    })
            })
        };
        let source = declared.or_else(promised)?;
        let mut fields: Vec<OirField> = source
            .iter()
            .filter_map(|f| {
                let mut field = OirField::new(
                    f.name.clone()?,
                    orbital_core_base::ir::types::field_shape(f)
                        .unwrap_or_else(|_| OirType::erased()),
                );
                field.required = f.required.unwrap_or(false) || f.default.is_some();
                Some(field)
            })
            .collect();
        for (system, ty) in [
            ("id", OirType::String),
            ("createdAt", OirType::DateTime),
            ("updatedAt", OirType::DateTime),
        ] {
            if !fields.iter().any(|f| f.name == system) {
                fields.push(OirField::new(system.to_string(), ty));
            }
        }
        Some(OirType::Struct {
            additional: None,
            fields: fields.into(),
        })
    }

    fn named_type(&self, name: &str) -> Option<orbital_core_base::ir::OirType> {
        crate::phases::resolve::declared_struct_type(self.schema?, name)
            .ok()
            .flatten()
    }

    fn declared_type(&self, name: &str) -> Option<bool> {
        let schema = self.schema?;
        Some(
            self.types
                .is_some_and(|types| types.type_params.contains(name))
                || schema
                    .types
                    .as_ref()
                    .is_some_and(|types| types.contains_key(name))
                || schema
                    .orbitals
                    .iter()
                    .any(|o| o.types.as_ref().is_some_and(|t| t.contains_key(name)))
                || orbital_core_base::language::program_types::program_type(name).is_some(),
        )
    }
}

/// The full declared types of the bindings a transition reads: its event's payload fields, the viewer, the trait's
/// config knobs.
#[derive(Debug, Clone)]
pub(crate) struct BindingTypes {
    pub payload: HashMap<String, orbital_core_base::ir::OirType>,
    pub user: orbital_core_base::ir::OirType,
    pub config: HashMap<String, orbital_core_base::ir::OirType>,
    /// The payload shared by every event the trait listens to, when they agree.
    pub prev_payload: Option<orbital_core_base::ir::OirType>,
    /// The payload fields of the delivery the declaration does not require: an emitter may omit them.
    pub optional_payload: std::collections::HashSet<String>,
    /// The locals an enclosing effect-level `let` binds, by name: an effect nested in its body reads them.
    pub locals: HashMap<String, orbital_core_base::ir::OirType>,
    /// The trait's type parameters (`(p : Entity)`): each names the type a call site binds, so it is a declared type.
    pub type_params: std::collections::HashSet<String>,
}

impl BindingTypes {
    /// Types every payload field the event does not require as `T?`, so each reader sees one declared type.
    pub(crate) fn type_optional_payload(&mut self) {
        for name in &self.optional_payload {
            if let Some(shape) = self.payload.get_mut(name) {
                if !matches!(shape, orbital_core_base::ir::OirType::Optional { .. }) {
                    *shape = orbital_core_base::ir::OirType::optional(shape.clone());
                }
            }
        }
    }

    pub(crate) fn record_of(
        fields: &HashMap<String, orbital_core_base::ir::OirType>,
    ) -> orbital_core_base::ir::OirType {
        let mut fields: Vec<orbital_core_base::ir::OirField> = fields
            .iter()
            .map(|(n, t)| orbital_core_base::ir::OirField::new(n.clone(), t.clone()))
            .collect();
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        orbital_core_base::ir::OirType::Struct {
            additional: None,
            fields: fields.into(),
        }
    }

    pub(crate) fn payload_struct(&self) -> orbital_core_base::ir::OirType {
        Self::record_of(&self.payload)
    }

    pub(crate) fn config_struct(&self) -> orbital_core_base::ir::OirType {
        Self::record_of(&self.config)
    }
}

/// Extract `<field>` from a `@entity.<field>` string binding. Returns `None`
/// for path-expression forms or non-entity targets.
fn set_entity_field(target: &SExpression) -> Option<&str> {
    let SExpression::String(s) = target else {
        return None;
    };
    s.strip_prefix("@entity.").filter(|f| !f.is_empty())
}

/// Extract `<field>` from a `?<field>` or `@payload.<field>` read. Only a
/// single-segment field name is returned; nested paths are skipped.
fn payload_field_ref(value: &SExpression) -> Option<&str> {
    let SExpression::String(s) = value else {
        return None;
    };
    let field = if let Some(rest) = s.strip_prefix("@payload.") {
        rest
    } else if let Some(rest) = s.strip_prefix('?') {
        rest
    } else {
        return None;
    };
    if field.is_empty() || field.contains('.') {
        return None;
    }
    Some(field)
}

/// Per-field metadata the set-effect checks need: the field's declared type
/// (lowercase primitive string, for message text and the produced-shape
/// dialect) and its declared enum members (`values: [...]`, empty when the
/// field is not an enum — for the membership check). Both ride in one map so
/// the entity is walked once and the `validate_effect` signature stays a
/// single entity-field argument.
///
/// `field_type` is the FLAT `field_type_str` tag (`"array"`, `"relation"`,
/// …) — message text and the `== "relation"` / `== "union"` branches read
/// it. `shape` is the structural `OirType` (`orbital_core::field_shape`,
/// recursing into `items`) — every actual type-compat comparison reads it,
/// so an array element type compares structurally instead of by the erased
/// `"array"` tag.
pub(crate) struct EntityFieldMeta {
    pub field_type: String,
    pub shape: orbital_core_base::ir::OirType,
    pub required: bool,
    /// Whether the field declares a default, so a row never lacks it.
    pub defaulted: bool,
}

/// Build the `{ fieldName -> EntityFieldMeta }` map for an entity definition.
/// The type is the lowercase primitive string (matching `PayloadField.field_type`'s
/// vocabulary) so the two sides compare on the same alphabet. Fields without a
/// name (nested item/property carriers) are skipped.
pub(crate) fn entity_field_type_map(entity: &EntityDefinition) -> HashMap<String, EntityFieldMeta> {
    field_type_map(&entity.fields)
}

/// The same map over the fields an `expects entity` promises: the shape of a trait's linked entity no orbital declares.
pub(crate) fn field_type_map(
    fields: &[orbital_core_base::schema::FieldDefinition],
) -> HashMap<String, EntityFieldMeta> {
    fields
        .iter()
        .filter_map(|f| {
            let name = f.name.clone()?;
            let shape = orbital_core_base::ir::types::field_shape(f)
                .unwrap_or_else(|_| orbital_core_base::ir::OirType::erased());
            Some((
                name,
                EntityFieldMeta {
                    field_type: field_type_str(&f.field_type).to_string(),
                    shape,
                    required: f.required.unwrap_or(false),
                    defaulted: f.default.is_some(),
                },
            ))
        })
        .collect()
}

/// Map a `FieldType` to the lowercase primitive string used by
/// `PayloadField.field_type`. Mirrors the `#[serde(rename_all = "lowercase")]`
/// representation on `FieldType` — exhaustive, no fallthrough.
fn field_type_str(ft: &FieldType) -> std::borrow::Cow<'static, str> {
    match ft {
        FieldType::String => "string".into(),
        FieldType::Number => "number".into(),
        FieldType::Int => "int".into(),
        FieldType::Sized(width) => width.name().into(),
        FieldType::Boolean => "boolean".into(),
        FieldType::Date => "date".into(),
        FieldType::Datetime => "datetime".into(),
        FieldType::Timestamp => "timestamp".into(),
        FieldType::Instant => "instant".into(),
        FieldType::Email => "email".into(),
        FieldType::Url => "url".into(),
        FieldType::Phone => "phone".into(),
        FieldType::Uuid => "uuid".into(),
        FieldType::Image => "image".into(),
        FieldType::Money => "money".into(),
        FieldType::File => "file".into(),
        FieldType::Bytes => "bytes".into(),
        FieldType::CallKey => "callkey".into(),
        FieldType::Array | FieldType::Tuple => "array".into(),
        FieldType::Object => "object".into(),
        FieldType::Enum => "enum".into(),
        FieldType::Relation => "relation".into(),
        FieldType::Trait => "trait".into(),
        FieldType::Orbital => "orbital".into(),
        FieldType::Behavior => "behavior".into(),
        FieldType::Slot => "slot".into(),
        FieldType::Pattern => "pattern".into(),
        FieldType::Node => "node".into(),
        FieldType::Event => "event".into(),
        FieldType::EventAddress => "EventAddress".into(),
        FieldType::Field => "field".into(),
        FieldType::SExpr => "SExpr".into(),
        FieldType::Union => "union".into(),
        FieldType::TypeVar(v) => format!("${}", v.0).into(),
    }
}

#[cfg(test)]
mod return_type_tests {
    use super::*;

    fn field(name: &str, ty: &str) -> HashMap<String, EntityFieldMeta> {
        let mut m = HashMap::new();
        m.insert(
            name.to_string(),
            EntityFieldMeta {
                field_type: ty.into(),
                shape: orbital_core_base::type_compat::compat_kind_oir(ty),
                required: false,
                defaulted: false,
            },
        );
        m
    }

    fn set_call(field: &str, op: &str, args: &[&str]) -> Vec<SExpression> {
        let mut call = vec![SExpression::String(op.into())];
        call.extend(args.iter().map(|a| SExpression::String((*a).into())));
        vec![
            SExpression::String("set".into()),
            SExpression::string(format!("@entity.{field}")),
            SExpression::Array(call.into()),
        ]
    }

    fn typed_schema() -> orbital_core_base::schema::OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "collection": "es", "fields": [] },
                "traits": [],
                "pages": [],
                "types": {
                    "Point": [{ "name": "x", "type": "number" }],
                    "Points": [{ "name": "", "type": "[Point]" }]
                }
            }]
        }))
        .unwrap()
    }

    fn set_parsed_time_into(field: serde_json::Value, value: &str) -> usize {
        let entity: EntityDefinition = serde_json::from_value(serde_json::json!({
            "name": "E", "collection": "es", "fields": [field]
        }))
        .unwrap();
        let fields = entity_field_type_map(&entity);
        let source = |text: &str| -> SExpression { serde_json::from_str(text).unwrap() };
        let arr = vec![
            SExpression::String("set".into()),
            SExpression::string("@entity.t"),
            source(&format!(
                r#"["let", [["x", ["time/parse", "@entity.s"]]], ["if", ["=", "@x", null], null, {value}]]"#
            )),
        ];
        validate_set_return_type(&arr, "p", &fields, &HashMap::new(), None, None, None)
            .errors
            .len()
    }

    #[test]
    fn a_numeric_literal_set_into_a_number_field_must_fit_its_type() {
        let errors = |ty: &str, literal: f64| {
            let entity: EntityDefinition = serde_json::from_value(serde_json::json!({
                "name": "E", "collection": "es", "fields": [{ "name": "n", "type": ty, "nullable": true }]
            }))
            .unwrap();
            let arr = vec![
                SExpression::String("set".into()),
                SExpression::string("@entity.n"),
                SExpression::Number(literal),
            ];
            validate_set_return_type(
                &arr,
                "p",
                &entity_field_type_map(&entity),
                &HashMap::new(),
                None,
                None,
                None,
            )
            .errors
            .len()
        };
        assert_eq!(errors("u8", 255.0), 0);
        assert_eq!(errors("u8", 300.0), 1);
        assert_eq!(errors("u8", -1.0), 1);
        assert_eq!(errors("int", 2.5), 1, "a fraction is no int");
        assert_eq!(errors("int", 2.0), 0);
        assert_eq!(errors("number", 2.5), 0, "control: any number fits number");
        assert_eq!(
            errors("string", 3.0),
            0,
            "control: a non-numeric field is left to its own check"
        );
    }

    #[test]
    fn a_union_field_takes_a_parsed_time_and_never_mixes_implicitly() {
        let union = |nullable: bool| {
            serde_json::json!({
                "name": "t", "type": "union", "nullable": nullable, "values": ["date", "datetime"],
                "properties": { "date": { "name": "date", "type": "date" }, "datetime": { "name": "datetime", "type": "datetime" } }
            })
        };
        assert_eq!(
            set_parsed_time_into(union(true), r#""@x""#),
            0,
            "date | datetime into (date | datetime)?"
        );
        assert_eq!(
            set_parsed_time_into(
                serde_json::json!({ "name": "t", "type": "number", "nullable": true }),
                r#""@x""#
            ),
            1,
            "control: a number"
        );
        assert_eq!(
            set_parsed_time_into(
                serde_json::json!({ "name": "t", "type": "datetime", "nullable": true }),
                r#""@x""#
            ),
            1,
            "control: no implicit datetime"
        );
        assert_eq!(
            set_parsed_time_into(
                serde_json::json!({ "name": "t", "type": "instant", "nullable": true }),
                r#""@x""#
            ),
            1,
            "control: no implicit instant"
        );
        assert_eq!(
            set_parsed_time_into(
                serde_json::json!({ "name": "t", "type": "instant", "nullable": true }),
                r#"["as", "@x", "instant"]"#
            ),
            0,
            "as narrows to instant"
        );
    }

    #[test]
    fn json_parse_over_a_list_alias_into_a_list_field_is_not_an_error() {
        let schema = typed_schema();
        let mut arr = set_call("rows", "json/parse", &["[]", "Points"]);
        let parsed = arr.pop().unwrap();
        arr.push(SExpression::Array(
            vec![
                SExpression::String("default".into()),
                parsed,
                SExpression::Array(vec![].into()),
            ]
            .into(),
        ));
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("rows", "[object]"),
            &HashMap::new(),
            Some(&schema),
            None,
            None,
        );
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn control_json_parse_answers_null_on_a_misfit_so_a_required_list_field_refuses_it() {
        let schema = typed_schema();
        let arr = set_call("rows", "json/parse", &["[]", "Points"]);
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("rows", "[object]"),
            &HashMap::new(),
            Some(&schema),
            None,
            None,
        );
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
    }

    #[test]
    fn control_json_parse_over_a_struct_alias_into_a_list_field_is_an_error() {
        let schema = typed_schema();
        let arr = set_call("rows", "json/parse", &["{}", "Point"]);
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("rows", "[object]"),
            &HashMap::new(),
            Some(&schema),
            None,
            None,
        );
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
    }

    #[test]
    fn control_a_string_op_into_a_list_field_is_still_an_error() {
        let arr = set_call("rows", "str/concat", &["a", "b"]);
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("rows", "[object]"),
            &HashMap::new(),
            None,
            None,
            None,
        );
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
    }

    #[test]
    fn string_op_into_number_field_errors() {
        // `(set @entity.count (str/concat "a" "b"))` — str/concat returns string.
        let arr = set_call("count", "str/concat", &["a", "b"]);
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("count", "number"),
            &HashMap::new(),
            None,
            None,
            None,
        );
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
        assert_eq!(r.errors[0].code, ErrorCode::SetReturnTypeMismatch);
    }

    #[test]
    fn bytes_op_into_string_field_errors() {
        let arr = set_call("title", "bytes/from-string", &["hi"]);
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("title", "string"),
            &HashMap::new(),
            None,
            None,
            None,
        );
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
        assert_eq!(r.errors[0].code, ErrorCode::SetReturnTypeMismatch);
    }

    #[test]
    fn bytes_op_into_bytes_field_passes() {
        let arr = set_call("blob", "bytes/from-string", &["hi"]);
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("blob", "bytes"),
            &HashMap::new(),
            None,
            None,
            None,
        );
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn bytes_to_string_into_string_field_passes() {
        let arr = set_call("title", "bytes/to-string", &["AAEC"]);
        let r = validate_set_return_type(
            &arr,
            "p",
            &field("title", "string"),
            &HashMap::new(),
            None,
            None,
            None,
        );
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    }

    #[test]
    fn string_op_into_string_field_passes() {
        let arr = set_call("title", "str/concat", &["a", "b"]);
        assert!(validate_set_return_type(
            &arr,
            "p",
            &field("title", "string"),
            &HashMap::new(),
            None,
            None,
            None
        )
        .errors
        .is_empty());
    }

    #[test]
    fn input_dependent_op_is_conservative() {
        // object/get is input-dependent (object-key-lookup → Json); never rejects.
        let arr = set_call("count", "object/get", &["@entity.data", "k"]);
        assert!(validate_set_return_type(
            &arr,
            "p",
            &field("count", "number"),
            &HashMap::new(),
            None,
            None,
            None
        )
        .errors
        .is_empty());
    }

    #[test]
    fn literal_value_skips() {
        // a scalar literal is not array-form — handled by the type/enum checks.
        let arr = vec![
            SExpression::String("set".into()),
            SExpression::String("@entity.count".into()),
            SExpression::String("5".into()),
        ];
        assert!(validate_set_return_type(
            &arr,
            "p",
            &field("count", "number"),
            &HashMap::new(),
            None,
            None,
            None
        )
        .errors
        .is_empty());
    }

    #[test]
    fn binding_type_array_field_infers_element_type() {
        // `@entity.tags : [string]` must resolve to a real `Array<String>` —
        // not the erased `Array<Json>` — so a return-type comparison against
        // it sees the element type, not just "it's an array".
        use crate::type_infer::TypeCtx;
        use orbital_core_base::ir::{OirBinding, OirBindingRoot, OirType};

        let mut entity = field("tags", "[string]");
        entity.get_mut("tags").unwrap().required = true;
        let payload = HashMap::new();
        let ctx = ExprReturnCtx {
            entity: &entity,
            payload: &payload,
            types: None,
            schema: None,
        };
        let binding = OirBinding::new(OirBindingRoot::Entity, vec!["tags".to_string()]);
        assert_eq!(
            ctx.binding_type(&binding),
            Some(OirType::Array {
                inner: Box::new(OirType::String)
            })
        );
    }

    #[test]
    fn an_expected_entity_or_identity_is_a_declared_row_type() {
        use crate::type_infer::TypeCtx;
        use orbital_core_base::ir::OirType;

        let schema: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Own", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "expects": [
                    { "kind": "identity", "name": "Viewer", "shape": [{ "name": "role", "type": "string" }] },
                    { "kind": "entity", "name": "Note", "shape": [{ "name": "title", "type": "string" }] }
                ],
                "traits": [],
                "pages": []
            }]
        }))
        .expect("schema");
        let entity = HashMap::new();
        let payload = HashMap::new();
        let ctx = ExprReturnCtx {
            entity: &entity,
            payload: &payload,
            types: None,
            schema: Some(&schema),
        };
        let fields = |ty: Option<OirType>| match ty {
            Some(OirType::Struct { fields, .. }) => {
                fields.into_iter().map(|f| f.name).collect::<Vec<_>>()
            }
            other => panic!("a struct row, got {other:?}"),
        };
        assert!(fields(ctx.entity_type("Viewer")).contains(&"role".to_string()));
        assert!(fields(ctx.entity_type("Note")).contains(&"title".to_string()));
        assert!(
            fields(ctx.entity_type("Own")).contains(&"id".to_string()),
            "control: a declared entity still resolves"
        );
        assert_eq!(
            ctx.entity_type("Nobody"),
            None,
            "control: a name nothing declares or expects"
        );
    }

    #[test]
    fn a_language_program_type_is_a_declared_type_a_local_one_shadows() {
        use crate::type_infer::TypeCtx;
        use orbital_core_base::ir::OirType;

        let schema_with = |types: serde_json::Value| -> orbital_core_base::schema::OrbitalSchema {
            serde_json::from_value(serde_json::json!({
                "name": "App",
                "orbitals": [{
                    "name": "O",
                    "entity": { "name": "Own", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                    "types": types,
                    "traits": [],
                    "pages": []
                }]
            }))
            .expect("schema")
        };
        let fields = |ty: Option<OirType>| match ty {
            Some(OirType::Struct { fields, .. }) => {
                fields.into_iter().map(|f| f.name).collect::<Vec<_>>()
            }
            other => panic!("a struct, got {other:?}"),
        };
        let entity = HashMap::new();
        let payload = HashMap::new();

        let plain = schema_with(serde_json::json!({}));
        let ctx = ExprReturnCtx {
            entity: &entity,
            payload: &payload,
            types: None,
            schema: Some(&plain),
        };
        assert!(ctx.named_type("BehaviorRef").is_some());
        assert!(fields(ctx.named_type("Viewer")).contains(&"email".to_string()));
        assert_eq!(ctx.declared_type("Viewer"), Some(true));
        assert_eq!(
            ctx.named_type("Viewr"),
            None,
            "control: neither local nor a program type"
        );
        assert_eq!(ctx.declared_type("Viewr"), Some(false));

        let shadowed =
            schema_with(serde_json::json!({ "Viewer": [{ "name": "seat", "type": "number" }] }));
        let ctx = ExprReturnCtx {
            entity: &entity,
            payload: &payload,
            types: None,
            schema: Some(&shadowed),
        };
        assert_eq!(
            fields(ctx.named_type("Viewer")),
            vec!["seat".to_string()],
            "the local definition wins"
        );
    }
}

#[cfg(test)]
mod required_field_from_optional_knob_tests {
    use super::*;

    fn meta(ty: &str, required: bool) -> HashMap<String, EntityFieldMeta> {
        let mut m = HashMap::new();
        m.insert(
            "value".to_string(),
            EntityFieldMeta {
                field_type: ty.into(),
                shape: orbital_core_base::type_compat::compat_kind_oir(ty),
                required,
                defaulted: false,
            },
        );
        m
    }

    fn optional_meta(ty: &str) -> HashMap<String, EntityFieldMeta> {
        let mut m = meta(ty, false);
        if let Some(field) = m.get_mut("value") {
            field.shape = orbital_core_base::ir::OirType::optional(field.shape.clone());
        }
        m
    }

    fn set_effect(field: &str, value: &str) -> SExpression {
        SExpression::Array(
            vec![
                SExpression::String("set".into()),
                SExpression::string(format!("@entity.{field}")),
                SExpression::String(value.into()),
            ]
            .into(),
        )
    }

    #[test]
    fn unresolved_config_token_into_required_field_errors() {
        let effect = set_effect("value", "@config.value");
        let r = validate_set_required_field_from_optional_knob(
            &effect,
            "p",
            "InputBinding",
            &meta("string", true),
        );
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
        assert_eq!(
            r.errors[0].code,
            ErrorCode::SetRequiredFieldFromOptionalKnob
        );
        assert!(r.errors[0].message.contains("value"));
        assert!(r.errors[0].message.contains("InputBinding"));
    }

    #[test]
    fn unresolved_config_token_into_optional_field_passes() {
        let effect = set_effect("value", "@config.value");
        let r = validate_set_required_field_from_optional_knob(
            &effect,
            "p",
            "InputBinding",
            &optional_meta("string"),
        );
        assert!(r.errors.is_empty());
    }

    #[test]
    fn unresolved_config_token_into_a_non_required_non_null_field_errors() {
        let effect = set_effect("value", "@config.value");
        let r = validate_set_required_field_from_optional_knob(
            &effect,
            "p",
            "InputBinding",
            &meta("string", false),
        );
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
    }

    #[test]
    fn resolved_literal_value_passes() {
        // A knob WITH a default (or a call-site override) is substituted
        // before this ever runs — the literal that reaches here is the
        // resolved value, not `@config.<k>`.
        let effect = set_effect("value", "hello");
        let r = validate_set_required_field_from_optional_knob(
            &effect,
            "p",
            "InputBinding",
            &meta("string", true),
        );
        assert!(r.errors.is_empty());
    }

    #[test]
    fn non_set_effect_skips() {
        let effect = SExpression::Array(
            vec![
                SExpression::String("emit".into()),
                SExpression::String("DONE".into()),
            ]
            .into(),
        );
        let r = validate_set_required_field_from_optional_knob(
            &effect,
            "p",
            "InputBinding",
            &meta("string", true),
        );
        assert!(r.errors.is_empty());
    }

    #[test]
    fn unknown_field_skips() {
        let effect = set_effect("other", "@config.value");
        let r = validate_set_required_field_from_optional_knob(
            &effect,
            "p",
            "InputBinding",
            &meta("string", true),
        );
        assert!(r.errors.is_empty());
    }
}

#[cfg(test)]
mod comparison_operand_tests {
    use super::*;

    fn field(name: &str, ty: &str) -> HashMap<String, EntityFieldMeta> {
        let mut m = HashMap::new();
        m.insert(
            name.to_string(),
            EntityFieldMeta {
                field_type: ty.into(),
                shape: orbital_core_base::type_compat::compat_kind_oir(ty),
                required: false,
                defaulted: false,
            },
        );
        m
    }

    // ── ORB_X_COMPARISON_OPERAND_TYPE_MISMATCH ───────────────────────────────

    fn a(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items.into())
    }
    fn st(v: &str) -> SExpression {
        SExpression::String(v.into())
    }
    fn comparison_errors(expr: SExpression, entity: HashMap<String, EntityFieldMeta>) -> usize {
        validate_comparison_operands(&expr, "p", &entity, &HashMap::new())
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ComparisonOperandTypeMismatch)
            .count()
    }

    #[test]
    fn number_compared_with_a_string_is_an_error() {
        // std-notify-on-event quiet hours: (>= (time/hour @now) (str/slice … 0 2))
        let slice = a(vec![
            st("str/slice"),
            st("@entity.q"),
            SExpression::Number(0.0),
            SExpression::Number(2.0),
        ]);
        let expr = a(vec![st(">="), a(vec![st("time/hour"), st("@now")]), slice]);
        assert_eq!(comparison_errors(expr, field("q", "string")), 1);
    }

    #[test]
    fn a_nested_comparison_is_found_once() {
        let slice = a(vec![
            st("str/slice"),
            st("@entity.q"),
            SExpression::Number(0.0),
            SExpression::Number(2.0),
        ]);
        let cmp = a(vec![st("<"), a(vec![st("time/hour"), st("@now")]), slice]);
        let expr = a(vec![
            st("if"),
            a(vec![st("and"), cmp, SExpression::Bool(true)]),
            st("a"),
            st("b"),
        ]);
        assert_eq!(comparison_errors(expr, field("q", "string")), 1);
    }

    #[test]
    fn now_is_text_so_it_never_orders_against_a_number() {
        let against_number = a(vec![st("<"), st("@now"), SExpression::Number(5.0)]);
        assert_eq!(comparison_errors(against_number, HashMap::new()), 1);
        let against_text = a(vec![st("<"), st("@now"), st("2026-01-01T00:00:00Z")]);
        assert_eq!(
            comparison_errors(against_text, HashMap::new()),
            0,
            "control: ISO text against text"
        );
    }

    #[test]
    fn an_entity_number_against_a_string_literal_is_an_error() {
        let expr = a(vec![st(">"), st("@entity.count"), st("22")]);
        assert_eq!(comparison_errors(expr, field("count", "number")), 1);
    }

    #[test]
    fn control_same_kind_operands_pass() {
        let hour = a(vec![st("time/hour"), st("@now")]);
        assert_eq!(
            comparison_errors(
                a(vec![st(">="), hour, SExpression::Number(22.0)]),
                HashMap::new()
            ),
            0
        );
        let padded = a(vec![
            st("str/padStart"),
            a(vec![
                st("str/concat"),
                st(""),
                a(vec![st("time/hour"), st("@now")]),
            ]),
            SExpression::Number(2.0),
            st("0"),
        ]);
        let slice = a(vec![
            st("str/slice"),
            st("@entity.q"),
            SExpression::Number(0.0),
            SExpression::Number(2.0),
        ]);
        assert_eq!(
            comparison_errors(a(vec![st(">="), padded, slice]), field("q", "string")),
            0
        );
    }

    #[test]
    fn control_an_unknown_side_never_rejects() {
        // `@config.quietStartHour` is not resolvable here → inferred Json → skipped.
        let expr = a(vec![
            st(">="),
            a(vec![st("time/hour"), st("@now")]),
            st("@config.quietStartHour"),
        ]);
        assert_eq!(comparison_errors(expr, HashMap::new()), 0);
    }
}

#[cfg(test)]
mod set_value_type_tests {
    use super::*;
    use orbital_core_base::ir::{OirField, OirType};

    fn record(fields: &[(&str, OirType)]) -> OirType {
        OirType::Struct {
            additional: None,
            fields: fields
                .iter()
                .map(|(n, t)| OirField::new(n.to_string(), t.clone()))
                .collect(),
        }
    }

    fn check(entity_shape: OirType, kind: &str, payload_shape: OirType) -> ValidationResult {
        check_in(entity_shape, kind, payload_shape, None)
    }

    fn check_in(
        entity_shape: OirType,
        kind: &str,
        payload_shape: OirType,
        schema: Option<&OrbitalSchema>,
    ) -> ValidationResult {
        let entity = HashMap::from([(
            "out".to_string(),
            EntityFieldMeta {
                field_type: kind.into(),
                shape: entity_shape,
                required: false,
                defaulted: false,
            },
        )]);
        let payload = HashMap::from([("input".to_string(), payload_shape)]);
        let arr = vec![
            SExpression::String("set".into()),
            SExpression::String("@entity.out".into()),
            SExpression::String("?input".into()),
        ];
        validate_set_value_type(&arr, "p", &entity, &payload, schema, None, None)
    }

    fn st(s: &str) -> SExpression {
        SExpression::String(s.into())
    }

    fn call(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items.into())
    }

    fn check_guarded(value: &str, guard: Option<SExpression>) -> ValidationResult {
        let entity = HashMap::from([(
            "out".to_string(),
            EntityFieldMeta {
                field_type: "string".into(),
                shape: OirType::String,
                required: true,
                defaulted: false,
            },
        )]);
        let payload = HashMap::from([
            ("note".to_string(), OirType::optional(OirType::String)),
            ("other".to_string(), OirType::optional(OirType::String)),
        ]);
        let types = BindingTypes {
            payload: payload.clone(),
            user: OirType::Struct {
                additional: None,
                fields: Default::default(),
            },
            config: HashMap::new(),
            prev_payload: None,
            optional_payload: Default::default(),
            locals: HashMap::new(),
            type_params: Default::default(),
        };
        let arr = vec![st("set"), st("@entity.out"), st(value)];
        validate_set_value_type(
            &arr,
            "p",
            &entity,
            &payload,
            None,
            Some(&types),
            guard.as_ref(),
        )
    }

    fn present(field: &str) -> SExpression {
        call(vec![st("!="), st(field), SExpression::Null])
    }

    #[test]
    fn a_guard_proving_presence_narrows_a_payload_set() {
        for value in ["?note", "@payload.note"] {
            let r = check_guarded(value, Some(present("@payload.note")));
            assert!(r.errors.is_empty(), "{value}: {:?}", r.errors);
        }
        let both = call(vec![
            st("and"),
            present("@payload.other"),
            present("@payload.note"),
        ]);
        assert!(check_guarded("?note", Some(both)).errors.is_empty());
    }

    #[test]
    fn control_an_unguarded_optional_payload_set_is_refused() {
        let r = check_guarded("?note", None);
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.errors[0].code, ErrorCode::SetValueTypeMismatch);
    }

    fn check_declared_optional(
        optional: bool,
        target_shape: OirType,
        value: &str,
        guard: Option<SExpression>,
    ) -> ValidationResult {
        let entity = HashMap::from([(
            "id".to_string(),
            EntityFieldMeta {
                field_type: "string".into(),
                shape: target_shape,
                required: true,
                defaulted: false,
            },
        )]);
        let mut types = BindingTypes {
            payload: HashMap::from([("id".to_string(), OirType::String)]),
            user: OirType::Struct {
                additional: None,
                fields: Default::default(),
            },
            config: HashMap::new(),
            prev_payload: None,
            optional_payload: if optional {
                ["id".to_string()].into_iter().collect()
            } else {
                Default::default()
            },
            locals: HashMap::new(),
            type_params: Default::default(),
        };
        types.type_optional_payload();
        let payload = types.payload.clone();
        let arr = vec![st("set"), st("@entity.id"), st(value)];
        validate_set_value_type(
            &arr,
            "p",
            &entity,
            &payload,
            None,
            Some(&types),
            guard.as_ref(),
        )
    }

    #[test]
    fn g_orb_232_a_required_field_set_from_a_payload_field_the_event_declares_optional_is_refused() {
        for value in ["?id", "@payload.id"] {
            let r = check_declared_optional(true, OirType::String, value, None);
            assert_eq!(r.errors.len(), 1, "{value}: {:?}", r.errors);
            assert_eq!(r.errors[0].code, ErrorCode::SetValueTypeMismatch);
        }
        let r = check_declared_optional(true, OirType::String, "?id", Some(present("@payload.id")));
        assert!(r.errors.is_empty(), "control: a presence guard narrows: {:?}", r.errors);
        let r = check_declared_optional(false, OirType::String, "?id", None);
        assert!(r.errors.is_empty(), "control: a required (`!`) payload field fits: {:?}", r.errors);
        let r = check_declared_optional(true, OirType::optional(OirType::String), "?id", None);
        assert!(r.errors.is_empty(), "control: a nullable target takes it: {:?}", r.errors);
    }

    #[test]
    fn g_orb_232_a_default_that_fell_through_proves_the_payload_field_present() {
        let defaulted = |fallback: &str, compared: &str| {
            call(vec![
                st("!="),
                call(vec![st("default"), st("@payload.id"), st(fallback)]),
                st(compared),
            ])
        };
        let r = check_declared_optional(true, OirType::String, "?id", Some(defaulted("", "")));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let r = check_declared_optional(true, OirType::String, "?id", Some(defaulted("d", "d")));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let r = check_declared_optional(true, OirType::String, "?id", Some(defaulted("", "zz")));
        assert_eq!(r.errors.len(), 1, "control: a different literal proves nothing: {:?}", r.errors);
    }

    #[test]
    fn control_a_guard_on_a_different_field_does_not_narrow() {
        let r = check_guarded("?note", Some(present("@payload.other")));
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    }

    #[test]
    fn control_an_or_guard_that_does_not_prove_presence_does_not_narrow() {
        let either = call(vec![
            st("or"),
            present("@payload.note"),
            present("@payload.other"),
        ]);
        let r = check_guarded("?note", Some(either));
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    }

    #[test]
    fn a_mismatch_names_both_types_in_full() {
        let r = check(
            OirType::map(OirType::String, OirType::String),
            "object",
            OirType::map(OirType::String, OirType::optional(OirType::String)),
        );
        let message = &r.errors[0].message;
        assert!(message.contains("(type 'Map string string')"), "{message}");
        assert!(message.contains("(type 'Map string string?')"), "{message}");
    }

    #[test]
    fn record_payload_is_accepted_by_the_same_record_field() {
        let shape = record(&[("value", OirType::String)]);
        let r = check(shape.clone(), "object", shape);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn record_payload_missing_a_required_field_is_rejected() {
        let r = check(
            record(&[("value", OirType::String)]),
            "object",
            record(&[("other", OirType::String)]),
        );
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.errors[0].code, ErrorCode::SetValueTypeMismatch);
    }

    #[test]
    fn scalar_payload_is_rejected_by_a_number_field() {
        let r = check(OirType::Number, "number", OirType::String);
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.errors[0].code, ErrorCode::SetValueTypeMismatch);
    }

    fn schema_with_message_alias() -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "Chat",
                "entity": { "name": "Thread", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "types": { "M": [{ "name": "content", "type": "string", "required": true }] },
                "traits": [],
                "pages": []
            }]
        }))
        .expect("program json")
    }

    fn named(name: &str) -> OirType {
        OirType::TypeRef {
            name: name.to_string(),
        }
    }

    #[test]
    fn a_list_of_an_alias_is_accepted_by_a_list_field_of_the_expanded_alias() {
        let schema = schema_with_message_alias();
        let expanded = OirType::array(record(&[("content", OirType::String)]));
        let r = check_in(
            expanded.clone(),
            "array",
            OirType::array(named("M")),
            Some(&schema),
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let r = check_in(
            OirType::array(named("M")),
            "array",
            OirType::array(named("M")),
            Some(&schema),
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let r = check_in(OirType::array(named("M")), "array", expanded, Some(&schema));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn a_list_of_an_alias_is_rejected_by_a_list_field_of_a_different_member() {
        let schema = schema_with_message_alias();
        let other = OirType::array(record(&[("content", OirType::Number)]));
        let r = check_in(other, "array", OirType::array(named("M")), Some(&schema));
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.errors[0].code, ErrorCode::SetValueTypeMismatch);
    }

    #[test]
    fn a_string_operator_result_never_fills_a_call_key_field() {
        let concat = SExpression::Array(
            vec![
                SExpression::String("str/concat".into()),
                SExpression::String("search".into()),
                SExpression::String("x".into()),
            ]
            .into(),
        );
        let resolved = crate::phases::resolve::resolve_sexpr_standalone(&concat).expect("resolves");
        struct Empty;
        impl crate::type_infer::TypeCtx for Empty {
            fn binding_type(&self, _: &orbital_core_base::ir::OirBinding) -> Option<OirType> {
                None
            }
        }
        let inferred = crate::type_infer::infer_oir_expr_type(&resolved, &Empty);
        assert_eq!(
            inferred,
            Some(OirType::String),
            "str/concat infers a string"
        );
        let entity = HashMap::new();
        assert!(
            expr_return_incompatible(&concat, &OirType::CallKey, &entity, &HashMap::new(), None)
                .is_some(),
            "a string is not a call key"
        );
    }
}

#[cfg(test)]
mod absent_entity_read_tests {
    use super::*;
    use crate::type_infer::TypeCtx;
    use orbital_core_base::ir::{OirBinding, OirBindingRoot, OirType};

    fn entity() -> HashMap<String, EntityFieldMeta> {
        let entity: EntityDefinition = serde_json::from_value(serde_json::json!({
            "name": "E", "collection": "es", "fields": [
                { "name": "loose", "type": "string" },
                { "name": "needed", "type": "string", "required": true },
                { "name": "kept", "type": "string", "default": "" },
                { "name": "maybe", "type": "string", "nullable": true },
                { "name": "dst", "type": "string", "required": true },
                { "name": "slot", "type": "string", "nullable": true }
            ]
        }))
        .unwrap();
        entity_field_type_map(&entity)
    }

    fn read_type(field: &str) -> Option<OirType> {
        let fields = entity();
        let payload = HashMap::new();
        let ctx = ExprReturnCtx {
            entity: &fields,
            payload: &payload,
            types: None,
            schema: None,
        };
        ctx.binding_type(&OirBinding::new(
            OirBindingRoot::Entity,
            vec![field.to_string()],
        ))
    }

    fn set_errors(target: &str, value: &str, guard: Option<&str>) -> usize {
        let fields = entity();
        let source = |text: &str| -> SExpression { serde_json::from_str(text).unwrap() };
        let arr = vec![
            SExpression::String("set".into()),
            SExpression::string(format!("@entity.{target}")),
            source(value),
        ];
        let guard = guard.map(source);
        validate_set_return_type(
            &arr,
            "p",
            &fields,
            &HashMap::new(),
            None,
            None,
            guard.as_ref(),
        )
        .errors
        .len()
    }

    #[test]
    fn a_read_of_a_non_required_non_defaulted_field_types_as_optional() {
        assert_eq!(read_type("loose"), Some(OirType::optional(OirType::String)));
        assert_eq!(
            read_type("needed"),
            Some(OirType::String),
            "control: required"
        );
        assert_eq!(
            read_type("kept"),
            Some(OirType::String),
            "control: defaulted"
        );
        assert_eq!(
            read_type("maybe"),
            Some(OirType::optional(OirType::String)),
            "control: nullable stays one optional"
        );
    }

    #[test]
    fn an_unguarded_set_of_an_optional_read_into_a_non_null_field_is_refused() {
        assert_eq!(set_errors("dst", r#""@entity.loose""#, None), 1);
        assert_eq!(set_errors("dst", r#""@entity.maybe""#, None), 1);
        assert_eq!(
            set_errors("dst", r#""@entity.needed""#, None),
            0,
            "control: required read"
        );
        assert_eq!(
            set_errors("dst", r#""@entity.kept""#, None),
            0,
            "control: defaulted read"
        );
        assert_eq!(
            set_errors("slot", r#""@entity.loose""#, None),
            0,
            "control: a nullable target takes it"
        );
    }

    #[test]
    fn g_orbital_compiler_003_a_default_less_knob_set_into_a_non_null_field_is_refused() {
        let fields = entity();
        let knob_errors = |target: &str| {
            let effect: SExpression =
                serde_json::from_value(serde_json::json!(["set", format!("@entity.{target}"), "@config.k"])).unwrap();
            validate_set_required_field_from_optional_knob(&effect, "p", "T", &fields).errors.len()
        };
        assert_eq!(knob_errors("dst"), 1, "control: a required field is refused today");
        assert_eq!(knob_errors("slot"), 0, "control: a nullable field takes the null");
        assert_eq!(knob_errors("kept"), 1, "a defaulted, non-null field is refused by the kernel at dispatch");
        assert_eq!(knob_errors("loose"), 1, "a non-required, non-null field is refused by the kernel at dispatch");
    }

    #[test]
    fn a_narrowed_set_of_an_optional_read_is_accepted() {
        assert_eq!(
            set_errors(
                "dst",
                r#""@entity.loose""#,
                Some(r#"["!=", "@entity.loose", null]"#)
            ),
            0,
            "guard"
        );
        assert_eq!(
            set_errors("dst", r#"["default", "@entity.loose", ""]"#, None),
            0,
            "default"
        );
        assert_eq!(
            set_errors(
                "dst",
                r#"["if", ["!=", "@entity.loose", null], "@entity.loose", ""]"#,
                None
            ),
            0,
            "if"
        );
    }
}
