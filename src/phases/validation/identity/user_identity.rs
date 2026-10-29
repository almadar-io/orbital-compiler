//! The shape of `@user`: the viewer plus the program's `[identity]` row.

use orbital_core_base::schema::{EntityDefinition, EntityRef, Orbital, OrbitalSchema};

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

fn identity_expectation_shape(e: &orbital_core_base::schema::ExpectDeclaration) -> Option<Option<Vec<orbital_core_base::schema::FieldDefinition>>> {
    match e {
        orbital_core_base::schema::ExpectDeclaration::Identity { shape, .. } => Some(shape.clone()),
        _ => None,
    }
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
        .and_then(|o| o.expects.as_ref()?.iter().find_map(identity_expectation_shape))
        .and_then(|shape| shape);
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
                let shape = orbital_core_base::ir::types::field_shape(f).unwrap_or_else(|_| orbital_core_base::ir::OirType::erased());
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

