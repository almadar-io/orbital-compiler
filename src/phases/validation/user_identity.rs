//! Validators for the `[identity]` entity — the declared shape of `@user`.
//!
//! Named `user_identity` rather than `identity` because `identity`/`id_integrity`
//! already mean the Rabit V4 dual-carry NODE ids in this crate.
//!
//! A program with no `[identity]` entity is validated against the **implicit
//! default identity shape** — what `normalizeUserContext` produces (id, uid,
//! email, name) plus an unconstrained `role : string`, minus `permissions`
//! (`Almadar_LOLO_Identity.md` §11 "Also settled", lands with I.3). The corpus
//! reads `@user` on exactly those fields, and the std library's six `@user`
//! atoms read only `id`/`role`, so the default keeps every existing program
//! compiling while making a typo'd field (which nothing can ever carry) an
//! error. `role` carries no declared members in the default, so no literal
//! comparison can be rejected without a declaration — the membership win stays
//! exactly proportional to declaring.
//!
//! The walk is over the whole serialized trait rather than over guards alone.
//! std-healthcare's 13 dead `"patient"` comparisons live in **config knob
//! defaults** (`navItems : [NavItem] = (if ["=", @user.role, "patient"] …)`),
//! not in transitions, so a guard-only walker would report zero problems on the
//! very organism that motivated the rule.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{EntityDefinition, EntityRef, FieldDefinition, OrbitalSchema};
use orbital_core::type_compat::enum_value_is_member;
use serde_json::Value;

/// The sigil root this module is about.
const USER_SIGIL: &str = "@user.";

/// Comparison operators whose two operands are compared for equality. `=` and
/// `==` both occur in committed `.orb` files. Shared with `entity_access.rs`.
pub(super) const EQUALITY_OPS: &[&str] = &["=", "==", "!=", "!==", "eq", "neq"];

/// Every `[identity]` entity declared anywhere in the schema. Shared with
/// `entity_access.rs`, which needs the same "what does `@user` resolve to"
/// answer for the four access directives.
pub(super) fn identity_entities(schema: &OrbitalSchema) -> Vec<&EntityDefinition> {
    let mut out = Vec::new();
    for orbital in &schema.orbitals {
        for er in std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten())
        {
            if let EntityRef::Inline(def) = er {
                if def.identity {
                    out.push(def);
                }
            }
        }
    }
    out
}

/// The declared vocabulary for an identity field, following `items` for the
/// array form. `roles : ["a" | "b"]` keeps the union on the ELEMENT, so a
/// multi-valued role field must be read through `items`.
pub(super) fn vocabulary(field: &FieldDefinition) -> Option<&Vec<String>> {
    if let Some(values) = field.values.as_ref().filter(|v| !v.is_empty()) {
        return Some(values);
    }
    field
        .items
        .as_ref()
        .and_then(|item| item.values.as_ref())
        .filter(|v| !v.is_empty())
}

/// Look a field up on the identity entity by name.
pub(super) fn identity_field<'a>(identity: &'a EntityDefinition, name: &str) -> Option<&'a FieldDefinition> {
    identity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some(name))
}

/// `@user.role` → `Some("role")`. Only a direct field read; `@user` alone and
/// deeper paths (`@user.profile.x`) are not field claims this rule can judge.
pub(super) fn user_field_of(s: &str) -> Option<&str> {
    let rest = s.strip_prefix(USER_SIGIL)?;
    if rest.is_empty() || rest.contains('.') || rest.contains('[') {
        return None;
    }
    Some(rest)
}

pub fn validate_user_identity(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    let declared = identity_entities(schema);

    // Two DIFFERENTLY-NAMED identity entities. Same-name duplicates across
    // orbitals are already a hard error upstream (`validate_entity_uniqueness`).
    let mut names: Vec<&str> = declared.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    if names.len() > 1 {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaIdentityNotUnique,
                "orbitals",
                format!(
                    "{} entities are tagged [identity]: {}. `@user` resolves to a single viewer, \
                     so more than one declaration makes every `@user.x` ambiguous.",
                    names.len(),
                    names.join(", ")
                ),
            )
            .with_suggestion(
                "Keep exactly one [identity] entity and reference it from the other orbitals \
                 (`ownerId : <IdentityEntity>`).",
            ),
        );
    }

    // Undeclared identity ⇒ `@user` still carries the implicit DEFAULT shape —
    // what `normalizeUserContext` produces (id, uid, email, name) plus an
    // unconstrained `role : string`, minus `permissions`
    // (`Almadar_LOLO_Identity.md` §11 "Also settled"). `role` has no declared
    // members, so no literal comparison can be rejected; only a field outside
    // the shape (a typo nothing can ever carry) is an error.
    let default_identity;
    let (identity, declared_in_source) = match declared.first() {
        Some(identity) => (*identity, true),
        None => {
            default_identity = default_identity_shape();
            (&default_identity, false)
        }
    };

    for (trait_def, path) in super::inline_traits(schema) {
        let Ok(value) = serde_json::to_value(trait_def) else {
            continue;
        };
        walk(&value, &path, identity, declared_in_source, &mut result);
    }

    ctx.merge(result);
}

