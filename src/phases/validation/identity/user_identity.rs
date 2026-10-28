//! Validators for the `[identity]` entity — the declared shape of `@user`.
//!
//! Named `user_identity` rather than `identity` because `identity`/id-integrity
//! already mean the Rabit V4 dual-carry NODE ids in this crate.
//!
//! `@user` is the `Viewer` (the installed language's program type) plus the
//! program's `[identity]` row: a program with no `[identity]` entity reads the
//! viewer alone. `role` carries no declared members on the viewer, so no
//! literal comparison can be rejected without a declaration — the membership
//! win stays exactly proportional to declaring.
//!
//! The walk is over the whole serialized trait rather than over guards alone.
//! std-healthcare's 13 dead `"patient"` comparisons live in **config knob
//! defaults** (`navItems : [NavItem] = (if ["=", @user.role, "patient"] …)`),
//! not in transitions, so a guard-only walker would report zero problems on the
//! very organism that motivated the rule.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{
    EntityDefinition, EntityRef, Orbital, OrbitalSchema, TraitReference,
};
use orbital_core_base::type_compat::enum_value_is_member;
use serde_json::Value;

/// Where the identity shape a `@user` check resolves against came from. An
/// `expects entity X [identity] { shape }` is the consumer's own declared source of
/// truth — reads outside it belong to `ORB_S_EXPECTATION_FIELD_UNDECLARED`
/// (the lint rule), not the unknown-identity-field code.
#[derive(Clone, Copy)]
pub(super) enum IdentitySource {
    /// A real `[identity]` entity declared in the schema.
    Declared,
    /// No declaration anywhere — the implicit default shape.
    Default,
    /// The orbital's own `expects entity [identity]` declaration (shape present).
    Expectation,
}

/// Shared `@user.<f>` shape readers; they live in `orbital-core-base` so
/// `role_satisfying_policy` reads the same shape.
pub(crate) use orbital_core_base::entity_vocabulary::{
    identity_field, user_field_of, vocabulary, EQUALITY_OPS,
};

/// Every `[identity]`-tagged entity in `orbitals`, primaries first, each
/// paired with whether it is an orbital's PRIMARY entity (`true`) or an
/// auxiliary copy pulled in by an import (`false`). The schema-wide and
/// orbital-slice callers (Stage B's
/// `identity_entities_of`) both apply the SAME primary-shadows-import rule
/// over whatever orbital slice they're given — this is the one walker.
fn identity_entities_tagged_of(orbitals: &[Orbital]) -> Vec<(&EntityDefinition, bool)> {
    let mut out = Vec::new();
    for orbital in orbitals {
        if let EntityRef::Inline(def) = &orbital.entity {
            if def.identity {
                out.push((def, true));
            }
        }
    }
    for orbital in orbitals {
        for er in orbital.auxiliary_entities.iter().flatten() {
            if let EntityRef::Inline(def) = er {
                if def.identity {
                    out.push((def, false));
                }
            }
        }
    }
    out
}

/// The `[identity]` entities that decide what `@user` resolves to, computed
/// over an arbitrary orbital slice — same primary-shadows-import rule
/// `identity_entities` applies schema-wide (see its doc comment for the
/// rationale). Stage B's orbital-import splice calls this over one alias's
/// `AliasEntry.orbitals` to find the UPSTREAM roster a `roles {}` remap
/// resolves against, independent of the consumer schema's own roster.
pub(crate) fn identity_entities_of(orbitals: &[Orbital]) -> Vec<&EntityDefinition> {
    let tagged = identity_entities_tagged_of(orbitals);
    let has_primary = tagged.iter().any(|(_, primary)| *primary);
    tagged
        .into_iter()
        .filter(|(_, primary)| *primary || !has_primary)
        .map(|(def, _)| def)
        .collect()
}

/// The `[identity]` entities that decide what `@user` resolves to.
///
/// A behavior declares its own roster so it runs standalone — that entity is
/// its DEFAULT viewer. Composing the behavior never imports the orbital that
/// owns it, but a trait bound to one of its siblings drags the roster along as
/// an auxiliary copy (Gap #22), tag and all, which would read as a second
/// roster. So a PRIMARY roster shadows every imported copy: the composing app
/// declares who the viewer is, and the copies yield.
///
/// With no primary roster the copies still count — a thin app that wraps one
/// behavior and declares no roster of its own inherits that behavior's, and
/// two imported rosters with no host roster still collide, because nothing in
/// the program says which one `@user` is.
///
/// Shared with `entity_access.rs`, which needs the same answer for the four
/// access directives.
pub(crate) fn identity_entities(schema: &OrbitalSchema) -> Vec<&EntityDefinition> {
    identity_entities_of(&schema.orbitals)
}

