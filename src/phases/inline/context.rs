//! Reference parsing and the inline alias context.

use super::InlineError;
use orbital_core::schema::{
    ConfigField, EntityDefinition, EntityRef, ExpectDeclaration, Orbital, PageRef, PayloadField,
    ServiceRef, ThemeRef, TraitReference,
};
use std::collections::{BTreeMap, HashMap};

// ============================================================================
// Reference Parser
// ============================================================================

/// Parsed reference components
#[derive(Debug, Clone)]
pub(crate) enum ParsedReference {
    /// Alias.entity
    Entity { alias: String },
    /// Alias.traits.TraitName
    Trait { alias: String, name: String },
    /// Alias.pages.PageName
    Page { alias: String, name: String },
    /// Alias.theme
    Theme { alias: String },
    /// Alias.services.ServiceName
    Service { alias: String, name: String },
    /// Alias.orbitals.OrbitalName
    Orbital { alias: String, name: String },
}

/// Parse a reference string into components
pub(crate) fn parse_reference(reference: &str) -> Result<ParsedReference, InlineError> {
    let parts: Vec<&str> = reference.split('.').collect();

    if parts.len() < 2 {
        return Err(InlineError::InvalidReference(reference.to_string()));
    }

    let alias = parts[0].to_string();

    match parts[1] {
        "entity" => {
            if parts.len() != 2 {
                return Err(InlineError::InvalidReference(reference.to_string()));
            }
            Ok(ParsedReference::Entity { alias })
        }
        "traits" => {
            if parts.len() != 3 {
                return Err(InlineError::InvalidReference(reference.to_string()));
            }
            Ok(ParsedReference::Trait {
                alias,
                name: parts[2].to_string(),
            })
        }
        "pages" => {
            if parts.len() != 3 {
                return Err(InlineError::InvalidReference(reference.to_string()));
            }
            Ok(ParsedReference::Page {
                alias,
                name: parts[2].to_string(),
            })
        }
        "theme" => {
            if parts.len() != 2 {
                return Err(InlineError::InvalidReference(reference.to_string()));
            }
            Ok(ParsedReference::Theme { alias })
        }
        "services" => {
            if parts.len() != 3 {
                return Err(InlineError::InvalidReference(reference.to_string()));
            }
            Ok(ParsedReference::Service {
                alias,
                name: parts[2].to_string(),
            })
        }
        "orbitals" => {
            if parts.len() != 3 {
                return Err(InlineError::InvalidReference(reference.to_string()));
            }
            Ok(ParsedReference::Orbital {
                alias,
                name: parts[2].to_string(),
            })
        }
        _ => Err(InlineError::InvalidReference(reference.to_string())),
    }
}

// ============================================================================
// Inline Context
// ============================================================================

/// One imported behavior's orbitals plus the source label used in errors.
///
/// `orbitals` holds every orbital from the imported `.orb` — not just orbital
/// 0 — so a trait/page/service that lives in orbital 1+ of a multi-orbital
/// behavior (all 32 organism registries fit this shape) can still be found by
/// `Alias.traits.X` / `Alias.pages.X` / etc. See the bug fix in the module
/// doc above.
///
/// `source_label` is the human-readable name of the imported behavior (e.g.
/// `"std-arcade-game"` for `std/behaviors/std-arcade-game`). It appears in
/// `*NotFound` error messages instead of "orbital 0's name", which was
/// misleading once the resolver started searching all orbitals — the author
/// needs to know which imported behavior is missing the symbol.
#[derive(Clone)]
pub(crate) struct AliasEntry {
    /// Shared: one inlined import serves every orbital that uses it.
    pub(crate) orbitals: std::rc::Rc<Vec<Orbital>>,
    pub(crate) source_label: String,
}

