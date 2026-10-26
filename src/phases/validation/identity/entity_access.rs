//! Validator for `@user.<field>` reads inside the four entity access
//! directives — `@read`/`@create`/`@update`/`@delete`.
//!
//! Entity-level directives live outside any trait, so `validate_user_identity`'s
//! trait-body walk never sees them; this is the one place that checks a
//! `@user.<field>` read inside one against the identity shape. The remaining
//! access-directive rules (owner columns, bindings, entity fields, role
//! literals) are the `orb lint entity-access` rules.

use super::user_identity::{
    default_identity_shape, identity_entities, identity_field, user_field_of, with_viewer,
    IdentitySource,
};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{EntityDefinition, EntityRef, OrbitalSchema};
use serde_json::Value;

pub fn validate_entity_access(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    let declared_identity = identity_entities(schema).into_iter().next();
    let default_identity = default_identity_shape();

    for orbital in &schema.orbitals {
        // Per-orbital identity environment: a declared [identity] entity wins
        // schema-wide; otherwise the orbital's own `expects entity [identity] …
        // { shape }` is its declared source of truth; otherwise the implicit
        // default shape (today's behavior for undeclared programs).
        let expectation = super::expectations::identity_expectation(orbital);
        let expectation_entity;
        let (identity, source) = match (declared_identity, &expectation) {
            (Some(identity), _) => (identity, IdentitySource::Declared),
            (None, Some(exp)) if exp.shape.is_some() => {
                expectation_entity = super::expectations::shape_as_identity_entity(
                    exp.name.unwrap_or("@user"),
                    exp.shape.unwrap_or(&[]),
                );
                (&expectation_entity, IdentitySource::Expectation)
            }
            (None, _) => (&default_identity, IdentitySource::Default),
        };
        let identity = &with_viewer(identity);

        for er in
            std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten())
        {
            let EntityRef::Inline(entity) = er else {
                continue;
            };
            for (sigil, policy) in [
                ("read", &entity.read_policy),
                ("create", &entity.create_policy),
                ("update", &entity.update_policy),
                ("delete", &entity.delete_policy),
            ] {
                let Some(expr) = policy else { continue };
                let path = format!("{}.@{}", entity.name, sigil);
                let Ok(value) = serde_json::to_value(expr) else {
                    continue;
                };
                walk(&value, &path, identity, source, &mut result);
            }
        }
    }

    ctx.merge(result);
}

fn walk(
    value: &Value,
    path: &str,
    identity: &EntityDefinition,
    source: IdentitySource,
    result: &mut ValidationResult,
) {
    match value {
        Value::String(s) => {
            if let Some(field) = user_field_of(s) {
                if identity_field(identity, field).is_none() {
                    unknown_user_field(s, field, path, identity, source, result);
                }
            }
        }
        Value::Array(items) => {
            check_object_get(items, path, identity, source, result);
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{}[{}]", path, i), identity, source, result);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                walk(v, &format!("{}.{}", path, k), identity, source, result);
            }
        }
        _ => {}
    }
}

/// An `@user.<field>` read inside an access directive that the identity shape
/// does not declare. An expectation-sourced shape reports through
/// `ORB_S_EXPECTATION_FIELD_UNDECLARED` (the lint rule), not here.
fn unknown_user_field(
    binding_repr: &str,
    field: &str,
    path: &str,
    identity: &EntityDefinition,
    source: IdentitySource,
    result: &mut ValidationResult,
) {
    if matches!(source, IdentitySource::Expectation) {
        return;
    }
    let mut declared: Vec<&str> = identity
        .fields
        .iter()
        .filter_map(|f| f.name.as_deref())
        .collect();
    declared.sort_unstable();
    result.add_error(
        ValidationError::error(
            ErrorCode::SchemaUnknownIdentityField,
            path,
            format!(
                "`{}` is not a field of the [identity] entity `{}`. It \
                 resolves to null, so this access directive silently denies \
                 everyone.",
                binding_repr, identity.name
            ),
        )
        .with_suggestion(format!(
            "Declare `{}` on entity {}, or read one of: {}",
            field,
            identity.name,
            declared.join(", ")
        )),
    );
}

/// `(object/get @user field)` — the call-shape authors actually use in the
/// corpus (`std-helpdesk.lolo`), alongside the bare dotted-string form.
fn check_object_get(
    items: &[Value],
    path: &str,
    identity: &EntityDefinition,
    source: IdentitySource,
    result: &mut ValidationResult,
) {
    let Some(Value::String(op)) = items.first() else {
        return;
    };
    if op != "object/get" || items.len() < 3 {
        return;
    }
    let (Value::String(root_str), Value::String(field)) = (&items[1], &items[2]) else {
        return;
    };
    if root_str == "@user" && identity_field(identity, field).is_none() {
        unknown_user_field(
            &format!("(object/get @user \"{}\")", field),
            field,
            path,
            identity,
            source,
            result,
        );
    }
}
