//! Validators for the four entity access directives — `@read`/`@create`/
//! `@update`/`@delete`.
//!
//! Each directive is a per-row predicate: `@entity` binds one row of the
//! OWNING entity (or the incoming create payload), `@user` binds the viewer.
//! No other binding root is meaningful here — an access directive has no
//! call site, so `@payload`/`@config`/`@state` (legal inside a guard or
//! effect) are errors inside one of these.
//!
//! Reuses `user_identity`'s field/vocabulary machinery for the `@user` half
//! of each directive (same "what does `@user` resolve to" answer, same
//! literal-membership rule) rather than re-deriving it.

use super::user_identity::{
    check_comparison, default_identity_shape, identity_entities, identity_field, user_field_of,
};
use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{EntityDefinition, EntityRef, FieldDefinition, OrbitalSchema};
use serde_json::Value;

/// `@entity.field` → `Some("field")`. Mirrors `user_identity::user_field_of`
/// but for the entity-row root.
fn entity_field_of(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("@entity.")?;
    if rest.is_empty() || rest.contains('.') || rest.contains('[') {
        return None;
    }
    Some(rest)
}

/// Look a field up on an arbitrary entity by name.
fn field_on<'a>(entity: &'a EntityDefinition, name: &str) -> Option<&'a FieldDefinition> {
    entity.fields.iter().find(|f| f.name.as_deref() == Some(name))
}

/// The bare sigil root of a binding string (`@entity.x` → `entity`, `@user` →
/// `user`), or `None` if `s` is not a sigil at all.
fn sigil_root(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('@')?;
    Some(rest.split(['.', '[']).next().unwrap_or(rest))
}

pub fn validate_entity_access(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    let identity_declared = !identity_entities(schema).is_empty();
    let default_identity;
    let identity = match identity_entities(schema).into_iter().next() {
        Some(identity) => identity,
        None => {
            default_identity = default_identity_shape();
            &default_identity
        }
    };

    for orbital in &schema.orbitals {
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
                let mut uses_user = false;
                walk(
                    &value,
                    &path,
                    entity,
                    identity,
                    &mut uses_user,
                    &mut result,
                );
                if uses_user && !identity_declared {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::SchemaAccessPolicyRequiresIdentity,
                            &path,
                            format!(
                                "`@{}` on entity `{}` reads `@user`, but this program declares no \
                                 [identity] entity. An access directive needs a real roster to \
                                 resolve against — declare `[identity]` on the entity that models \
                                 your app's viewer.",
                                sigil, entity.name
                            ),
                        )
                        .with_suggestion(
                            "Tag your user-directory entity `[persistent: <collection>, identity]`.",
                        ),
                    );
                }
            }
        }
    }

    ctx.merge(result);
}

fn walk(
    value: &Value,
    path: &str,
    owner: &EntityDefinition,
    identity: &EntityDefinition,
    uses_user: &mut bool,
    result: &mut ValidationResult,
) {
    match value {
        Value::String(s) => check_binding(s, path, owner, identity, uses_user, result),
        Value::Array(items) => {
            check_comparison(items, path, identity, result);
            check_object_get(items, path, owner, identity, uses_user, result);
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{}[{}]", path, i), owner, identity, uses_user, result);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                walk(v, &format!("{}.{}", path, k), owner, identity, uses_user, result);
            }
        }
        _ => {}
    }
}

/// A bare sigil-path string: `@entity.field`, `@user.field`, or something else.
fn check_binding(
    s: &str,
    path: &str,
    owner: &EntityDefinition,
    identity: &EntityDefinition,
    uses_user: &mut bool,
    result: &mut ValidationResult,
) {
    let Some(root) = sigil_root(s) else {
        return;
    };
    match root {
        "entity" => {
            if let Some(field) = entity_field_of(s) {
                check_entity_field(field, s, path, owner, result);
            }
        }
        "user" => {
            *uses_user = true;
            if let Some(field) = user_field_of(s) {
                if identity_field(identity, field).is_none() {
                    // Entity-level directives live outside any trait, so
                    // `validate_user_identity`'s trait-body walk never sees
                    // them — this is the one place that checks @user.<field>
                    // inside an access directive.
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
                                "`@user.{}` is not a field of the [identity] entity `{}`. It \
                                 resolves to null, so this access directive silently denies \
                                 everyone.",
                                field, identity.name
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
            }
        }
        _ => {
            result.add_error(
                ValidationError::error(
                    ErrorCode::SchemaAccessPolicyInvalidBinding,
                    path,
                    format!(
                        "`@{}` is not a legal binding inside an access directive — only `@entity` \
                         (the candidate row) and `@user` (the viewer) are. Access directives have \
                         no call site, so `@payload`/`@config`/`@state` are not meaningful here.",
                        root
                    ),
                )
                .with_suggestion("Read only @entity.<field> or @user.<field>."),
            );
        }
    }
}

/// `(object/get @entity field)` / `(object/get @user field)` — the call-shape
/// authors actually use in the corpus (`std-helpdesk.lolo`), alongside the
/// bare dotted-string form `check_binding` already handles.
fn check_object_get(
    items: &[Value],
    path: &str,
    owner: &EntityDefinition,
    identity: &EntityDefinition,
    uses_user: &mut bool,
    result: &mut ValidationResult,
) {
    let Some(Value::String(op)) = items.first() else {
        return;
    };
    if op != "object/get" || items.len() < 3 {
        return;
    }
    let Value::String(root_str) = &items[1] else {
        return;
    };
    let Value::String(field) = &items[2] else {
        return;
    };
    match root_str.as_str() {
        "@entity" => check_entity_field(field, root_str, path, owner, result),
        "@user" => {
            *uses_user = true;
            if identity_field(identity, field).is_none() {
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
                            "`(object/get @user \"{}\")` is not a field of the [identity] entity \
                             `{}`. It resolves to null, so this access directive silently denies \
                             everyone.",
                            field, identity.name
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
        }
        _ => {}
    }
}

fn check_entity_field(
    field: &str,
    binding_repr: &str,
    path: &str,
    owner: &EntityDefinition,
    result: &mut ValidationResult,
) {
    if field_on(owner, field).is_some() {
        return;
    }
    let mut declared: Vec<&str> = owner.fields.iter().filter_map(|f| f.name.as_deref()).collect();
    declared.sort_unstable();
    result.add_error(
        ValidationError::error(
            ErrorCode::SchemaAccessPolicyUnknownEntityField,
            path,
            format!(
                "`{}` is not a field of entity `{}`. It resolves to null, so this access \
                 directive silently denies everyone.",
                binding_repr, owner.name
            ),
        )
        .with_suggestion(format!(
            "Declare `{}` on entity {}, or read one of: {}",
            field,
            owner.name,
            declared.join(", ")
        )),
    );
}