/// Context for inlining references within an orbital
/// Uses owned copies to avoid borrow checker issues with loader cache
pub(crate) struct InlineContext {
    /// Mapping from alias to the full set of orbitals loaded under that alias.
    pub(crate) aliases: HashMap<String, AliasEntry>,
    /// When true, this orbital is being resolved as an IMPORT (not the
    /// top-level compilation target), so `@config.X` bindings inside resolved
    /// traits MUST NOT be substituted with the import's own declared defaults.
    /// The defaults are still folded into the resolved trait's `config[k].default`
    /// (so they carry forward as the new defaults), but the `@config.X` strings
    /// in the state machine are left intact so the consumer's
    /// `apply_overrides_to_trait` pass can replace them. Without this, a config
    /// override on a variation (consumer → variation → base) is clobbered by the
    /// variation's own default at import-load time. See the long note on
    /// `inline_orbital`'s `is_top_level` param.
    pub(crate) defer_config_subst: bool,
}

impl InlineContext {
    pub(crate) fn new() -> Self {
        Self {
            aliases: HashMap::new(),
            defer_config_subst: false,
        }
    }

    /// Back-compat helper for tests and call sites that import a single
    /// orbital. Wraps the orbital in an `AliasEntry` using the orbital's own
    /// `name` as the source label.
    #[allow(dead_code)] // used only by #[cfg(test)] call sites
    pub(crate) fn add_alias(&mut self, alias: &str, orbital: &Orbital) {
        let label = orbital.name.clone();
        self.aliases.insert(
            alias.to_string(),
            AliasEntry {
                orbitals: std::rc::Rc::new(vec![orbital.clone()]),
                source_label: label,
            },
        );
    }

    /// Register an alias whose imported behavior has multiple orbitals. The
    /// resolver searches every orbital in `orbitals` when resolving
    /// `Alias.traits.X`, `Alias.pages.X`, `Alias.services.X`, and
    /// `Alias.entity`. `source_label` is used only in error messages.
    pub(crate) fn add_alias_multi(
        &mut self,
        alias: &str,
        orbitals: impl Into<std::rc::Rc<Vec<Orbital>>>,
        source_label: String,
    ) {
        self.aliases.insert(
            alias.to_string(),
            AliasEntry {
                orbitals: orbitals.into(),
                source_label,
            },
        );
    }

    pub(crate) fn get_entry(&self, alias: &str) -> Result<&AliasEntry, InlineError> {
        self.aliases
            .get(alias)
            .ok_or_else(|| InlineError::UnknownAlias(alias.to_string()))
    }

    /// Find the entity belonging to the specific orbital that contains a
    /// given trait name. Used by the auto-merge logic: when a caller's trait
    /// ref points at `Alias.traits.X` with a `linkedEntity`, we need the
    /// entity from the orbital that owns trait `X` (not necessarily orbital 0).
    /// Returns `None` if the trait is not found or the owning orbital has no
    /// inline entity.
    ///
    /// `trait_ref_id` is the V4 dual-carry id sibling of the caller's ref
    /// (trait def `id`, or a `Reference`'s `ref_id`). When present it is
    /// tried FIRST against every orbital's trait id; only when it is absent
    /// or matches nothing does the existing name-based scan below run —
    /// so old imports without ids resolve exactly as before.
    pub(crate) fn get_entity_for_trait_orbital(
        &self,
        alias: &str,
        trait_name: &str,
        trait_ref_id: Option<&str>,
    ) -> Option<EntityDefinition> {
        let entry = self.aliases.get(alias)?;

        if let Some(want_id) = trait_ref_id {
            for orbital in entry.orbitals.iter() {
                let has_trait = orbital.traits.iter().any(|tr| match tr {
                    TraitReference::Inline(def) => def.id.as_deref() == Some(want_id),
                    TraitReference::Reference {
                        ref_id: Some(rid), ..
                    } => rid == want_id,
                    _ => false,
                });
                if has_trait {
                    if let EntityRef::Inline(entity_def) = &orbital.entity {
                        return Some(entity_def.clone());
                    }
                }
            }
        }

        for orbital in entry.orbitals.iter() {
            let has_trait = orbital.traits.iter().any(|tr| match tr {
                TraitReference::Inline(def) => def.name == trait_name,
                TraitReference::Reference { name: Some(n), .. } => n == trait_name,
                TraitReference::Reference {
                    reference: Some(r), ..
                } => r.ends_with(&format!(".{}", trait_name)),
                _ => false,
            });
            if has_trait {
                if let EntityRef::Inline(entity_def) = &orbital.entity {
                    return Some(entity_def.clone());
                }
            }
        }
        None
    }

