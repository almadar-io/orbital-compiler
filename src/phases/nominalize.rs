//! Nominalization phase (spec Phase 3D, flag-gated by `CompilerOptions::nominalize`).
//!
//! Walks every type position of the resolved [`OirModule`] (entity fields —
//! including trait-owned `data_entities` — trait config knobs, event payload
//! schemas, emit payload fields) bottom-up and replaces each anonymous
//! [`OirType::Struct`]/[`OirType::Union`] with an [`OirType::TypeRef`] to a
//! name registered in `module.named_types` — the EXISTING table the resolver
//! already populates for recursive lolo aliases (first-registration-wins).
//! Every codegen backend with a named-type facility (all five shells after
//! the Phase-3D shell readiness) hoists one declaration per entry, so a
//! backend's `TypeRef` arm renders the bare name and the shape lives once at
//! file top.
//!
//! Naming follows the lolo-ui positional scheme
//! (`tools/almadar-pattern-sync/src/shared/lolo-emit.ts`): the name hint
//! starts at `{Owner}{Field}` and grows positionally (`${hint}${pascal(field)}`
//! per nesting level); arrays append `Item` to the ELEMENT hint, maps append
//! `Value` to the VALUE hint (`lolo-emit.ts:307/:345/:432/:476`). Names are
//! minted with `split_to_pascal_case` (tail-preserving — the same converter
//! the mobile shells name their hoisted declarations with).
//!
//! Three invariants:
//! - **Canonical field order.** The two resolve paths diverge
//!   (`resolve_field_shape` sorts struct fields at `resolve.rs:797`, the
//!   payload path preserves source order at `resolve.rs:2827-2842`). The
//!   canonical order — chosen HERE — is **sorted by field name** at every
//!   struct level (stable sort, so equal names keep source order), applied
//!   before the dedup identity is computed. The registered entry uses it, so
//!   a shape whose fields differ only in declaration order dedups to ONE
//!   entry regardless of which resolve path produced it.
//! - **Never overwrite.** `named_types` is the recursive-alias namespace
//!   (first-registration-wins, `resolve.rs:2912-2915`). A positional name
//!   already taken by a DIFFERENT shape is never overwritten: the anonymous
//!   type is left in place (its children still nominalized) and the collision
//!   is counted. A name taken by an IDENTICAL canonical shape reuses the
//!   entry (that is dedup, not overwrite).
//! - **Reserve-before-recurse cycle guard** (same discipline as
//!   `lolo-emit.ts:441-485`): the name is reserved in an in-progress set
//!   before the shape's own children are nominalized; a re-entrant request
//!   for the same name (only reachable via an empty field name re-deriving an
//!   ancestor's positional name — anonymous IR trees are otherwise finite)
//!   resolves to [`OirType::TypeRef`] of the enclosing definition instead of
//!   recursing forever.
//!
//! `enum_values` and every other `OirField` sidecar ride the field into the
//! registered entry untouched — the shells' field-layer enum handling reads
//! them off the hoisted declaration exactly as it does off an inline one.

use orbital_core_base::ir::{OirEntity, OirField, OirModule, OirType};
use orbital_core_base::str_utils::split_to_pascal_case;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Outcome counters of one [`nominalize_module`] run — surfaced so a caller
/// (or test) can assert the no-overwrite and dedup invariants held.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NominalizeStats {
    /// New entries inserted into `module.named_types`.
    pub registered: usize,
    /// Use-sites resolved to a PRE-EXISTING entry (recursive-alias
    /// namespace) whose canonical shape matched the anonymous shape.
    pub reused_existing: usize,
    /// Use-sites resolved to an entry another anonymous position in THIS run
    /// registered (shape-level dedup — one entry, many use-sites).
    pub reused_dedup: usize,
    /// Positional names taken by a different shape (a pre-existing entry, or
    /// another anonymous position earlier in this run) — left anonymous,
    /// never overwritten.
    pub skipped_collisions: usize,
}

struct Ctx {
    /// Owned for the duration of the walk (taken out of the module so the
    /// rest of the module is free to mutate) — merged back at the end.
    named_types: BTreeMap<String, OirType>,
    /// Canonical-shape key → registered name; seeded with the pre-existing
    /// (recursive-alias) entries so an anonymous twin dedups onto them.
    key_to_name: HashMap<String, String>,
    /// Reserve-before-recurse guard.
    in_progress: HashSet<String>,
    registrations: Vec<(String, OirType)>,
    stats: NominalizeStats,
}

/// Canonical dedup identity: the serde shape of the fully-nominalized type
/// after field-order canonicalization. Deterministic — below the
/// struct/union nodes `OirType` carries only Vec/Box/String scalars, so a
/// canonical field order yields a canonical byte string.
fn canonical_key(ty: &OirType) -> String {
    serde_json::to_string(ty).unwrap_or_default()
}

/// Canonical field order (see the module doc): sorted by field name, stable.
fn sorted_fields(mut fields: Vec<OirField>) -> Vec<OirField> {
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    fields
}

/// Nominalize one type position. `hint` is the positional name hint this
/// position contributes; containers extend it (`Item` per array level,
/// `Value` per map-value level) exactly as lolo-ui's emitter does.
fn nominalize_type(ty: &OirType, hint: &str, ctx: &mut Ctx) -> OirType {
    match ty {
        OirType::Array { inner } => OirType::Array {
            inner: Box::new(nominalize_type(inner, &format!("{}Item", hint), ctx)),
        },
        OirType::Map { key, value } => OirType::Map {
            key: key.clone(),
            value: Box::new(nominalize_type(value, &format!("{}Value", hint), ctx)),
        },
        OirType::Optional { inner } => OirType::Optional {
            inner: Box::new(nominalize_type(inner, hint, ctx)),
        },
        OirType::Struct { fields, additional } => {
            nominalize_shape(sorted_fields(fields.clone()), hint, ctx, ShapeKind::Struct(additional.clone()))
        }
        OirType::Union { variants } => {
            nominalize_shape(sorted_fields(variants.clone()), hint, ctx, ShapeKind::Union)
        }
        other => other.clone(),
    }
}