pub fn validate_user_identity(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    let declared = identity_entities(schema);

    // Undeclared identity ⇒ `@user` still carries the implicit DEFAULT shape —
    // what `normalizeUserContext` produces (id, uid, email, name) plus an
    // unconstrained `role : string`, minus `permissions`
    // (`Almadar_LOLO_Identity.md` §11 "Also settled"). `role` has no declared
    // members, so no literal comparison can be rejected; only a field outside
    // the shape (a typo nothing can ever carry) is an error.
    //
    // Per-orbital resolution: a declared `[identity]` entity wins schema-wide.
    // Otherwise an orbital's own `expects entity [identity] … { shape }` is its declared
    // source of truth (self-consistency: reads outside the declared shape fail
    // against the orbital's OWN declaration). A shape-less `expects entity [identity]`
    // leaves the default shape in place — existence-only, as unchecked as today.
    let default_identity = default_identity_shape();
    let declared_identity = declared.first().copied();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let expectation_entity;
        let (identity, source) = match declared_identity {
            Some(identity) => (identity, IdentitySource::Declared),
            None => match super::expectations::identity_expectation(orbital) {
                Some(exp) if exp.shape.is_some() => {
                    expectation_entity = super::expectations::shape_as_identity_entity(
                        exp.name.unwrap_or("@user"),
                        exp.shape.unwrap_or(&[]),
                    );
                    (&expectation_entity, IdentitySource::Expectation)
                }
                _ => (&default_identity, IdentitySource::Default),
            },
        };
        let identity = &with_viewer(identity);
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            // An `expects entity [identity]` declaration covers the orbital's AUTHORED
            // surface — its own inline traits and call-site config. Bodies
            // inlined from `uses[]` imports carry `source_behavior`; their
            // internal `@user` reads are the imported behavior's own contract
            // and invisible to derivation, so checking them against the
            // derived declaration manufactures false mismatches
            // (G-EXPECT-DERIVE-IMPORTS: std-browse's scopeField filter reads
            // `@user.id` in every BrowseList consumer → spurious
            // FIELD_UNDECLARED → demote on a params-{} factory build).
            if matches!(source, IdentitySource::Expectation) && trait_def.source_behavior.is_some()
            {
                continue;
            }
            let path = super::trait_path(i, &orbital.name, j, trait_ref);
            let Ok(value) = serde_json::to_value(trait_def) else {
                continue;
            };
            walk(&value, &path, identity, source, &mut result);
            check_config_role_lists(trait_def, &path, identity, &mut result);
        }
    }

    ctx.merge(result);
}

/// Every literal of a config-held role list (`allowedRoles`, nav-item / action
/// `roles`, nested `children` roles) must be a declared member of the role field
/// the trait tests it against. Sites come from the same detector the
/// orbital-import remap uses (`inline::rewrite::role_list_sites`), so a list
/// that is never compared to a role source is never checked.
fn check_config_role_lists(
    trait_def: &orbital_core_base::schema::TraitDefinition,
    path: &str,
    identity: &EntityDefinition,
    result: &mut ValidationResult,
) {
    use crate::phases::inline::rewrite::{role_list_literals, role_list_sites};
    use std::collections::HashMap;
    let mut role_fields: HashMap<String, Vec<String>> = HashMap::new();
    for field in &identity.fields {
        if let (Some(name), Some(values)) = (field.name.as_ref(), vocabulary(field)) {
            role_fields.insert(name.clone(), values.clone());
        }
    }
    if role_fields.is_empty() {
        return;
    }
    let Some(cfg) = trait_def.config.as_ref() else {
        return;
    };
    for site in role_list_sites(trait_def, &role_fields) {
        let Some(default) = cfg.get(&site.knob).and_then(|f| f.default.as_ref()) else {
            continue;
        };
        let Some(vocab) = role_fields.get(&site.field) else {
            continue;
        };
        for literal in role_list_literals(default, &site) {
            if enum_value_is_member(&literal, vocab) {
                continue;
            }
            result.add_error(role_literal_error(
                &literal,
                &site.field,
                identity,
                vocab,
                &format!("{}.config.{}", path, site.knob),
            ));
        }
    }
}