    /// The imported orbital's OWN auxiliary entities (inline defs) for the
    /// orbital that carries `trait_name`. Companion to
    /// `get_entity_for_trait_orbital`: that returns the trait-bound primary,
    /// this returns the sibling entities the trait's body references by name
    /// (fetch/persist across a multi-entity atom). Same orbital-matching
    /// rules — `ref_id` first, then name.
    pub(crate) fn get_aux_entities_for_trait_orbital(
        &self,
        alias: &str,
        trait_name: &str,
        trait_ref_id: Option<&str>,
    ) -> Vec<EntityDefinition> {
        let Some(entry) = self.aliases.get(alias) else {
            return Vec::new();
        };
        let collect = |orbital: &Orbital| -> Vec<EntityDefinition> {
            orbital
                .auxiliary_entities
                .iter()
                .flatten()
                .filter_map(|e| match e {
                    EntityRef::Inline(def) => Some(def.clone()),
                    _ => None,
                })
                .collect()
        };
        if let Some(want_id) = trait_ref_id {
            for orbital in entry.orbitals.iter() {
                let has_trait = orbital.traits.iter().any(|tr| match tr {
                    TraitReference::Inline(def) => def.id.as_deref() == Some(want_id),
                    TraitReference::Reference {
                        ref_id: Some(rid), ..
                    } => rid == want_id,
                    _ => false,
                });
                if has_trait {
                    return collect(orbital);
                }
            }
        }
        for orbital in entry.orbitals.iter() {
            let has_trait = orbital.traits.iter().any(|tr| match tr {
                TraitReference::Inline(def) => def.name == trait_name,
                TraitReference::Reference { name: Some(n), .. } => n == trait_name,
                TraitReference::Reference {
                    reference: Some(r), ..
                } => r.ends_with(&format!(".{}", trait_name)),
                _ => false,
            });
            if has_trait {
                return collect(orbital);
            }
        }
        Vec::new()
    }

    /// The `expects` declarations of the orbital that owns an imported trait.
    ///
    /// Expectations must travel with an import for exactly the reason auxiliary
    /// entities do: `uses` inlines individual TRAITS, never orbitals, so an
    /// organism's `expects identity { … }` is otherwise invisible to the app
    /// composing it. The contract would then be checked only when the organism
    /// is validated STANDALONE — where its own roster satisfies it trivially —
    /// and a composing app whose roster lacks a field the organism reads off
    /// `@user` would validate clean. Merging the declarations onto the consuming
    /// orbital is what makes the host's roster actually get verified.
    pub(crate) fn get_expects_for_trait_orbital(
        &self,
        alias: &str,
        trait_name: &str,
        trait_ref_id: Option<&str>,
    ) -> Vec<ExpectDeclaration> {
        let Some(entry) = self.aliases.get(alias) else {
            return Vec::new();
        };
        let Some((orbital, _)) = Self::find_orbital_for_trait(entry, trait_name, trait_ref_id)
        else {
            return Vec::new();
        };
        orbital.expects.clone().unwrap_or_default()
    }

    /// The `types` map (Gap A struct aliases, `docs/Almadar_LOLO.md`) of the
    /// orbital that owns an imported trait — the LEXICAL scope a `::`
    /// struct-alias arg or a declared param default resolves against for a
    /// spliced trait. `inline_orbital`'s pre-pass merges the SPECIFIC alias
    /// shapes such a clause references onto the consuming orbital's own
    /// `types` map (never onto the trait itself), so the existing
    /// per-orbital sentinel-resolution pass — which only ever consults the
    /// CURRENT orbital's `types` — finds an alias declared in the imported
    /// atom's file. Same orbital-matching rules as `get_expects_for_trait_orbital`.
    pub(crate) fn get_types_for_trait_orbital(
        &self,
        alias: &str,
        trait_name: &str,
        trait_ref_id: Option<&str>,
    ) -> BTreeMap<String, Vec<PayloadField>> {
        let Some(entry) = self.aliases.get(alias) else {
            return BTreeMap::new();
        };
        let Some((orbital, _)) = Self::find_orbital_for_trait(entry, trait_name, trait_ref_id)
        else {
            return BTreeMap::new();
        };
        orbital.types.clone().unwrap_or_default()
    }

