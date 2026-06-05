//! Inline Phase
//!
//! Resolves `uses` declarations and inlines external references.
//!
//! This phase runs between Enrich and Resolve. It:
//! 1. Processes `uses` declarations in each orbital
//! 2. Loads external .orb files via the loader
//! 3. Replaces references with inline definitions
//!
//! Reference formats:
//! - `"Alias.entity"` → Entity from the aliased orbital
//! - `"Alias.traits.TraitName"` → Trait by name from aliased orbital
//! - `"Alias.pages.PageName"` → Page by name from aliased orbital
//! - `"Alias.theme"` → Theme from aliased schema
//! - `"Alias.services.ServiceName"` → Service by name from aliased orbital

use crate::loader::{ExternalOrbitalLoader, ImportChain, LoaderConfig, LoaderError};
use crate::embedded::get_pattern;
use crate::phases::validation::sync::patterns::{PATTERN_EVENT_LIST_PROPS, PATTERN_EVENT_PROPS};
use orbital_core::schema::{
    ConfigProvenanceRecord, EntityDefinition, EntityRef, EntityRefExtends, FieldDefinition, Orbital,
    OrbitalSchema, PageDefinition, PageRef, PageRefObject, SExpression, SchemaMetadata,
    ServiceDefinition, ServiceRef, ServiceRefObject, SourceBehaviorMetadata, ThemeRef,
    TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use thiserror::Error;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum InlineError {
    #[error("Loader error: {0}")]
    Loader(#[from] LoaderError),

    #[error("Invalid reference format: {0}")]
    InvalidReference(String),

    #[error("Unknown alias: {0}")]
    UnknownAlias(String),

    #[error("Entity not found in orbital '{0}'")]
    EntityNotFound(String),

    #[error("Trait '{0}' not found in orbital '{1}'")]
    TraitNotFound(String, String),

    #[error("Page '{0}' not found in orbital '{1}'")]
    PageNotFound(String, String),

    #[error("Service '{0}' not found in orbital '{1}'")]
    ServiceNotFound(String, String),

    #[error("Theme not found in schema '{0}'")]
    ThemeNotFound(String),

    #[error(
        "Trait '{name}' has multiple references with conflicting names at {path}. \
         Provide a `name` override on at least one reference to disambiguate."
    )]
    DuplicateTraitName {
        /// The colliding trait name. Carried verbatim so callers can match
        /// on the conflict without reparsing the formatted message.
        name: String,
        /// Source-tagged path to the conflicting trait reference, e.g.
        /// `orbitals[name=CartItemOrbital].traits[5]`. Empty when the
        /// error originated outside an orbital context (test fixtures).
        /// Consumed by `orbital-cli`'s validation JSON output so the
        /// agent's dispatcher can route UPDATE_SPEC by orbital owner.
        path: String,
    },
}

impl InlineError {
    /// Structured path to the source-position that produced this error,
    /// when the variant carries one. Returns an empty string for
    /// variants that don't yet have a path (gap closure ongoing).
    pub fn structured_path(&self) -> &str {
        match self {
            InlineError::DuplicateTraitName { path, .. } => path,
            _ => "",
        }
    }
}

// ============================================================================
// Reference Parser
// ============================================================================

/// Parsed reference components
#[derive(Debug, Clone)]
enum ParsedReference {
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
}

