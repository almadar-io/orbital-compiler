//! Cross-file id-edge resolution (Almadar Rabit V4, W2/P9).
//!
//! `orbital-lolo`'s `stamp_identity` mints ids and populates dual-carry id
//! siblings for every IN-FILE reference (identity.rs `resolve_orbital_edges`);
//! a dotted `Alias.traits.X` / `Alias.entity` reference is left with a `None`
//! id sibling by construction, because the single-file lowerer has no view of
//! the imported declarations. This pass runs AFTER `stamp_identity`, loads the
//! imported schemas through the shared `ExternalOrbitalLoader`, and fills in
//! exactly the same dual-carry positions `resolve_orbital_edges` fills for
//! in-file refs — sourced from the imported schema's `IdentityLedger` instead
//! of the local `OrbitalIds` map. The `uses` block and reference position are
//! left intact (no inlining): this is id resolution, not composition.

use orbital_core::identity::{IdKind, IdentityLedger};
use orbital_core::schema::{
    EntityRef, FieldDefinition, Orbital, OrbitalSchema, PageRef, TraitReference,
};
use std::collections::HashMap;

use crate::loader::{ExternalOrbitalLoader, ImportChain, LoaderError};

/// Reserved for genuine resolution failures. Per-import load/ledger misses are
/// warned and skipped (a leaf that hasn't been stamped yet is an expected
/// transition-period state, not a hard error) — this variant exists so the
/// signature can surface a real error if one is ever needed without another
/// breaking API change.
#[derive(Debug, thiserror::Error)]
pub enum CrossFileIdError {
    #[error("loader error: {0}")]
    Loader(#[from] LoaderError),
}

/// What an imported `.orb` schema resolves to for cross-file id lookups:
/// the `(kind, bakedName) -> id` reverse index built from its ledger, plus
/// the id of "the entity" (`Alias.entity` is, by the same convention as the
/// inline phase's `InlineContext::get_entity`, the FIRST orbital's entity).
#[derive(Debug, Clone, Default)]
struct ResolvedImport {
    reverse: HashMap<(IdKind, String), String>,
    entity_id: Option<String>,
}

impl ResolvedImport {
    fn trait_id(&self, name: &str) -> Option<String> {
        self.reverse
            .get(&(IdKind::Trait, name.to_string()))
            .cloned()
    }

    fn page_id(&self, name: &str) -> Option<String> {
        self.reverse.get(&(IdKind::Page, name.to_string())).cloned()
    }
}

fn build_reverse_index(ledger: &IdentityLedger) -> HashMap<(IdKind, String), String> {
    ledger
        .entries
        .values()
        .map(|e| ((e.kind, e.baked_name.clone()), e.id.clone()))
        .collect()
}

/// `Alias.entity`'s target id: the first orbital's entity, mirroring
/// `InlineContext::get_entity`'s "first orbital is primary" convention. Reads
/// the entity's own dual-carry `id` (already stamped in-file on the import) —
/// no ledger lookup needed since the imported entity carries its id directly.
fn first_orbital_entity_id(schema: &OrbitalSchema) -> Option<String> {
    match &schema.orbitals.first()?.entity {
        EntityRef::Inline(entity) => entity.id.clone(),
        EntityRef::Reference(_) | EntityRef::Extends(_) => None,
    }
}

/// Parsed dotted cross-file reference grammar: `Alias.entity`,
/// `Alias.traits.X`, `Alias.pages.X`. Anything else (no dot, or a middle
/// segment other than `entity`/`traits`/`pages`) is left unparsed — the
/// caller leaves such refs untouched rather than treating them as errors.
enum DottedRef<'a> {
    Entity { alias: &'a str },
    Trait { alias: &'a str, name: &'a str },
    Page { alias: &'a str, name: &'a str },
}

fn parse_dotted(r: &str) -> Option<DottedRef<'_>> {
    let mut parts = r.splitn(3, '.');
    let alias = parts.next()?;
    let kind = parts.next()?;
    match kind {
        "entity" if parts.next().is_none() => Some(DottedRef::Entity { alias }),
        "traits" => Some(DottedRef::Trait {
            alias,
            name: parts.next()?,
        }),
        "pages" => Some(DottedRef::Page {
            alias,
            name: parts.next()?,
        }),
        _ => None,
    }
}

fn resolve_ref_id(r: &str, aliases: &HashMap<String, ResolvedImport>) -> Option<String> {
    match parse_dotted(r)? {
        DottedRef::Entity { alias } => aliases.get(alias)?.entity_id.clone(),
        DottedRef::Trait { alias, name } => aliases.get(alias)?.trait_id(name),
        DottedRef::Page { alias, name } => aliases.get(alias)?.page_id(name),
    }
}