enum ShapeKind {
    Struct(Option<Box<OirType>>),
    Union,
}

fn nominalize_shape(fields: Vec<OirField>, hint: &str, ctx: &mut Ctx, kind: ShapeKind) -> OirType {
    let name = split_to_pascal_case(hint);
    if !ctx.in_progress.insert(name.clone()) {
        // Reserve-before-recurse: the enclosing frame for this same
        // positional name owns the registration — a re-entrant request IS the
        // self-recursive back-reference (lolo-emit.ts:453).
        return OirType::TypeRef { name };
    }
    let child_fields = fields
        .iter()
        .map(|f| {
            let mut f = f.clone();
            f.field_type = nominalize_type(
                &f.field_type,
                &format!("{}{}", hint, split_to_pascal_case(&f.name)),
                ctx,
            );
            f
        })
        .collect::<Vec<_>>();
    let additional = match &kind {
        ShapeKind::Struct(additional) => additional.as_deref().map(|ty| {
            Box::new(nominalize_type(ty, &format!("{}Value", hint), ctx))
        }),
        ShapeKind::Union => None,
    };
    ctx.in_progress.remove(&name);

    let shape = match kind {
        ShapeKind::Struct(_) => OirType::Struct {
            fields: child_fields,
            additional,
        },
        ShapeKind::Union => OirType::Union {
            variants: child_fields,
        },
    };
    let key = canonical_key(&shape);

    // Dedup first: `key_to_name` is seeded with the pre-existing
    // (recursive-alias) entries, so an anonymous twin of one reuses its name
    // instead of colliding with it.
    if let Some(prev) = ctx.key_to_name.get(&key) {
        if ctx.named_types.contains_key(prev) {
            ctx.stats.reused_existing += 1;
        } else {
            ctx.stats.reused_dedup += 1;
        }
        return OirType::TypeRef { name: prev.clone() };
    }
    let name_claimed =
        ctx.named_types.contains_key(&name) || ctx.registrations.iter().any(|(n, _)| *n == name);
    if name_claimed {
        // The positional name is taken by a DIFFERENT shape — the
        // recursive-alias namespace and earlier registrations are never
        // overwritten. Left anonymous (children were still nominalized above).
        ctx.stats.skipped_collisions += 1;
        return shape;
    }
    ctx.stats.registered += 1;
    ctx.key_to_name.insert(key, name.clone());
    ctx.registrations.push((name.clone(), shape));
    OirType::TypeRef { name }
}

/// One canonical-order clone of a pre-existing entry, for key comparison
/// only — the entry itself is never mutated.
fn canonicalize(ty: &OirType) -> OirType {
    match ty {
        OirType::Struct { fields, additional } => OirType::Struct {
            fields: sorted_fields(fields.clone()),
            additional: additional.clone(),
        },
        OirType::Union { variants } => OirType::Union {
            variants: sorted_fields(variants.clone()),
        },
        other => other.clone(),
    }
}

fn nominalize_entity_fields(entity: &mut OirEntity, ctx: &mut Ctx) {
    let owner = split_to_pascal_case(&entity.name);
    let fields = std::mem::take(&mut entity.fields);
    entity.fields = fields
        .into_iter()
        .map(|mut f| {
            let hint = format!("{}{}", owner, split_to_pascal_case(&f.name));
            f.field_type = nominalize_type(&f.field_type, &hint, ctx);
            f
        })
        .collect();
}

/// Run nominalization over every type position of a resolved module.
pub fn nominalize_module(module: &mut OirModule) -> NominalizeStats {
    let existing = std::mem::take(&mut module.named_types);
    let mut ctx = Ctx {
        key_to_name: existing
            .iter()
            .map(|(n, t)| (canonical_key(&canonicalize(t)), n.clone()))
            .collect(),
        named_types: existing,
        in_progress: HashSet::new(),
        registrations: Vec::new(),
        stats: NominalizeStats::default(),
    };

    for entity in &mut module.entities {
        nominalize_entity_fields(entity, &mut ctx);
    }
    for trait_def in &mut module.traits {
        let owner = split_to_pascal_case(&trait_def.name);
        for entity in &mut trait_def.data_entities {
            nominalize_entity_fields(entity, &mut ctx);
        }
        if let Some(schema) = &mut trait_def.declared_config_schema {
            for (knob, field_type) in schema.iter_mut() {
                let hint = format!("{}{}", owner, split_to_pascal_case(knob));
                field_type.field_type = nominalize_type(&field_type.field_type, &hint, &mut ctx);
            }
        }
        for event in &mut trait_def.events {
            let Some(schema) = &mut event.payload_schema else {
                continue;
            };
            for (field, ty) in schema.iter_mut() {
                let hint = format!(
                    "{}{}{}",
                    owner,
                    split_to_pascal_case(&event.key),
                    split_to_pascal_case(field)
                );
                *ty = nominalize_type(ty, &hint, &mut ctx);
            }
        }
        for emit in &mut trait_def.emits {
            for field in &mut emit.payload {
                let hint = format!(
                    "{}{}{}",
                    owner,
                    split_to_pascal_case(&emit.event),
                    split_to_pascal_case(&field.name)
                );
                field.field_type = nominalize_type(&field.field_type, &hint, &mut ctx);
            }
        }
    }

    module.named_types = ctx.named_types;
    module.named_types.extend(ctx.registrations.into_iter());
    ctx.stats
}