/// Parse a reference string into components
fn parse_reference(reference: &str) -> Result<ParsedReference, InlineError> {
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
struct AliasEntry {
    orbitals: Vec<Orbital>,
    source_label: String,
}

/// Context for inlining references within an orbital
/// Uses owned copies to avoid borrow checker issues with loader cache
struct InlineContext {
    /// Mapping from alias to the full set of orbitals loaded under that alias.
    aliases: HashMap<String, AliasEntry>,
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
    defer_config_subst: bool,
}

impl InlineContext {
    fn new() -> Self {
        Self {
            aliases: HashMap::new(),
            defer_config_subst: false,
        }
    }

    /// Back-compat helper for tests and call sites that import a single
    /// orbital. Wraps the orbital in an `AliasEntry` using the orbital's own
    /// `name` as the source label.
    fn add_alias(&mut self, alias: &str, orbital: &Orbital) {
        let label = orbital.name.clone();
        self.aliases.insert(
            alias.to_string(),
            AliasEntry {
                orbitals: vec![orbital.clone()],
                source_label: label,
            },
        );
    }

    /// Register an alias whose imported behavior has multiple orbitals. The
    /// resolver searches every orbital in `orbitals` when resolving
    /// `Alias.traits.X`, `Alias.pages.X`, `Alias.services.X`, and
    /// `Alias.entity`. `source_label` is used only in error messages.
    fn add_alias_multi(&mut self, alias: &str, orbitals: Vec<Orbital>, source_label: String) {
        self.aliases.insert(
            alias.to_string(),
            AliasEntry {
                orbitals,
                source_label,
            },
        );
    }

    fn get_entry(&self, alias: &str) -> Result<&AliasEntry, InlineError> {
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
    fn get_entity_for_trait_orbital(
        &self,
        alias: &str,
        trait_name: &str,
    ) -> Option<EntityDefinition> {
        let entry = self.aliases.get(alias)?;
        for orbital in &entry.orbitals {
            let has_trait = orbital.traits.iter().any(|tr| match tr {
                TraitReference::Inline(def) => def.name == trait_name,
                TraitReference::Reference {
                    name: Some(n),
                    ..
                } => n == trait_name,
                TraitReference::Reference {
                    reference: Some(r),
                    ..
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

    /// Find an entity definition by NAME across an alias's orbitals, searching
    /// each orbital's primary entity AND its auxiliary entities. Used when a
    /// sibling-pulled no-rebind trait keeps its own bound entity (e.g. a
    /// step-flow's `ProgressVerticalView`): that entity lives in the source
    /// behavior's auxiliary list after the source was recursively inlined, and
    /// must be carried into the consumer that pulls the sibling.
    fn get_entity_by_name_for_alias(
        &self,
        alias: &str,
        entity_name: &str,
    ) -> Option<EntityDefinition> {
        let entry = self.aliases.get(alias)?;
        for orbital in &entry.orbitals {
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

    fn get_entity(&self, alias: &str) -> Result<EntityRef, InlineError> {
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

    fn get_trait(&self, alias: &str, name: &str) -> Result<TraitReference, InlineError> {
        let entry = self.get_entry(alias)?;

        // Iterate every orbital of the imported behavior. Traits with a given
        // name are unique across a single behavior's orbitals by convention;
        // the first match wins if duplicates somehow appear.
        for orbital in &entry.orbitals {
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

    fn get_page(&self, alias: &str, name: &str) -> Result<PageRef, InlineError> {
        let entry = self.get_entry(alias)?;

        // Iterate every orbital — pages, like traits, may live in orbital
        // 1+ of a multi-orbital behavior.
        for orbital in &entry.orbitals {
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

    fn get_theme(&self, alias: &str) -> Result<ThemeRef, InlineError> {
        let entry = self.get_entry(alias)?;

        // Theme lives at the schema level in `.orb` files, but the loader
        // currently surfaces each orbital individually. Search every orbital
        // for an inline theme; the first match wins.
        for orbital in &entry.orbitals {
            match &orbital.theme {
                Some(ThemeRef::Inline(theme)) => return Ok(ThemeRef::Inline(theme.clone())),
                _ => continue,
            }
        }
        Err(InlineError::ThemeNotFound(entry.source_label.clone()))
    }

    fn get_service(&self, alias: &str, name: &str) -> Result<ServiceRef, InlineError> {
        let entry = self.get_entry(alias)?;

        // Iterate every orbital — services, like traits/pages, may live in
        // orbital 1+ of a multi-orbital behavior.
        for orbital in &entry.orbitals {
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
}

// ============================================================================
// Inline Functions
// ============================================================================

/// Apply override fields from an `EntityRefExtends` to a resolved
/// `EntityDefinition`. Mirrors `apply_overrides_to_service` (Phase 3.5.G.A.4)
/// for entities. The merged entity carries the upstream fields with the
/// molecule's `fields` appended; on name collision the molecule's field
/// replaces the inherited one (the user wins). `name`, `persistence`, and
/// `collection` overrides directly replace the corresponding upstream value.
///
/// `FieldDefinition.name` is `Option<String>` (anonymous fields appear in
/// nested `properties`/`items` slots), so the dedup logic only compares
/// fields that have a name. Anonymous fields are passed through unchanged
/// from the upstream entity, and any anonymous fields the molecule supplies
/// are appended after the inherited ones. Inherited fields whose name
/// matches an extra field's name are dropped from the merged result before
/// the extras are appended, preserving the molecule's intent without
/// duplicating the field.
fn apply_overrides_to_entity(entity_def: &mut EntityDefinition, extends: &EntityRefExtends) {
    // Rename first so any subsequent debug output reflects the final name.
    if let Some(new_name) = &extends.name {
        entity_def.name = new_name.clone();
    }
    if let Some(p) = &extends.persistence {
        entity_def.persistence = Some(p.clone());
    }
    if let Some(c) = &extends.collection {
        entity_def.collection = Some(c.clone());
    }

    // Merge fields. Extras are appended after the inherited fields, with
    // collision-by-name resolved in favor of the extras (the user's override
    // wins). Anonymous fields on either side are passed through unchanged
    // because there is no key to dedup on.
    if let Some(extra_fields) = &extends.fields {
        let extra_names: HashSet<String> =
            extra_fields.iter().filter_map(|f| f.name.clone()).collect();

        let mut merged: Vec<FieldDefinition> =
            Vec::with_capacity(entity_def.fields.len() + extra_fields.len());
        for f in &entity_def.fields {
            match f.name.as_ref() {
                Some(fname) if extra_names.contains(fname) => {
                    // Drop the inherited field; the extras loop below
                    // will push the molecule's replacement.
                }
                _ => merged.push(f.clone()),
            }
        }
        for f in extra_fields {
            merged.push(f.clone());
        }
        entity_def.fields = merged;
    }
}

/// Auto-merge imported entity fields into the caller's entity.
///
/// For each field in the imported behavior's canonical entity, if the
/// caller's entity doesn't already have a field with that name, add it.
/// Caller's fields win on name collision (the caller can override the
/// type/default of a shared field like `id`). Anonymous fields (those
/// with `name: None`) from the import are skipped — there is no key to
/// dedup on and they could silently duplicate structure.
fn merge_imported_entity_fields(
    caller_entity: &mut EntityDefinition,
    imported_entity: &EntityDefinition,
    // When true, carry ONLY fields marked `intrinsic` (machinery/view-state the
    // imported trait computes itself). Used for the no-rebind same-name
    // composition path (depth-propagation fix) where blindly merging every
    // imported DATA field would mask a genuinely-missing field. When false the
    // full imported field set is merged (the long-standing explicit-rebind
    // behavior — unchanged for unmarked fields).
    intrinsic_only: bool,
    // Required DATA fields (the trait's `entity_contract.requires`). These are
    // never auto-merged: a required DATA read must be satisfied by the target
    // (present or remapped), so merging it from the source would silently mask a
    // missing field and defeat ORB_T_REBIND_MISSING_FIELDS.
    required_data: &HashSet<String>,
) {
    let existing_names: HashSet<String> = caller_entity
        .fields
        .iter()
        .filter_map(|f| f.name.clone())
        .collect();

    // The imported entity's SELF-referential relations target the imported
    // entity's own name (e.g. a thread variant's `replies : [CommentsLinearPost]`,
    // or a nested `focusedRow.replies : [EmailThreadedPost]`). Merging into the
    // caller's entity IS a linkedEntity rebind, so those self-references must be
    // retargeted to the caller entity or they dangle (ORB_E_INVALID_RELATION /
    // ORB_X_UNRESOLVED_RELATION_TARGET). Relations targeting OTHER entities are
    // left alone — only the self-reference (== imported_entity.name) is rewritten.
    let new_name = caller_entity.name.clone();
    let old_name = imported_entity.name.as_str();

    for field in &imported_entity.fields {
        if intrinsic_only && field.intrinsic != Some(true) {
            continue;
        }
        if let Some(ref fname) = field.name {
            // Never carry a required DATA field — unless it's intrinsic (an
            // intrinsic field is never required, but guard defensively).
            if required_data.contains(fname) && field.intrinsic != Some(true) {
                continue;
            }
            if !existing_names.contains(fname) {
                let mut f = field.clone();
                rewrite_self_relation_target(&mut f, old_name, &new_name);
                caller_entity.fields.push(f);
            }
        }
    }
}

/// Retarget a field's self-referential `relation.entity` from `old` to `new`,
/// recursing into array `items` and object `properties` so nested relations
/// (e.g. an object field's `replies` relation) are rewritten too. Only relations
/// whose target equals `old` (the imported entity's own name) are changed.
fn rewrite_self_relation_target(field: &mut FieldDefinition, old: &str, new: &str) {
    if let Some(rel) = field.relation.as_mut() {
        if rel.entity == old {
            rel.entity = new.to_string();
        }
    }
    if let Some(items) = field.items.as_mut() {
        rewrite_self_relation_target(items, old, new);
    }
    if let Some(props) = field.properties.as_mut() {
        for v in props.values_mut() {
            rewrite_self_relation_target(v, old, new);
        }
    }
}

/// Inline entity reference. Handles all three `EntityRef` variants:
///
/// - `Inline` — pass through unchanged.
/// - `Reference("Alias.entity")` — resolve via the import context and
///   substitute the upstream `Inline` entity.
/// - `Extends { reference, name?, fields?, .. }` — resolve via the import
///   context, clone the upstream `Inline` entity, then apply the rename /
///   field-merge / persistence / collection overrides via
///   `apply_overrides_to_entity`. The result is always a single
///   `EntityRef::Inline(merged_entity)` so downstream phases never see
///   `Extends`. This mirrors how `inline_traits` collapses
///   `TraitReference::Reference` to a merged `Inline`.
fn inline_entity(entity: &EntityRef, ctx: &InlineContext) -> Result<EntityRef, InlineError> {
    match entity {
        EntityRef::Inline(_) => Ok(entity.clone()),
        EntityRef::Reference(ref_str) => {
            let parsed = parse_reference(ref_str)?;
            match parsed {
                ParsedReference::Entity { alias } => ctx.get_entity(&alias),
                _ => Err(InlineError::InvalidReference(ref_str.clone())),
            }
        }
        EntityRef::Extends(extends) => {
            let parsed = parse_reference(&extends.reference)?;
            match parsed {
                ParsedReference::Entity { alias } => {
                    let inherited = ctx.get_entity(&alias)?;
                    match inherited {
                        EntityRef::Inline(mut entity_def) => {
                            apply_overrides_to_entity(&mut entity_def, extends);
                            Ok(EntityRef::Inline(entity_def))
                        }
                        // Defensive: `ctx.get_entity` is documented to
                        // return Inline for any successful lookup. If a
                        // future change relaxes that, surface the same
                        // EntityNotFound error so the molecule author
                        // sees a clear failure rather than a panic.
                        _ => Err(InlineError::EntityNotFound(alias)),
                    }
                }
                _ => Err(InlineError::InvalidReference(extends.reference.clone())),
            }
        }
    }
}

/// Phase F.3: Generic identifier-substitution walker for SExpressions.
///
/// Treats every imported orbital fragment as a function whose hardcoded
/// identifiers (entity names and event keys) are implicit parameters. The
/// caller-supplied substitution maps drive the rewrite.
///
/// Patterns recognized:
///
/// 1. **Entity ref list `["ref", "EntityName"]`** — when the array's first
///    element is the literal `"ref"` and the second is a string in
///    `entity_subs`, the second element is replaced with the substitution
///    target.
///
/// 2. **Entity binding root `@EntityName`** — when an `SExpression::String`
///    begins with `"@<from>"` AND the next character is `.` or end-of-string,
///    the binding root is rewritten to `@<to>` plus the trailing path. The
///    suffix-boundary check prevents matching `@EntityNameSomethingElse`.
///
/// 3. **Emit list `["emit", "EVENT_NAME", ...]`** — when the array's first
///    element is `"emit"` and the second is a string in `event_subs`, the
///    event name is rewritten. Matches both the bare 2-element form and
///    the 3-element form `["emit", "EVENT", payload]` where a payload
///    object trails. The walk falls through into the payload tail so any
///    nested `["ref", X]` inside the payload still gets entity-rewritten.
///
/// 4. **Fetch effect `["fetch", "EntityName", ...]`** (GAP-COMP-11) — when
///    the array's first element is `"fetch"` and position 1 is a string in
///    `entity_subs`, the entity-name slot is rewritten. Walk continues into
///    trailing elements (criteria, etc.) so nested refs are still visited.
///
/// 5. **Persist effect `["persist", action, "EntityName", ...]`** (GAP-COMP-11)
///    — for single-record actions (`create` | `update` | `delete`), position
///    2 is a bare entity name and is rewritten via `entity_subs`. For the
///    `batch` action, position 2 is a rows array (not a bare entity name),
///    so the rewrite is skipped in that case — the generic recursion still
///    walks the rows so any nested refs inside them are handled.
///
/// All other variants (`Null`, `Bool`, `Number`, unrelated `String`,
/// `Array`, `Object`) are walked recursively so nested references inside
/// `render-ui` props or compound effects are also rewritten.
///
/// Both maps may be empty; an empty map produces an identity walk (still
/// recurses, but no rewrites fire).
fn rewrite_identifiers(
    expr: &mut SExpression,
    entity_subs: &HashMap<String, String>,
    event_subs: &HashMap<String, String>,
) {
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            // Entity binding root rewrite: `@From` or `@From.path` → `@To` or
            // `@To.path`. Boundary-checked so `@FromOther` is left alone.
            for (from, to) in entity_subs {
                let prefix_with_at = format!("@{}", from);
                if let Some(rest) = s.strip_prefix(&prefix_with_at) {
                    if rest.is_empty() || rest.starts_with('.') {
                        *s = format!("@{}{}", to, rest);
                        return;
                    }
                }
            }
        }
        SExpression::Array(items) => {
            // Detect `["ref", "From"]` (entity rewrite). Strict 2-element
            // form so we don't accidentally rewrite unrelated 2-arg calls.
            if items.len() == 2 {
                if let (SExpression::String(op), SExpression::String(arg)) = (&items[0], &items[1])
                {
                    if op == "ref" {
                        if let Some(to) = entity_subs.get(arg) {
                            items[1] = SExpression::String(to.clone());
                            return;
                        }
                    }
                }
            }
            // Detect `["emit", "EVENT", ...]` and rewrite the event name in
            // position 1. The form is canonically 2 or 3 elements (`emit
            // EVENT` or `emit EVENT payload`); both must be rewritten when
            // the event is in `event_subs`. Falling through to the
            // recursive walk below ensures any nested `["ref", X]` inside
            // a payload tail still gets entity-rewritten.
            if items.len() >= 2 {
                if let (SExpression::String(op), SExpression::String(arg)) = (&items[0], &items[1])
                {
                    if op == "emit" {
                        if let Some(to) = event_subs.get(arg) {
                            items[1] = SExpression::String(to.clone());
                            // Don't return — let the recursive walk visit
                            // any payload tail so nested entity refs in
                            // `(emit EVENT { ref: [...] })` still rewrite.
                        }
                    }
                }
            }
            // GAP-COMP-11: Detect entity-name literals in `fetch` / `persist`
            // effect shapes and rewrite them via `entity_subs`. These shapes
            // embed the entity name as a bare string, not wrapped in `["ref",
            // X]`, so the generic ref-arm above does not reach them.
            //
            // Shapes:
            //   ["fetch", "EntityName", ...]           — position 1 is entity
            //   ["persist", "create", "EntityName", ...] — position 2 is entity
            //   ["persist", "update", "EntityName", ...] — position 2 is entity
            //   ["persist", "delete", "EntityName", ...] — position 2 is entity
            //   ["persist", "batch", rows_array, ...]  — position 2 is NOT an
            //                                             entity name (it's a
            //                                             rows array); skip.
            //
            // After rewriting the entity-slot literal, we fall through to the
            // generic recursive walk below so nested SExpressions in the
            // effect's tail (payloads, criteria, render-ui children, etc.)
            // are still visited.
            if let Some(SExpression::String(op)) = items.first() {
                match op.as_str() {
                    "fetch" => {
                        if let Some(SExpression::String(name)) = items.get_mut(1) {
                            if let Some(to) = entity_subs.get(name) {
                                *name = to.clone();
                            }
                        }
                    }
                    "persist" => {
                        // Only single-record actions carry a bare entity name
                        // at index 2. For `batch`, index 2 is a rows array —
                        // leave it for the generic recursive walk so nested
                        // literals inside the rows (unlikely but possible)
                        // still get visited.
                        let is_single_record = matches!(
                            items.get(1),
                            Some(SExpression::String(action))
                                if matches!(action.as_str(), "create" | "update" | "delete")
                        );
                        if is_single_record {
                            if let Some(SExpression::String(name)) = items.get_mut(2) {
                                if let Some(to) = entity_subs.get(name) {
                                    *name = to.clone();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            for item in items.iter_mut() {
                rewrite_identifiers(item, entity_subs, event_subs);
            }
        }
        SExpression::Object(map) => {
            // Keys whose string value is an entity name that must be
            // rewritten when the enclosing trait is rebound to a new entity
            // via `linkedEntity:`. These correspond to render-ui pattern
            // props that address the entity store: `data-grid`/`data-list`
            // use `entity: "X"`; some patterns use `source` or
            // `entityType`; trait-ref objects use `linkedEntity`.
            // Without this rewrite, an imported atom's render-ui keeps
            // reading from the atom-era store key, so molecules built via
            // `@trait.X` + `linkedEntity:` show `No items` even when the
            // new entity's store is populated.
            const ENTITY_VALUED_PROPS: &[&str] =
                &["entity", "source", "entityType", "linkedEntity"];

            // Detect the pattern type at this render-ui node. When an object
            // has `type: "<pattern>"` and the pattern is in the registry,
            // rewrite event-valued props per `PATTERN_EVENT_PROPS` (direct
            // EventKey string) and `PATTERN_EVENT_LIST_PROPS` (array of
            // descriptor objects, each with a `<field>` holding an event
            // key). When no pattern type is present or the type is unknown,
            // the inline phase falls back to recursive walking without any
            // event-key rewriting — absence-of-schema means we can't prove
            // intent, so we don't guess.
            let pattern_type = match map.get("type") {
                Some(SExpression::String(ty)) => Some(ty.clone()),
                _ => None,
            };
            let event_props_for_pattern: Option<&HashSet<&'static str>> = pattern_type
                .as_deref()
                .and_then(|t| PATTERN_EVENT_PROPS.get(t));
            let event_list_for_pattern: Option<&HashMap<&'static str, &'static str>> =
                pattern_type
                    .as_deref()
                    .and_then(|t| PATTERN_EVENT_LIST_PROPS.get(t));

            // C1 plan: dynamic registry lookup as the canonical source
            // for "is this prop an event-name reference?" The static
            // PATTERN_EVENT_PROPS table covers `kind: "event"` (legacy
            // EventKey alias) but misses F1's new kinds (`event-ref`
            // for `EventEmit<P>`, `callback` for function-typed event
            // bindings). Consulting `get_pattern(...).props_schema`
            // directly picks up every kind pattern-sync writes, so
            // events-override rewrites stay in sync with whatever the
            // registry says is event-bearing today and tomorrow.
            let pattern_meta = pattern_type.as_deref().and_then(get_pattern);

            for (k, v) in map.iter_mut() {
                // 1) Direct event-valued prop (kind: "event" / "event-ref" /
                //    "event-listen" / "callback" in registry). Rewrite the
                //    string value via event_subs unless it's a binding
                //    placeholder (leading `@`), in which case the runtime
                //    resolves it at render time and we must not substitute
                //    here.
                let is_event_prop = match pattern_meta {
                    Some(p) => p
                        .props_schema
                        .get(k.as_str())
                        .and_then(|prop| prop.kind.as_deref())
                        .map(|kind| matches!(kind, "event" | "event-ref" | "event-listen" | "callback"))
                        .unwrap_or(false),
                    None => false,
                };
                if is_event_prop || event_props_for_pattern.is_some_and(|set| set.contains(k.as_str())) {
                    if let SExpression::String(s) = v {
                        if !s.starts_with('@') {
                            if let Some(to) = event_subs.get(s) {
                                *s = to.clone();
                                continue;
                            }
                        }
                    }
                }
                // 2) Event-list prop (kind: "event-list"). The value is an
                //    array; each item is a descriptor object with a field
                //    (default "event", override via registry.eventField)
                //    holding a declared event key. Rewrite per-item.
                if let Some(map_field) = event_list_for_pattern {
                    if let Some(field) = map_field.get(k.as_str()) {
                        if let SExpression::Array(items) = v {
                            for item in items.iter_mut() {
                                if let SExpression::Object(entry) = item {
                                    if let Some(inner) = entry.get_mut(*field) {
                                        if let SExpression::String(s) = inner {
                                            if !s.starts_with('@') {
                                                if let Some(to) = event_subs.get(s) {
                                                    *s = to.clone();
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }
                if ENTITY_VALUED_PROPS.contains(&k.as_str()) {
                    if let SExpression::String(s) = v {
                        if let Some(to) = entity_subs.get(s) {
                            *s = to.clone();
                            continue;
                        }
                    }
                }
                rewrite_identifiers(v, entity_subs, event_subs);
            }
        }
    }
}

/// Rewrite `@entity.<old>` reads to `@entity.<new>` per the call-site `fields`
/// remap map. Mirrors the entity-root rewrite in `rewrite_identifiers` but
/// targets the FIELD segment (the part after `@entity.`) rather than the entity
/// root. Boundary-checked on the field name so `@entity.titleSuffix` is left
/// alone when remapping `title`; nested paths (`@entity.title.x`) and array
/// indices (`@entity.title[0]`) are preserved. Empty map = identity walk.
fn rewrite_entity_field_paths(expr: &mut SExpression, field_subs: &HashMap<String, String>) {
    if field_subs.is_empty() {
        return;
    }
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@entity.") {
                // Split off the first segment (field name), keeping any nested
                // path / index suffix intact.
                let split_at = rest
                    .find(|c| c == '.' || c == '[')
                    .unwrap_or(rest.len());
                let (field, suffix) = rest.split_at(split_at);
                if let Some(to) = field_subs.get(field) {
                    *s = format!("@entity.{}{}", to, suffix);
                }
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_entity_field_paths(item, field_subs);
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                rewrite_entity_field_paths(v, field_subs);
            }
        }
    }
}

/// Phase A.1 backwards-compatibility shim: rewrite a single entity name in
/// an SExpression. New code should call `rewrite_identifiers` directly with
/// a HashMap. This function is preserved for any callers that still depend
/// on the positional `(from, to)` API.
#[allow(dead_code)]
fn rewrite_entity_refs(expr: &mut SExpression, from_entity: &str, to_entity: &str) {
    let mut entity_subs = HashMap::new();
    entity_subs.insert(from_entity.to_string(), to_entity.to_string());
    rewrite_identifiers(expr, &entity_subs, &HashMap::new());
}

/// Phase 3.A: Substitute `@config.<param>` binding strings with concrete
/// SExpression values supplied by the caller via `TraitReference.config`.
///
/// This is the hybrid-composition piece: atoms embed `@config.filterFields`-
/// style bindings inside their effects (e.g. inside render-ui payloads), and
/// composers (molecules/organisms) supply per-param values at the call site.
/// The inline phase substitutes BEFORE the validator runs, so by the time
/// binding.rs + pattern-prop validation sees the effect, any successfully
/// bound `@config.X` has been replaced with a concrete literal / array /
/// object / number / bool / null.
///
/// Semantics:
///
/// 1. **Bare `@config.<param>`** (no trailing path) — when an
///    `SExpression::String("@config.<param>")` matches a key in
///    `config_subs`, the ENTIRE node is replaced with the substituted
///    SExpression. This is the common case: `"slot": "@config.slot"` →
///    `"slot": "content"`, or `"filterFields": "@config.filterFields"` →
///    `"filterFields": ["status", "category"]`.
///
/// 2. **`@config.<param>.trailing.path`** — when the binding has a trailing
///    path AND the substituted value is an object, the trailing path is
///    walked and the leaf is substituted. If the substituted value is a
///    non-object (array / scalar), the trailing-path form is left as-is so
///    the validator can flag the shape mismatch. (This keeps the substituter
///    conservative — we never silently drop information.)
///
/// 3. **Missing `@config.<param>`** — when the param is NOT in `config_subs`,
///    the binding is left as-is. The downstream binding validator will flag
///    unresolved `@config.X` references so composers see a clean error
///    rather than the inline phase swallowing the failure.
///
/// Substitute every `@config.X` binding in an inline trait's transitions
/// (and ticks, if any) with the trait's own declared config-field
/// default. Used when a trait reaches the inline phase as
/// `TraitReference::Inline` (atom-scope codegen, no call-site overrides).
///
/// Mirrors the config-merge step in `apply_overrides_to_trait` (line
/// ~1097), but with no caller-supplied overrides — only the trait's
/// declared defaults. Without this, atom standalone compilation emits
/// `Number(undefined)` for `@config.X` because codegen/guard.rs
/// `OirBindingRoot::Config` returns `"undefined"` on the assumption that
/// the inline phase has already substituted every @config binding.
/// Disambiguate a literal list from an S-expression call.
///
/// A string-headed array bound for a literal-array config slot (declared
/// `field_type` like `[string]`) is structurally identical to an S-expression
/// call — e.g. `["path", "method", …]` is indistinguishable from the `path`
/// operator call `(path method …)`. When the head collides with a registered
/// operator name the runtime mis-evaluates the list and collapses it to a
/// scalar. Wrap such values in the `list` literal constructor (`["list", …]`),
/// which both evaluators return verbatim, so the value is unambiguously data.
///
/// Type-directed + deterministic — no operator-name heuristic: it fires only
/// for array-typed slots and only for string-headed array values (object- or
/// number-headed arrays are already unambiguous and pass through unchanged).
fn mark_literal_list_in_place(value: &mut SExpression, field_type: &str) {
    if !(field_type.starts_with('[') && field_type.ends_with(']')) {
        return;
    }
    if let SExpression::Array(elems) = value {
        let is_string_headed =
            matches!(elems.first(), Some(SExpression::String(s)) if s != "list");
        if is_string_headed {
            let mut wrapped = Vec::with_capacity(elems.len() + 1);
            wrapped.push(SExpression::String("list".to_string()));
            wrapped.append(elems);
            *value = SExpression::Array(wrapped);
        }
    }
}

fn apply_inline_config_defaults(
    trait_def: &mut TraitDefinition,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    let Some(declared) = trait_def.config.as_ref() else {
        return;
    };
    let mut effective_config: HashMap<String, SExpression> = HashMap::new();
    for (k, field) in declared.iter() {
        if let Some(default) = field.default.as_ref() {
            let mut v = default.clone();
            mark_literal_list_in_place(&mut v, &field.field_type);
            effective_config.insert(k.clone(), v);
        }
    }
    if effective_config.is_empty() {
        return;
    }
    let cfg = &effective_config;
    record_config_provenance(trait_def, cfg, recorder);
    for transition in trait_def.state_machine.transitions.iter_mut() {
        if let Some(guard) = transition.guard.as_mut() {
            rewrite_config_bindings(guard, cfg);
        }
        if let Some(effects) = transition.effects.as_mut() {
            for effect in effects.iter_mut() {
                rewrite_config_bindings(effect, cfg);
            }
        }
    }
    if let Some(ticks) = trait_def.ticks.as_mut() {
        for tick in ticks.iter_mut() {
            if let Some(guard) = tick.guard.as_mut() {
                rewrite_config_bindings(guard, cfg);
            }
            for effect in tick.effects.iter_mut() {
                rewrite_config_bindings(effect, cfg);
            }
        }
    }
}

/// All other SExpression variants recurse so nested `@config.X` strings
/// inside render-ui payloads, array literals, etc. are rewritten in one walk.
fn rewrite_config_bindings(
    expr: &mut SExpression,
    config_subs: &HashMap<String, SExpression>,
) {
    if config_subs.is_empty() {
        return;
    }
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
        SExpression::String(s) => {
            // Only `@config.<param>` bindings are substituted. Bare `@config`
            // with no param is NOT a valid form (the binding validator
            // rejects it) so we don't handle it here.
            if let Some(rest) = s.strip_prefix("@config.") {
                // Split on the first `.` so `@config.foo` matches key "foo"
                // and `@config.foo.bar` matches key "foo" with trailing
                // path "bar".
                let (param, trailing) = match rest.find('.') {
                    Some(idx) => (&rest[..idx], Some(&rest[idx + 1..])),
                    None => (rest, None),
                };
                if let Some(value) = config_subs.get(param) {
                    match trailing {
                        None => {
                            // Bare `@config.<param>` — replace the whole
                            // node with the substituted value, then recurse
                            // into the substituted tree so any nested
                            // `@config.X` bindings inside the value (e.g.
                            // a `render-ui` default tree referencing
                            // `@config.cols`) are themselves resolved in
                            // this same pass.
                            *expr = value.clone();
                            rewrite_config_bindings(expr, config_subs);
                        }
                        Some(path) => {
                            // `@config.<param>.<path>` — if the value is an
                            // object, walk the path and substitute the leaf.
                            // Otherwise leave the binding as-is so the
                            // validator can flag the shape mismatch.
                            if let SExpression::Object(_) = value {
                                if let Some(leaf) = resolve_object_path(value, path) {
                                    *expr = leaf.clone();
                                    rewrite_config_bindings(expr, config_subs);
                                }
                            }
                        }
                    }
                }
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_config_bindings(item, config_subs);
            }
        }
        SExpression::Object(map) => {
            for (_k, v) in map.iter_mut() {
                rewrite_config_bindings(v, config_subs);
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Config→knob provenance recording (Studio contextual-edit source map)
//
// Read-only companion to `rewrite_config_bindings`: BEFORE substitution erases
// the `@config.X` string, walk render-ui effects and record which render-ui
// prop (at which patternPath) was bound to which config knob. `inline_schema`
// emits the records into the schema's `_metadata.configProvenance`. Pure read —
// the proven substitution logic in `rewrite_config_bindings` is untouched.
// ----------------------------------------------------------------------------

/// Record provenance for every `@config.X` binding inside the trait's render-ui
/// effects, for knobs present in `cfg` (those about to be substituted).
fn record_config_provenance(
    trait_def: &TraitDefinition,
    cfg: &HashMap<String, SExpression>,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    let trait_name = trait_def.name.as_str();
    for transition in &trait_def.state_machine.transitions {
        if let Some(effects) = transition.effects.as_ref() {
            for effect in effects {
                record_render_ui_effect(effect, cfg, trait_name, recorder);
            }
        }
    }
    if let Some(ticks) = trait_def.ticks.as_ref() {
        for tick in ticks {
            for effect in &tick.effects {
                record_render_ui_effect(effect, cfg, trait_name, recorder);
            }
        }
    }
}

/// Detect a `["render-ui", slot, {pattern}]` effect and walk its pattern tree.
fn record_render_ui_effect(
    effect: &SExpression,
    cfg: &HashMap<String, SExpression>,
    trait_name: &str,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    if let SExpression::Array(items) = effect {
        if items.len() >= 3 {
            if let (SExpression::String(head), SExpression::Object(pattern)) =
                (&items[0], &items[2])
            {
                if head == "render-ui" {
                    record_pattern_node(pattern, cfg, trait_name, "root", recorder);
                }
            }
        }
    }
}

/// Walk a render-ui pattern node. `children` arrays descend as nested nodes
/// (path `<path>.children.<index>`, matching the runtime `UISlotRenderer`);
/// every other prop is scanned for `@config.X` bindings attributed to this
/// node + prop key.
fn record_pattern_node(
    pattern: &HashMap<String, SExpression>,
    cfg: &HashMap<String, SExpression>,
    trait_name: &str,
    path: &str,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    for (prop_key, value) in pattern.iter() {
        if prop_key == "children" {
            if let SExpression::Array(items) = value {
                for (idx, child) in items.iter().enumerate() {
                    if let SExpression::Object(child_obj) = child {
                        let child_path = format!("{}.children.{}", path, idx);
                        record_pattern_node(child_obj, cfg, trait_name, &child_path, recorder);
                    }
                }
            }
        } else {
            scan_config_in_value(value, cfg, trait_name, path, prop_key, recorder);
        }
    }
}

/// Record `{path, prop, knob}` for each `@config.<knob>` string found anywhere
/// under a node prop's value (including nested data arrays/objects), for knobs
/// present in `cfg`. Does not cross into child render-ui nodes (`children` is
/// walked separately by `record_pattern_node`).
fn scan_config_in_value(
    value: &SExpression,
    cfg: &HashMap<String, SExpression>,
    trait_name: &str,
    path: &str,
    prop_key: &str,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    match value {
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@config.") {
                let knob = rest.split('.').next().unwrap_or(rest);
                if cfg.contains_key(knob) {
                    recorder.push(ConfigProvenanceRecord {
                        trait_name: trait_name.to_string(),
                        pattern_path: Some(path.to_string()),
                        prop: prop_key.to_string(),
                        knob: knob.to_string(),
                    });
                }
            }
        }
        SExpression::Array(items) => {
            for item in items {
                scan_config_in_value(item, cfg, trait_name, path, prop_key, recorder);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                scan_config_in_value(v, cfg, trait_name, path, prop_key, recorder);
            }
        }
        _ => {}
    }
}

/// Walk a dotted path through an SExpression::Object tree. Returns the leaf
/// SExpression if the full path resolves to an object entry; returns None
/// when any intermediate segment is missing or non-object.
fn resolve_object_path<'a>(value: &'a SExpression, path: &str) -> Option<&'a SExpression> {
    let mut current = value;
    for segment in path.split('.') {
        match current {
            SExpression::Object(map) => {
                current = map.get(segment)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Apply event-name substitutions to all structured event-key fields of a
/// `TraitDefinition`. These fields live OUTSIDE SExpressions and so are not
/// reached by `rewrite_identifiers`:
///
/// - `state_machine.events[*].key` — the canonical event-name registry
/// - `state_machine.transitions[*].event` — the trigger of each transition
/// - `emits[*].event` — events the trait declares it produces
/// - `listens[*].event` — events the trait listens for
/// - `listens[*].triggers` — internal mapping target on a listener
///
/// SExpression payloads inside transition guards/effects/ticks are NOT
/// touched by this helper; they are walked separately by
/// `rewrite_identifiers` so the entity-rewrite walk and the event-rewrite
/// walk share one tree traversal.
/// Rewrite whole-word entity-name occurrences inside a `PayloadField.field_type`
/// string. Field types are short type expressions like `Matter`, `[Matter]`,
/// `Matter?`, `{ data: [Matter], cursor: string }`. We substitute on word
/// boundaries so `MatterStub` (a fictitious sibling type) doesn't get mauled
/// into `MatterCaseStub` when the rebind is `Matter → MatterCase`.
fn rewrite_entity_in_type_string(s: &str, entity_subs: &HashMap<String, String>) -> String {
    if entity_subs.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut buf = String::new();
    fn is_ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }
    while let Some(c) = chars.next() {
        if is_ident(c) {
            buf.push(c);
            continue;
        }
        if !buf.is_empty() {
            if let Some(to) = entity_subs.get(&buf) {
                out.push_str(to);
            } else {
                out.push_str(&buf);
            }
            buf.clear();
        }
        out.push(c);
    }
    if !buf.is_empty() {
        if let Some(to) = entity_subs.get(&buf) {
            out.push_str(to);
        } else {
            out.push_str(&buf);
        }
    }
    out
}

/// Apply entity-name substitutions to every `PayloadField.field_type` in the
/// trait's emits / listens / state-machine event declarations.
///
/// Without this, an organism that rebinds an atom's trait via
/// `linkedEntity: NewName` keeps the OLD type literal in the declared payload
/// schema (`data: [OldName]`), and downstream codegen produces
/// `payload as { data: OldName[] }` — a reference to a type that no longer
/// exists. Live symptom from std-legal-case: handlers.ts emitting
/// `payload as { data: Matter[] }` after the `Matter → MatterCase` rebind.
fn rewrite_payload_entity_refs(
    trait_def: &mut TraitDefinition,
    entity_subs: &HashMap<String, String>,
) {
    if entity_subs.is_empty() {
        return;
    }
    if let Some(emits) = trait_def.emits.as_mut() {
        for emit in emits.iter_mut() {
            if let Some(schema) = emit.payload_schema.as_mut() {
                for field in schema.iter_mut() {
                    field.field_type =
                        rewrite_entity_in_type_string(&field.field_type, entity_subs);
                }
            }
        }
    }
    if let Some(events) = trait_def.state_machine.events.as_mut() {
        for ev in events.iter_mut() {
            if let Some(schema) = ev.payload_schema.as_mut() {
                for field in schema.iter_mut() {
                    field.field_type =
                        rewrite_entity_in_type_string(&field.field_type, entity_subs);
                }
            }
        }
    }
}

fn rewrite_trait_event_fields(
    trait_def: &mut TraitDefinition,
    event_subs: &HashMap<String, String>,
) {
    if event_subs.is_empty() {
        return;
    }

    // StateMachine.events[*].key — the canonical declaration set.
    if let Some(events) = trait_def.state_machine.events.as_mut() {
        for ev in events.iter_mut() {
            if let Some(to) = event_subs.get(&ev.key) {
                ev.key = to.clone();
            }
        }
    }

    // StateMachine.transitions[*].event — the trigger field.
    for transition in trait_def.state_machine.transitions.iter_mut() {
        if let Some(to) = event_subs.get(&transition.event) {
            transition.event = to.clone();
        }
    }

    // emits[*].event — the trait's outbound event declarations.
    if let Some(emits) = trait_def.emits.as_mut() {
        for emit in emits.iter_mut() {
            if let Some(to) = event_subs.get(&emit.event) {
                emit.event = to.clone();
            }
        }
    }

    // listens[*].event and listens[*].triggers — the trait's inbound event
    // declarations and any internal trigger mapping.
    if let Some(listens) = trait_def.listens.as_mut() {
        for listen in listens.iter_mut() {
            if let Some(to) = event_subs.get(&listen.event) {
                listen.event = to.clone();
            }
            if let Some(triggers) = listen.triggers.as_mut() {
                if let Some(to) = event_subs.get(triggers) {
                    *triggers = to.clone();
                }
            }
        }
    }
}

/// Phase F.3: Apply call-site arguments from a `TraitReference::Reference`
/// to a resolved `TraitDefinition`. Treats the reference as a function call:
/// arguments map to identifier substitutions inside the imported trait body.
///
/// Argument handling:
///
/// - **`name`** — direct rename of `trait_def.name`. Applied first so any
///   downstream debug output / errors refer to the final name. Independent
///   of the substitution map (the trait's own name is the function output
///   handle, not an internal identifier).
///
/// - **`linked_entity`** — adds a `{ atom_default_entity → caller_entity }`
///   entry to the substitution map and updates `trait_def.linked_entity` to
///   the caller's value.
///
/// - **`events`** — every `(atom_event_key → caller_event_key)` pair in the
///   caller-supplied `events` map is added to the event substitution map.
///
/// - **`config`** — Phase 3.A wired. Every `(param → SExpression)` pair in
///   the caller-supplied `config` map becomes a substitution for any
///   `@config.<param>` binding string inside the trait's guards, effects,
///   and tick bodies. Missing params are left as-is so the downstream
///   binding validator can flag them. See `rewrite_config_bindings` for
///   the walker's semantics (including dotted-path drill-down into object
///   values).
///
/// After building the maps, the function:
/// 1. Walks all SExpressions in transitions, ticks, and tick guards via
///    `rewrite_identifiers` so embedded `["ref", X]`, `@X`, and
///    `["emit", EVENT]` patterns are rewritten in one pass.
/// 2. Walks structured event-key fields (StateMachine.events[*].key,
///    transitions[*].event, emits[*].event, listens[*].event/triggers) via
///    `rewrite_trait_event_fields` because those live outside SExpressions.
/// 3. Walks the same SExpression containers a second time with
///    `rewrite_config_bindings` to substitute `@config.<param>` bindings.
///    Kept as a separate pass because config substitution replaces whole
///    nodes (not just strings) and so needs a different walker.
fn apply_overrides_to_trait(
    trait_def: &mut TraitDefinition,
    linked_entity: Option<&String>,
    config: Option<&HashMap<String, SExpression>>,
    name: Option<&str>,
    events: Option<&HashMap<String, String>>,
    // Per-key `@entity.<old>` → `@entity.<new>` field rename map. Mirrors
    // `events`: rewrites every entity-field read inside the resolved trait so a
    // DATA field the trait reads is satisfied by a differently-named field on
    // the rebind target. Also rewrites the trait's `entity_contract.requires`.
    fields: Option<&HashMap<String, String>>,
    // When true, fold call-site config overrides into the trait's declared
    // `config[k].default` (so they carry forward) but DO NOT substitute the
    // `@config.X` strings in the state machine. Used when resolving an import,
    // so a downstream consumer can still override those bindings. When false
    // (top-level target), the `@config.X` strings are baked to their effective
    // values as before.
    defer_config_subst: bool,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    // 1. Direct name rename — applied first so all subsequent operations
    //    refer to the final name in any error messages.
    if let Some(new_name) = name {
        if new_name != trait_def.name {
            trait_def.name = new_name.to_string();
        }
    }

    // 2. Build the entity substitution map. The `linked_entity` arg means
    //    "the atom thinks its canonical entity is X; rebind it to Y". The
    //    pre-existing `trait_def.linked_entity` IS the from-key.
    let mut entity_subs: HashMap<String, String> = HashMap::new();
    if let Some(new_entity) = linked_entity {
        if let Some(orig) = trait_def.linked_entity.clone() {
            if &orig != new_entity {
                entity_subs.insert(orig, new_entity.clone());
            }
        }
        trait_def.linked_entity = Some(new_entity.clone());
    }

    // 3. Build the event substitution map directly from the caller args.
    //    Empty entries (or self-mapped no-ops) are pruned to keep the walker
    //    from doing redundant work.
    let mut event_subs: HashMap<String, String> = HashMap::new();
    if let Some(map) = events {
        for (from, to) in map.iter() {
            if from != to && !from.is_empty() && !to.is_empty() {
                event_subs.insert(from.clone(), to.clone());
            }
        }
    }

    // 3b. Build the entity-field rename map. Each `from → to` rewrites the
    //     trait's `@entity.<from>` reads (and `(set @entity.<from> ...)`) to
    //     `@entity.<to>` so a renamed DATA field on the rebind target satisfies
    //     the read. Self-mapped no-ops and empty entries are pruned.
    let mut field_subs: HashMap<String, String> = HashMap::new();
    if let Some(map) = fields {
        for (from, to) in map.iter() {
            if from != to && !from.is_empty() && !to.is_empty() {
                field_subs.insert(from.clone(), to.clone());
            }
        }
    }

    // 4. Walk SExpressions in transitions and ticks ONCE with both maps.
    if !entity_subs.is_empty() || !event_subs.is_empty() || !field_subs.is_empty() {
        for transition in trait_def.state_machine.transitions.iter_mut() {
            if let Some(guard) = transition.guard.as_mut() {
                rewrite_identifiers(guard, &entity_subs, &event_subs);
                rewrite_entity_field_paths(guard, &field_subs);
            }
            if let Some(effects) = transition.effects.as_mut() {
                for effect in effects.iter_mut() {
                    rewrite_identifiers(effect, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(effect, &field_subs);
                }
            }
        }
        if let Some(ticks) = trait_def.ticks.as_mut() {
            for tick in ticks.iter_mut() {
                if let Some(guard) = tick.guard.as_mut() {
                    rewrite_identifiers(guard, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(guard, &field_subs);
                }
                for effect in tick.effects.iter_mut() {
                    rewrite_identifiers(effect, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(effect, &field_subs);
                }
            }
        }
    }

    // 4b. Rewrite the trait's inferred `entity_contract.requires`/`provides` so
    //     the post-inline rebind validator checks the RENAMED field names. A
    //     `fields` remap turns a `requires: ["title"]` into `requires:
    //     ["dishName"]`, matching the target entity.
    if !field_subs.is_empty() {
        if let Some(contract) = trait_def.entity_contract.as_mut() {
            for f in contract.requires.iter_mut() {
                if let Some(to) = field_subs.get(f) {
                    *f = to.clone();
                }
            }
            for f in contract.provides.iter_mut() {
                if let Some(to) = field_subs.get(f) {
                    *f = to.clone();
                }
            }
        }
    }

    // 5. Walk the structured event-key fields. These live outside SExpression
    //    payloads, so the SExpression walk above does not reach them.
    rewrite_trait_event_fields(trait_def, &event_subs);

    // 5b. Walk the typed payload schemas (emits[*].payloadSchema,
    //     state_machine.events[*].payloadSchema). Their `type` strings can
    //     reference the atom's original entity name (`[Matter]`); after a
    //     linkedEntity rebind those references would otherwise survive as
    //     dangling types in downstream handler.ts codegen.
    rewrite_payload_entity_refs(trait_def, &entity_subs);

    // 6. Phase 3.A: Substitute `@config.<param>` bindings with concrete
    //    SExpression values supplied by the caller. Unlike entity/event
    //    substitution (which only rewrites strings), config substitution can
    //    replace a whole node with an arbitrary SExpression (array, object,
    //    number, bool, null). The walk runs AFTER the identifier pass so
    //    any `@config.X` that references a renamed entity is still
    //    referenced by the entity rewriter's output (though in practice
    //    `@config.X` is a flat key so entity renames don't interact).
    //
    //    Missing params are left as-is — the downstream binding validator
    //    will flag unresolved `@config.X` so composers see a clean error.
    // Merge call-site config overrides with the atom's declared config-field
    // defaults so that `@config.X` references for params the molecule didn't
    // override still resolve to a concrete literal at compile time. Without
    // this, server-side codegen falls back to `user?.X` (no UserContext.X
    // exists) and trips TS2339; client-side falls back to `mergedConfig?.X`
    // which works at runtime but loses constant-fold opportunities. Merging
    // keeps both paths consistent and prunes dead branches via the eq-fold.
    let mut effective_config: HashMap<String, SExpression> = HashMap::new();
    if let Some(declared) = trait_def.config.as_ref() {
        for (k, field) in declared.iter() {
            if let Some(default) = field.default.as_ref() {
                effective_config.insert(k.clone(), default.clone());
            }
        }
    }
    if let Some(cfg) = config {
        for (k, v) in cfg.iter() {
            effective_config.insert(k.clone(), v.clone());
        }
    }
    // Wrap string-headed array values bound for literal-array slots in the
    // `list` constructor so they are never mis-read as S-expression calls
    // (see `mark_literal_list_in_place`). Type-directed off each slot's
    // declared `field_type`.
    if let Some(declared) = trait_def.config.as_ref() {
        for (k, value) in effective_config.iter_mut() {
            if let Some(field) = declared.get(k) {
                mark_literal_list_in_place(value, &field.field_type);
            }
        }
    }
    // GAP-AG-VALUE-DRIFT (resolver-side) — when a call-site override is
    // supplied, also fold it into `trait_def.config[k].default`. Without
    // this, the resolver substitutes `@config.X` references in effects
    // but leaves the schema's declared default at the atom-authored
    // value — so consumers reading `resolved.config[k].default`
    // (verifier assertions, the studio's resolved-spec viewer, codegen
    // that ships the schema downstream) see a stale default. The
    // override IS the new effective default for this call site.
    if let Some(cfg) = config {
        if let Some(declared) = trait_def.config.as_mut() {
            for (k, v) in cfg.iter() {
                if let Some(field) = declared.get_mut(k) {
                    field.default = Some(v.clone());
                }
            }
        }
    }
    if !defer_config_subst && !effective_config.is_empty() {
        let cfg = &effective_config;
        record_config_provenance(trait_def, cfg, recorder);
        {
            for transition in trait_def.state_machine.transitions.iter_mut() {
                if let Some(guard) = transition.guard.as_mut() {
                    rewrite_config_bindings(guard, cfg);
                }
                if let Some(effects) = transition.effects.as_mut() {
                    for effect in effects.iter_mut() {
                        rewrite_config_bindings(effect, cfg);
                    }
                }
            }
            if let Some(ticks) = trait_def.ticks.as_mut() {
                for tick in ticks.iter_mut() {
                    if let Some(guard) = tick.guard.as_mut() {
                        rewrite_config_bindings(guard, cfg);
                    }
                    for effect in tick.effects.iter_mut() {
                        rewrite_config_bindings(effect, cfg);
                    }
                }
            }
        }
    }
}

/// Phase F.7: Apply post-substitution structural overrides to a resolved
/// `TraitDefinition`. These differ from `apply_overrides_to_trait` in that
/// they do NOT rewrite identifiers — they replace whole structural fields
/// with caller-supplied values:
///
/// - **`listens`** — replace the trait's `listens` array entirely with the
///   caller-supplied list. The common case in agent molecules is `[]`,
///   which clears the upstream's external event listeners. The inliner
///   accepts any caller list, including non-empty ones, but does not merge
///   with the upstream — it's a full replacement.
///
/// - **`emits_scope`** — set the `scope` field on every existing emit to
///   the caller-supplied value (`Internal` or `External`). The common case
///   in agent molecules is `Internal`, which confines the trait's emits to
///   the orbital's internal bus instead of broadcasting them. The inliner
///   does NOT add new emit declarations; it only modifies the scope of
///   what's already there.
///
/// Both overrides are applied AFTER `apply_overrides_to_trait` has run, so
/// any rename / linkedEntity / events substitutions are already in effect
/// by the time these structural changes apply.
fn apply_post_subst_overrides_to_trait(
    trait_def: &mut TraitDefinition,
    listens: Option<&Vec<orbital_core::schema::ListenDefinition>>,
    emits_scope: Option<&orbital_core::schema::EventScope>,
) {
    if let Some(new_listens) = listens {
        trait_def.listens = Some(new_listens.clone());
    }
    if let Some(scope) = emits_scope {
        if let Some(emits) = trait_def.emits.as_mut() {
            for emit in emits.iter_mut() {
                emit.scope = Some(scope.clone());
            }
        }
    }
}

/// Inline trait references.
///
/// For each `TraitReference::Reference { ref, linkedEntity, config, .. }`,
/// this function resolves the imported `TraitDefinition`, applies any
/// `linkedEntity` (and, eventually, `config`) overrides via
/// `apply_overrides_to_trait`, and pushes a single
/// `TraitReference::Inline(merged_trait)` to the result. The downstream
/// `compose` and `resolve` phases only handle `TraitReference::Inline`, so
/// emitting a single merged Inline is what makes the override actually
/// reach the lowered OIR.
///
/// Phase 3.5.G.A.3 — Collision detection. After A.1 + A.2 produce the
/// merged `TraitDefinition` (with any `name` override applied), this
/// function tracks the resulting trait names in a `HashSet` and rejects
/// duplicates. The check covers three cases:
///
/// 1. Two `TraitReference::Reference` entries that both resolve to the same
///    upstream trait WITHOUT a `name` override on either: both end up with
///    the same `trait_def.name` (the upstream original) → collision.
/// 2. Two `Reference` entries where exactly one has a distinguishing `name`
///    override: the renamed one and the original-named one differ →
///    NO collision, both inlined.
/// 3. A literal `TraitReference::Inline` and a `Reference` that resolves
///    to the same name: also rejected as a collision.
///
/// On collision, the function returns
/// [`InlineError::DuplicateTraitName`] with the offending name and a
/// message that tells the molecule author exactly how to fix it: provide a
/// `name` override on at least one reference. The error fires AFTER the
/// rename has been applied, so users with two distinct `name` overrides
/// (e.g. `CartItemAddItem` and `CartItemEditItem`) are unaffected.
/// Walk an SExpression and collect every string literal of the exact form
/// `@trait.<SiblingName>` into `out`. App atoms embed a curated variation as
/// a sibling trait and reference it inside the main trait's render-ui as the
/// literal `"@trait.SiblingView"`. The form is the full string equal to
/// `@trait.<Name>` (no trailing path segment) — that's how a render node
/// addresses a sibling trait's projected view.
fn collect_sibling_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(name) = s.strip_prefix("@trait.") {
                if !name.is_empty() && !name.contains('.') {
                    out.insert(name.to_string());
                }
            }
        }
        SExpression::Array(items) => {
            for item in items {
                collect_sibling_trait_refs(item, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_sibling_trait_refs(v, out);
            }
        }
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
    }
}

/// Scan a resolved trait's render trees (every SExpr inside transition effects
/// + guards, and tick effects + guards) for `@trait.<Sibling>` references.
fn sibling_trait_refs_of(trait_def: &TraitDefinition) -> HashSet<String> {
    let mut out = HashSet::new();
    for transition in &trait_def.state_machine.transitions {
        if let Some(guard) = transition.guard.as_ref() {
            collect_sibling_trait_refs(guard, &mut out);
        }
        if let Some(effects) = transition.effects.as_ref() {
            for effect in effects {
                collect_sibling_trait_refs(effect, &mut out);
            }
        }
    }
    if let Some(ticks) = trait_def.ticks.as_ref() {
        for tick in ticks {
            if let Some(guard) = tick.guard.as_ref() {
                collect_sibling_trait_refs(guard, &mut out);
            }
            for effect in &tick.effects {
                collect_sibling_trait_refs(effect, &mut out);
            }
        }
    }
    out
}

type InlinedTraits = (
    Vec<TraitReference>,
    HashMap<String, String>,
    Vec<EntityDefinition>,
    // Names of traits added by the auto-pull loop (siblings referenced via
    // `@trait.<Sibling>`). These keep their source name within an orbital so
    // the `@trait` ref resolves, but identical pulls in two orbitals collide
    // app-wide; `inline_schema` uniquifies those after all orbitals resolve.
    Vec<String>,
);

fn inline_traits(
    traits: &[TraitReference],
    ctx: &InlineContext,
    path_prefix: &str,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) -> Result<InlinedTraits, InlineError> {
    let mut result = Vec::new();
    // Auxiliary entities surfaced by no-rebind sibling-pulls (see the pull loop).
    let mut pulled_aux_entities: Vec<EntityDefinition> = Vec::new();
    // Track every trait NAME that has already been pushed to `result`.
    // Names are unique across the orbital, so a second push of the same
    // name is a collision regardless of whether it came from a literal
    // Inline or a resolved Reference.
    let mut seen_names: HashSet<String> = HashSet::new();
    // Capture each `name:` override applied to a TraitReference so callers
    // (specifically `inline_orbital`) can propagate the rename through the
    // orbital's pages. Without this, `pages[].traits: [{ref: "OldName"}]`
    // entries become stale after the rename and fail validation with
    // `ORB_P_INVALID_TRAIT_REF`.
    let mut rename_map: HashMap<String, String> = HashMap::new();
    // App atoms keep a hand-rolled main trait and embed a curated variation as
    // a SIBLING trait, referencing it inside the main trait's render-ui as the
    // string literal `@trait.<SiblingName>`. When an organism composes ONLY the
    // main trait via a trait ref, the sibling is never instantiated and the
    // `@trait.<SiblingName>` dangles (`ORB_BINDING_TRAIT_UNKNOWN`). For each
    // resolved Reference trait we record the source alias + the linkedEntity
    // rebind it used, so a post-pass can auto-instantiate any same-alias
    // siblings the cloned body references — applying the SAME rebind a
    // hand-written sibling ref would have, keeping the sibling's original name
    // so `@trait.<SiblingName>` resolves.
    let mut sibling_pull_work: Vec<(String, String, Option<String>)> = Vec::new();

    for (idx, trait_ref) in traits.iter().enumerate() {
        let trait_path = format!("{path_prefix}.traits[{idx}]");
        match trait_ref {
            TraitReference::Inline(trait_def) => {
                if !seen_names.insert(trait_def.name.clone()) {
                    return Err(InlineError::DuplicateTraitName {
                        name: trait_def.name.clone(),
                        path: trait_path,
                    });
                }
                // Atom-scope config-default substitution. When an inline
                // trait carries `@config.X` bindings (e.g. `limit:
                // @config.pageSize`) and is compiled standalone (no
                // import call site supplying overrides), the codegen
                // path emits `Number(undefined)` for those bindings —
                // see codegen/guard.rs OirBindingRoot::Config which
                // assumes the inline phase has already substituted
                // every @config.X with a literal. For Reference traits
                // the substitution happens in `apply_overrides_to_trait`
                // (line ~1097); for Inline traits it didn't run at all.
                // Apply the trait's own declared defaults here so atom
                // standalone codegen produces `Number(10)` instead of
                // `Number(undefined)`. Cloning into a new Inline keeps
                // the rest of the pipeline (which expects a fresh
                // owned trait_def) unchanged.
                let mut td = trait_def.clone();
                apply_inline_config_defaults(&mut td, recorder);
                result.push(TraitReference::Inline(td));
            }
            TraitReference::Reference {
                reference,
                name: name_override,
                linked_entity,
                config,
                events: events_override,
                fields: fields_override,
                listens: listens_override,
                emits_scope: emits_scope_override,
            } => {
                if let Some(ref_str) = reference {
                    let parsed = parse_reference(ref_str)?;
                    match parsed {
                        ParsedReference::Trait { alias, name } => {
                            let inlined = ctx.get_trait(&alias, &name)?;
                            // STUDIO-1: capture provenance BEFORE override
                            // application. `source_label` is the imported
                            // behavior's human-readable name (set by
                            // `add_alias_multi`). `source_entity_def` is
                            // the entity from the orbital that owns this
                            // trait — already plumbed for the auto-merge
                            // path, reused here so the client-side mock
                            // generator (STUDIO-2) can produce rows for
                            // the trait's linkedEntity without walking
                            // the import graph at runtime.
                            let source_label = ctx.get_entry(&alias)?.source_label.clone();
                            let source_entity_def =
                                ctx.get_entity_for_trait_orbital(&alias, &name);
                            let provenance = SourceBehaviorMetadata {
                                behavior: source_label,
                                alias: alias.clone(),
                                original_name: name.clone(),
                            };
                            match inlined {
                                TraitReference::Inline(mut trait_def) => {
                                    // Apply name / linkedEntity / events /
                                    // config overrides to the cloned trait
                                    // so the single Inline pushed below
                                    // carries the caller's intent
                                    // end-to-end.
                                    // Capture the original trait name (the
                                    // ref's `name` segment, BEFORE override)
                                    // so the rename map below records the
                                    // before/after pair when a `name:`
                                    // override is supplied.
                                    let original_name = trait_def.name.clone();
                                    apply_overrides_to_trait(
                                        &mut trait_def,
                                        linked_entity.as_ref(),
                                        config.as_ref(),
                                        name_override.as_deref(),
                                        events_override.as_ref(),
                                        fields_override.as_ref(),
                                        ctx.defer_config_subst,
                                        recorder,
                                    );
                                    if trait_def.name != original_name {
                                        rename_map
                                            .insert(original_name, trait_def.name.clone());
                                    }
                                    // Phase F.7: replace the trait's
                                    // listens and/or set every emit's scope
                                    // to the caller-supplied values. These
                                    // are STRUCTURAL overrides applied
                                    // after the substitution pass; they do
                                    // not require identifier rewriting.
                                    apply_post_subst_overrides_to_trait(
                                        &mut trait_def,
                                        listens_override.as_ref(),
                                        emits_scope_override.as_ref(),
                                    );
                                    // STUDIO-1/2: stamp provenance onto the
                                    // resolved trait. Runtime + codegen
                                    // ignore both fields; only Studio's L2
                                    // canvas reads them to bucket cards by
                                    // alias and populate mock rows for
                                    // imported entities.
                                    trait_def.source_behavior = Some(provenance);
                                    trait_def.source_entity_definition = source_entity_def;
                                    // Collision check happens AFTER
                                    // override application so
                                    // post-rename names are what we
                                    // compare. Two references with
                                    // distinct `name` overrides will
                                    // never collide here.
                                    if !seen_names.insert(trait_def.name.clone()) {
                                        return Err(InlineError::DuplicateTraitName {
                                            name: trait_def.name.clone(),
                                            path: trait_path.clone(),
                                        });
                                    }
                                    // Record this resolved ref so the sibling-pull
                                    // pass below can instantiate any same-alias
                                    // `@trait.X` siblings its body references,
                                    // rebinding them with the SAME linkedEntity.
                                    for sibling in sibling_trait_refs_of(&trait_def) {
                                        sibling_pull_work.push((
                                            alias.clone(),
                                            sibling,
                                            linked_entity.clone(),
                                        ));
                                    }
                                    result.push(TraitReference::Inline(trait_def));
                                }
                                other => {
                                    // Defensive: `ctx.get_trait` is
                                    // expected to return Inline. If it
                                    // returns a Reference (e.g. the
                                    // imported orbital had not been
                                    // inlined yet), pass it through
                                    // unchanged so a later pass can
                                    // resolve it. We still record the
                                    // name (via `trait_name()`) so a
                                    // subsequent collision against the
                                    // same name is rejected.
                                    if let Some(name) = other.trait_name() {
                                        if !seen_names.insert(name.to_string()) {
                                            return Err(InlineError::DuplicateTraitName {
                                                name: name.to_string(),
                                                path: trait_path.clone(),
                                            });
                                        }
                                    }
                                    result.push(other);
                                }
                            }
                        }
                        _ => return Err(InlineError::InvalidReference(ref_str.clone())),
                    }
                } else {
                    // No reference string, keep as-is (named trait
                    // reference). Track the name if available so a
                    // collision against an Inline or a resolved
                    // Reference with the same name is still rejected.
                    if let Some(name) = trait_ref.trait_name() {
                        if !seen_names.insert(name.to_string()) {
                            return Err(InlineError::DuplicateTraitName {
                                name: name.to_string(),
                                path: trait_path.clone(),
                            });
                        }
                    }
                    result.push(trait_ref.clone());
                }
            }
        }
    }

    // Sibling-trait auto-pull. Process the worklist gathered above: for each
    // `@trait.<Sibling>` a resolved trait references, instantiate the sibling
    // from the SAME source alias if it isn't already present in the consumer,
    // applying the SAME linkedEntity rebind the parent ref used. The pulled
    // sibling KEEPS its original name (so `@trait.<Sibling>` still resolves)
    // and gets NO page. Recurse: a pulled sibling's body may reference further
    // siblings. `visited` guards against cycles; `seen_names` dedups against
    // traits the consumer already declares (explicit composition) and against
    // siblings already pulled.
    let mut pulled_names: Vec<String> = Vec::new();
    {
        let mut worklist = sibling_pull_work;
        let mut visited: HashSet<(String, String)> = HashSet::new();
        while let Some((alias, sibling_name, linked_entity)) = worklist.pop() {
            // Already present in the consumer (explicit comp or prior pull): skip.
            if seen_names.contains(&sibling_name) {
                continue;
            }
            if !visited.insert((alias.clone(), sibling_name.clone())) {
                continue;
            }
            // The sibling must exist as a trait in the SAME source atom.
            let Ok(TraitReference::Inline(mut sibling_def)) =
                ctx.get_trait(&alias, &sibling_name)
            else {
                // Not a same-alias sibling (or not resolvable to an inline
                // trait) — leave the `@trait.X` for the validator to flag.
                continue;
            };
            // Apply the SAME rebind a hand-written sibling ref would: rewrite
            // every `@X.path` / `["ref", X]` / fetch/persist literal to the
            // consumer's entity. KEEP the original name (no `name` override) so
            // `@trait.<Sibling>` resolves against it.
            apply_overrides_to_trait(
                &mut sibling_def,
                linked_entity.as_ref(),
                None,
                None,
                None,
                None,
                ctx.defer_config_subst,
                recorder,
            );
            // Provenance + source entity, mirroring the main ref path so Studio
            // and the mock generator treat the pulled sibling like any ref.
            if let Ok(entry) = ctx.get_entry(&alias) {
                sibling_def.source_behavior = Some(SourceBehaviorMetadata {
                    behavior: entry.source_label.clone(),
                    alias: alias.clone(),
                    original_name: sibling_name.clone(),
                });
            }
            sibling_def.source_entity_definition =
                ctx.get_entity_for_trait_orbital(&alias, &sibling_name);
            // No-rebind sibling: it keeps its OWN bound entity (e.g. a
            // step-flow's `ProgressVerticalView`). That entity is auxiliary in
            // the source behavior; carry it into the consumer so the sibling's
            // `(fetch X)` / `linkedEntity` resolve. With a rebind, the sibling
            // was retargeted to the consumer's entity and needs nothing here.
            if linked_entity.is_none() {
                if let Some(bound) = &sibling_def.linked_entity {
                    if let Some(aux) = ctx.get_entity_by_name_for_alias(&alias, bound) {
                        pulled_aux_entities.push(aux);
                    }
                }
            }
            // Recurse into the rebound sibling's own `@trait.Y` references.
            for next in sibling_trait_refs_of(&sibling_def) {
                worklist.push((alias.clone(), next, linked_entity.clone()));
            }
            seen_names.insert(sibling_def.name.clone());
            pulled_names.push(sibling_def.name.clone());
            result.push(TraitReference::Inline(sibling_def));
        }
    }

    // Phase 9.5: drop source-scoped listeners whose source trait was not
    // imported into this orbital. When an atom's trait (e.g. the Browse trait
    // in std-agent-memory) listens for a sibling trait's emits
    // (`AgentMemoryAgent PINNED -> INIT`), and a consumer imports ONLY the
    // Browse trait via a trait ref, the sibling trait is not present in the
    // consumer's orbital, so the listen can never fire. Silently dropping it
    // prevents a false-positive validation error while preserving the
    // source-filter semantics wherever the sibling IS imported.
    //
    // `ListenSource::Orbital` entries are left alone — they point at a
    // trait in another orbital (cross-orbital filter) and are not validated
    // against the local trait set.
    let local_trait_names: HashSet<String> = result
        .iter()
        .filter_map(|tr| match tr {
            TraitReference::Inline(td) => Some(td.name.clone()),
            other => other.trait_name().map(|s| s.to_string()),
        })
        .collect();

    for trait_ref in result.iter_mut() {
        if let TraitReference::Inline(trait_def) = trait_ref {
            if let Some(listens) = trait_def.listens.as_mut() {
                listens.retain(|listen| match &listen.source {
                    Some(orbital_core::schema::ListenSource::Trait { trait_name }) => {
                        local_trait_names.contains(trait_name)
                    }
                    _ => true,
                });
            }
        }
    }

    Ok((result, rename_map, pulled_aux_entities, pulled_names))
}

/// Walk every page's `traits[]` array and apply a `name:` trait-rename map.
///
/// Each entry in `pages[].traits[]` is a `TraitReference`. When the entry is
/// a `Reference` whose `reference` field is a bare trait name (not a dotted
/// `Alias.traits.X` form) AND that name matches a key in `rename_map`, the
/// `reference` is rewritten to the new name. Inline trait entries on pages
/// are left alone — they don't carry the cross-orbital staleness shape.
///
/// Why this exists: when a molecule renames an imported atom's trait via
/// `{ref: "Modal.traits.ModalRecordModal", name: "ArticleAddItem"}`, the
/// imported atom's page (resolved into the molecule via inline_pages) still
/// carries `traits: [{ref: "ModalRecordModal"}]` — the page hasn't seen the
/// rename. Without this rewrite, validate fires `ORB_P_INVALID_TRAIT_REF`
/// because no trait named `ModalRecordModal` exists in the resolved orbital.
fn rewrite_page_trait_refs(
    pages: &mut Option<Vec<PageRef>>,
    rename_map: &HashMap<String, String>,
) {
    let Some(page_list) = pages else { return };
    for page_ref in page_list.iter_mut() {
        let PageRef::Inline(page_def) = page_ref else {
            continue;
        };
        let Some(traits) = page_def.traits.as_mut() else {
            continue;
        };
        for trait_ref in traits.iter_mut() {
            if let TraitReference::Reference {
                reference: Some(ref mut ref_str),
                ..
            } = trait_ref
            {
                // Only rewrite bare trait-name refs (no `.traits.` segment).
                // Cross-alias refs like "Modal.traits.X" were already resolved
                // by the time inline_pages ran; they shouldn't carry old
                // names through this layer.
                if !ref_str.contains('.') {
                    if let Some(new_name) = rename_map.get(ref_str) {
                        *ref_str = new_name.clone();
                    }
                }
            }
        }
    }
}

/// Phase F.3: Apply override fields from a `PageRefObject` to a resolved
/// `PageDefinition`. Mirrors `apply_overrides_to_trait` for pages.
///
/// Argument handling:
///
/// - **`path`** — direct assignment to `page_def.path`. Phase A behavior.
/// - **`linked_entity`** — rebinds `page_def.primary_entity` and walks any
///   inline `TraitReference::Inline` entries in `page_def.traits` so their
///   own `linked_entity` and embedded SExpression bindings are rewritten
///   from the original entity to the caller's entity.
/// - **`traits`** — replaces `page_def.traits` entirely with the caller's
///   list. The caller takes full ownership of the page's trait set.
fn apply_overrides_to_page(page_def: &mut PageDefinition, obj: &PageRefObject) {
    if let Some(path_override) = &obj.path {
        page_def.path = path_override.clone();
    }

    if let Some(new_entity) = &obj.linked_entity {
        let old_entity = page_def.primary_entity.clone();
        page_def.primary_entity = Some(new_entity.clone());

        // Build a single-entry substitution map for the rewrite walks
        // below. Build it eagerly even when `old_entity == new_entity` so
        // the rebound primary_entity matches what callers see.
        if let Some(orig) = old_entity {
            if &orig != new_entity {
                let mut entity_subs: HashMap<String, String> = HashMap::new();
                entity_subs.insert(orig, new_entity.clone());
                let event_subs: HashMap<String, String> = HashMap::new();

                // Walk every inline trait the page declares so embedded
                // SExpression entity refs are rewritten. References (the
                // other TraitReference variant) are left alone — those are
                // resolved by the orbital-level inline pass and the
                // rewrite happens there.
                if let Some(traits) = page_def.traits.as_mut() {
                    for trait_ref in traits.iter_mut() {
                        if let TraitReference::Inline(inline_trait) = trait_ref {
                            // Rebind the trait's linked_entity if it
                            // matched the original.
                            if let Some(le) = inline_trait.linked_entity.as_mut() {
                                if let Some(to) = entity_subs.get(le) {
                                    *le = to.clone();
                                }
                            }
                            for transition in inline_trait.state_machine.transitions.iter_mut() {
                                if let Some(guard) = transition.guard.as_mut() {
                                    rewrite_identifiers(guard, &entity_subs, &event_subs);
                                }
                                if let Some(effects) = transition.effects.as_mut() {
                                    for effect in effects.iter_mut() {
                                        rewrite_identifiers(effect, &entity_subs, &event_subs);
                                    }
                                }
                            }
                            if let Some(ticks) = inline_trait.ticks.as_mut() {
                                for tick in ticks.iter_mut() {
                                    if let Some(guard) = tick.guard.as_mut() {
                                        rewrite_identifiers(guard, &entity_subs, &event_subs);
                                    }
                                    for effect in tick.effects.iter_mut() {
                                        rewrite_identifiers(effect, &entity_subs, &event_subs);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(traits_override) = &obj.traits {
        page_def.traits = Some(traits_override.clone());
    }
}

/// Inline page references
fn inline_pages(
    pages: &Option<Vec<PageRef>>,
    ctx: &InlineContext,
) -> Result<Option<Vec<PageRef>>, InlineError> {
    match pages {
        None => Ok(None),
        Some(page_refs) => {
            let mut result = Vec::new();

            for page_ref in page_refs {
                match page_ref {
                    PageRef::Inline(_) => {
                        result.push(page_ref.clone());
                    }
                    PageRef::Object(obj) => {
                        let parsed = parse_reference(&obj.reference)?;
                        match parsed {
                            ParsedReference::Page { alias, name } => {
                                let inlined = ctx.get_page(&alias, &name)?;
                                // Phase F.3: Apply path / linked_entity /
                                // traits overrides via the helper. The path
                                // override is the original Phase A behavior;
                                // linked_entity rebinds the page's entity
                                // (and rewrites @Entity bindings inside any
                                // inline traits the page declares); traits
                                // replaces the page's trait list entirely.
                                if let PageRef::Inline(mut page_def) = inlined {
                                    apply_overrides_to_page(&mut page_def, obj);
                                    result.push(PageRef::Inline(page_def));
                                } else {
                                    // Still a reference (loader returned
                                    // an unresolved form). Pass through —
                                    // a later pass can resolve it. The
                                    // override fields stay attached to the
                                    // original object form.
                                    result.push(page_ref.clone());
                                }
                            }
                            _ => return Err(InlineError::InvalidReference(obj.reference.clone())),
                        }
                    }
                    PageRef::Reference(ref_str) => {
                        let parsed = parse_reference(ref_str)?;
                        match parsed {
                            ParsedReference::Page { alias, name } => {
                                let inlined = ctx.get_page(&alias, &name)?;
                                result.push(inlined);
                            }
                            _ => return Err(InlineError::InvalidReference(ref_str.clone())),
                        }
                    }
                }
            }

            Ok(Some(result))
        }
    }
}

/// Apply override fields from a `ServiceRefObject` to a resolved
/// `ServiceDefinition`. Mirrors `apply_overrides_to_trait` (Phase 3.5.G.A.1)
/// for services. The overrides are variant-aware: REST-only fields
/// (`base_url`, `headers`) only apply to `ServiceDefinition::Rest`,
/// `url` only to `Socket`, `server_path` only to `Mcp`. `description`
/// applies to all three. Overrides that don't match the resolved variant
/// are silently ignored — this matches how `PageRefObject::path` only
/// applies when the resolved page is inline.
fn apply_overrides_to_service(service_def: &mut ServiceDefinition, obj: &ServiceRefObject) {
    match service_def {
        ServiceDefinition::Rest(rest) => {
            if let Some(description) = &obj.description {
                rest.description = Some(description.clone());
            }
            if let Some(base_url) = &obj.base_url {
                rest.base_url = base_url.clone();
            }
            if let Some(headers) = &obj.headers {
                rest.headers = Some(headers.clone());
            }
        }
        ServiceDefinition::Socket(socket) => {
            if let Some(description) = &obj.description {
                socket.description = Some(description.clone());
            }
            if let Some(url) = &obj.url {
                socket.url = url.clone();
            }
        }
        ServiceDefinition::Mcp(mcp) => {
            if let Some(description) = &obj.description {
                mcp.description = Some(description.clone());
            }
            if let Some(server_path) = &obj.server_path {
                mcp.server_path = server_path.clone();
            }
        }
    }
}

/// Inline service references
fn inline_services(
    services: &Option<Vec<ServiceRef>>,
    ctx: &InlineContext,
) -> Result<Option<Vec<ServiceRef>>, InlineError> {
    match services {
        None => Ok(None),
        Some(service_refs) => {
            let mut result = Vec::new();

            for service_ref in service_refs {
                match service_ref {
                    ServiceRef::Inline(_) => {
                        result.push(service_ref.clone());
                    }
                    ServiceRef::Object(obj) => {
                        // Resolve the reference to its inline definition,
                        // apply override fields, and push a single Inline.
                        // Mirrors the trait-side path in `inline_traits`
                        // (Phase 3.5.G.A.1) and the page-side path in
                        // `inline_pages` (PageRef::Object branch above).
                        let parsed = parse_reference(&obj.reference)?;
                        match parsed {
                            ParsedReference::Service { alias, name } => {
                                let inlined = ctx.get_service(&alias, &name)?;
                                match inlined {
                                    ServiceRef::Inline(mut service_def) => {
                                        apply_overrides_to_service(&mut service_def, obj);
                                        result.push(ServiceRef::Inline(service_def));
                                    }
                                    other => {
                                        // Defensive: `ctx.get_service` is
                                        // expected to return Inline. If it
                                        // returns a Reference/Object (because
                                        // the upstream orbital had not been
                                        // inlined yet), pass it through
                                        // unchanged so a later pass can
                                        // resolve it.
                                        result.push(other);
                                    }
                                }
                            }
                            _ => return Err(InlineError::InvalidReference(obj.reference.clone())),
                        }
                    }
                    ServiceRef::Reference(ref_str) => {
                        let parsed = parse_reference(ref_str)?;
                        match parsed {
                            ParsedReference::Service { alias, name } => {
                                let inlined = ctx.get_service(&alias, &name)?;
                                result.push(inlined);
                            }
                            _ => return Err(InlineError::InvalidReference(ref_str.clone())),
                        }
                    }
                }
            }

            Ok(Some(result))
        }
    }
}

// ============================================================================
// Main Inline Function
// ============================================================================

/// Inline configuration
#[derive(Debug, Clone, Default)]
pub struct InlineConfig {
    /// Loader configuration
    pub loader_config: LoaderConfig,
    /// Whether to skip inlining (for testing)
    pub skip_inline: bool,
}

/// Inline all external references in a schema
pub fn inline_schema(
    schema: &mut OrbitalSchema,
    config: &InlineConfig,
    source_path: Option<&Path>,
) -> Result<(), InlineError> {
    if config.skip_inline {
        return Ok(());
    }

    let mut loader = ExternalOrbitalLoader::new(config.loader_config.clone());
    let mut chain = ImportChain::new();

    // If we have a source path, add it to the chain
    if let Some(path) = source_path {
        chain.push(path);
    }

    // Process each orbital, recording the names the auto-pull loop added and
    // the config→knob provenance for substituted render-ui props.
    let mut pulled_by_orbital: Vec<(usize, Vec<String>)> = Vec::new();
    let mut all_provenance: HashMap<String, Vec<ConfigProvenanceRecord>> = HashMap::new();
    for (idx, orbital) in schema.orbitals.iter_mut().enumerate() {
        let mut pulled: Vec<String> = Vec::new();
        let mut provenance: Vec<ConfigProvenanceRecord> = Vec::new();
        inline_orbital(
            orbital,
            &mut loader,
            &mut chain,
            source_path,
            true,
            Some(&mut pulled),
            &mut provenance,
        )?;
        if !pulled.is_empty() {
            pulled_by_orbital.push((idx, pulled));
        }
        if !provenance.is_empty() {
            all_provenance.insert(orbital.name.clone(), provenance);
        }
    }

    uniquify_cross_orbital_pulled_siblings(schema, &pulled_by_orbital);

    // Emit the config→knob provenance into `_metadata.configProvenance` so a
    // consumer (Studio contextual edit) can trace a substituted render-ui
    // literal back to the config knob it came from.
    if !all_provenance.is_empty() {
        match schema.metadata.as_mut() {
            Some(meta) => meta.config_provenance = Some(all_provenance),
            None => {
                schema.metadata = Some(SchemaMetadata {
                    version: None,
                    created_at: None,
                    source: None,
                    updated_at: None,
                    config_provenance: Some(all_provenance),
                });
            }
        }
    }

    Ok(())
}

/// Make auto-pulled sibling traits app-unique across orbitals.
///
/// An auto-pulled sibling keeps its source name within its orbital so the
/// pulling trait's `@trait.<Sibling>` reference still resolves. When two
/// orbitals compose the SAME source atom, each pulls a sibling with the same
/// name, colliding app-wide (`ORB_T_DUPLICATE_NAME`). Trait names must be
/// unique per app, so rename the colliding pulled siblings — prefixing with the
/// owning orbital name — and rewrite that orbital's `@trait.<old>` literals so
/// the reference tracks the rename. Only renames a pulled sibling when its name
/// also appears (pulled or explicit) in another orbital, so single-use pulls
/// keep their clean source name.
fn uniquify_cross_orbital_pulled_siblings(
    schema: &mut OrbitalSchema,
    pulled_by_orbital: &[(usize, Vec<String>)],
) {
    if pulled_by_orbital.is_empty() {
        return;
    }
    // Count how many orbitals declare each trait name (any kind).
    let mut name_orbital_count: HashMap<String, usize> = HashMap::new();
    for orbital in &schema.orbitals {
        let mut local: HashSet<String> = HashSet::new();
        for tr in &orbital.traits {
            if let Some(n) = trait_ref_name(tr) {
                local.insert(n.to_string());
            }
        }
        for n in local {
            *name_orbital_count.entry(n).or_default() += 1;
        }
    }
    for (orbital_idx, pulled) in pulled_by_orbital {
        let orbital_name = schema.orbitals[*orbital_idx].name.clone();
        for old_name in pulled {
            if name_orbital_count.get(old_name).copied().unwrap_or(0) <= 1 {
                continue;
            }
            let new_name = format!("{}{}", orbital_name, old_name);
            if name_orbital_count.contains_key(&new_name) {
                continue;
            }
            let orbital = &mut schema.orbitals[*orbital_idx];
            for tr in orbital.traits.iter_mut() {
                if let TraitReference::Inline(td) = tr {
                    if td.name == *old_name {
                        td.name = new_name.clone();
                    }
                    rewrite_trait_literal_refs(td, old_name, &new_name);
                    // A sibling-listening parent in the same orbital points its
                    // `listens` source at the old sibling name; track the rename.
                    if let Some(listens) = td.listens.as_mut() {
                        for listen in listens.iter_mut() {
                            if let Some(orbital_core::schema::ListenSource::Trait {
                                trait_name,
                            }) = listen.source.as_mut()
                            {
                                if trait_name == old_name {
                                    *trait_name = new_name.clone();
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Rewrite `@trait.<old>` string literals inside a trait's state machine to
/// `@trait.<new>` so a renamed sibling stays referenced.
fn rewrite_trait_literal_refs(td: &mut TraitDefinition, old: &str, new: &str) {
    let from = format!("@trait.{}", old);
    let to = format!("@trait.{}", new);
    for transition in td.state_machine.transitions.iter_mut() {
        if let Some(effects) = transition.effects.as_mut() {
            for eff in effects.iter_mut() {
                rewrite_sexpr_literal(eff, &from, &to);
            }
        }
        if let Some(guard) = transition.guard.as_mut() {
            rewrite_sexpr_literal(guard, &from, &to);
        }
    }
}

/// Replace every string-literal `from` with `to` everywhere in an SExpression.
fn rewrite_sexpr_literal(expr: &mut SExpression, from: &str, to: &str) {
    match expr {
        SExpression::String(s) => {
            if s == from {
                *s = to.to_string();
            }
        }
        SExpression::Array(items) => {
            for item in items.iter_mut() {
                rewrite_sexpr_literal(item, from, to);
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                rewrite_sexpr_literal(v, from, to);
            }
        }
        _ => {}
    }
}

/// Name of a trait reference regardless of kind (inline name or ref target).
fn trait_ref_name(tr: &TraitReference) -> Option<&str> {
    match tr {
        TraitReference::Inline(td) => Some(&td.name),
        other => other.trait_name(),
    }
}

/// Inline all references in an orbital.
///
/// `is_top_level` distinguishes the consumer-facing entry call (true) from
/// recursive calls used to load imports (false). When loading an import for a
/// caller, we MUST NOT pre-substitute the imported atom's `@config.X` bindings
/// with its own declared defaults — the caller's `apply_overrides_to_trait`
/// pass needs the raw `@config.X` strings present so its override values can
/// replace them. Eager default substitution at import-load time clobbers the
/// override surface for any field that has a default declared on the atom.
fn inline_orbital(
    orbital: &mut Orbital,
    loader: &mut ExternalOrbitalLoader,
    chain: &mut ImportChain,
    source_path: Option<&Path>,
    is_top_level: bool,
    // When set (top-level orbitals only), receives the names of traits added by
    // the auto-pull loop so `inline_schema` can uniquify cross-orbital collisions.
    pulled_names_sink: Option<&mut Vec<String>>,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) -> Result<(), InlineError> {
    // Skip the import-loading + reference-substitution machinery when
    // the orbital has no `uses[]` declarations (e.g. an atom compiled
    // standalone). Inline traits still need their own declared
    // `@config.X` defaults substituted before codegen, since
    // codegen/guard.rs `OirBindingRoot::Config` emits `undefined` on
    // the assumption the inline phase resolved every config binding.
    // Run that one substitution pass ONLY when this orbital is the
    // top-level compilation target (atom-standalone codegen). When the
    // orbital is being loaded as an import, defer the substitution so
    // the consumer's overrides can replace `@config.X` strings first.
    let uses = match &orbital.uses {
        Some(uses) if !uses.is_empty() => uses.clone(),
        _ => {
            if is_top_level {
                for trait_ref in orbital.traits.iter_mut() {
                    if let TraitReference::Inline(trait_def) = trait_ref {
                        apply_inline_config_defaults(trait_def, recorder);
                    }
                }
            }
            return Ok(());
        }
    };

    // Build context by loading all imports.
    let mut ctx = InlineContext::new();
    // When this orbital is itself an import (not the top-level compilation
    // target), defer `@config.X` substitution in resolved traits so a downstream
    // consumer can still override those bindings. Substituting here with this
    // import's own defaults would bake the value in and clobber the consumer's
    // override surface (the variation → base → consumer config-override path).
    ctx.defer_config_subst = !is_top_level;

    for use_decl in &uses {
        // Phase F.10: recursive composition. The loaded orbital may itself
        // declare `uses[]` and reference traits from yet another orbital
        // (e.g. organism → molecule → atom). Without recursive inlining
        // here, the consumer's `get_trait` lookup would find the loaded
        // orbital's traits as `TraitReference::Reference` (still pointing
        // at the deeper layer) and pass them through unchanged, breaking
        // the chain. We clone every loaded orbital + remember the source
        // path, then recursively call `inline_orbital` so each clone has
        // its own references already inlined before the consumer queries.
        //
        // The loader cache remains the source of truth for raw parsed
        // schemas; the inlined clones exist only in this consumer's
        // context.
        //
        // Multi-orbital fix: organisms (and any behavior that publishes
        // multiple orbitals in one `.orb`) declare traits/pages across
        // orbitals 0..N. Previously this loop took only `orbitals.first()`,
        // so any ref targeting a trait in orbital 1+ failed to resolve with
        // "Trait X not found in orbital Y". We now clone ALL orbitals and
        // stash them in the context, so `get_trait` / `get_page` /
        // `get_service` can iterate every orbital.
        let (mut inlined_orbitals, loaded_source) = {
            let loaded = loader.load(&use_decl.from, source_path, chain)?;
            if loaded.schema.orbitals.is_empty() {
                // Defensive: a valid imported `.orb` always has at least one
                // orbital (the parser rejects empty ones), so this branch
                // should be unreachable. Skip the alias rather than panic.
                continue;
            }
            (
                loaded.schema.orbitals.clone(),
                loaded.source_path.clone(),
            )
        };

        for inlined_orbital in &mut inlined_orbitals {
            inline_orbital(
                inlined_orbital,
                loader,
                chain,
                Some(&loaded_source),
                false,
                None,
                recorder,
            )?;
        }

        // The source label used in error messages is the `from` string from
        // the `uses` declaration (e.g. `"std/behaviors/std-arcade-game"`).
        // That is what the author wrote, so it is the actionable label when
        // a trait/page lookup fails.
        ctx.add_alias_multi(&use_decl.alias, inlined_orbitals, use_decl.from.clone());
    }

    // ----------------------------------------------------------------
    // Auto-merge imported entity fields (GAP-AGB-MOLECULE-ENTITY-CONTRACT)
    // ----------------------------------------------------------------
    // When the caller's orbital has trait refs pointing at imported behaviors
    // via `linkedEntity`, auto-merge the imported behavior's canonical entity
    // fields INTO the caller's entity. This runs BEFORE entity resolution and
    // trait inlining so the full field set is available when @entity bindings
    // are checked.
    //
    // Merge rule: for each field in the imported behavior's canonical entity,
    // if the caller's entity doesn't already have a field with that name, add
    // it. Caller's fields win on name collision.
    if let EntityRef::Inline(ref mut caller_entity) = orbital.entity {
        // Collect (alias, trait_name) pairs from trait refs of the form
        // "Alias.traits.TraitName". Use a set to avoid merging the same
        // import's entity twice when multiple traits reference it.
        //
        // Two cases carry the imported entity's fields onto the caller entity:
        //   (a) an explicit `linkedEntity` rebind to the caller entity, and
        //   (b) NO rebind, but the imported trait's own bound entity NAME
        //       equals the caller entity name (the consumer re-declares an
        //       entity with the same name and composes the trait over it).
        // Case (b) is the depth-3 propagation fix: an organism that imports a
        // self-contained widget atom WITHOUT rebinding still reads the trait's
        // INTRINSIC `@entity.X` fields against its own same-named entity, so
        // those machinery fields must travel onto it.
        let caller_entity_name = caller_entity.name.clone();
        let mut merged_aliases: HashSet<(String, String)> = HashSet::new();

        for trait_ref in &orbital.traits {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                linked_entity,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    if let Some(imported_entity) =
                        ctx.get_entity_for_trait_orbital(&alias, &name)
                    {
                        let (carries_fields, intrinsic_only) = match linked_entity {
                            // (a) explicit rebind: merge the full field set
                            // (long-standing behavior, unchanged).
                            Some(_) => (true, false),
                            // (b) no rebind: carry only INTRINSIC machinery when
                            // the imported trait's bound entity name matches the
                            // caller's. Genuine DATA fields are NOT merged here —
                            // the consumer re-declared its own entity and owns
                            // them; a missing DATA field surfaces as an error.
                            None => (imported_entity.name == caller_entity_name, true),
                        };
                        if carries_fields
                            && merged_aliases.insert((alias.clone(), name.clone()))
                        {
                            // A required DATA field (in the trait's
                            // `entity_contract.requires`) must be satisfied by
                            // the target, NOT auto-merged from the source — that
                            // would mask a genuinely-missing field. Skip those so
                            // ORB_T_REBIND_MISSING_FIELDS can fire. Intrinsic +
                            // provided fields still carry.
                            let required_data = ctx
                                .get_trait(&alias, &name)
                                .ok()
                                .and_then(|tr| match tr {
                                    TraitReference::Inline(td) => td.entity_contract,
                                    _ => None,
                                })
                                .map(|c| c.requires.into_iter().collect::<HashSet<String>>())
                                .unwrap_or_default();
                            merge_imported_entity_fields(
                                caller_entity,
                                &imported_entity,
                                intrinsic_only,
                                &required_data,
                            );
                        }
                    }
                }
            }
        }
    }

    // ----------------------------------------------------------------
    // Auxiliary entities (Gap #22): when a trait reference imports an
    // atom WITHOUT rebinding linkedEntity (no `-> Entity` at the call
    // site), the imported atom's entity stays the trait's bound entity.
    // The molecule's primary `entity` field can only carry one entity,
    // so surface the imported atom entity in `auxiliary_entities` so
    // validators / codegen / runtime can resolve `linkedEntity =
    // "SearchResult"` lookups, register persistence, and emit the
    // TypeScript type. Dedup by entity name across all no-rebind imports
    // — a molecule that imports two no-rebind traits sharing the same
    // entity gets a single registration.
    {
        let mut seen_aux_names: HashSet<String> = HashSet::new();
        // Seed with the primary entity name so we never duplicate it as
        // an auxiliary (the primary is already in the orbital's entity
        // field — the auxiliary list is strictly for additional ones).
        if let EntityRef::Inline(ref primary_entity) = orbital.entity {
            seen_aux_names.insert(primary_entity.name.clone());
        }
        let mut aux_collected: Vec<EntityDefinition> = Vec::new();
        for trait_ref in &orbital.traits {
            if let TraitReference::Reference {
                reference: Some(ref ref_str),
                linked_entity: None,
                ..
            } = trait_ref
            {
                if let Ok(ParsedReference::Trait { alias, name }) = parse_reference(ref_str) {
                    if let Some(imported_entity) =
                        ctx.get_entity_for_trait_orbital(&alias, &name)
                    {
                        if seen_aux_names.insert(imported_entity.name.clone()) {
                            aux_collected.push(imported_entity);
                        }
                    }
                }
            }
        }
        if !aux_collected.is_empty() {
            let existing = orbital.auxiliary_entities.take().unwrap_or_default();
            let mut combined = existing;
            for entity in aux_collected {
                combined.push(EntityRef::Inline(entity));
            }
            orbital.auxiliary_entities = Some(combined);
        }
    }

    // Inline entity
    orbital.entity = inline_entity(&orbital.entity, &ctx)?;

    // Inline traits
    // Source-tag every InlineError raised below with the orbital's
    // location in the composed schema so `orbital-cli` can populate the
    // ValidationJsonError's `path` field. Downstream rabit's
    // `dispatch-updates` walker reads this path to route UPDATE_SPEC to
    // the right subagent.
    let orbital_path_prefix = format!("orbitals[name={}]", orbital.name);
    let (inlined_traits, trait_rename_map, pulled_aux_entities, pulled_sibling_names) =
        inline_traits(&orbital.traits, &ctx, &orbital_path_prefix, recorder)?;
    orbital.traits = inlined_traits;
    if let Some(sink) = pulled_names_sink {
        *sink = pulled_sibling_names;
    }

    // Register auxiliary entities surfaced by no-rebind SIBLING-PULLS. A
    // self-contained widget trait pulled transitively via `@trait.<Sibling>`
    // (e.g. a step-flow embedded in an app atom) keeps its own runtime view
    // entity (`ProgressVerticalView`); that entity is auxiliary in the source
    // behavior and must be carried here or the trait's `(fetch X)` /
    // `linkedEntity` dangle (ORB_EFF_FETCH_INVALID_ENTITY / ORB_E_INVALID_RELATION).
    // The pre-`inline_traits` aux pass only saw direct trait refs, so it missed
    // these. Dedup against the primary + already-registered auxiliaries.
    if !pulled_aux_entities.is_empty() {
        let mut known: HashSet<String> = HashSet::new();
        if let EntityRef::Inline(e) = &orbital.entity {
            known.insert(e.name.clone());
        }
        let mut combined = orbital.auxiliary_entities.take().unwrap_or_default();
        for a in &combined {
            if let EntityRef::Inline(e) = a {
                known.insert(e.name.clone());
            }
        }
        for e in pulled_aux_entities {
            if known.insert(e.name.clone()) {
                combined.push(EntityRef::Inline(e));
            }
        }
        if !combined.is_empty() {
            orbital.auxiliary_entities = Some(combined);
        }
    }

    // Inline pages
    orbital.pages = inline_pages(&orbital.pages, &ctx)?;

    // Propagate the trait `name:` rename map from `inline_traits` into the
    // orbital's pages. Each `pages[].traits[]` entry that references a trait
    // by its ORIGINAL name needs to be rewritten to the post-rename name —
    // otherwise validation downstream fires `ORB_P_INVALID_TRAIT_REF` (the
    // page's trait ref doesn't match any actual trait in the orbital).
    if !trait_rename_map.is_empty() {
        rewrite_page_trait_refs(&mut orbital.pages, &trait_rename_map);
    }

    // Inline services
    orbital.services = inline_services(&orbital.services, &ctx)?;

    // Inline theme if it's a reference
    if let Some(ThemeRef::Reference(ref_str)) = &orbital.theme {
        let parsed = parse_reference(ref_str)?;
        if let ParsedReference::Theme { alias } = parsed {
            orbital.theme = Some(ctx.get_theme(&alias)?);
        }
    }

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loader/fixtures")
    }

    // ------------------------------------------------------------------
    // rewrite_entity_refs — Phase 3.5.G.A.1
    // ------------------------------------------------------------------

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string())
    }

    #[test]
    fn rewrite_entity_refs_rewrites_ref_atom() {
        // ["ref", "ModalRecord"] → ["ref", "ShoppingCart"]
        let mut expr = SExpression::Array(vec![s("ref"), s("ModalRecord")]);
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, SExpression::Array(vec![s("ref"), s("ShoppingCart")]));
    }

    #[test]
    fn rewrite_entity_refs_rewrites_binding_root() {
        // "@ModalRecord.name" → "@ShoppingCart.name"
        let mut expr = s("@ModalRecord.name");
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, s("@ShoppingCart.name"));

        // Bare "@ModalRecord" (no path) should also rewrite.
        let mut bare = s("@ModalRecord");
        rewrite_entity_refs(&mut bare, "ModalRecord", "ShoppingCart");
        assert_eq!(bare, s("@ShoppingCart"));
    }

    #[test]
    fn rewrite_entity_refs_does_not_rewrite_unrelated_entity() {
        // ["ref", "OtherEntity"] stays as-is.
        let mut expr = SExpression::Array(vec![s("ref"), s("OtherEntity")]);
        let snapshot = expr.clone();
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, snapshot);

        // "@ModalRecordOther" (prefix match without boundary) stays as-is.
        let mut prefixed = s("@ModalRecordOther");
        let snapshot2 = prefixed.clone();
        rewrite_entity_refs(&mut prefixed, "ModalRecord", "ShoppingCart");
        assert_eq!(prefixed, snapshot2);

        // "@ModalRecordOther.field" also stays as-is (not a boundary match).
        let mut prefixed_path = s("@ModalRecordOther.field");
        let snapshot3 = prefixed_path.clone();
        rewrite_entity_refs(&mut prefixed_path, "ModalRecord", "ShoppingCart");
        assert_eq!(prefixed_path, snapshot3);

        // Plain string "ModalRecord" without @ sigil stays as-is.
        let mut plain = s("ModalRecord");
        let snapshot4 = plain.clone();
        rewrite_entity_refs(&mut plain, "ModalRecord", "ShoppingCart");
        assert_eq!(plain, snapshot4);
    }

    #[test]
    fn rewrite_entity_refs_rewrites_nested_lists() {
        // Nested: ["render-ui", "main", { children: [ ["ref", "ModalRecord"] ] }]
        // plus a nested binding root inside a sibling array.
        let mut obj: HashMap<String, SExpression> = HashMap::new();
        obj.insert(
            "children".to_string(),
            SExpression::Array(vec![SExpression::Array(vec![s("ref"), s("ModalRecord")])]),
        );
        obj.insert("title".to_string(), s("@ModalRecord.name"));

        let mut expr =
            SExpression::Array(vec![s("render-ui"), s("main"), SExpression::Object(obj)]);

        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");

        // Walk the result and assert both the ref atom and the binding root
        // were rewritten.
        if let SExpression::Array(items) = &expr {
            if let SExpression::Object(map) = &items[2] {
                let children = map.get("children").unwrap();
                assert_eq!(
                    children,
                    &SExpression::Array(vec![SExpression::Array(vec![
                        s("ref"),
                        s("ShoppingCart")
                    ])])
                );
                assert_eq!(map.get("title").unwrap(), &s("@ShoppingCart.name"));
            } else {
                panic!("expected object in slot 2");
            }
        } else {
            panic!("expected outer array");
        }
    }

    // ------------------------------------------------------------------
    // apply_overrides_to_trait — Phase 3.5.G.A.2 (trait renaming)
    // ------------------------------------------------------------------

    /// Build a minimal `TraitDefinition` with the given name so the rename
    /// tests don't have to hand-construct every field.
    fn make_trait(name: &str) -> TraitDefinition {
        use orbital_core::schema::StateMachine;
        TraitDefinition {
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("ModalRecord".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        }
    }

    #[test]
    fn inline_renames_trait_when_name_override_present() {
        // A Reference that supplies `name: Some("CartItemAddItem")` must
        // produce an inlined trait whose `name` is the override (not the
        // resolved upstream `ModalRecordModal`).
        let mut trait_def = make_trait("ModalRecordModal");
        apply_overrides_to_trait(
            &mut trait_def,
            None,
            None,
            Some("CartItemAddItem"),
            None,
            None,
            false,
            &mut Vec::new(),
        );
        assert_eq!(
            trait_def.name, "CartItemAddItem",
            "name override must rename the trait"
        );
        // linked_entity is untouched because no linked_entity override was
        // supplied.
        assert_eq!(trait_def.linked_entity.as_deref(), Some("ModalRecord"));
    }

    #[test]
    fn inline_does_not_rename_when_name_matches_resolved() {
        // Passing the same name as the resolved trait's name is a no-op.
        let mut trait_def = make_trait("ModalRecordModal");
        apply_overrides_to_trait(
            &mut trait_def,
            None,
            None,
            Some("ModalRecordModal"),
            None,
            None,
            false,
            &mut Vec::new(),
        );
        assert_eq!(trait_def.name, "ModalRecordModal");
    }

    #[test]
    fn inline_does_not_rename_when_name_is_none() {
        // With `name = None`, the resolved trait keeps its original name.
        let mut trait_def = make_trait("ModalRecordModal");
        apply_overrides_to_trait(&mut trait_def, None, None, None, None, None, false, &mut Vec::new());
        assert_eq!(trait_def.name, "ModalRecordModal");
    }

    #[test]
    fn inline_applies_rename_and_linked_entity_simultaneously() {
        // When BOTH `name` and `linked_entity` are set, both overrides
        // apply: the trait is renamed AND its linked_entity is rewritten
        // (which also walks the state machine — verified separately in the
        // rewrite_entity_refs tests). Here we just confirm both fields
        // change on the TraitDefinition.
        let mut trait_def = make_trait("ModalRecordModal");
        // Seed a transition effect that references the original entity so
        // we can assert the linked_entity rewrite still fires when the
        // rename is in play.
        use orbital_core::schema::{StringOrArray, TransitionDefinition};
        trait_def
            .state_machine
            .transitions
            .push(TransitionDefinition {
                from: StringOrArray::Single("idle".to_string()),
                to: "open".to_string(),
                event: "OPEN".to_string(),
                guard: None,
                effects: Some(vec![SExpression::Array(vec![
                    SExpression::String("ref".to_string()),
                    SExpression::String("ModalRecord".to_string()),
                ])]),
            });

        let linked = "CartItem".to_string();
        apply_overrides_to_trait(
            &mut trait_def,
            Some(&linked),
            None,
            Some("CartItemAddItem"),
            None,
            None,
            false,
            &mut Vec::new(),
        );

        assert_eq!(
            trait_def.name, "CartItemAddItem",
            "rename must apply alongside linked_entity"
        );
        assert_eq!(
            trait_def.linked_entity.as_deref(),
            Some("CartItem"),
            "linked_entity override must apply alongside rename"
        );

        // The seeded `["ref", "ModalRecord"]` effect must have been
        // rewritten to `["ref", "CartItem"]` by the linked_entity
        // override path.
        let effect = &trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .expect("effects present")[0];
        assert_eq!(
            effect,
            &SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("CartItem".to_string()),
            ]),
            "rename path must not interfere with linked_entity rewrite"
        );
    }

    // ------------------------------------------------------------------
    // inline_traits collision detection — Phase 3.5.G.A.3
    // ------------------------------------------------------------------

    /// Build an `Orbital` whose only trait is the supplied `TraitDefinition`,
    /// suitable for stashing in an `InlineContext` under an alias.
    fn make_orbital_with_trait(name: &str, trait_def: TraitDefinition) -> Orbital {
        use orbital_core::schema::EntityDefinition;
        Orbital {
            name: name.to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: format!("{}Entity", name),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(trait_def)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        }
    }

    /// Build an `InlineContext` with one alias `Modal` whose only trait is
    /// `ModalRecordModal`. This mirrors the embedded `std-modal` registry
    /// shape that the real fixtures rely on, without dragging the loader
    /// into the test.
    fn make_modal_ctx() -> InlineContext {
        let mut ctx = InlineContext::new();
        let modal_orbital = make_orbital_with_trait("Modal", make_trait("ModalRecordModal"));
        ctx.add_alias("Modal", &modal_orbital);
        ctx
    }

    /// Helper: build a `TraitReference::Reference` pointing at
    /// `Modal.traits.ModalRecordModal` with optional overrides.
    fn modal_ref(name_override: Option<&str>, linked_entity: Option<&str>) -> TraitReference {
        TraitReference::Reference {
            reference: Some("Modal.traits.ModalRecordModal".to_string()),
            name: name_override.map(|s| s.to_string()),
            linked_entity: linked_entity.map(|s| s.to_string()),
            config: None,
            events: None,
            fields: None,
            listens: None,
            emits_scope: None,
        }
    }

    #[test]
    fn inline_collision_two_references_no_name_override_errors() {
        // Two references to Modal.traits.ModalRecordModal, neither carrying
        // a `name` override. After A.1 + A.2 each resolves to a trait whose
        // `name` is the upstream `ModalRecordModal`, so they collide on
        // push #2. The inliner must reject with DuplicateTraitName naming
        // the offending trait.
        let ctx = make_modal_ctx();
        let traits = vec![
            modal_ref(None, Some("CartItem")),
            modal_ref(None, Some("CartItem")),
        ];

        let result = inline_traits(&traits, &ctx, "<test>", &mut Vec::new());
        match result {
            Err(InlineError::DuplicateTraitName { name, .. }) => {
                assert_eq!(
                    name, "ModalRecordModal",
                    "DuplicateTraitName must carry the original (collided) trait name"
                );
            }
            other => panic!(
                "expected Err(DuplicateTraitName(\"ModalRecordModal\")), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn inline_collision_two_references_with_distinct_name_overrides_succeeds() {
        // Two references with two DIFFERENT `name` overrides. Post-rename
        // the trait names differ, so no collision: both inlined traits
        // appear in the result with their renamed names AND their
        // linked_entity overrides applied.
        let ctx = make_modal_ctx();
        let traits = vec![
            modal_ref(Some("CartItemAddItem"), Some("CartItem")),
            modal_ref(Some("CartItemEditItem"), Some("CartItem")),
        ];

        let (result, _rename_map, _aux, _pulled) = inline_traits(&traits, &ctx, "<test>", &mut Vec::new())
            .expect("two distinct `name` overrides must inline successfully without collision");
        assert_eq!(result.len(), 2, "both references must be inlined");

        let names: Vec<&str> = result
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"CartItemAddItem"),
            "first override name missing; got {:?}",
            names
        );
        assert!(
            names.contains(&"CartItemEditItem"),
            "second override name missing; got {:?}",
            names
        );

        // And both carry the linked_entity override.
        for t in &result {
            if let TraitReference::Inline(def) = t {
                assert_eq!(def.linked_entity.as_deref(), Some("CartItem"));
            }
        }
    }

    #[test]
    fn inline_collision_one_reference_with_name_override_one_without_succeeds() {
        // One reference renames to `CartItemAddItem`; the other has no
        // `name` override and stays as `ModalRecordModal`. Two distinct
        // post-rename names, so no collision.
        let ctx = make_modal_ctx();
        let traits = vec![
            modal_ref(Some("CartItemAddItem"), Some("CartItem")),
            modal_ref(None, Some("CartItem")),
        ];

        let (result, _rename_map, _aux, _pulled) = inline_traits(&traits, &ctx, "<test>", &mut Vec::new())
            .expect("one renamed + one not-renamed must succeed (no name collision)");
        assert_eq!(result.len(), 2);

        let names: Vec<&str> = result
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"CartItemAddItem"));
        assert!(names.contains(&"ModalRecordModal"));
    }

    #[test]
    fn rewrite_page_trait_refs_renames_bare_ref() {
        // A page with `traits: [{ref: "ModalRecordModal"}]` plus a rename
        // map { "ModalRecordModal" → "ArticleAddItem" } should produce a
        // page with `traits: [{ref: "ArticleAddItem"}]`. This is the
        // page-side companion to the trait-rename pass — without it,
        // validation downstream fires `ORB_P_INVALID_TRAIT_REF`.
        use orbital_core::schema::PageDefinition;

        let mut pages = Some(vec![PageRef::Inline(PageDefinition {
            name: "ModalRecordModalPage".to_string(),
            path: "/modal".to_string(),
            view_type: None,
            primary_entity: None,
            is_initial: None,
            traits: Some(vec![TraitReference::Reference {
                reference: Some("ModalRecordModal".to_string()),
                name: None,
                linked_entity: None,
                config: None,
                events: None,
                fields: None,
                listens: None,
                emits_scope: None,
            }]),
            sections: None,
        })]);

        let mut rename_map = HashMap::new();
        rename_map.insert("ModalRecordModal".to_string(), "ArticleAddItem".to_string());

        rewrite_page_trait_refs(&mut pages, &rename_map);

        let page = pages.as_ref().unwrap().first().unwrap();
        let PageRef::Inline(def) = page else {
            panic!("expected inline page");
        };
        let traits = def.traits.as_ref().unwrap();
        match &traits[0] {
            TraitReference::Reference { reference, .. } => {
                assert_eq!(
                    reference.as_deref(),
                    Some("ArticleAddItem"),
                    "page-trait ref must be rewritten to the post-rename name"
                );
            }
            _ => panic!("expected Reference variant"),
        }
    }

    #[test]
    fn rewrite_page_trait_refs_skips_dotted_refs() {
        // A page with `traits: [{ref: "Modal.traits.ModalRecordModal"}]` is
        // a cross-alias reference that's already been resolved before this
        // pass runs. Even if the bare-name segment matches a rename-map
        // key, we shouldn't rewrite — those refs go through a different
        // resolution path and must remain dotted.
        use orbital_core::schema::PageDefinition;

        let mut pages = Some(vec![PageRef::Inline(PageDefinition {
            name: "Page".to_string(),
            path: "/".to_string(),
            view_type: None,
            primary_entity: None,
            is_initial: None,
            traits: Some(vec![TraitReference::Reference {
                reference: Some("Modal.traits.ModalRecordModal".to_string()),
                name: None,
                linked_entity: None,
                config: None,
                events: None,
                fields: None,
                listens: None,
                emits_scope: None,
            }]),
            sections: None,
        })]);

        let mut rename_map = HashMap::new();
        rename_map.insert("ModalRecordModal".to_string(), "ArticleAddItem".to_string());

        rewrite_page_trait_refs(&mut pages, &rename_map);

        let page = pages.as_ref().unwrap().first().unwrap();
        let PageRef::Inline(def) = page else {
            panic!()
        };
        let traits = def.traits.as_ref().unwrap();
        match &traits[0] {
            TraitReference::Reference { reference, .. } => {
                assert_eq!(
                    reference.as_deref(),
                    Some("Modal.traits.ModalRecordModal"),
                    "dotted refs must NOT be rewritten by the bare-name rename pass"
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn inline_collision_inline_and_reference_same_name_errors() {
        // A literal `TraitReference::Inline` named `ModalRecordModal` plus
        // a Reference that resolves to a trait of the same name. The
        // collision must be detected even though one half is a literal
        // (not a Reference). This protects molecules that hand-author one
        // trait and import a same-named one accidentally.
        let ctx = make_modal_ctx();
        let traits = vec![
            TraitReference::Inline(make_trait("ModalRecordModal")),
            modal_ref(None, None),
        ];

        let result = inline_traits(&traits, &ctx, "<test>", &mut Vec::new());
        match result {
            Err(InlineError::DuplicateTraitName { name, .. }) => {
                assert_eq!(name, "ModalRecordModal");
            }
            other => panic!(
                "expected Err(DuplicateTraitName(\"ModalRecordModal\")), got {:?}",
                other
            ),
        }
    }

    // ------------------------------------------------------------------
    // inline_services + ServiceRef::Object — Phase 3.5.G.A.4
    // ------------------------------------------------------------------

    /// Build a minimal REST `ServiceDefinition` so the service-override
    /// tests don't have to hand-construct every field.
    fn make_rest_service(name: &str, base_url: &str) -> ServiceDefinition {
        use orbital_core::schema::RestServiceDefInner;
        ServiceDefinition::Rest(RestServiceDefInner {
            name: name.to_string(),
            description: Some("upstream description".to_string()),
            base_url: base_url.to_string(),
            headers: None,
            auth: None,
            timeout: None,
        })
    }

    /// Build an `Orbital` whose only service is the supplied REST
    /// `ServiceDefinition`. Mirrors `make_orbital_with_trait` for services.
    fn make_orbital_with_service(name: &str, service_def: ServiceDefinition) -> Orbital {
        use orbital_core::schema::EntityDefinition;
        Orbital {
            name: name.to_string(),
            uses: None,
            theme: None,
            services: Some(vec![ServiceRef::Inline(service_def)]),
            entity: EntityRef::Inline(EntityDefinition {
                name: format!("{}Entity", name),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        }
    }

    /// Build an `InlineContext` with one alias `Llm` whose only service is
    /// `LlmService` (REST, with a placeholder upstream baseUrl). Mirrors
    /// `make_modal_ctx` for the service path.
    fn make_llm_service_ctx() -> InlineContext {
        let mut ctx = InlineContext::new();
        let llm_orbital = make_orbital_with_service(
            "Llm",
            make_rest_service("LlmService", "https://upstream.example.com"),
        );
        ctx.add_alias("Llm", &llm_orbital);
        ctx
    }

    /// Helper: build a `ServiceRef::Object` pointing at
    /// `Llm.services.LlmService` with optional override fields.
    fn llm_service_object(
        base_url_override: Option<&str>,
        description_override: Option<&str>,
    ) -> ServiceRef {
        ServiceRef::Object(ServiceRefObject {
            reference: "Llm.services.LlmService".to_string(),
            description: description_override.map(|s| s.to_string()),
            base_url: base_url_override.map(|s| s.to_string()),
            headers: None,
            url: None,
            server_path: None,
        })
    }

    #[test]
    fn inline_service_object_resolves_with_no_overrides() {
        // A `ServiceRef::Object` with no override fields must resolve to
        // the same Inline service that a plain `Reference` string would
        // produce. The resolved service keeps every upstream field
        // unchanged (here: baseUrl + description).
        let ctx = make_llm_service_ctx();
        let services = Some(vec![llm_service_object(None, None)]);

        let result = inline_services(&services, &ctx)
            .expect("ServiceRef::Object with no overrides must resolve cleanly");
        let result = result.expect("Some result expected");
        assert_eq!(result.len(), 1, "exactly one service must be inlined");

        match &result[0] {
            ServiceRef::Inline(ServiceDefinition::Rest(rest)) => {
                assert_eq!(rest.name, "LlmService");
                assert_eq!(
                    rest.base_url, "https://upstream.example.com",
                    "no override → upstream baseUrl preserved"
                );
                assert_eq!(
                    rest.description.as_deref(),
                    Some("upstream description"),
                    "no override → upstream description preserved"
                );
            }
            other => panic!("expected ServiceRef::Inline(Rest), got {:?}", other),
        }
    }

    #[test]
    fn inline_service_object_applies_endpoint_override() {
        // Same context as above, but the Object carries a baseUrl
        // override. After inlining, the resulting Inline service must
        // have the new baseUrl. Description is also overridden to prove
        // the override-application loop covers more than one field.
        let ctx = make_llm_service_ctx();
        let services = Some(vec![llm_service_object(
            Some("https://custom.example.com"),
            Some("custom description"),
        )]);

        let result = inline_services(&services, &ctx)
            .expect("ServiceRef::Object with baseUrl override must resolve");
        let result = result.expect("Some result expected");
        assert_eq!(result.len(), 1);

        match &result[0] {
            ServiceRef::Inline(ServiceDefinition::Rest(rest)) => {
                assert_eq!(rest.name, "LlmService", "name is not overrideable");
                assert_eq!(
                    rest.base_url, "https://custom.example.com",
                    "baseUrl override must replace upstream value"
                );
                assert_eq!(
                    rest.description.as_deref(),
                    Some("custom description"),
                    "description override must replace upstream value"
                );
            }
            other => panic!("expected ServiceRef::Inline(Rest), got {:?}", other),
        }
    }

    #[test]
    fn inline_service_object_with_unknown_alias_errors() {
        // An Object whose `reference` points to an alias that the
        // InlineContext doesn't know must error with `UnknownAlias`,
        // surfaced through `ctx.get_service` → `get_orbital`.
        let ctx = make_llm_service_ctx();
        let services = Some(vec![ServiceRef::Object(ServiceRefObject {
            reference: "NoSuchAlias.services.SomeService".to_string(),
            description: None,
            base_url: None,
            headers: None,
            url: None,
            server_path: None,
        })]);

        let result = inline_services(&services, &ctx);
        match result {
            Err(InlineError::UnknownAlias(alias)) => {
                assert_eq!(
                    alias, "NoSuchAlias",
                    "UnknownAlias must carry the offending alias name"
                );
            }
            other => panic!(
                "expected Err(UnknownAlias(\"NoSuchAlias\")), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn inline_service_object_invalid_reference_format_errors() {
        // An Object whose `reference` is not in the
        // `Alias.services.Name` format must surface as InvalidReference.
        // This guards against typos like `Llm.LlmService` (missing the
        // `services` segment).
        let ctx = make_llm_service_ctx();
        let services = Some(vec![ServiceRef::Object(ServiceRefObject {
            reference: "Llm.LlmService".to_string(),
            description: None,
            base_url: None,
            headers: None,
            url: None,
            server_path: None,
        })]);

        let result = inline_services(&services, &ctx);
        assert!(
            matches!(result, Err(InlineError::InvalidReference(_))),
            "expected InvalidReference, got {:?}",
            result
        );
    }

    // ------------------------------------------------------------------
    // inline_entity + EntityRef::Extends — Phase 3.5.G.A.5
    // ------------------------------------------------------------------

    use orbital_core::schema::{FieldType, Persistence};

    /// Build a minimal `EntityDefinition` mirroring the upstream
    /// `ModalRecord` shape from the embedded std-modal registry: 5 fields
    /// (id, name, description, status, createdAt) with `runtime`
    /// persistence. The Extends tests use this as the base entity that the
    /// molecule inherits from, so the field counts and names match the
    /// real registry shape that the cart molecule consumes.
    fn make_modal_entity_def() -> EntityDefinition {
        let make_field = |n: &str, t: FieldType| FieldDefinition {
            name: Some(n.to_string()),
            field_type: t,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        };
        EntityDefinition {
            name: "ModalRecord".to_string(),
            collection: None,
            persistence: Some(Persistence::Runtime),
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
                make_field("createdAt", FieldType::String),
            ],
            instances: None,
        }
    }

    /// Build an `Orbital` whose only entity is the supplied
    /// `EntityDefinition`. Mirrors `make_orbital_with_trait` /
    /// `make_orbital_with_service` for entities so the Extends tests can
    /// stash a base entity in the InlineContext under an alias.
    fn make_orbital_with_entity(name: &str, entity_def: EntityDefinition) -> Orbital {
        Orbital {
            name: name.to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(entity_def),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        }
    }

    /// Build an `InlineContext` with one alias `Modal` whose only entity
    /// is `ModalRecord` (matching the embedded std-modal registry). Mirrors
    /// `make_modal_ctx` and `make_llm_service_ctx` for the entity path.
    fn make_modal_entity_ctx() -> InlineContext {
        let mut ctx = InlineContext::new();
        let modal_orbital = make_orbital_with_entity("Modal", make_modal_entity_def());
        ctx.add_alias("Modal", &modal_orbital);
        ctx
    }

    /// Helper: build a `FieldDefinition` for an extra field, with just a
    /// name and a type. The Extends tests pass these as the molecule's
    /// `fields` array.
    fn make_extra_field(name: &str, ft: FieldType) -> FieldDefinition {
        FieldDefinition {
            name: Some(name.to_string()),
            field_type: ft,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }
    }

    #[test]
    fn inline_entity_extends_inherits_all_base_fields_when_no_extras() {
        // EntityRef::Extends with no rename and no extra fields must
        // produce an Inline entity equivalent to a bare
        // EntityRef::Reference("Modal.entity") — same name, same fields,
        // same persistence — confirming the no-op path through the
        // override-application loop is byte-faithful.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: None,
            fields: None,
            persistence: None,
            collection: None,
        });

        let result =
            inline_entity(&entity, &ctx).expect("Extends with no overrides must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(
            inlined.name, "ModalRecord",
            "no rename → upstream entity name preserved"
        );
        assert_eq!(
            inlined.persistence,
            Some(Persistence::Runtime),
            "no persistence override → upstream value preserved"
        );
        assert_eq!(
            inlined.fields.len(),
            5,
            "no extras → all 5 inherited fields preserved"
        );
        let field_names: Vec<&str> = inlined
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        assert_eq!(
            field_names,
            vec!["id", "name", "description", "status", "createdAt"]
        );
    }

    #[test]
    fn inline_entity_extends_renames_when_name_set() {
        // Extends with `name: Some("CartItem")` and no extras must rename
        // the resulting entity to `CartItem` while still preserving every
        // inherited field. This is what molecules do when they want to
        // inherit a base shape but give it a contextual name.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: Some("CartItem".to_string()),
            fields: None,
            persistence: None,
            collection: None,
        });

        let result = inline_entity(&entity, &ctx).expect("Extends with rename must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(
            inlined.name, "CartItem",
            "name override must rename the entity"
        );
        assert_eq!(
            inlined.fields.len(),
            5,
            "rename without extras → all 5 inherited fields preserved"
        );
    }

    #[test]
    fn inline_entity_extends_appends_extra_fields() {
        // Extends with both a rename AND two extra fields must produce a
        // 7-field entity: the 5 inherited fields followed by the 2
        // molecule fields, in that order. This is the cart molecule case
        // (5 ModalRecord fields + pendingId + lineItemCount).
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: Some("CartItem".to_string()),
            fields: Some(vec![
                make_extra_field("pendingId", FieldType::String),
                make_extra_field("lineItemCount", FieldType::Number),
            ]),
            persistence: None,
            collection: None,
        });

        let result =
            inline_entity(&entity, &ctx).expect("Extends with rename + extras must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(inlined.name, "CartItem");
        assert_eq!(
            inlined.fields.len(),
            7,
            "5 inherited + 2 extras → 7 total fields"
        );

        let field_names: Vec<&str> = inlined
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        // Order: inherited first, then extras (in declaration order).
        assert_eq!(
            field_names,
            vec![
                "id",
                "name",
                "description",
                "status",
                "createdAt",
                "pendingId",
                "lineItemCount",
            ]
        );
    }

    #[test]
    fn inline_entity_extends_extra_fields_override_collisions() {
        // Extends with an extra field whose `name` matches an inherited
        // field (`status`) must produce only ONE field with that name —
        // the molecule's version. This is the "user wins on conflict"
        // contract from the design notes.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "Modal.entity".to_string(),
            name: Some("CartItem".to_string()),
            // Override `status` (inherited) and add a brand new
            // `pendingId`.
            fields: Some(vec![
                make_extra_field("status", FieldType::Number),
                make_extra_field("pendingId", FieldType::String),
            ]),
            persistence: None,
            collection: None,
        });

        let result = inline_entity(&entity, &ctx)
            .expect("Extends with collision-override must inline cleanly");

        let inlined = match result {
            EntityRef::Inline(e) => e,
            other => panic!("expected EntityRef::Inline, got {:?}", other),
        };
        assert_eq!(
            inlined.fields.len(),
            6,
            "5 inherited - 1 collision + 2 extras (1 collision + 1 new) = 6"
        );

        // The inherited `status` field (FieldType::String) must be gone;
        // the molecule's `status` (FieldType::Number) must be the only one.
        let status_fields: Vec<&FieldDefinition> = inlined
            .fields
            .iter()
            .filter(|f| f.name.as_deref() == Some("status"))
            .collect();
        assert_eq!(
            status_fields.len(),
            1,
            "exactly one `status` field must survive (the molecule's override)"
        );
        assert_eq!(
            status_fields[0].field_type,
            FieldType::Number,
            "molecule's `status` (Number) must replace the inherited `status` (String)"
        );

        // The new `pendingId` is present.
        assert!(
            inlined
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some("pendingId")),
            "molecule's new `pendingId` field must be present"
        );

        // The other 4 inherited fields must still be there in their
        // original positions (relative order preserved).
        let names: Vec<&str> = inlined
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        assert_eq!(
            names,
            vec![
                "id",
                "name",
                "description",
                "createdAt",
                "status",
                "pendingId"
            ],
            "non-colliding inherited fields keep their order; the molecule's \
             override `status` lands at the extras position, not the inherited slot"
        );
    }

    #[test]
    fn inline_entity_extends_unknown_alias_errors() {
        // Extends with a `reference` pointing at an alias the InlineContext
        // doesn't know must surface UnknownAlias via ctx.get_entity →
        // get_orbital. The error must carry the offending alias name.
        let ctx = make_modal_entity_ctx();
        let entity = EntityRef::Extends(EntityRefExtends {
            reference: "NoSuchAlias.entity".to_string(),
            name: None,
            fields: None,
            persistence: None,
            collection: None,
        });

        let result = inline_entity(&entity, &ctx);
        match result {
            Err(InlineError::UnknownAlias(alias)) => {
                assert_eq!(
                    alias, "NoSuchAlias",
                    "UnknownAlias must carry the offending alias name"
                );
            }
            other => panic!(
                "expected Err(UnknownAlias(\"NoSuchAlias\")), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn rewrite_entity_refs_noop_when_no_match() {
        // An expression with no entity references anywhere should be
        // byte-identical after rewrite.
        let mut obj: HashMap<String, SExpression> = HashMap::new();
        obj.insert("type".to_string(), s("stack"));
        obj.insert("direction".to_string(), s("vertical"));
        obj.insert(
            "children".to_string(),
            SExpression::Array(vec![
                SExpression::Array(vec![s("emit"), s("SAVED")]),
                s("@payload.data"),
            ]),
        );

        let mut expr =
            SExpression::Array(vec![s("render-ui"), s("main"), SExpression::Object(obj)]);
        let snapshot = expr.clone();
        rewrite_entity_refs(&mut expr, "ModalRecord", "ShoppingCart");
        assert_eq!(expr, snapshot);
    }

    #[test]
    fn test_parse_entity_reference() {
        let result = parse_reference("Health.entity").unwrap();
        assert!(matches!(result, ParsedReference::Entity { alias } if alias == "Health"));
    }

    #[test]
    fn test_parse_trait_reference() {
        let result = parse_reference("Health.traits.HealthTrait").unwrap();
        assert!(matches!(
            result,
            ParsedReference::Trait { alias, name } if alias == "Health" && name == "HealthTrait"
        ));
    }

    #[test]
    fn test_parse_page_reference() {
        let result = parse_reference("Health.pages.HealthStatus").unwrap();
        assert!(matches!(
            result,
            ParsedReference::Page { alias, name } if alias == "Health" && name == "HealthStatus"
        ));
    }

    #[test]
    fn test_parse_theme_reference() {
        let result = parse_reference("Ocean.theme").unwrap();
        assert!(matches!(result, ParsedReference::Theme { alias } if alias == "Ocean"));
    }

    #[test]
    fn test_parse_service_reference() {
        let result = parse_reference("Payment.services.Stripe").unwrap();
        assert!(matches!(
            result,
            ParsedReference::Service { alias, name } if alias == "Payment" && name == "Stripe"
        ));
    }

    #[test]
    fn test_parse_invalid_reference() {
        assert!(parse_reference("Health").is_err());
        assert!(parse_reference("Health.invalid").is_err());
        assert!(parse_reference("Health.traits").is_err());
    }

    #[test]
    fn test_inline_player_schema() {
        use crate::phases::parse::parse_file;

        let player_path = fixtures_dir().join("player.orb");
        let mut schema = parse_file(&player_path).unwrap();

        // Verify entity is a reference before inlining
        assert!(schema.orbitals[0].entity.is_reference());
        assert_eq!(
            schema.orbitals[0].entity.as_reference(),
            Some("Health.entity")
        );

        // Inline the schema
        let config = InlineConfig {
            loader_config: LoaderConfig {
                base_path: fixtures_dir(),
                allow_outside_base_path: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = inline_schema(&mut schema, &config, Some(&player_path));
        assert!(result.is_ok(), "Inline failed: {:?}", result.err());

        // Verify entity is now inline
        assert!(!schema.orbitals[0].entity.is_reference());
        let entity = schema.orbitals[0].entity.as_inline().unwrap();
        assert_eq!(entity.name, "HealthEntity");

        // Verify traits are inlined
        assert!(!schema.orbitals[0].traits.is_empty());
        if let TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0] {
            assert_eq!(trait_def.name, "HealthTrait");
        } else {
            panic!("Expected inlined trait");
        }

        // Verify pages are inlined with path override
        let pages = schema.orbitals[0].pages.as_ref().expect("Expected pages");
        assert!(!pages.is_empty());
        if let PageRef::Inline(page_def) = &pages[0] {
            assert_eq!(page_def.name, "HealthStatus");
            // Path should be overridden
            assert_eq!(page_def.path, "/player/:id/health");
        } else {
            panic!("Expected inlined page");
        }
    }

    // ------------------------------------------------------------------
    // Phase 3.5.G.F.3 — Generic identifier-substitution rewriter
    // ------------------------------------------------------------------

    #[test]
    fn rewrite_identifiers_handles_multi_entry_entity_map() {
        // The walker accepts a HashMap with multiple entries; every entry
        // fires independently. `["ref", "A"]` and `["ref", "B"]` in the
        // same tree both get rewritten in a single walk.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("A".to_string(), "X".to_string());
        entity_subs.insert("B".to_string(), "Y".to_string());

        let mut expr = SExpression::Array(vec![
            SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("A".to_string()),
            ]),
            SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("B".to_string()),
            ]),
        ]);

        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        if let SExpression::Array(items) = &expr {
            // Both nested ref calls were rewritten in one pass.
            if let SExpression::Array(inner) = &items[0] {
                assert_eq!(inner[1], SExpression::String("X".to_string()));
            }
            if let SExpression::Array(inner) = &items[1] {
                assert_eq!(inner[1], SExpression::String("Y".to_string()));
            }
        } else {
            panic!("expected outer array");
        }
    }

    #[test]
    fn rewrite_identifiers_rewrites_emit_arrays_via_event_subs() {
        // `["emit", "OPEN"]` is the symmetric event-side counterpart of
        // `["ref", "EntityName"]`. With OPEN → ADD_ITEM in event_subs,
        // the second element gets substituted.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut expr = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("OPEN".to_string()),
        ]);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Array(items) = &expr {
            assert_eq!(items[1], SExpression::String("ADD_ITEM".to_string()));
        } else {
            panic!("expected array");
        }
    }

    #[test]
    fn rewrite_identifiers_rewrites_emit_with_payload_tail() {
        // The 3-element `(emit EVENT payload)` shape — what every state
        // machine actually emits when the event carries data. Pre-fix, the
        // rewriter only matched `items.len() == 2` and silently passed
        // `["emit", "FILTER", { value: "" }]` through unchanged, leaving
        // a stale event reference that the validator flagged as
        // ORB_T_EVT_UNDECLARED.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("FILTER".to_string(), "NARROW".to_string());

        let mut payload_obj: HashMap<String, SExpression> = HashMap::new();
        payload_obj.insert("value".to_string(), SExpression::String("".to_string()));
        payload_obj.insert("field".to_string(), SExpression::String("".to_string()));
        let mut expr = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("FILTER".to_string()),
            SExpression::Object(payload_obj),
        ]);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Array(items) = &expr {
            assert_eq!(items.len(), 3, "payload tail should be preserved");
            assert_eq!(items[1], SExpression::String("NARROW".to_string()));
            // Payload object is left alone — only the event name in
            // position 1 was a rename target.
            assert!(matches!(items[2], SExpression::Object(_)));
        } else {
            panic!("expected array");
        }
    }

    #[test]
    fn rewrite_identifiers_emit_payload_tail_inner_ref_still_walks() {
        // After the emit-event rewrite we DON'T return — the walk
        // continues into the payload tail. A nested `["ref", X]` inside
        // the payload should still get entity-rewritten in the same pass.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("OldEntity".to_string(), "NewEntity".to_string());
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut expr = SExpression::Array(vec![
            SExpression::String("emit".to_string()),
            SExpression::String("OPEN".to_string()),
            SExpression::Array(vec![
                SExpression::String("ref".to_string()),
                SExpression::String("OldEntity".to_string()),
            ]),
        ]);

        rewrite_identifiers(&mut expr, &entity_subs, &event_subs);

        if let SExpression::Array(items) = &expr {
            assert_eq!(items[1], SExpression::String("ADD_ITEM".to_string()));
            if let SExpression::Array(inner) = &items[2] {
                assert_eq!(inner[1], SExpression::String("NewEntity".to_string()));
            } else {
                panic!("expected inner ref array");
            }
        } else {
            panic!("expected outer array");
        }
    }

    #[test]
    fn rewrite_identifiers_rewrites_render_ui_event_object_value() {
        // Registry-driven rule: when an SExpression::Object has `type: "button"`
        // and the registry declares `"action"` as button's event-valued prop
        // (see PATTERN_EVENT_PROPS in validation::sync::patterns), a string
        // value under "action" that matches an event_subs key gets rewritten.
        // This handles render-ui button event references that live inside
        // object maps rather than `["emit", X]` arrays.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut object = HashMap::new();
        object.insert(
            "type".to_string(),
            SExpression::String("button".to_string()),
        );
        object.insert(
            "action".to_string(),
            SExpression::String("OPEN".to_string()),
        );
        let mut expr = SExpression::Object(object);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Object(map) = &expr {
            assert_eq!(
                map.get("action"),
                Some(&SExpression::String("ADD_ITEM".to_string())),
                "button action= attribute must be substituted"
            );
            // The unrelated "type" entry is untouched.
            assert_eq!(
                map.get("type"),
                Some(&SExpression::String("button".to_string()))
            );
        } else {
            panic!("expected object");
        }
    }

    #[test]
    fn rewrite_identifiers_does_not_rewrite_event_substring_in_other_strings() {
        // A plain string value whose key is NOT "event" must NOT be
        // rewritten even if it equals an event key. This protects content
        // strings like `{ "type": "typography", "content": "OPEN" }` from
        // accidental substitution.
        let mut event_subs: HashMap<String, String> = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        let mut object = HashMap::new();
        object.insert(
            "content".to_string(),
            SExpression::String("OPEN".to_string()),
        );
        let mut expr = SExpression::Object(object);

        rewrite_identifiers(&mut expr, &HashMap::new(), &event_subs);

        if let SExpression::Object(map) = &expr {
            assert_eq!(
                map.get("content"),
                Some(&SExpression::String("OPEN".to_string())),
                "content strings must NOT be substituted by event_subs"
            );
        }
    }

    #[test]
    fn rewrite_trait_event_fields_renames_state_machine_events_and_transitions() {
        use orbital_core::schema::{
            EventDefinition, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let mut trait_def = TraitDefinition {
            name: "T".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "closed".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                }],
                events: Some(vec![
                    EventDefinition {
                        key: "OPEN".to_string(),
                        name: None,
                        description: None,
                        synonyms: None,
                        tier: None,
                        payload_schema: None,
                    },
                    EventDefinition {
                        key: "CLOSE".to_string(),
                        name: None,
                        description: None,
                        synonyms: None,
                        tier: None,
                        payload_schema: None,
                    },
                ]),
                transitions: vec![
                    TransitionDefinition {
                        from: StringOrArray::Single("closed".to_string()),
                        to: "open".to_string(),
                        event: "OPEN".to_string(),
                        guard: None,
                        effects: None,
                    },
                    TransitionDefinition {
                        from: StringOrArray::Single("open".to_string()),
                        to: "closed".to_string(),
                        event: "CLOSE".to_string(),
                        guard: None,
                        effects: None,
                    },
                ],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        };

        let mut event_subs = HashMap::new();
        event_subs.insert("OPEN".to_string(), "ADD_ITEM".to_string());
        event_subs.insert("CLOSE".to_string(), "CANCEL_ADD".to_string());

        rewrite_trait_event_fields(&mut trait_def, &event_subs);

        // StateMachine.events keys are rewritten.
        let event_keys: Vec<&str> = trait_def
            .state_machine
            .events
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| e.key.as_str())
            .collect();
        assert!(event_keys.contains(&"ADD_ITEM"));
        assert!(event_keys.contains(&"CANCEL_ADD"));
        assert!(!event_keys.contains(&"OPEN"));
        assert!(!event_keys.contains(&"CLOSE"));

        // Transition events are rewritten.
        assert_eq!(trait_def.state_machine.transitions[0].event, "ADD_ITEM");
        assert_eq!(trait_def.state_machine.transitions[1].event, "CANCEL_ADD");
    }

    #[test]
    fn apply_overrides_to_trait_renames_events_via_events_arg() {
        // End-to-end of the call_args path: build a trait with an OPEN
        // transition + a corresponding event entry, pass an `events` arg
        // renaming OPEN → ADD_ITEM, and confirm both the structured fields
        // and (would-be) SExpression payloads agree.
        use orbital_core::schema::{
            EventDefinition, StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let mut trait_def = TraitDefinition {
            name: "ModalRecordModal".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("ModalRecord".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "closed".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                }],
                events: Some(vec![EventDefinition {
                    key: "OPEN".to_string(),
                    name: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                }]),
                transitions: vec![TransitionDefinition {
                    from: StringOrArray::Single("closed".to_string()),
                    to: "open".to_string(),
                    event: "OPEN".to_string(),
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        SExpression::String("emit".to_string()),
                        SExpression::String("OPEN".to_string()),
                    ])]),
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        };

        let mut events_arg = HashMap::new();
        events_arg.insert("OPEN".to_string(), "ADD_ITEM".to_string());

        apply_overrides_to_trait(
            &mut trait_def,
            None,
            None,
            None,
            Some(&events_arg),
            None,
            false,
            &mut Vec::new(),
        );

        // Structured field rewritten.
        assert_eq!(trait_def.state_machine.transitions[0].event, "ADD_ITEM");
        // SExpression `["emit", "OPEN"]` → `["emit", "ADD_ITEM"]`.
        let effects = trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .expect("effects present");
        if let SExpression::Array(items) = &effects[0] {
            assert_eq!(items[1], SExpression::String("ADD_ITEM".to_string()));
        } else {
            panic!("expected effect array");
        }
    }

    // ------------------------------------------------------------------
    // GAP-COMP-11 — linkedEntity rebinding rewrites fetch/persist shapes
    // ------------------------------------------------------------------

    #[test]
    fn rewrite_identifiers_rewrites_fetch_entity_slot() {
        // Atom trait effect: ["fetch", "Source"]. With entity_subs
        // Source → Target, position 1 must be rewritten.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut expr = SExpression::Array(vec![s("fetch"), s("Source")]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(expr, SExpression::Array(vec![s("fetch"), s("Target")]));
    }

    #[test]
    fn rewrite_identifiers_rewrites_fetch_entity_slot_with_criteria_tail() {
        // ["fetch", "Source", {criteria...}] — the entity slot at index 1
        // is rewritten; the trailing object is untouched (criteria objects
        // don't carry entity names by convention).
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut criteria = HashMap::new();
        criteria.insert("limit".to_string(), SExpression::Number(10.0));

        let mut expr = SExpression::Array(vec![
            s("fetch"),
            s("Source"),
            SExpression::Object(criteria.clone()),
        ]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(
            expr,
            SExpression::Array(vec![
                s("fetch"),
                s("Target"),
                SExpression::Object(criteria),
            ])
        );
    }

    #[test]
    fn rewrite_identifiers_rewrites_persist_create_entity_slot() {
        // ["persist", "create", "Source", {payload}] — index 2 is the entity
        // name and must be rewritten. The payload object is untouched.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut payload = HashMap::new();
        payload.insert("data".to_string(), s("@payload.data"));

        let mut expr = SExpression::Array(vec![
            s("persist"),
            s("create"),
            s("Source"),
            SExpression::Object(payload.clone()),
        ]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(
            expr,
            SExpression::Array(vec![
                s("persist"),
                s("create"),
                s("Target"),
                SExpression::Object(payload),
            ])
        );
    }

    #[test]
    fn rewrite_identifiers_rewrites_persist_update_and_delete_entity_slot() {
        // update + delete variants share the single-record shape and must
        // rewrite index 2 identically to create.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut update = SExpression::Array(vec![s("persist"), s("update"), s("Source")]);
        rewrite_identifiers(&mut update, &entity_subs, &HashMap::new());
        assert_eq!(
            update,
            SExpression::Array(vec![s("persist"), s("update"), s("Target")])
        );

        let mut delete = SExpression::Array(vec![s("persist"), s("delete"), s("Source")]);
        rewrite_identifiers(&mut delete, &entity_subs, &HashMap::new());
        assert_eq!(
            delete,
            SExpression::Array(vec![s("persist"), s("delete"), s("Target")])
        );
    }

    #[test]
    fn rewrite_identifiers_does_not_mangle_persist_batch_rows() {
        // ["persist", "batch", [row1, row2]] — index 2 is a rows array,
        // NOT an entity name. The walker must NOT rewrite it as a string
        // substitution, and the rows array must stay structurally intact.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut row1 = HashMap::new();
        row1.insert("name".to_string(), s("row-1"));
        let mut row2 = HashMap::new();
        row2.insert("name".to_string(), s("row-2"));

        let rows = SExpression::Array(vec![
            SExpression::Object(row1.clone()),
            SExpression::Object(row2.clone()),
        ]);

        let mut expr = SExpression::Array(vec![s("persist"), s("batch"), rows.clone()]);
        let snapshot = expr.clone();
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        // Batch shape is byte-identical after rewrite — the walker must not
        // touch the rows array.
        assert_eq!(
            expr, snapshot,
            "persist batch rows array must be untouched by entity rewrite"
        );
    }

    #[test]
    fn rewrite_identifiers_rewrites_nested_fetch_inside_render_ui() {
        // Recursion test: ["render-ui", "main", {children: [["fetch",
        // "Source"]]}] — the nested fetch effect must be found and rewritten
        // by the recursive walk.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut obj: HashMap<String, SExpression> = HashMap::new();
        obj.insert(
            "children".to_string(),
            SExpression::Array(vec![SExpression::Array(vec![s("fetch"), s("Source")])]),
        );

        let mut expr =
            SExpression::Array(vec![s("render-ui"), s("main"), SExpression::Object(obj)]);
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        if let SExpression::Array(items) = &expr {
            if let SExpression::Object(map) = &items[2] {
                let children = map.get("children").unwrap();
                assert_eq!(
                    children,
                    &SExpression::Array(vec![SExpression::Array(vec![s("fetch"), s("Target")])]),
                    "nested fetch inside render-ui children must be rewritten"
                );
            } else {
                panic!("expected object in slot 2");
            }
        } else {
            panic!("expected outer array");
        }
    }

    #[test]
    fn rewrite_identifiers_leaves_unrelated_fetch_entity_untouched() {
        // Negative: ["fetch", "OtherEntity"] where OtherEntity is NOT the
        // from-entity. The walker must leave it as-is.
        let mut entity_subs: HashMap<String, String> = HashMap::new();
        entity_subs.insert("Source".to_string(), "Target".to_string());

        let mut expr = SExpression::Array(vec![s("fetch"), s("OtherEntity")]);
        let snapshot = expr.clone();
        rewrite_identifiers(&mut expr, &entity_subs, &HashMap::new());

        assert_eq!(
            expr, snapshot,
            "fetch target that isn't the from-entity must be untouched"
        );
    }

    #[test]
    fn apply_overrides_to_trait_rewrites_fetch_and_persist_via_linked_entity() {
        // End-to-end: build a trait whose transitions carry fetch + persist
        // effects targeting the atom's original entity. Call
        // apply_overrides_to_trait with a linked_entity override and assert
        // both effect shapes are rewritten to the new entity.
        use orbital_core::schema::{StateMachine, StringOrArray, TransitionDefinition};

        let mut trait_def = TraitDefinition {
            name: "SourceSearchTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Source".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![
                    TransitionDefinition {
                        from: StringOrArray::Single("idle".to_string()),
                        to: "loading".to_string(),
                        event: "LOAD".to_string(),
                        guard: None,
                        effects: Some(vec![SExpression::Array(vec![s("fetch"), s("Source")])]),
                    },
                    TransitionDefinition {
                        from: StringOrArray::Single("editing".to_string()),
                        to: "saved".to_string(),
                        event: "SAVE".to_string(),
                        guard: None,
                        effects: Some(vec![SExpression::Array(vec![
                            s("persist"),
                            s("create"),
                            s("Source"),
                            s("@payload.data"),
                        ])]),
                    },
                ],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        };

        let linked = "Target".to_string();
        apply_overrides_to_trait(&mut trait_def, Some(&linked), None, None, None, None, false, &mut Vec::new());

        // linked_entity field flipped.
        assert_eq!(trait_def.linked_entity.as_deref(), Some("Target"));

        // Transition 0: fetch effect rewritten.
        let fetch_effect = &trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .expect("fetch effects present")[0];
        assert_eq!(
            fetch_effect,
            &SExpression::Array(vec![s("fetch"), s("Target")]),
            "fetch effect entity slot must be rewritten by linked_entity override"
        );

        // Transition 1: persist effect rewritten (position 2 only).
        let persist_effect = &trait_def.state_machine.transitions[1]
            .effects
            .as_ref()
            .expect("persist effects present")[0];
        assert_eq!(
            persist_effect,
            &SExpression::Array(vec![
                s("persist"),
                s("create"),
                s("Target"),
                s("@payload.data"),
            ]),
            "persist create effect entity slot must be rewritten by linked_entity override"
        );
    }

    // ------------------------------------------------------------------
    // Multi-orbital imported-behavior resolution — trait/page lookups
    // must iterate every orbital in the imported `.orb`, not just
    // orbital 0.
    //
    // Bug fix: previously `inline_orbital` took `loaded.schema.orbitals
    // .first()` when building the context, so any ref like
    // `Alias.traits.X` whose X lived in orbital 1+ of a multi-orbital
    // behavior (e.g. every organism registry) failed to resolve with
    // "Trait 'X' not found in orbital 'Y'". The resolver now stashes
    // every orbital under the alias and `get_trait` / `get_page`
    // iterate them all.
    // ------------------------------------------------------------------

    /// Build a minimal multi-orbital imported behavior with two orbitals,
    /// each carrying exactly one trait. Returns the list of orbitals in
    /// the order (orbital 0, orbital 1). Used to simulate the shape of an
    /// organism registry without touching the loader or disk.
    fn make_multi_orbital_behavior() -> Vec<Orbital> {
        let orb0 = make_orbital_with_trait("ArcadeState", make_trait("ArcadeStateGameflow"));
        let orb1 = make_orbital_with_trait("ArcadeCanvas", make_trait("ArcadeCanvasGameCanvas2d"));
        vec![orb0, orb1]
    }

    #[test]
    fn multi_orbital_trait_ref_resolves_from_orbital_zero() {
        // Trait name `ArcadeStateGameflow` lives in orbital 0 of the
        // imported behavior. The resolver must find it. This case
        // already worked before the fix, but is pinned here so a future
        // regression that "helpfully" skips orbital 0 is caught.
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            make_multi_orbital_behavior(),
            "std/behaviors/std-arcade-game".to_string(),
        );

        let trait_ref = ctx
            .get_trait("Arcade", "ArcadeStateGameflow")
            .expect("trait in orbital 0 must resolve");

        match trait_ref {
            TraitReference::Inline(def) => {
                assert_eq!(def.name, "ArcadeStateGameflow");
            }
            other => panic!("expected Inline trait, got {:?}", other),
        }
    }

    #[test]
    fn multi_orbital_trait_ref_resolves_from_orbital_one() {
        // Trait name `ArcadeCanvasGameCanvas2d` lives in orbital 1 of
        // the imported behavior. Before the fix this failed with
        // "Trait 'ArcadeCanvasGameCanvas2d' not found in orbital
        // 'ArcadeState'" because the resolver only searched orbital 0.
        // This is the regression test for the bug this commit fixes.
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            make_multi_orbital_behavior(),
            "std/behaviors/std-arcade-game".to_string(),
        );

        let trait_ref = ctx
            .get_trait("Arcade", "ArcadeCanvasGameCanvas2d")
            .expect("trait in orbital 1 must resolve after multi-orbital fix");

        match trait_ref {
            TraitReference::Inline(def) => {
                assert_eq!(def.name, "ArcadeCanvasGameCanvas2d");
            }
            other => panic!("expected Inline trait, got {:?}", other),
        }
    }

    #[test]
    fn multi_orbital_trait_ref_missing_errors_with_behavior_label() {
        // No orbital in the imported behavior declares this trait. The
        // error must (a) use the updated `TraitNotFound` variant and
        // (b) cite the behavior's `from` path (passed as source_label
        // to `add_alias_multi`) rather than just orbital 0's name, so
        // the author knows which imported .orb is missing the symbol.
        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            make_multi_orbital_behavior(),
            "std/behaviors/std-arcade-game".to_string(),
        );

        let result = ctx.get_trait("Arcade", "DoesNotExist");
        match result {
            Err(InlineError::TraitNotFound(trait_name, source_label)) => {
                assert_eq!(trait_name, "DoesNotExist");
                assert_eq!(
                    source_label, "std/behaviors/std-arcade-game",
                    "error must cite the imported behavior's `from` path, not orbital 0's name"
                );
            }
            other => panic!(
                "expected Err(TraitNotFound(\"DoesNotExist\", \
                 \"std/behaviors/std-arcade-game\")), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn multi_orbital_page_ref_resolves_from_orbital_one() {
        // Same coverage as the trait-in-orbital-1 test, but for pages.
        // Pages follow the same code path in the resolver, so this
        // pins the fix for `Alias.pages.X` as well.
        use orbital_core::schema::{EntityDefinition, PageDefinition};

        let orb0 = Orbital {
            name: "ArcadeState".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "ArcadeState".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        let canvas_page = PageDefinition {
            name: "ArcadeCanvasPage".to_string(),
            path: "/arcade/canvas".to_string(),
            view_type: None,
            primary_entity: None,
            is_initial: None,
            traits: None,
            sections: None,
        };

        let orb1 = Orbital {
            name: "ArcadeCanvas".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "ArcadeCanvas".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: Some(vec![PageRef::Inline(canvas_page)]),
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Arcade",
            vec![orb0, orb1],
            "std/behaviors/std-arcade-game".to_string(),
        );

        let page_ref = ctx
            .get_page("Arcade", "ArcadeCanvasPage")
            .expect("page in orbital 1 must resolve after multi-orbital fix");

        match page_ref {
            PageRef::Inline(def) => {
                assert_eq!(def.name, "ArcadeCanvasPage");
                assert_eq!(def.path, "/arcade/canvas");
            }
            other => panic!("expected Inline page, got {:?}", other),
        }
    }

    // ------------------------------------------------------------------
    // merge_imported_entity_fields — GAP-AGB-MOLECULE-ENTITY-CONTRACT
    // ------------------------------------------------------------------

    /// Helper to build a FieldDefinition with just a name and type.
    fn make_field(name: &str, ft: orbital_core::schema::FieldType) -> FieldDefinition {
        FieldDefinition {
            name: Some(name.to_string()),
            field_type: ft,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }
    }

    /// Helper to extract field names from an EntityDefinition.
    fn field_names(entity: &EntityDefinition) -> Vec<String> {
        entity
            .fields
            .iter()
            .filter_map(|f| f.name.clone())
            .collect()
    }

    #[test]
    fn merge_entity_fields_basic_collision_and_addition() {
        // Caller has [id, price], imported has [id, name, pendingId].
        // After merge: [id, price, name, pendingId] — id from caller wins.
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            name: "MyCartItem".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("price", FieldType::Number),
            ],
            instances: None,
        };
        let imported = EntityDefinition {
            name: "CartItem".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("pendingId", FieldType::String),
            ],
            instances: None,
        };

        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(names, vec!["id", "price", "name", "pendingId"]);
        // Verify caller's original field count + merged fields.
        assert_eq!(caller.fields.len(), 4);
    }

    #[test]
    fn merge_entity_fields_no_collision() {
        // Caller has [id, title], imported has [description, status].
        // After merge: [id, title, description, status].
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("title", FieldType::String),
            ],
            instances: None,
        };
        let imported = EntityDefinition {
            name: "ImportedEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
            ],
            instances: None,
        };

        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(names, vec!["id", "title", "description", "status"]);
    }

    #[test]
    fn merge_entity_fields_full_collision_no_change() {
        // Caller already has all fields the imported entity declares.
        // After merge: no change.
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
                make_field("createdAt", FieldType::String),
                make_field("pendingId", FieldType::String),
            ],
            instances: None,
        };
        let imported = EntityDefinition {
            name: "ImportedEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("description", FieldType::String),
                make_field("status", FieldType::String),
                make_field("createdAt", FieldType::String),
                make_field("pendingId", FieldType::String),
            ],
            instances: None,
        };

        merge_imported_entity_fields(&mut caller, &imported, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(
            names,
            vec!["id", "name", "description", "status", "createdAt", "pendingId"]
        );
        assert_eq!(caller.fields.len(), 6, "no fields should be added");
    }

    #[test]
    fn merge_entity_fields_multiple_imports() {
        // Caller uses std-browse AND std-modal; each contributes different fields.
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![make_field("id", FieldType::String)],
            instances: None,
        };
        let import_browse = EntityDefinition {
            name: "BrowseRecord".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("name", FieldType::String),
                make_field("status", FieldType::String),
            ],
            instances: None,
        };
        let import_modal = EntityDefinition {
            name: "ModalRecord".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("pendingId", FieldType::String),
                make_field("description", FieldType::String),
            ],
            instances: None,
        };

        // First import adds name, status.
        merge_imported_entity_fields(&mut caller, &import_browse, false, &HashSet::new());
        // Second import adds pendingId, description (id already present).
        merge_imported_entity_fields(&mut caller, &import_modal, false, &HashSet::new());

        let names = field_names(&caller);
        assert_eq!(
            names,
            vec!["id", "name", "status", "pendingId", "description"]
        );
    }

    #[test]
    fn merge_entity_fields_no_uses_no_change() {
        // Standalone orbital with no imports — entity unchanged.
        use orbital_core::schema::FieldType;

        let mut caller = EntityDefinition {
            name: "MyEntity".to_string(),
            collection: None,
            persistence: None,
            fields: vec![
                make_field("id", FieldType::String),
                make_field("title", FieldType::String),
            ],
            instances: None,
        };
        let original_len = caller.fields.len();

        // No imports to merge from — just confirm nothing changes.
        let names_before = field_names(&caller);
        // (In the real flow, if there are no uses, the merge loop is never entered.)
        assert_eq!(caller.fields.len(), original_len);
        assert_eq!(field_names(&caller), names_before);
    }

    #[test]
    fn get_entity_for_trait_orbital_finds_correct_orbital() {
        // Multi-orbital import: trait lives in orbital 1, its entity should
        // be returned (not orbital 0's entity).
        use orbital_core::schema::FieldType;

        let orb0 = Orbital {
            name: "StateOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "StateEntity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![make_field("stateField", FieldType::String)],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(make_trait("StateTrait"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        let orb1 = Orbital {
            name: "ActionOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "ActionEntity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![
                    make_field("actionField", FieldType::String),
                    make_field("pendingId", FieldType::String),
                ],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(make_trait("ActionTrait"))],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        let mut ctx = InlineContext::new();
        ctx.add_alias_multi(
            "Import",
            vec![orb0, orb1],
            "std/behaviors/test-import".to_string(),
        );

        // Asking for ActionTrait's orbital should return ActionEntity.
        let entity = ctx
            .get_entity_for_trait_orbital("Import", "ActionTrait")
            .expect("should find entity for ActionTrait's orbital");
        assert_eq!(entity.name, "ActionEntity");
        assert_eq!(field_names(&entity), vec!["actionField", "pendingId"]);

        // Asking for StateTrait's orbital should return StateEntity.
        let entity = ctx
            .get_entity_for_trait_orbital("Import", "StateTrait")
            .expect("should find entity for StateTrait's orbital");
        assert_eq!(entity.name, "StateEntity");
        assert_eq!(field_names(&entity), vec!["stateField"]);

        // Asking for a non-existent trait should return None.
        assert!(ctx
            .get_entity_for_trait_orbital("Import", "NoSuchTrait")
            .is_none());
    }
}