/// The trait-id a reference resolves to — either the inline trait's own id,
/// an already-resolved `ref_id`, or a freshly cross-file-resolved one. Mirrors
/// `orbital-lolo/identity.rs`'s `trait_ref_id_of`, extended to dotted refs.
fn trait_ref_id_of(
    tr: &TraitReference,
    aliases: &HashMap<String, ResolvedImport>,
) -> Option<String> {
    match tr {
        TraitReference::Inline(td) => td.id.clone(),
        TraitReference::Reference {
            reference, ref_id, ..
        } => ref_id
            .clone()
            .or_else(|| reference.as_ref().and_then(|r| resolve_ref_id(r, aliases))),
    }
}

/// Resolve a single trait reference's dual-carry `ref_id` / `linked_entity_id`
/// against `aliases`. No-op for `Inline` (an inline trait carries no dotted
/// ref of its own) and for anything already resolved. Returns the count of
/// fields newly resolved.
fn resolve_trait_ref_edges(
    tr: &mut TraitReference,
    aliases: &HashMap<String, ResolvedImport>,
) -> usize {
    let TraitReference::Reference {
        reference,
        ref_id,
        linked_entity,
        linked_entity_id,
        ..
    } = tr
    else {
        return 0;
    };

    let mut count = 0;
    if ref_id.is_none() {
        if let Some(id) = reference.as_ref().and_then(|r| resolve_ref_id(r, aliases)) {
            *ref_id = Some(id);
            count += 1;
        }
    }
    if linked_entity_id.is_none() {
        if let Some(id) = linked_entity
            .as_ref()
            .and_then(|n| resolve_ref_id(n, aliases))
        {
            *linked_entity_id = Some(id);
            count += 1;
        }
    }
    count
}

/// Resolve a relation field's cross-file `entity_id`, recursing into array
/// items and object properties — mirrors `orbital-lolo/identity.rs`'s
/// `resolve_field_relations`.
fn resolve_field_relations(
    field: &mut FieldDefinition,
    aliases: &HashMap<String, ResolvedImport>,
) -> usize {
    let mut count = 0;
    if let Some(rel) = field.relation.as_mut() {
        if rel.entity_id.is_none() {
            if let Some(id) = resolve_ref_id(&rel.entity, aliases) {
                rel.entity_id = Some(id);
                count += 1;
            }
        }
    }
    if let Some(items) = field.items.as_mut() {
        count += resolve_field_relations(items, aliases);
    }
    if let Some(props) = field.properties.as_mut() {
        for p in props.values_mut() {
            count += resolve_field_relations(p, aliases);
        }
    }
    count
}

/// Resolve every representable dotted-ref id position on one orbital.
/// `aliases` is this orbital's OWN `uses` alias map (an alias can shadow
/// across orbitals in the same schema, so this is rebuilt per orbital rather
/// than shared schema-wide).
///
/// Representable positions (mirrors `resolve_orbital_edges` exactly):
/// entity `Extends.extends_id`, trait `Reference.ref_id`/`linked_entity_id`,
/// page `PageRefObject.ref_id`/`linked_entity_id`/`trait_ref_ids`, and
/// relation-field `entity_id`. `EntityRef::Reference(String)` (the bare
/// `"Alias.entity"` string form) has NO id sibling on that variant — it
/// cannot carry a cross-file id without inventing a schema field, so it is
/// left untouched by design (see module docs on the compiler-side caller).
fn resolve_orbital(orbital: &mut Orbital, aliases: &HashMap<String, ResolvedImport>) -> usize {
    let mut count = 0;

    if let EntityRef::Extends(e) = &mut orbital.entity {
        if e.extends_id.is_none() {
            if let Some(id) = resolve_ref_id(&e.reference, aliases) {
                e.extends_id = Some(id);
                count += 1;
            }
        }
    }
    if let EntityRef::Inline(entity) = &mut orbital.entity {
        for f in &mut entity.fields {
            count += resolve_field_relations(f, aliases);
        }
    }

    for tr in orbital.traits.iter_mut() {
        count += resolve_trait_ref_edges(tr, aliases);
    }

    for page in orbital.pages.iter_mut().flatten() {
        match page {
            PageRef::Inline(p) => {
                if let Some(traits) = p.traits.as_mut() {
                    for tr in traits {
                        count += resolve_trait_ref_edges(tr, aliases);
                    }
                }
            }
            PageRef::Object(o) => {
                if o.ref_id.is_none() {
                    if let Some(id) = resolve_ref_id(&o.reference, aliases) {
                        o.ref_id = Some(id);
                        count += 1;
                    }
                }
                if o.linked_entity_id.is_none() {
                    if let Some(id) = o
                        .linked_entity
                        .as_ref()
                        .and_then(|n| resolve_ref_id(n, aliases))
                    {
                        o.linked_entity_id = Some(id);
                        count += 1;
                    }
                }
                if let Some(traits) = o.traits.as_mut() {
                    for tr in traits.iter_mut() {
                        count += resolve_trait_ref_edges(tr, aliases);
                    }
                    let mut ref_ids = o.trait_ref_ids.clone().unwrap_or_default();
                    ref_ids.resize(traits.len(), String::new());
                    let mut any_set = ref_ids.iter().any(|s| !s.is_empty());
                    for (i, tr) in traits.iter().enumerate() {
                        if ref_ids[i].is_empty() {
                            if let Some(id) = trait_ref_id_of(tr, aliases) {
                                ref_ids[i] = id;
                                any_set = true;
                                count += 1;
                            }
                        }
                    }
                    if any_set {
                        o.trait_ref_ids = Some(ref_ids);
                    }
                }
            }
            PageRef::Reference(_) => {}
        }
    }

    count
}