    /// §52 (C-RESOLVE-ATOM-MERGE-FIRST-ENTITY): the entity the trait's OWN
    /// `->` binding names, resolved across the owning orbital's primary AND
    /// auxiliary entities. Falls back to the orbital's primary — the
    /// historical `get_entity_for_trait_orbital` result — when the trait
    /// declares no binding or the name matches nothing, so every
    /// single-entity source resolves exactly as before. Same
    /// orbital-matching rules: `ref_id` first, then name.
    pub(crate) fn get_bound_entity_for_trait_orbital(
        &self,
        alias: &str,
        trait_name: &str,
        trait_ref_id: Option<&str>,
    ) -> Option<EntityDefinition> {
        let entry = self.aliases.get(alias)?;
        let (orbital, matched) = Self::find_orbital_for_trait(entry, trait_name, trait_ref_id)?;
        let bound = match matched {
            TraitReference::Inline(def) => def.linked_entity.as_deref(),
            TraitReference::Reference { linked_entity, .. } => linked_entity.as_deref(),
        };
        if let Some(name) = bound {
            if let EntityRef::Inline(e) = &orbital.entity {
                if e.name == name {
                    return Some(e.clone());
                }
            }
            for aux in orbital.auxiliary_entities.iter().flatten() {
                if let EntityRef::Inline(e) = aux {
                    if e.name == name {
                        return Some(e.clone());
                    }
                }
            }
        }
        self.get_entity_for_trait_orbital(alias, trait_name, trait_ref_id)
    }

    /// §B4-R5: the declared `config {}` of the orbital that OWNS an imported
    /// trait — the rung a `Alias.traits.X` reference falls through to when
    /// neither the consumer's embedder chain nor its own `orbital.config`
    /// resolves one of the trait's `@config.<knob>` forwards. Mirrors
    /// `get_expects_for_trait_orbital`/`get_types_for_trait_orbital`: same
    /// orbital-matching rules (`trait_ref_id` first, then the name scan),
    /// same shared `find_orbital_for_trait` lookup — one owner for "which
    /// orbital declares this trait".
    pub(crate) fn get_orbital_config_for_trait(
        &self,
        alias: &str,
        trait_name: &str,
        trait_ref_id: Option<&str>,
    ) -> Option<HashMap<String, ConfigField>> {
        let entry = self.aliases.get(alias)?;
        let (orbital, _) = Self::find_orbital_for_trait(entry, trait_name, trait_ref_id)?;
        orbital.config.clone()
    }