fn role_literal_error(
    literal: &str,
    field_name: &str,
    identity: &EntityDefinition,
    vocab: &[String],
    path: &str,
) -> ValidationError {
    ValidationError::error(
        ErrorCode::SchemaRoleLiteralNotMember,
        path,
        format!(
            "\"{}\" is not a declared value of `@user.{}` on the [identity] entity `{}`. \
             This comparison can never be true — the branch is dead.",
            literal, field_name, identity.name
        ),
    )
    .with_valid_values(vocab.to_vec())
    .with_suggestion(format!(
        "Use one of: {}, or add \"{}\" to `{}.{}`.",
        vocab.join(", "),
        literal,
        identity.name,
        field_name
    ))
}

/// The type of `@user` for the trait named `trait_name`: the viewer plus the program's `[identity]` roster, else
/// plus the declaring orbital's `expects entity [identity] { … }` shape, else the viewer alone.
pub(crate) fn identity_type(
    schema: &OrbitalSchema,
    trait_name: &str,
) -> orbital_core_base::ir::OirType {
    let orbital = schema.orbitals.iter().find(|o| {
        o.traits
            .iter()
            .any(|t| matches!(t, orbital_core_base::schema::TraitReference::Inline(def) if def.name == trait_name))
    });
    let expected = orbital
        .and_then(super::expectations::identity_expectation)
        .and_then(|e| e.shape.map(<[_]>::to_vec));
    let roster = identity_entities(schema)
        .into_iter()
        .next()
        .map(|def| def.fields.clone());
    let row = EntityDefinition {
        timestamps: None,
        soft_delete: None,
        description: None,
        visual_prompt: None,
        asset_ref: None,
        fields: roster.or(expected).unwrap_or_default(),
        ..default_identity_shape()
    };
    let fields = with_viewer(&row).fields;
    orbital_core_base::ir::OirType::Struct {
        additional: None,
        fields: fields
            .iter()
            .filter_map(|f| {
                let name = f.name.clone()?;
                let shape = orbital_core_base::ir::types::field_shape(f);
                let guaranteed = viewer_guarantees(&name) || f.required == Some(true);
                let shape = if guaranteed || shape.is_optional() {
                    shape
                } else {
                    orbital_core_base::ir::OirType::optional(shape)
                };
                Some(orbital_core_base::ir::OirField::new(name, shape))
            })
            .collect(),
    }
}

/// Whether the `Viewer` program type declares member `name` required: the members every viewer carries.
fn viewer_guarantees(name: &str) -> bool {
    orbital_core_base::language::program_types::viewer_fields()
        .iter()
        .any(|f| f.name.as_deref() == Some(name) && f.required == Some(true))
}

/// The `@user` shape of a program with no `[identity]` entity: the viewer alone.
pub(super) fn default_identity_shape() -> EntityDefinition {
    let bare: EntityDefinition =
        serde_json::from_value(serde_json::json!({ "name": "@user", "fields": [] }))
            .expect("bare identity shape must deserialize");
    with_viewer(&bare)
}

/// `@user` over `identity`: its fields, then each `Viewer` field it does not declare itself.
pub(crate) fn with_viewer(identity: &EntityDefinition) -> EntityDefinition {
    let mut merged = identity.clone();
    for field in orbital_core_base::language::program_types::viewer_fields() {
        if field
            .name
            .as_deref()
            .is_some_and(|name| identity_field(identity, name).is_none())
        {
            merged.fields.push(field.clone());
        }
    }
    merged
}

fn walk(
    value: &Value,
    path: &str,
    identity: &EntityDefinition,
    source: IdentitySource,
    result: &mut ValidationResult,
) {
    match value {
        Value::String(s) => check_field_exists(s, path, identity, source, result),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{}[{}]", path, i), identity, source, result);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                // `sourceEntityDefinition` is PROVENANCE — the atom's original
                // entity, kept for tooling. It is not an entity of the composed
                // program, so its access policies govern nothing here and their
                // `@user.role` literals belong to the ATOM's role vocabulary,
                // not the host's.
                //
                // Walking it reports the organism's own roles as dead branches
                // against the consumer's roster: composing one `std-time-tracking`
                // trait into an app whose roster is `Person` produced 17
                // `ORB_S_ROLE_LITERAL_NOT_MEMBER` errors for `"approver"` /
                // `"employee"` — literals of a `Timesheet`/`Employee` entity that
                // is nowhere in the app. The consumer had already overridden the
                // binding (`-> PfTimesheet`), which is the language's own answer.
                //
                // Nothing is lost: when a trait is NOT rebound, the atom's entity
                // IS materialised — as the orbital's primary or an auxiliary —
                // and those same policies are validated there, on a definition
                // that really is in the program.
                if k == "sourceEntityDefinition" {
                    continue;
                }
                walk(v, &format!("{}.{}", path, k), identity, source, result);
            }
        }
        _ => {}
    }
}