/// The implicit `@user` shape of a program with no `[identity]` entity.
pub(super) fn default_identity_shape() -> EntityDefinition {
    serde_json::from_value(serde_json::json!({
        "name": "@user",
        "fields": [
            { "name": "id", "type": "string" },
            { "name": "uid", "type": "string" },
            { "name": "email", "type": "string" },
            { "name": "name", "type": "string" },
            { "name": "role", "type": "string" }
        ]
    }))
    .expect("default identity shape must deserialize")
}

fn walk(
    value: &Value,
    path: &str,
    identity: &EntityDefinition,
    declared_in_source: bool,
    result: &mut ValidationResult,
) {
    match value {
        Value::String(s) => check_field_exists(s, path, identity, declared_in_source, result),
        Value::Array(items) => {
            check_comparison(items, path, identity, result);
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{}[{}]", path, i), identity, declared_in_source, result);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                walk(v, &format!("{}.{}", path, k), identity, declared_in_source, result);
            }
        }
        _ => {}
    }
}

/// `@user.<field>` must name a declared field of the identity entity — or, when
/// no `[identity]` entity is declared, a field of the implicit default shape.
fn check_field_exists(
    s: &str,
    path: &str,
    identity: &EntityDefinition,
    declared_in_source: bool,
    result: &mut ValidationResult,
) {
    let Some(field) = user_field_of(s) else {
        return;
    };
    if identity_field(identity, field).is_some() {
        return;
    }
    let mut declared: Vec<&str> = identity
        .fields
        .iter()
        .filter_map(|f| f.name.as_deref())
        .collect();
    declared.sort_unstable();
    let (message, suggestion) = if declared_in_source {
        (
            format!(
                "`@user.{}` is not a field of the [identity] entity `{}`. It resolves to null, \
                 so every branch reading it silently takes its negative arm.",
                field, identity.name
            ),
            format!(
                "Declare `{}` on entity {}, or read one of: {}",
                field,
                identity.name,
                declared.join(", ")
            ),
        )
    } else {
        (
            format!(
                "`@user.{}` is not part of the default identity shape ({}). No [identity] \
                 entity is declared, so nothing can carry this field — it resolves to null and \
                 every branch reading it silently takes its negative arm.",
                field,
                declared.join(", ")
            ),
            format!(
                "Declare an [identity] entity carrying `{}`, or read one of: {}",
                field,
                declared.join(", ")
            ),
        )
    };
    result.add_error(
        ValidationError::error(ErrorCode::SchemaUnknownIdentityField, path, message)
            .with_suggestion(suggestion),
    );
}

/// A literal compared against a closed-vocabulary `@user.<field>` must be one of
/// that field's declared members.
pub(super) fn check_comparison(
    items: &[Value],
    path: &str,
    identity: &EntityDefinition,
    result: &mut ValidationResult,
) {
    let Some(Value::String(op)) = items.first() else {
        return;
    };
    let is_equality = EQUALITY_OPS.contains(&op.as_str());
    let is_includes = op == "array/includes";
    if !is_equality && !is_includes {
        return;
    }
    let args = &items[1..];
    if args.len() < 2 {
        return;
    }

    // `array/includes` is directional: the vocabulary side is the HAYSTACK.
    let (sigil_side, literal_side): (&[Value], &[Value]) = if is_includes {
        (&args[..1], &args[1..2])
    } else {
        (args, args)
    };

    let Some((field_name, field)) = sigil_side.iter().find_map(|arg| {
        let Value::String(s) = arg else { return None };
        let name = user_field_of(s)?;
        identity_field(identity, name).map(|f| (name, f))
    }) else {
        return;
    };
    let Some(vocab) = vocabulary(field) else {
        return;
    };

    for arg in literal_side {
        let Value::String(literal) = arg else {
            continue;
        };
        // Skip the sigil operand itself and any other binding.
        if literal.starts_with('@') {
            continue;
        }
        if enum_value_is_member(literal, vocab) {
            continue;
        }
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaRoleLiteralNotMember,
                path,
                format!(
                    "\"{}\" is not a declared value of `@user.{}` on the [identity] entity `{}`. \
                     This comparison can never be true — the branch is dead.",
                    literal, field_name, identity.name
                ),
            )
            .with_valid_values(vocab.clone())
            .with_suggestion(format!(
                "Use one of: {}, or add \"{}\" to `{}.{}`.",
                vocab.join(", "),
                literal,
                identity.name,
                field_name
            )),
        );
    }
}