    /// Locate the orbital under `entry` that carries `trait_name` and the
    /// matched trait itself — `trait_ref_id` first, then the name scan, the
    /// same precedence `get_entity_for_trait_orbital` applies.
    fn find_orbital_for_trait<'a>(
        entry: &'a AliasEntry,
        trait_name: &str,
        trait_ref_id: Option<&str>,
    ) -> Option<(&'a Orbital, &'a TraitReference)> {
        if let Some(want_id) = trait_ref_id {
            for orbital in entry.orbitals.iter() {
                let matched = orbital.traits.iter().find(|tr| match tr {
                    TraitReference::Inline(def) => def.id.as_deref() == Some(want_id),
                    TraitReference::Reference {
                        ref_id: Some(rid), ..
                    } => rid == want_id,
                    _ => false,
                });
                if let Some(tr) = matched {
                    return Some((orbital, tr));
                }
            }
        }
        for orbital in entry.orbitals.iter() {
            let matched = orbital.traits.iter().find(|tr| match tr {
                TraitReference::Inline(def) => def.name == trait_name,
                TraitReference::Reference { name: Some(n), .. } => n == trait_name,
                TraitReference::Reference {
                    reference: Some(r), ..
                } => r.ends_with(&format!(".{}", trait_name)),
                _ => false,
            });
            if let Some(tr) = matched {
                return Some((orbital, tr));
            }
        }
        None
    }

    /// V4: resolve a trait by its dual-carry `ref_id` (an imported
    /// `TraitDefinition.id`, or a same-alias `Reference`'s own `ref_id`)
    /// instead of by name. Returns `None` — not an error — when nothing
    /// matches, so callers try this first and fall back to `get_trait` by
    /// name unchanged; this never replaces the name path.
    pub(crate) fn get_trait_by_ref_id(&self, alias: &str, ref_id: &str) -> Option<TraitReference> {
        let entry = self.aliases.get(alias)?;
        for orbital in entry.orbitals.iter() {
            for trait_ref in &orbital.traits {
                match trait_ref {
                    TraitReference::Inline(trait_def) => {
                        if trait_def.id.as_deref() == Some(ref_id) {
                            return Some(TraitReference::Inline(trait_def.clone()));
                        }
                    }
                    TraitReference::Reference {
                        ref_id: rid, id, ..
                    } => {
                        // Match the entry's TARGET id (transitive re-export
                        // semantics) OR its LOCAL declaration id (the
                        // composed-surface backbone mints one per Reference
                        // declaration, and cross-file refIds now resolve to it).
                        if rid.as_deref() == Some(ref_id) || id.as_deref() == Some(ref_id) {
                            return Some(trait_ref.clone());
                        }
                    }
                }
            }
        }
        None
    }

    /// V4: resolve a page by its dual-carry `refId` instead of by name.
    /// Returns `None` when nothing matches so callers fall back to
    /// `get_page` by name unchanged.
    pub(crate) fn get_page_by_ref_id(&self, alias: &str, ref_id: &str) -> Option<PageRef> {
        let entry = self.aliases.get(alias)?;
        for orbital in entry.orbitals.iter() {
            if let Some(pages) = &orbital.pages {
                for page_ref in pages {
                    if let PageRef::Inline(page_def) = page_ref {
                        if page_def.id.as_deref() == Some(ref_id) {
                            return Some(PageRef::Inline(page_def.clone()));
                        }
                    }
                }
            }
        }
        None
    }

    /// V4: resolve an entity by its dual-carry `extendsId` instead of by
    /// alias-only lookup. Searches each orbital's primary AND auxiliary
    /// entities. Returns `None` when nothing matches so callers fall back
    /// to `get_entity` (alias's first orbital) unchanged.
    pub(crate) fn get_entity_by_extends_id(
        &self,
        alias: &str,
        extends_id: &str,
    ) -> Option<EntityDefinition> {
        let entry = self.aliases.get(alias)?;
        for orbital in entry.orbitals.iter() {
            if let EntityRef::Inline(e) = &orbital.entity {
                if e.id.as_deref() == Some(extends_id) {
                    return Some(e.clone());
                }
            }
            if let Some(aux) = &orbital.auxiliary_entities {
                for a in aux {
                    if let EntityRef::Inline(e) = a {
                        if e.id.as_deref() == Some(extends_id) {
                            return Some(e.clone());
                        }
                    }
                }
            }
        }
        None
    }

    /// Find an entity definition by NAME across an alias's orbitals, searching
    /// each orbital's primary entity AND its auxiliary entities. Used when a
    /// sibling-pulled no-rebind trait keeps its own bound entity (e.g. a
    /// step-flow's `ProgressVerticalView`): that entity lives in the source
    /// behavior's auxiliary list after the source was recursively inlined, and
    /// must be carried into the consumer that pulls the sibling.
    /// Find an entity definition by NAME across EVERY alias in scope, not just
    /// one. A pulled sibling REBOUND by its embedder can be retargeted to an
    /// entity that came from a DIFFERENT `uses` alias than the one the sibling
    /// was pulled from — e.g. `AuditBrowseList = Browse.traits.BrowseItemBrowse
    /// -> AuditEntry`, where `AuditEntry` belongs to `std-audit-capture`, not
    /// to `std-browse`. Searching only the pull's own alias misses it, and the
    /// consumer orbital then holds a trait bound to an entity it does not have.
    pub(crate) fn get_entity_by_name_any_alias(
        &self,
        entity_name: &str,
    ) -> Option<EntityDefinition> {
        let mut aliases: Vec<&String> = self.aliases.keys().collect();
        // Deterministic: two aliases can expose the same entity name, and the
        // pick must not depend on HashMap iteration order.
        aliases.sort();
        for alias in aliases {
            if let Some(found) = self.get_entity_by_name_for_alias(alias, entity_name) {
                return Some(found);
            }
        }
        None
    }

    pub(crate) fn get_entity_by_name_for_alias(
        &self,
        alias: &str,
        entity_name: &str,
    ) -> Option<EntityDefinition> {
        let entry = self.aliases.get(alias)?;
        for orbital in entry.orbitals.iter() {
            if let EntityRef::Inline(e) = &orbital.entity {
                if e.name == entity_name {
                    return Some(e.clone());
                }
            }
            if let Some(aux) = &orbital.auxiliary_entities {
                for a in aux {
                    if let EntityRef::Inline(e) = a {
                        if e.name == entity_name {
                            return Some(e.clone());
                        }
                    }
                }
            }
        }
        None
    }

    pub(crate) fn get_entity(&self, alias: &str) -> Result<EntityRef, InlineError> {
        let entry = self.get_entry(alias)?;

        // `Alias.entity` is historically the first orbital's entity — every
        // std atom/molecule has exactly one orbital, so this preserves
        // long-standing behaviour. For multi-orbital imports the first
        // orbital is treated as the "primary" entity. Callers that need a
        // specific orbital's entity go through the trait/page/service
        // lookups which do iterate all orbitals.
        let Some(orbital) = entry.orbitals.first() else {
            return Err(InlineError::EntityNotFound(entry.source_label.clone()));
        };

        match &orbital.entity {
            EntityRef::Inline(entity) => Ok(EntityRef::Inline(entity.clone())),
            EntityRef::Reference(_) | EntityRef::Extends(_) => {
                // The referenced orbital itself still has a non-inline
                // entity (Reference or Extends). The inline phase processes
                // imports breadth-first via the loader, so by the time the
                // alias is added to this context the upstream entity should
                // already be Inline. If it isn't, the import order is wrong
                // or the upstream registry produced a non-inline entity —
                // either way the molecule cannot resolve it here. Surface
                // the same EntityNotFound the original `Reference` arm
                // returned so callers don't need a new error variant.
                Err(InlineError::EntityNotFound(entry.source_label.clone()))
            }
        }
    }

    pub(crate) fn get_trait(&self, alias: &str, name: &str) -> Result<TraitReference, InlineError> {
        let entry = self.get_entry(alias)?;

        // Iterate every orbital of the imported behavior. Traits with a given
        // name are unique across a single behavior's orbitals by convention;
        // the first match wins if duplicates somehow appear.
        for orbital in entry.orbitals.iter() {
            for trait_ref in &orbital.traits {
                match trait_ref {
                    TraitReference::Inline(trait_def) => {
                        if trait_def.name == name {
                            return Ok(TraitReference::Inline(trait_def.clone()));
                        }
                    }
                    TraitReference::Reference {
                        reference,
                        name: trait_name,
                        ..
                    } => {
                        // Check if the reference matches or name matches
                        if trait_name.as_deref() == Some(name) {
                            // Return as-is - it's still a reference that might need further resolution
                            return Ok(trait_ref.clone());
                        }
                        if let Some(ref_str) = reference {
                            // If ref matches the pattern we're looking for
                            if ref_str.ends_with(&format!(".{}", name)) {
                                return Ok(trait_ref.clone());
                            }
                        }
                    }
                }
            }
        }

        Err(InlineError::TraitNotFound(
            name.to_string(),
            entry.source_label.clone(),
        ))
    }

    /// The trait `name` as the orbital of `alias` declaring the trait `beside` has it; the
    /// alias-wide lookup ([`Self::get_trait`]) only when no orbital declaring `beside` has it.
    /// Two orbitals composing one atom each hold that atom's traits under the same names.
    pub(crate) fn get_trait_beside(
        &self,
        alias: &str,
        beside: &str,
        name: &str,
    ) -> Result<TraitReference, InlineError> {
        let entry = self.get_entry(alias)?;
        let declares = |traits: &[TraitReference], wanted: &str| {
            traits.iter().find_map(|t| match t {
                TraitReference::Inline(td) if td.name == wanted => Some(t.clone()),
                _ => None,
            })
        };
        for orbital in entry.orbitals.iter() {
            if declares(&orbital.traits, beside).is_some() {
                if let Some(found) = declares(&orbital.traits, name) {
                    return Ok(found);
                }
            }
        }
        self.get_trait(alias, name)
    }

    pub(crate) fn get_page(&self, alias: &str, name: &str) -> Result<PageRef, InlineError> {
        let entry = self.get_entry(alias)?;

        // Iterate every orbital — pages, like traits, may live in orbital
        // 1+ of a multi-orbital behavior.
        for orbital in entry.orbitals.iter() {
            if let Some(pages) = &orbital.pages {
                for page_ref in pages {
                    match page_ref {
                        PageRef::Inline(page_def) => {
                            if page_def.name == name {
                                return Ok(PageRef::Inline(page_def.clone()));
                            }
                        }
                        PageRef::Object(obj) => {
                            // Object references need further resolution
                            if obj.reference.ends_with(&format!(".{}", name)) {
                                return Ok(page_ref.clone());
                            }
                        }
                        PageRef::Reference(ref_str) => {
                            if ref_str.ends_with(&format!(".{}", name)) {
                                return Ok(page_ref.clone());
                            }
                        }
                    }
                }
            }
        }

        Err(InlineError::PageNotFound(
            name.to_string(),
            entry.source_label.clone(),
        ))
    }

    pub(crate) fn get_theme(&self, alias: &str) -> Result<ThemeRef, InlineError> {
        let entry = self.get_entry(alias)?;

        // Theme lives at the schema level in `.orb` files, but the loader
        // currently surfaces each orbital individually. Search every orbital
        // for an inline theme; the first match wins.
        for orbital in entry.orbitals.iter() {
            match &orbital.theme {
                Some(ThemeRef::Inline(theme)) => return Ok(ThemeRef::Inline(theme.clone())),
                _ => continue,
            }
        }
        Err(InlineError::ThemeNotFound(entry.source_label.clone()))
    }

    pub(crate) fn get_service(&self, alias: &str, name: &str) -> Result<ServiceRef, InlineError> {
        let entry = self.get_entry(alias)?;

        // Iterate every orbital — services, like traits/pages, may live in
        // orbital 1+ of a multi-orbital behavior.
        for orbital in entry.orbitals.iter() {
            if let Some(services) = &orbital.services {
                for service_ref in services {
                    match service_ref {
                        ServiceRef::Inline(service_def) => {
                            if service_def.name() == name {
                                return Ok(ServiceRef::Inline(service_def.clone()));
                            }
                        }
                        ServiceRef::Object(obj) => {
                            // Object references inside the upstream registry
                            // need further resolution. Mirrors the PageRef::Object
                            // case in `get_page` above.
                            if obj.reference.ends_with(&format!(".{}", name)) {
                                return Ok(service_ref.clone());
                            }
                        }
                        ServiceRef::Reference(ref_str) => {
                            if ref_str.ends_with(&format!(".{}", name)) {
                                return Ok(service_ref.clone());
                            }
                        }
                    }
                }
            }
        }

        Err(InlineError::ServiceNotFound(
            name.to_string(),
            entry.source_label.clone(),
        ))
    }

    /// Look up an orbital by name within an imported behavior — the
    /// orbital-keyed twin of `get_trait`/`get_page`/`get_service`. The
    /// returned `Orbital` is already fully inlined: `add_alias_multi` only
    /// ever stores orbitals that have themselves been through
    /// `inline_orbital` (recursively, for every loaded import), so a
    /// reference-form orbital among `entry.orbitals` has already had its own
    /// `reference` resolved by the time this runs — nested orbital-imports
    /// come for free.
    pub(crate) fn get_orbital(&self, alias: &str, name: &str) -> Result<Orbital, InlineError> {
        let entry = self.get_entry(alias)?;
        entry
            .orbitals
            .iter()
            .find(|o| o.name == name)
            .cloned()
            .ok_or_else(|| {
                InlineError::OrbitalNotFound(name.to_string(), entry.source_label.clone())
            })
    }
}