/// Testable core: resolve every orbital's dotted refs given an already-built
/// `from -> ResolvedImport` table (no loader involved). Split out from
/// `resolve_cross_file_id_edges` so unit tests can exercise the resolution
/// logic with in-memory fixtures.
fn resolve_schema_with_imports(
    schema: &mut OrbitalSchema,
    by_from: &HashMap<String, ResolvedImport>,
) -> usize {
    let mut total = 0;
    for orbital in &mut schema.orbitals {
        let Some(uses) = orbital.uses.clone() else {
            continue;
        };
        let mut aliases: HashMap<String, ResolvedImport> = HashMap::new();
        for u in &uses {
            if let Some(ri) = by_from.get(&u.from) {
                aliases.insert(u.alias.clone(), ri.clone());
            }
        }
        if aliases.is_empty() {
            continue;
        }
        total += resolve_orbital(orbital, &aliases);
    }
    total
}

/// Resolve every distinct `from` path referenced by `schema`'s orbitals into a
/// `ResolvedImport`, loading each through `loader` once. A load failure or a
/// ledger-less import is warned and skipped (not a hard error): during the
/// V4 transition not every leaf is stamped yet, and post-flip that graduates
/// to a real validator error owned elsewhere, not this pass.
fn load_imports(
    schema: &OrbitalSchema,
    loader: &mut ExternalOrbitalLoader,
) -> HashMap<String, ResolvedImport> {
    let mut by_from: HashMap<String, ResolvedImport> = HashMap::new();
    for orbital in &schema.orbitals {
        for u in orbital.uses.iter().flatten() {
            if by_from.contains_key(&u.from) {
                continue;
            }
            let mut chain = ImportChain::new();
            let loaded = match loader.load(&u.from, None, &mut chain) {
                Ok(l) => l,
                Err(err) => {
                    tracing::warn!(
                        from = %u.from,
                        error = %err,
                        "cross-file id resolution: failed to load import; its dotted refs stay unresolved"
                    );
                    continue;
                }
            };
            let Some(ledger) = loaded.schema.ledger.as_ref() else {
                tracing::warn!(
                    from = %u.from,
                    "cross-file id resolution: imported schema has no identity ledger yet; \
                     its dotted refs stay unresolved (transition-period leaf, not yet stamped)"
                );
                continue;
            };
            let resolved = ResolvedImport {
                reverse: build_reverse_index(ledger),
                entity_id: first_orbital_entity_id(&loaded.schema),
            };
            by_from.insert(u.from.clone(), resolved);
        }
    }
    by_from
}

