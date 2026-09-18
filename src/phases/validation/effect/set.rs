//! set effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::schema::{EntityDefinition, FieldType, SExpression};
use std::collections::HashMap;

/// Validate set effect (array form)
pub(crate) fn validate_set(
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["set", "@entity.field", value] or ["set", ["path", ...], value]
    if arr.len() < 3 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectSetInvalidBinding,
                path,
                "set requires binding and value",
            )
            .with_suggestion("Use: [\"set\", \"@entity.field\", value]"),
        );
        return result;
    }

    // Check binding - can be string OR path expression
    match &arr[1] {
        SExpression::String(binding) => {
            if !binding.starts_with('@') {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectSetInvalidBinding,
                        path,
                        format!("set binding must start with @: '{}'", binding),
                    )
                    .with_suggestion("Use @entity.field, @EntityName.field, or @config.field"),
                );
            } else if binding.starts_with("@payload") {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectSetPayloadNotAllowed,
                        path,
                        "set cannot target @payload (payload is read-only)",
                    )
                    .with_suggestion("Use @entity to update the entity state")
                    .with_fix_guidance("Replace @payload with @entity in your set binding"),
                );
            } else if binding == "@entity" || binding == "@state" || binding == "@now" {
                // Missing field path
                result.add_error(
                    ValidationError::error(
                        ErrorCode::BindingSetTargetMissingPath,
                        path,
                        format!("set binding '{}' requires a field path", binding),
                    )
                    .with_suggestion("Add a field path: @entity.fieldName"),
                );
            }
        }
        SExpression::Array(path_expr) if is_path_expression(path_expr) => {
            // ["path", "segment1", "@payload.segment2"] is valid for dynamic field paths
            // Validate that path expression has at least one segment
            if path_expr.len() < 2 {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectSetInvalidBinding,
                        path,
                        "path expression requires at least one segment",
                    )
                    .with_suggestion("Use: [\"path\", \"fieldName\", \"@payload.subField\"]"),
                );
            }
        }
        other => {
            // Render what was actually written — run 32: a computed set
            // target spun with no visibility into the offending expression.
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectSetInvalidBinding,
                    path,
                    format!(
                        "set's first argument must be a `@entity.field` binding, got {}",
                        crate::phases::validation::sexpr::render_sexpr_brief(other)
                    ),
                )
                .with_suggestion("write `(set @entity.<field> <value>)` — the target is always a literal binding, never a computed expression; to update a looked-up row use `persist update` with the row id"),
            );
        }
    }

    // Optional trailing options carrying `emit:` at position 3. `set` is
    // synchronous — only `success` is meaningful (fires after the write).
    if arr.len() > 3 {
        if let SExpression::Object(options) = &arr[3] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["success"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// Check if an S-expression is a path expression ["path", ...]
pub(crate) fn is_path_expression(arr: &[SExpression]) -> bool {
    if arr.is_empty() {
        return false;
    }
    matches!(&arr[0], SExpression::String(op) if op == "path")
}

/// SET-EMITS-PAYLOAD type backstop (raw-`.orb` defense-in-depth).
///
/// For `(set @entity.X ?payloadField)`, compare the entity field's declared
/// type to the payload field's declared type. The lolo crate catches this at
/// author time for `.lolo`; raw `.orb` IR (agent/factory output) bypasses that
/// path, so this validator pass is the backstop.
///
/// `entity_field_types` is the `{ fieldName -> type }` map of the trait's
/// linked entity (from its `EntityDefinition.fields`). `payload_field_types`
/// is the `{ fieldName -> type }` map sourced from the trait's
/// `emits[].payload_schema` and `listens` payload metadata. A comparison only
/// fires when BOTH sides carry a known declared type — an unknown type on
/// either side is left to the structural/name checks elsewhere.
pub(crate) fn validate_set_value_type(
    arr: &[SExpression],
    path: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
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

    let (Some(entity_meta), Some(payload_ty)) = (
        entity_field_types.get(entity_field),
        payload_field_types.get(payload_field),
    ) else {
        return result;
    };
    let entity_ty = &entity_meta.field_type;

    if !orbital_core::set_value_type_compatible(entity_ty, payload_ty) {
        result.add_error(
            ValidationError::error(
                ErrorCode::SetValueTypeMismatch,
                path,
                format!(
                    "set @entity.{} (type '{}') is assigned payload field '{}' (type '{}'); \
                     the declared types do not match.",
                    entity_field, entity_ty, payload_field, payload_ty
                ),
            )
            .with_suggestion(if entity_ty == "relation" {
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

/// REQUIRED-FIELD-FROM-OPTIONAL-KNOB backstop (rung 2, item 1 of the
/// orbital-import Stage C emitter-debt close-out).
///
/// `(set @entity.<field> @config.<knob>)` where the value is STILL the
/// literal `@config.<knob>` token post-inline — no declared default resolved
/// it and no call-site override supplied one; a substituted value would no
/// longer read as the bare token — and `<field>` is declared REQUIRED on the
/// trait's linked entity. The emitter has nothing honest to assign: it
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
    if !meta.required {
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
                "set @entity.{} (required) on trait '{}' is assigned config knob '{}', which \
                 declares no default and receives no call-site value here — the token is still \
                 unresolved and the field is left `undefined`.",
                entity_field, trait_name, knob
            ),
        )
        .with_suggestion(format!(
            "Give knob '{}' a default value on trait '{}' (`{} : <type> = <default>`), or make \
             field '{}' optional if it is honestly allowed to be unset.",
            knob, trait_name, knob, entity_field
        )),
    );

    result
}

/// ENUM-UNENFORCED check (Gap G-LOLO-ENUM-UNENFORCED).
///
/// For `(set @entity.<field> "literal")` where `<field>` declares an enum
/// (`values: [...]`), reject a literal that is not one of the declared members.
/// A declared string-union previously validated + emitted but was never
/// actually enforced, so an out-of-set value passed silently. Only a plain
/// string LITERAL is checkable — bindings (`@…`/`?…`) and operator calls are
/// dynamic and left to the type/return checks.
pub(crate) fn validate_set_enum_value(
    arr: &[SExpression],
    path: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
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
    if meta.enum_values.is_empty() {
        return result; // not an enum field
    }
    // A UNION's `values` carry its VARIANT NAMES (`scalar`, `FileItem`), not a
    // set of legal literals. Reading them as enum members rejected every value
    // the union exists to admit — `(set @entity.v "x")` against `scalar |
    // FileItem` was reported as "not a declared member of the field's enum".
    // Union assignment is checked structurally against the variants, not by
    // literal membership.
    if meta.field_type == "union" {
        return result;
    }
    // Only a plain string literal carries a checkable value.
    let SExpression::String(value) = &arr[2] else {
        return result;
    };
    if value.starts_with('@') || value.starts_with('?') {
        return result; // a binding/payload read, not a literal
    }

    if !orbital_core::enum_value_is_member(value, &meta.enum_values) {
        result.add_error(
            ValidationError::error(
                ErrorCode::EnumValueNotMember,
                path,
                format!(
                    "set @entity.{} assigns '{}', which is not a declared member of \
                     the field's enum.",
                    entity_field, value
                ),
            )
            .with_valid_values(meta.enum_values.clone())
            .with_suggestion(format!(
                "Use one of the declared values for '{}', or add '{}' to its enum.",
                entity_field, value
            )),
        );
    }

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
    if let Some(inferred) = expr_return_incompatible(
        &arr[2],
        &meta.field_type,
        entity_field_types,
        payload_field_types,
    ) {
        result.add_error(
            ValidationError::error(
                ErrorCode::SetReturnTypeMismatch,
                path,
                format!(
                    "set @entity.{} (type '{}') is assigned a value that evaluates to \
                     '{}'; the declared types are incompatible.",
                    entity_field,
                    meta.field_type,
                    orbital_core::oir_type_kind(&inferred)
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

/// Infer an array-form value expression's result type — through the canonical
/// `infer_oir_expr_type` derivation, resolving `@entity.<f>` / `@payload.<f>`
/// bindings via the field-type maps — and return `Some(inferred)` when the
/// inferred kind is INCOMPATIBLE with `declared_field`, else `None`. The single
/// shared derivation behind BOTH the `(set @entity.X <expr>)` and the
/// `(emit EVT { field: <expr> })` return-type backstops, so the two can't drift.
/// **Never false-positives**: a non-array value (scalar/binding — covered by the
/// literal/enum checks), an unresolvable shape, or an unknowable `Json` inferred
/// type all yield `None`.
pub(crate) fn expr_return_incompatible(
    value: &SExpression,
    declared_field: &str,
    entity_field_types: &HashMap<String, EntityFieldMeta>,
    payload_field_types: &HashMap<String, String>,
) -> Option<orbital_core::ir::OirType> {
    // Only an array-form value (operator call / conditional / array literal)
    // carries an inferrable result type worth checking here.
    if !matches!(value, SExpression::Array(_)) {
        return None;
    }
    let oir_value = crate::phases::resolve::resolve_sexpr_standalone(value).ok()?;
    let ctx = ExprReturnCtx {
        entity: entity_field_types,
        payload: payload_field_types,
    };
    let inferred = crate::type_infer::infer_oir_expr_type(&oir_value, &ctx);
    if std::env::var("ORB_DEBUG_SET_INFER").is_ok() {
        eprintln!(
            "SET_INFER declared={declared_field} inferred={inferred:?} value={value:?}"
        );
    }
    if orbital_core::oir_type_satisfies_field(&inferred, declared_field) {
        None
    } else {
        Some(inferred)
    }
}

/// Binding-resolution context for expression-return inference (`set` + `emit`):
/// resolves a single-segment `@entity.<f>` / `@payload.<f>` binding to its
/// declared `OirType` from the validator's field-type maps. Deeper paths and
/// other roots resolve to `None` → the inferencer treats them as `Json`
/// (conservative).
struct ExprReturnCtx<'a> {
    entity: &'a HashMap<String, EntityFieldMeta>,
    payload: &'a HashMap<String, String>,
}

impl crate::type_infer::TypeCtx for ExprReturnCtx<'_> {
    fn binding_type(
        &self,
        binding: &orbital_core::ir::OirBinding,
    ) -> Option<orbital_core::ir::OirType> {
        use orbital_core::ir::OirBindingRoot;
        if binding.path.len() != 1 {
            return None;
        }
        let field = &binding.path[0];
        let kind = match binding.root {
            OirBindingRoot::Entity => self.entity.get(field).map(|m| m.field_type.as_str())?,
            OirBindingRoot::Payload => self.payload.get(field).map(|s| s.as_str())?,
            _ => return None,
        };
        Some(compat_kind_to_oir(kind))
    }
}

/// Map a lowercase `FieldType` / payload kind string to the `OirType` the
/// inferencer reasons over. Object-ish / unknowable kinds collapse to `Json` so
/// compat stays conservative. A structural `[T]` spelling maps to `Array<T>`
/// (mirroring `parse_compat_type`'s bracket arm) so a binding read through an
/// entity/payload array field carries its element type into inference — the
/// erased `Array<Json>` would falsely reject a concrete `Array<String>` result
/// assigned back into a `[string]` field.
fn compat_kind_to_oir(kind: &str) -> orbital_core::ir::OirType {
    use orbital_core::ir::OirType;
    if kind.starts_with('[') && kind.ends_with(']') && kind.len() > 1 {
        let inner = match &kind[1..kind.len() - 1] {
            "string" => OirType::String,
            "number" | "float" => OirType::Number,
            "int" | "integer" => OirType::Integer,
            "boolean" => OirType::Boolean,
            _ => OirType::Json,
        };
        return OirType::Array {
            inner: Box::new(inner),
        };
    }
    match kind {
        "string" => OirType::String,
        "number" | "integer" => OirType::Number,
        "boolean" => OirType::Boolean,
        "date" => OirType::Date,
        "datetime" | "timestamp" => OirType::DateTime,
        "array" => OirType::Array {
            inner: Box::new(OirType::Json),
        },
        _ => OirType::Json,
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
/// (lowercase primitive string, for the type-compat check) and its declared
/// enum members (`values: [...]`, empty when the field is not an enum — for the
/// membership check). Both ride in one map so the entity is walked once and the
/// `validate_effect` signature stays a single entity-field argument.
///
/// `field_type` is STRUCTURAL for arrays whose `items` declare a primitive
/// element kind: `[string]` instead of the erased `array`, so the structural
/// array arm of `sub` compares element types against the payload side's
/// declared `[string]` instead of rejecting `Array<String> → Array<Json>`.
/// Dialects that only speak the flat tag (the produced-shape comparison)
/// read [`EntityFieldMeta::flat_kind`].
pub(crate) struct EntityFieldMeta {
    pub field_type: String,
    pub enum_values: Vec<String>,
    pub required: bool,
}

impl EntityFieldMeta {
    /// The erased kind tag: a structural `[T]` spelling reports as `array`.
    /// Used only by the produced-shape dialect, which compares coarse
    /// `field_type_str` tags (`produced_type_accepts`'s `"array"` arm).
    pub(crate) fn flat_kind(&self) -> &str {
        if self.field_type.starts_with('[') && self.field_type.ends_with(']') {
            "array"
        } else {
            self.field_type.as_str()
        }
    }
}

/// Build the `{ fieldName -> EntityFieldMeta }` map for an entity definition.
/// The type is the lowercase primitive string (matching `PayloadField.field_type`'s
/// vocabulary) so the two sides compare on the same alphabet. Fields without a
/// name (nested item/property carriers) are skipped.
pub(crate) fn entity_field_type_map(entity: &EntityDefinition) -> HashMap<String, EntityFieldMeta> {
    entity
        .fields
        .iter()
        .filter_map(|f| {
            f.name.clone().map(|name| {
                (
                    name,
                    EntityFieldMeta {
                        field_type: field_type_str_structural(f),
                        enum_values: f.values.clone().unwrap_or_default(),
                        required: f.required.unwrap_or(false),
                    },
                )
            })
        })
        .collect()
}

/// Map a `FieldType` to the lowercase primitive string used by
/// `PayloadField.field_type`. Mirrors the `#[serde(rename_all = "lowercase")]`
/// representation on `FieldType` — exhaustive, no fallthrough.
fn field_type_str(ft: &FieldType) -> &'static str {
    match ft {
        FieldType::String => "string",
        FieldType::Number => "number",
        FieldType::Boolean => "boolean",
        FieldType::Date => "date",
        FieldType::Datetime => "datetime",
        FieldType::Timestamp => "timestamp",
        FieldType::Email => "email",
        FieldType::Url => "url",
        FieldType::Phone => "phone",
        FieldType::Uuid => "uuid",
        FieldType::Image => "image",
        FieldType::Money => "money",
        FieldType::File => "file",
        FieldType::Array => "array",
        FieldType::Object => "object",
        FieldType::Enum => "enum",
        FieldType::Relation => "relation",
        FieldType::Trait => "trait",
        FieldType::Slot => "slot",
        FieldType::Pattern => "pattern",
        FieldType::Node => "node",
        FieldType::Event => "event",
        FieldType::Scalar => "scalar",
        FieldType::Union => "union",
    }
}

/// The comparison spelling for an entity field: the flat kind tag, except an
/// `array` field whose declared `items` carry a primitive element kind the
/// compat vocabulary understands (`string`/`number`/`boolean`) — that keeps
/// the `[T]` item-preserving spelling (`parse_compat_type`'s bracket arm), so
/// the structural array arm of `sub` compares ELEMENT types rather than the
/// erased `Array<Json>`. Elements of any other shape (objects, named structs,
/// nested arrays) fall back to the flat `array` tag — no structural claim the
/// erased-kind alphabet can't express.
fn field_type_str_structural(f: &orbital_core::schema::FieldDefinition) -> String {
    if let FieldType::Array = f.field_type {
        if let Some(items) = &f.items {
            let inner = match field_type_str(&items.field_type) {
                "string" => "string",
                "number" => "number",
                "boolean" => "boolean",
                _ => return "array".to_string(),
            };
            return format!("[{inner}]");
        }
    }
    field_type_str(&f.field_type).to_string()
}

/// Validate set effect (object form)
pub(crate) fn validate_set_object(target: &SExpression, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if let SExpression::String(binding) = target {
        if binding.starts_with("@payload") {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectSetPayloadNotAllowed,
                    path,
                    "set cannot target @payload (payload is read-only)",
                )
                .with_suggestion("Use @entity to update the entity state"),
            );
        }
    }

    result
}

#[cfg(test)]
mod enum_tests {
    use super::*;

    fn set(field: &str, value: SExpression) -> Vec<SExpression> {
        vec![
            SExpression::String("set".into()),
            SExpression::String(format!("@entity.{field}")),
            value,
        ]
    }

    fn status_enum() -> HashMap<String, EntityFieldMeta> {
        let mut m = HashMap::new();
        m.insert(
            "status".to_string(),
            EntityFieldMeta {
                field_type: "string".into(),
                enum_values: vec!["open".into(), "closed".into(), "pending".into()],
                required: false,
            },
        );
        m
    }

    #[test]
    fn non_member_literal_errors() {
        let arr = set("status", SExpression::String("zzz".into()));
        let r = validate_set_enum_value(&arr, "p", &status_enum());
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].code, ErrorCode::EnumValueNotMember);
    }

    #[test]
    fn member_literal_passes() {
        let arr = set("status", SExpression::String("closed".into()));
        assert!(validate_set_enum_value(&arr, "p", &status_enum())
            .errors
            .is_empty());
    }

    #[test]
    fn binding_value_skips() {
        // a `@payload`/`?` read is dynamic — not enum-checkable.
        let arr = set("status", SExpression::String("@payload.status".into()));
        assert!(validate_set_enum_value(&arr, "p", &status_enum())
            .errors
            .is_empty());
    }

    #[test]
    fn non_enum_field_skips() {
        let mut m = HashMap::new();
        m.insert(
            "title".to_string(),
            EntityFieldMeta {
                field_type: "string".into(),
                enum_values: vec![],
                required: false,
            },
        );
        let arr = set("title", SExpression::String("anything".into()));
        assert!(validate_set_enum_value(&arr, "p", &m).errors.is_empty());
    }

    /// Safety test 3's L2 half: a field whose `field_type` is `"union"` (a
    /// recursive struct's back-reference resolves through the SAME kind
    /// lattice slot a tagged union does — `oir_type_kind`'s `TypeRef` arm
    /// reports `"object"`, and `entity_field_type_map`'s callers only ever
    /// populate `enum_values` from a field's own `values` list, which a
    /// struct-typed field never carries) must never be read as a string enum
    /// — `(set @entity.subMenu "MenuItem")` assigning the alias's OWN name
    /// must not be waved through as a legal "enum member".
    #[test]
    fn union_typed_field_never_reads_as_enum_even_with_stray_values() {
        let mut m = HashMap::new();
        // Defensive: even if some future caller mistakenly populated
        // `enum_values` for a union-typed field (it shouldn't — a struct
        // union's declared "values" are variant NAMES, not legal literals),
        // the explicit `field_type == "union"` skip must still win.
        m.insert(
            "subMenu".to_string(),
            EntityFieldMeta {
                field_type: "union".into(),
                enum_values: vec!["MenuItem".into()],
                required: false,
            },
        );
        let arr = set("subMenu", SExpression::String("MenuItem".into()));
        assert!(
            validate_set_enum_value(&arr, "p", &m).errors.is_empty(),
            "a union-typed field must never be checked as a string enum"
        );
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    fn meta_map(name: &str, ty: &str) -> HashMap<String, EntityFieldMeta> {
        let mut m = HashMap::new();
        m.insert(
            name.to_string(),
            EntityFieldMeta {
                field_type: ty.into(),
                enum_values: vec![],
                required: false,
            },
        );
        m
    }

    fn set(field: &str, value: SExpression) -> Vec<SExpression> {
        vec![
            SExpression::String("set".into()),
            SExpression::String(format!("@entity.{field}")),
            value,
        ]
    }

    #[test]
    fn probe_binding_literal_array() {
        let arr = set(
            "queue",
            SExpression::Array(vec![SExpression::String("@config.seedId".into())]),
        );
        let r = validate_set_return_type(&arr, "p", &meta_map("queue", "[number]"), &HashMap::new());
        eprintln!("binding-literal errors: {:?}", r.errors);
    }

    #[test]
    fn probe_filter_identity() {
        let arr = set(
            "monsterQueue",
            SExpression::Array(vec![
                SExpression::String("array/filter".into()),
                SExpression::String("@entity.initiative".into()),
                SExpression::Array(vec![
                    SExpression::String("fn".into()),
                    SExpression::String("iid".into()),
                    SExpression::Bool(true),
                ]),
            ]),
        );
        let r = validate_set_return_type(&arr, "p", &meta_map("monsterQueue", "[string]"), &HashMap::new());
        eprintln!("filter errors: {:?}", r.errors);
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
                enum_values: vec![],
                required: false,
            },
        );
        m
    }

    fn set_call(field: &str, op: &str, args: &[&str]) -> Vec<SExpression> {
        let mut call = vec![SExpression::String(op.into())];
        call.extend(args.iter().map(|a| SExpression::String((*a).into())));
        vec![
            SExpression::String("set".into()),
            SExpression::String(format!("@entity.{field}")),
            SExpression::Array(call),
        ]
    }

    #[test]
    fn string_op_into_number_field_errors() {
        // `(set @entity.count (str/concat "a" "b"))` — str/concat returns string.
        let arr = set_call("count", "str/concat", &["a", "b"]);
        let r = validate_set_return_type(&arr, "p", &field("count", "number"), &HashMap::new());
        assert_eq!(r.errors.len(), 1, "errors: {:?}", r.errors);
        assert_eq!(r.errors[0].code, ErrorCode::SetReturnTypeMismatch);
    }

    #[test]
    fn string_op_into_string_field_passes() {
        let arr = set_call("title", "str/concat", &["a", "b"]);
        assert!(
            validate_set_return_type(&arr, "p", &field("title", "string"), &HashMap::new())
                .errors
                .is_empty()
        );
    }

    #[test]
    fn input_dependent_op_is_conservative() {
        // object/get is input-dependent (object-key-lookup → Json); never rejects.
        let arr = set_call("count", "object/get", &["@entity.data", "k"]);
        assert!(
            validate_set_return_type(&arr, "p", &field("count", "number"), &HashMap::new())
                .errors
                .is_empty()
        );
    }

    #[test]
    fn literal_value_skips() {
        // a scalar literal is not array-form — handled by the type/enum checks.
        let arr = vec![
            SExpression::String("set".into()),
            SExpression::String("@entity.count".into()),
            SExpression::String("5".into()),
        ];
        assert!(
            validate_set_return_type(&arr, "p", &field("count", "number"), &HashMap::new())
                .errors
                .is_empty()
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
                enum_values: vec![],
                required,
            },
        );
        m
    }

    fn set_effect(field: &str, value: &str) -> SExpression {
        SExpression::Array(vec![
            SExpression::String("set".into()),
            SExpression::String(format!("@entity.{field}")),
            SExpression::String(value.into()),
        ])
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
            &meta("string", false),
        );
        assert!(r.errors.is_empty());
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
        let effect = SExpression::Array(vec![
            SExpression::String("emit".into()),
            SExpression::String("DONE".into()),
        ]);
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