/// `@user.<field>` must name a declared field of the identity entity — or, when
/// no `[identity]` entity is declared, a field of the orbital's `expects
/// identity` shape (its own declaration) or the implicit default shape.
fn check_field_exists(
    s: &str,
    path: &str,
    identity: &EntityDefinition,
    source: IdentitySource,
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
    if matches!(source, IdentitySource::Expectation) {
        return;
    }
    let declared_in_source = matches!(source, IdentitySource::Declared);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn orbital_with_identity(name: &str, entity_name: &str) -> Orbital {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "entity": {
                "name": entity_name,
                "identity": true,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": []
        }))
        .expect("orbital fixture must deserialize")
    }

    fn orbital_no_identity(name: &str, entity_name: &str) -> Orbital {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "entity": {
                "name": entity_name,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": []
        }))
        .expect("orbital fixture must deserialize")
    }

    /// Stage B (orbital-import splice) calls `identity_entities_of` over one
    /// alias's own orbitals — this is the same primary-shadows-import rule
    /// `identity_entities(schema)` applies schema-wide, just parameterized
    /// over an arbitrary orbital slice instead of `&schema.orbitals`.
    #[test]
    fn primary_shadows_auxiliary_import_over_an_arbitrary_orbital_slice() {
        let primary = orbital_with_identity("Main", "Person");
        let mut imported = orbital_no_identity("Imported", "Widget");
        imported.auxiliary_entities = Some(vec![orbital_core_base::schema::EntityRef::Inline(
            serde_json::from_value(serde_json::json!({
                "name": "ImportedOwner",
                "identity": true,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            }))
            .unwrap(),
        )]);
        let orbitals = [primary, imported];
        let names: Vec<&str> = identity_entities_of(&orbitals)
            .into_iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Person"],
            "the primary roster must shadow the imported aux copy"
        );
    }

    /// With no primary roster, the imported copies still count.
    #[test]
    fn no_primary_roster_falls_back_to_imported_copies() {
        let mut imported = orbital_no_identity("Imported", "Widget");
        imported.auxiliary_entities = Some(vec![orbital_core_base::schema::EntityRef::Inline(
            serde_json::from_value(serde_json::json!({
                "name": "ImportedOwner",
                "identity": true,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            }))
            .unwrap(),
        )]);
        let orbitals = [imported];
        let names: Vec<&str> = identity_entities_of(&orbitals)
            .into_iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["ImportedOwner"]);
    }

    /// `identity_entities(schema)` is now a thin wrapper — same result as
    /// calling `identity_entities_of(&schema.orbitals)` directly.
    #[test]
    fn identity_entities_delegates_to_identity_entities_of() {
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "t",
            "version": "1.0.0",
            "orbitals": [
                serde_json::to_value(orbital_with_identity("Main", "Person")).unwrap()
            ]
        }))
        .expect("schema fixture must deserialize");
        let names: Vec<&str> = identity_entities(&schema)
            .into_iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["Person"]);
    }

    fn reading(
        user_field: &str,
        identity_fields: Option<serde_json::Value>,
        expects: Option<serde_json::Value>,
    ) -> OrbitalSchema {
        let mut orbitals = Vec::new();
        if let Some(fields) = identity_fields {
            orbitals.push(serde_json::json!({
                "name": "PeopleOrbital",
                "entity": { "name": "Person", "identity": true, "persistence": "persistent", "collection": "people", "fields": fields },
                "traits": [], "pages": []
            }));
        }
        orbitals.push(serde_json::json!({
            "name": "DocOrbital",
            "expects": expects.into_iter().collect::<Vec<_>>(),
            "entity": { "name": "Doc", "persistence": "persistent", "collection": "docs",
                        "fields": [{ "name": "id", "type": "string", "required": true }] },
            "traits": [{
                "name": "DocView", "linkedEntity": "Doc",
                "stateMachine": { "events": [], "states": [], "transitions": [{
                    "from": "idle", "event": "OPEN", "to": "idle",
                    "guard": ["=", user_field, "x"]
                }]}
            }],
            "pages": []
        }));
        serde_json::from_value(serde_json::json!({ "name": "App", "orbitals": orbitals }))
            .expect("schema fixture")
    }

    fn identity_codes(schema: &OrbitalSchema) -> Vec<String> {
        let mut ctx = ValidationContext::new();
        validate_user_identity(schema, &mut ctx);
        ctx.result
            .errors
            .iter()
            .map(|e| e.code.as_str().to_string())
            .collect()
    }

    fn roster(extra: &[&str]) -> serde_json::Value {
        let mut fields =
            vec![serde_json::json!({ "name": "id", "type": "string", "required": true })];
        fields.extend(
            extra
                .iter()
                .map(|n| serde_json::json!({ "name": n, "type": "string" })),
        );
        serde_json::Value::Array(fields)
    }

    #[test]
    fn a_member_declared_without_bang_is_typed_optional_and_a_guaranteed_one_is_not() {
        use orbital_core_base::ir::OirType;
        let OirType::Struct { fields, .. } = identity_type(
            &reading("@user.id", Some(roster(&["teamId"])), None),
            "DocView",
        ) else {
            panic!("a record")
        };
        let optional = |name: &str| {
            fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("{name} declared"))
                .field_type
                .is_optional()
        };
        assert!(optional("teamId"), "a roster member without `!`");
        assert!(
            optional("name"),
            "a viewer member the viewer does not require"
        );
        assert!(optional("role"));
        assert!(!optional("id"), "control: the viewer's id is required");
        assert!(
            !optional("claims"),
            "control: the viewer's claims are required"
        );
    }

    #[test]
    fn a_viewer_field_reads_in_every_identity_environment() {
        let role_only = serde_json::json!({ "kind": "identity", "shape": [{ "name": "role", "type": "string" }] });
        for (label, schema) in [
            (
                "declared roster without email",
                reading("@user.email", Some(roster(&["teamId"])), None),
            ),
            ("no roster", reading("@user.email", None, None)),
            (
                "expects shape without email",
                reading("@user.email", None, Some(role_only.clone())),
            ),
            (
                "roster plus expects without email",
                reading("@user.email", Some(roster(&[])), Some(role_only)),
            ),
            ("claims", reading("@user.claims", None, None)),
        ] {
            assert_eq!(identity_codes(&schema), Vec::<String>::new(), "{label}");
        }
    }

    #[test]
    fn an_identity_row_field_reads_only_where_the_roster_declares_it() {
        assert_eq!(
            identity_codes(&reading("@user.teamId", Some(roster(&["teamId"])), None)),
            Vec::<String>::new()
        );
        assert_eq!(
            identity_codes(&reading("@user.teamId", Some(roster(&["tier"])), None)),
            vec!["ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()]
        );
        assert_eq!(
            identity_codes(&reading("@user.teamId", None, None)),
            vec!["ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()],
            "control: with no roster `@user` is the viewer alone"
        );
        assert_eq!(
            identity_codes(&reading("@user.uid", None, None)),
            vec!["ORB_S_UNKNOWN_IDENTITY_FIELD".to_string()],
            "the viewer has no `uid`"
        );
    }

    #[test]
    fn the_user_type_is_the_viewer_plus_the_roster_row() {
        let names = |schema: &OrbitalSchema| match identity_type(schema, "DocView") {
            orbital_core_base::ir::OirType::Struct { fields, .. } => {
                fields.into_iter().map(|f| f.name).collect::<Vec<_>>()
            }
            other => panic!("a record, got {other:?}"),
        };
        let with_roster = names(&reading("@user.id", Some(roster(&["teamId"])), None));
        for f in [
            "id",
            "teamId",
            "email",
            "name",
            "role",
            "permissions",
            "claims",
        ] {
            assert!(
                with_roster.contains(&f.to_string()),
                "{f} in {with_roster:?}"
            );
        }
        let viewer_only = names(&reading("@user.id", None, None));
        assert!(
            viewer_only.contains(&"email".to_string()),
            "{viewer_only:?}"
        );
        assert!(
            !viewer_only.contains(&"teamId".to_string()),
            "control: {viewer_only:?}"
        );
    }
}
