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

/// Two entity declarations over one `persistent:` collection must not declare
/// different access directives.
///
/// Access is a property of the DATA, so the runtime resolves policies by
/// collection: an entity that declares nothing inherits from a sibling bound to
/// the same collection, which is what stops an organism's `@read` from being
/// bypassed by a std-atom trait fetching its own entity on that collection
/// (`R-SHADOW-ENTITY-SHARES-COLLECTION-DIRECTIVES-DONT-APPLY`). Inheritance is
/// only well-defined while at most one declaration per collection exists; two
/// different ones are ambiguous and are rejected here rather than silently
/// resolved by declaration order.
fn validate_collection_policy_agreement(schema: &OrbitalSchema, result: &mut ValidationResult) {
    use std::collections::HashMap;

    // collection -> (entity that declared it, its four directives as JSON)
    let mut declared: HashMap<&str, (&str, Value)> = HashMap::new();

    for orbital in &schema.orbitals {
        for er in
            std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten())
        {
            let EntityRef::Inline(entity) = er else {
                continue;
            };
            let Some(collection) = entity.collection.as_deref() else {
                continue;
            };
            let directives = [
                &entity.read_policy,
                &entity.create_policy,
                &entity.update_policy,
                &entity.delete_policy,
            ];
            if directives.iter().all(|d| d.is_none()) {
                continue;
            }
            let Ok(shape) = serde_json::to_value(directives) else {
                continue;
            };

            match declared.get(collection) {
                None => {
                    declared.insert(collection, (entity.name.as_str(), shape));
                }
                Some((first, first_shape)) if *first_shape != shape => {
                    result.add_error(ValidationError::error(
                        ErrorCode::SchemaCollectionPolicyConflict,
                        format!("{}.@access", entity.name),
                        format!(
                            "entities `{}` and `{}` both bind collection `{}` but declare \
                             different access directives. One collection is one table and cannot \
                             have two visibility rules — give them identical directives, or bind \
                             one of them to its own collection.",
                            first, entity.name, collection
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
    }
}

pub fn validate_entity_access(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    validate_collection_policy_agreement(schema, &mut result);
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
            check_owner_field_identity_typed(items, path, owner, identity, result);
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

/// `["=", (object/get @entity F), @user.id]` — `F` must be a RELATION to the
/// `[identity]` entity.
///
/// This is what `ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED` was always specified to
/// mean; it was originally pointed at `std-row-access-control`'s `ownerField`
/// knob, and Phase D deleted every `RowAccessGate` mount, so it validated
/// nothing. Re-targeted at the access directive, which is the only sound
/// enforcement point left.
///
/// Why it matters: a plain scalar compared to `@user.id` type-checks and reads
/// like ownership, but the seeder only stamps viewer ids onto DECLARED
/// relations, so the predicate matches NO row and the policy silently denies
/// everyone. That is exactly the `std-helpdesk` `customerEmail` defect, found
/// by hand after it shipped.
fn check_owner_field_identity_typed(
    items: &[Value],
    path: &str,
    owner: &EntityDefinition,
    identity: &EntityDefinition,
    result: &mut ValidationResult,
) {
    let Some(Value::String(op)) = items.first() else {
        return;
    };
    if op != "=" && op != "==" {
        return;
    }
    let args = &items[1..];
    if args.len() < 2 {
        return;
    }
    // Both operand orders: whichever side reads the viewer's identity key.
    let compares_user_key = args.iter().any(|a| match a {
        Value::String(s) => s == "@user.id",
        Value::Array(inner) => matches!(
            (inner.first(), inner.get(1), inner.get(2)),
            (Some(Value::String(o)), Some(Value::String(r)), Some(Value::String(f)))
                if o == "object/get" && r == "@user" && f == "id"
        ),
        _ => false,
    });
    if !compares_user_key {
        return;
    }
    // …and whichever side reads a field off the candidate row, in either the
    // bare dotted form or the `object/get` call form.
    let Some(field_name) = args.iter().find_map(|a| match a {
        Value::String(s) => entity_field_of(s),
        Value::Array(inner) => match (inner.first(), inner.get(1), inner.get(2)) {
            (Some(Value::String(o)), Some(Value::String(r)), Some(Value::String(f)))
                if o == "object/get" && r == "@entity" =>
            {
                Some(f.as_str())
            }
            _ => None,
        },
        _ => None,
    }) else {
        return;
    };
    // `id` is the row's own key — an identity entity comparing itself to the
    // viewer is the self-read shape, not a mistyped owner column.
    if field_name == "id" {
        return;
    }
    let Some(field) = field_on(owner, field_name) else {
        return; // unknown field: SchemaAccessPolicyUnknownEntityField owns that
    };
    let targets_identity = field
        .relation
        .as_ref()
        .is_some_and(|r| r.entity == identity.name);
    if targets_identity {
        return;
    }
    result.add_error(
        ValidationError::error(
            ErrorCode::SchemaOwnerFieldNotIdentityTyped,
            path,
            format!(
                "`{}.{}` is compared to `@user.id` but is not a relation to the [identity] \
                 entity `{}` — it is `{:?}`. Viewer ids are only ever stamped onto declared \
                 relations, so this predicate matches no row and the directive denies everyone.",
                owner.name, field_name, identity.name, field.field_type
            ),
        )
        .with_suggestion(format!(
            "Type `{}` as a relation to {} (`{} : {}`), or compare against the field the \
             viewer value actually lives in",
            field_name, identity.name, field_name, identity.name
        )),
    );
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