/// Resolve cross-file `uses` id edges on an already in-file-stamped `schema`:
/// for every dotted `Alias.traits.X` / `Alias.entity` / `Alias.pages.X`
/// reference whose id sibling is still `None`, load the aliased import
/// through `loader` and write the imported declaration's id in place. The
/// `uses` block and every reference string are left untouched — this never
/// inlines. Returns the count of ids newly resolved.
pub fn resolve_cross_file_id_edges(
    schema: &mut OrbitalSchema,
    loader: &mut ExternalOrbitalLoader,
) -> Result<usize, CrossFileIdError> {
    let by_from = load_imports(schema, loader);
    Ok(resolve_schema_with_imports(schema, &by_from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::identity::{IdentityLedger, LedgerEntry, LedgerOwner};
    use orbital_core::schema::{EntityDefinition, Orbital, OrbitalSchema, UseDeclaration};

    fn trait_ledger_entry(id: &str, baked_name: &str) -> LedgerEntry {
        LedgerEntry {
            id: id.to_string(),
            kind: IdKind::Trait,
            baked_name: baked_name.to_string(),
            cur_name: baked_name.to_string(),
            renames: Vec::new(),
            owner: LedgerOwner::Std,
            parent: None,
        }
    }

    fn empty_entity(name: &str) -> EntityDefinition {
        EntityDefinition {
            name: name.to_string(),
            id: None,
            collection: None,
            persistence: None,
            fields: Vec::new(),
            instances: None,
            shared: false,
        }
    }

    fn reference_trait(reference: &str) -> TraitReference {
        TraitReference::Reference {
            reference: Some(reference.to_string()),
            ref_id: None,
            name: None,
            linked_entity: None,
            linked_entity_id: None,
            config: None,
            events: None,
            event_ids: None,
            fields: None,
            listens: None,
            emits_scope: None,
        }
    }

    fn orbital_with_use(alias: &str, from: &str, reference: &str) -> Orbital {
        Orbital {
            name: "Consumer".to_string(),
            id: None,
            uses: Some(vec![UseDeclaration {
                from: from.to_string(),
                alias: alias.to_string(),
            }]),
            theme: None,
            services: None,
            entity: EntityRef::Inline(empty_entity("ConsumerEntity")),
            auxiliary_entities: None,
            traits: vec![reference_trait(reference)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        }
    }

    /// (a) A `uses` import whose ledger has trait `Foo` resolves a
    /// `ref: "Alias.traits.Foo"` to the imported id, without touching the
    /// `reference` string (dual-carry, not inlining).
    #[test]
    fn resolves_dotted_trait_ref_from_imported_ledger() {
        let mut schema = OrbitalSchema {
            name: "Test".to_string(),
            version: None,
            description: None,
            orbitals: vec![orbital_with_use("Alias", "./foo.orb", "Alias.traits.Foo")],
            metadata: None,
            ledger: None,
            schema_version: None,
        };

        let mut by_from = HashMap::new();
        let mut reverse = HashMap::new();
        reverse.insert((IdKind::Trait, "Foo".to_string()), "trt_abc123".to_string());
        by_from.insert(
            "./foo.orb".to_string(),
            ResolvedImport {
                reverse,
                entity_id: None,
            },
        );

        let resolved = resolve_schema_with_imports(&mut schema, &by_from);
        assert_eq!(resolved, 1);

        let TraitReference::Reference {
            reference, ref_id, ..
        } = &schema.orbitals[0].traits[0]
        else {
            panic!("expected Reference variant");
        };
        assert_eq!(reference.as_deref(), Some("Alias.traits.Foo"));
        assert_eq!(ref_id.as_deref(), Some("trt_abc123"));
    }

    /// (b) A dotted ref whose alias or target name is absent from the
    /// imported ledger is left `None` — no panic, no fabricated id.
    #[test]
    fn leaves_unresolvable_dotted_ref_none_without_panic() {
        let mut schema = OrbitalSchema {
            name: "Test".to_string(),
            version: None,
            description: None,
            orbitals: vec![orbital_with_use(
                "Alias",
                "./foo.orb",
                "Alias.traits.Missing",
            )],
            metadata: None,
            ledger: None,
            schema_version: None,
        };

        let mut by_from = HashMap::new();
        let mut reverse = HashMap::new();
        reverse.insert((IdKind::Trait, "Foo".to_string()), "trt_abc123".to_string());
        by_from.insert(
            "./foo.orb".to_string(),
            ResolvedImport {
                reverse,
                entity_id: None,
            },
        );

        let resolved = resolve_schema_with_imports(&mut schema, &by_from);
        assert_eq!(resolved, 0);

        let TraitReference::Reference { ref_id, .. } = &schema.orbitals[0].traits[0] else {
            panic!("expected Reference variant");
        };
        assert!(ref_id.is_none());
    }

    /// A ref whose alias isn't in `uses` at all (unknown-alias defensive path)
    /// is likewise left untouched.
    #[test]
    fn leaves_unknown_alias_ref_none() {
        let mut schema = OrbitalSchema {
            name: "Test".to_string(),
            version: None,
            description: None,
            orbitals: vec![orbital_with_use(
                "Alias",
                "./foo.orb",
                "OtherAlias.traits.Foo",
            )],
            metadata: None,
            ledger: None,
            schema_version: None,
        };

        let mut by_from = HashMap::new();
        by_from.insert(
            "./foo.orb".to_string(),
            ResolvedImport {
                reverse: HashMap::from([(
                    (IdKind::Trait, "Foo".to_string()),
                    "trt_abc123".to_string(),
                )]),
                entity_id: None,
            },
        );

        let resolved = resolve_schema_with_imports(&mut schema, &by_from);
        assert_eq!(resolved, 0);
    }

    #[test]
    fn build_reverse_index_keys_by_kind_and_baked_name() {
        let mut entries = HashMap::new();
        entries.insert("trt_1".to_string(), trait_ledger_entry("trt_1", "Foo"));
        let ledger = IdentityLedger {
            schema_version: 1,
            entries,
        };
        let idx = build_reverse_index(&ledger);
        assert_eq!(
            idx.get(&(IdKind::Trait, "Foo".to_string())).cloned(),
            Some("trt_1".to_string())
        );
    }
}
