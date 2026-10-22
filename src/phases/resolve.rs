//! Resolve Phase
//!
//! Resolves all references and produces the final OIR (Orbital Intermediate Representation).

use crate::phases::embed_graph::EmbedGraph;
use orbital_core_base::ir::{
    OirBindingRoot, OirCardinality, OirConfigFieldType, OirConstraint, OirEffect, OirEmit,
    OirEntity, OirEvent, OirEventScope, OirExpr, OirField, OirInterval, OirListener, OirMetadata,
    OirModule, OirPage, OirPattern, OirPayloadField, OirPersistence, OirRelation, OirSection,
    OirState, OirTick, OirTrait, OirTraitBinding, OirTraitSource, OirTransition, OirTransitionFrom,
    OirType, OirValue,
};
use orbital_core_base::schema::{
    Cardinality, EntityDefinition, EntityRef, FieldDefinition, FieldType, OrbitalSchema,
    PageDefinition, PageRef, SExpression, SectionDefinition, ServiceRef, StringOrArray, ThemeRef,
    TraitDefinition, TraitReference, TransitionDefinition,
};
use orbital_core_eval::lower::{ExprLowerer, LowerError};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

/// Resolution error
#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("Failed to resolve entity reference: {0}")]
    EntityNotFound(String),

    #[error("Failed to resolve trait reference: {0}")]
    TraitNotFound(String),

    #[error("Failed to resolve type: {0}")]
    InvalidType(String),

    #[error("Failed to resolve S-expression: {0}")]
    InvalidSExpr(String),

    #[error("Invalid tick interval '{0}': expected 'frame', a duration ('5ms'/'5s'/'1m'/'1h'), or a 5-field cron expression")]
    InvalidTickInterval(String),
}

/// A schema id map (unordered) as the IR's, keys sorted so the IR is deterministic.
fn sorted_id_map(map: &HashMap<String, String>) -> orbital_core_base::ir::OirIdMap {
    let mut entries: Vec<(String, String)> =
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    entries.sort();
    entries.into_iter().collect()
}

impl From<LowerError> for ResolveError {
    fn from(error: LowerError) -> Self {
        match error {
            LowerError::InvalidSExpr(message) => ResolveError::InvalidSExpr(message),
        }
    }
}

/// Resolve a schema to OIR
pub fn resolve_to_oir(schema: &OrbitalSchema) -> Result<OirModule, ResolveError> {
    let mut resolver = Resolver::new(schema);
    let mut module = resolver.resolve()?;
    let predictions = super::await_render::predict_await_renders(schema);
    if !predictions.is_empty() {
        for oir_trait in &mut module.traits {
            for (idx, transition) in oir_trait.transitions.iter_mut().enumerate() {
                if let Some(renders) = predictions.get(&(oir_trait.name.clone(), idx)) {
                    transition.await_render = renders.clone();
                }
            }
        }
    }
    Ok(module)
}

/// Lower a standalone expression through the one canonical lowering
/// (`orbital_core::lower`).
pub fn resolve_sexpr_standalone(sexpr: &SExpression) -> Result<OirExpr, ResolveError> {
    Ok(orbital_core_eval::lower::lower_sexpr(sexpr)?)
}

/// Substrate operators — async ops that reach outside the app via
/// `getSubstrateService()` (`llm/*`, `workspace/*`, `session/*`, `memory/*`,
/// `trace/*`, `behavior/catalog|describe|source`,
/// `program/read|print|eval`).
/// Valid only as effects (statement position) with an `emit: { success,
/// failure }` options object; `resolve_sexpr_call` rejects them in expression
/// (value) position. The effect-resolution arm routes them to
/// `OirEffect::Custom`. Delegates to the canonical predicate in
/// `orbital_core::ir::is_substrate_operator` — the single source of truth
/// shared with the TS and Python shells (closes C-EMIT-SUBSTRATE-OPS-MARKER).
pub(crate) fn is_substrate_op(name: &str) -> bool {
    orbital_core_base::ir::is_substrate_operator(name)
}

/// Resolver state
pub struct Resolver<'a> {
    schema: &'a OrbitalSchema,
    /// Named struct types discovered while resolving entity/payload fields —
    /// see `OirModule::named_types`'s doc for the registration rule. Interior
    /// mutability because every resolve method below is `&self` (recursive
    /// descent shares the same borrow); flushed into the module at the end
    /// of `resolve()`.
    named_types: RefCell<BTreeMap<String, OirType>>,
    /// Names currently being looked up in `schema.orbitals[].types` (Gap A)
    /// to build a `named_types` entry — guards MUTUAL recursion (`type A = {
    /// b : B }`, `type B = { a : A }`): computing A's shape by name-lookup
    /// recurses into B's shape, which itself contains a `TypeRef` back to A;
    /// without this, that recursion never terminates (A isn't in
    /// `named_types` yet — it's still being computed). A name found here
    /// mid-lookup is left as a bare `TypeRef` for THIS occurrence; the outer
    /// frame that's already computing it owns the eventual registration.
    named_types_in_progress: RefCell<HashSet<String>>,
    /// Every entity name declared by this schema (primary + auxiliary, across
    /// every orbital) — computed once so `payload_field_to_oir_type` can tell
    /// "this payload field's `entity` marker names a real declared entity"
    /// from "this is an out-of-scope/foreign name the flattener stamped
    /// defensively". Mirrors exactly the entities `resolve()` itself pushes
    /// into `OirModule.entities` (`get_entity_from_ref` on `orbital.entity` +
    /// `orbital.auxiliary_entities`), so the two never disagree.
    entity_names: HashSet<String>,
}

impl<'a> Resolver<'a> {
    fn new(schema: &'a OrbitalSchema) -> Self {
        Self {
            schema,
            named_types: RefCell::new(BTreeMap::new()),
            named_types_in_progress: RefCell::new(HashSet::new()),
            entity_names: declared_entity_names(schema),
        }
    }

    /// Borrow the four pieces `register_named_type_children` /
    /// `payload_field_to_oir_type` / `lookup_named_type_shape` need, bundled
    /// as one `NamedTypeCtx`.
    fn named_type_ctx(&self) -> NamedTypeCtx<'_> {
        NamedTypeCtx {
            schema: self.schema,
            named_types: &self.named_types,
            in_progress: &self.named_types_in_progress,
            entity_names: &self.entity_names,
        }
    }

    /// Get entity definition from EntityRef
    fn get_entity_from_ref<'b>(
        &'b self,
        entity_ref: &'a EntityRef,
    ) -> Option<&'a EntityDefinition> {
        match entity_ref {
            EntityRef::Inline(entity) => Some(entity),
            EntityRef::Reference(_ref_str) => {
                // Format: "Alias.entity" - for now, we don't support external loading in the resolver
                // This would be handled by a pre-processing step that loads external orbitals
                // and replaces references with inline definitions
                None
            }
            EntityRef::Extends(_) => {
                // Phase 3.5.G.A.5: `EntityRef::Extends` is rewritten to
                // `EntityRef::Inline` by `inline_entity` before the resolve
                // phase runs. If an Extends survives into the resolver
                // there is no upstream entity to inline against, so the
                // resolver returns None just like it does for `Reference`.
                None
            }
        }
    }

    /// Get page definition from PageRef
    fn get_page_from_ref<'b>(&'b self, page_ref: &'a PageRef) -> Option<&'a PageDefinition> {
        match page_ref {
            PageRef::Inline(page) => Some(page),
            PageRef::Reference(_) | PageRef::Object(_) => {
                // References would be resolved by pre-processing step
                None
            }
        }
    }

    fn resolve(&mut self) -> Result<OirModule, ResolveError> {
        let version = self
            .schema
            .version
            .clone()
            .unwrap_or_else(|| "1.0.0".to_string());
        let mut module = OirModule::new(&self.schema.name, version);
        module.locales = self.schema.locales.clone().unwrap_or_default();
        module.site = self.schema.site.clone();
        module.description = self.schema.description.clone();

        // Carry `expects` names into the IR so OIR-level existence checks can
        // tell a promised entity from an undefined one (see the field doc on
        // `OirModule::expected_entities`). Names only — never an `OirEntity`.
        for orbital in &self.schema.orbitals {
            let Some(expects) = &orbital.expects else {
                continue;
            };
            for expectation in expects {
                let name = match expectation {
                    orbital_core_base::schema::ExpectDeclaration::Entity { name, .. } => Some(name),
                    orbital_core_base::schema::ExpectDeclaration::Identity { name, .. } => {
                        name.as_ref()
                    }
                    _ => None,
                };
                if let Some(name) = name {
                    if !module.expected_entities.iter().any(|n| n == name) {
                        module.expected_entities.push(name.clone());
                    }
                }
            }
        }

        // Resolve entities from all orbitals (only inline entities)
        for orbital in &self.schema.orbitals {
            if let Some(entity) = self.get_entity_from_ref(&orbital.entity) {
                module.entities.push(self.resolve_entity(entity)?);
            }
            // Gap #22 — auxiliary entities are imported atom entities
            // surfaced when a trait reference omits the `-> Entity` rebind.
            // They get the same OirEntity treatment as the primary entity
            // so codegen emits their TypeScript types + Zod schemas and
            // runtime registers persistence for them.
            if let Some(aux_list) = &orbital.auxiliary_entities {
                for aux_ref in aux_list {
                    if let Some(aux_entity) = self.get_entity_from_ref(aux_ref) {
                        module.entities.push(self.resolve_entity(aux_entity)?);
                    }
                }
            }
            // Note: EntityRef::Reference would be resolved by pre-processing
        }

        // Access is a property of the DATA, so an entity that declares no
        // directive adopts the one declared on its `persistent:` collection —
        // otherwise a spliced-in atom entity sharing the organism's collection
        // emits no policy const and the generated server serves it ALLOW-ALL
        // (`R-SHADOW-ENTITY-SHARES-COLLECTION-DIRECTIVES-DONT-APPLY`).
        module.resolve_collection_policy_inheritance();

        // Resolve inline traits from orbitals. Stamp each with its
        // owning orbital name so codegen / runtime can build the
        // qualified `Orbital.Trait` bus scope (gap #13). Without this,
        // the IR's flat trait list erases orbital boundaries and bus
        // qualification can't disambiguate same-named traits across
        // orbitals.
        for orbital in &self.schema.orbitals {
            // Same embed graph `@config.<knob>` forwarding uses
            // (`inline/trait.rs`, `forwarded_sibling_config_from`) — one
            // walk, so "who embeds whom" can't disagree between the two
            // consumers. Built per-orbital since `@trait.X` references
            // never cross an orbital boundary.
            let embed_graph = EmbedGraph::build(&orbital.traits);
            for trait_ref in &orbital.traits {
                if let TraitReference::Inline(trait_def) = trait_ref {
                    let mut resolved = self.resolve_trait(trait_def)?;
                    resolved.orbital_name = orbital.name.clone();
                    for ev in &mut resolved.events {
                        if let Some(input) = ev.external.as_mut() {
                            input.orbital = orbital.name.clone();
                        }
                    }
                    resolved.host_trait = embed_graph
                        .direct_host(&trait_def.name)
                        .map(|s| s.to_string());
                    module.traits.push(resolved);
                }
            }
        }

        // Resolve pages from orbitals (only inline pages)
        for orbital in &self.schema.orbitals {
            if let Some(pages) = &orbital.pages {
                for page_ref in pages {
                    if let Some(page) = self.get_page_from_ref(page_ref) {
                        module.pages.push(self.resolve_page(page)?);
                    }
                    // Note: PageRef::Reference would be resolved by pre-processing
                }
            }
        }

        // Collect every theme the program declares — app-level first, then
        // each orbital's — deduped by name. An inline theme is carried as-is; a
        // preset key (`"clay-dark"`) carries the installed preset definition, so
        // the shells emit its CSS the same way they emit an inline theme's.
        let presets = orbital_core_base::theme::ThemePresets::installed();
        let declared = self
            .schema
            .theme
            .iter()
            .chain(self.schema.orbitals.iter().filter_map(|o| o.theme.as_ref()));
        let mut seen_themes = std::collections::HashSet::new();
        for theme_ref in declared {
            let (definition, key) = match theme_ref {
                ThemeRef::Inline(theme) => (Some(theme), format!("{}-light", theme.name)),
                ThemeRef::Reference(key) if key.is_empty() => continue,
                ThemeRef::Reference(key) => {
                    let base = key
                        .strip_suffix("-dark")
                        .or_else(|| key.strip_suffix("-light"))
                        .unwrap_or(key);
                    (presets.get(base), key.clone())
                }
            };
            if module.default_theme.is_none() {
                module.default_theme = Some(key);
            }
            if let Some(theme) = definition {
                if seen_themes.insert(theme.name.clone()) {
                    module.themes.push(theme.clone());
                }
            }
        }
        // The themes it switches to inside a page: their CSS ships too, but none is the default.
        for key in self.schema.themes.iter().flatten() {
            let base = key
                .strip_suffix("-dark")
                .or_else(|| key.strip_suffix("-light"))
                .unwrap_or(key);
            if let Some(theme) = presets.get(base) {
                if seen_themes.insert(theme.name.clone()) {
                    module.themes.push(theme.clone());
                }
            }
        }

        // Collect services from orbitals (only inline services, skip duplicates)
        let mut seen_services = std::collections::HashSet::new();
        for orbital in &self.schema.orbitals {
            if let Some(services) = &orbital.services {
                for service_ref in services {
                    if let ServiceRef::Inline(service) = service_ref {
                        let service_name = match service {
                            orbital_core_base::schema::ServiceDefinition::Rest(r) => r.name.clone(),
                            orbital_core_base::schema::ServiceDefinition::Socket(s) => {
                                s.name.clone()
                            }
                            orbital_core_base::schema::ServiceDefinition::Mcp(m) => m.name.clone(),
                        };
                        if seen_services.insert(service_name) {
                            module.services.push(service_ref.clone());
                        }
                    }
                    // Note: ServiceRef::Reference would be resolved by pre-processing
                }
            }
        }

        // Add metadata
        module.metadata = OirMetadata {
            generated_at: Some(chrono::Utc::now().to_rfc3339()),
            compiler_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            source_file: None,
            checksum: None,
        };

        // P3 — resolve render-resolved schema sigils (`@pages`/`@currentTheme`)
        // to concrete values sourced from the OrbitalSchema. These are
        // config-default-only bindings; they must not survive as bindings into
        // codegen (which has no schema context). `@pages` → `NavItem[]`
        // (app-wide); `@currentTheme` → the per-orbital `data-theme` key.
        //
        // `@pages` is APP-WIDE: an orbital is authored in isolation and
        // cannot know which sibling orbitals will be registered in the
        // composed app, so the nav is assembled from the union of root pages
        // (paths without `:` params — detail pages aren't nav entries) across
        // ALL orbitals, deduped by path. Only a page that declares its nav
        // entry (`@label`) is listed, in declaration order, with its `@icon`
        // and `@roles` when present.
        let page_entries: Vec<NavEntry> = {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut entries = Vec::new();
            for o in &self.schema.orbitals {
                if let Some(ps) = o.pages.as_ref() {
                    for pr in ps {
                        if let Some(p) = self.get_page_from_ref(pr) {
                            let Some(label) = p.label.clone() else {
                                continue;
                            };
                            if p.path.contains(':') || !seen.insert(p.path.clone()) {
                                continue;
                            }
                            entries.push(NavEntry {
                                href: p.path.clone(),
                                label,
                                icon: p.icon.clone(),
                                roles: p.roles.clone(),
                            });
                        }
                    }
                }
            }
            entries
        };
        for oir_trait in &mut module.traits {
            let theme_key = resolve_theme_key(
                self.schema
                    .orbitals
                    .iter()
                    .find(|o| o.name == oir_trait.orbital_name)
                    .and_then(|o| o.theme.as_ref()),
                self.schema.theme.as_ref(),
            );
            for trans in &mut oir_trait.transitions {
                for effect in &mut trans.effects {
                    let old = std::mem::replace(effect, OirExpr::Null);
                    *effect = substitute_schema_sigils(old, &page_entries, &theme_key);
                }
            }
        }

        if let Some(exports) = &self.schema.types {
            for (name, fields) in exports {
                self.named_types.borrow_mut().entry(name.clone())
                    .or_insert_with(|| orbital_core_base::ir::types::declared_alias_shape(fields));
            }
        }
        module.named_types = self.named_types.take();

        Ok(module)
    }
}

/// The baseline theme `@currentTheme` falls back to when neither the orbital
/// nor the app header declares a `theme` (precedence: orbital → app →
/// this default). Keeps standalone renders (no rabit) styled; rabit stamps
/// per-orbital `Orbital.theme`. A full `<base>-<variant>` registry key.
const DEFAULT_THEME_KEY: &str = "minimalist-light";

/// One `@pages` nav entry: a root page that declares `@label`.
struct NavEntry {
    href: String,
    label: String,
    icon: Option<String>,
    roles: Option<Vec<String>>,
}

/// Derive the `data-theme` selector-key string from an orbital's `Orbital.theme`.
/// `Reference(name)` → the name; `Inline(ThemeDefinition { name, .. })` → the
/// name; `None` → empty. The active-variant suffix (`-light`/`-dark`) is the
/// variant-axis open decision; for now the base name is the key.
fn theme_data_key(theme: &ThemeRef) -> String {
    match theme {
        ThemeRef::Reference(name) => name.clone(),
        ThemeRef::Inline(def) => def.name.clone(),
    }
}

/// The `@currentTheme` precedence chain: the orbital's own `theme` beats the
/// app-header `theme "<key>"`, which beats the baseline default.
fn resolve_theme_key(orbital_theme: Option<&ThemeRef>, app_theme: Option<&ThemeRef>) -> String {
    orbital_theme
        .map(theme_data_key)
        .filter(|k| !k.is_empty())
        .or_else(|| app_theme.map(theme_data_key).filter(|k| !k.is_empty()))
        .unwrap_or_else(|| DEFAULT_THEME_KEY.to_string())
}

/// Recursively substitute render-resolved schema sigils with their concrete
/// values. `@pages` → a `NavItem[]` array; `@currentTheme` → the `data-theme`
/// key string. Every other expression form is rebuilt with substituted
/// children so the sigils resolve anywhere they appear (including inside
/// conditional/computed overrides like `(if @user.prefersDark "x" @currentTheme)`).
fn substitute_schema_sigils(expr: OirExpr, pages: &[NavEntry], theme_key: &str) -> OirExpr {
    let sub = |e: OirExpr| substitute_schema_sigils(e, pages, theme_key);
    match expr {
        OirExpr::Binding { binding } => match binding.root {
            OirBindingRoot::Pages => OirExpr::array(
                pages
                    .iter()
                    .map(|entry| {
                        let mut fields = orbital_core_base::ir::OirFields::new();
                        fields.insert("href".to_string(), OirExpr::string(entry.href.clone()));
                        fields.insert("label".to_string(), OirExpr::string(entry.label.clone()));
                        if let Some(ic) = &entry.icon {
                            fields.insert("icon".to_string(), OirExpr::string(ic.clone()));
                        }
                        if let Some(roles) = &entry.roles {
                            fields.insert(
                                "roles".to_string(),
                                OirExpr::array(
                                    roles.iter().cloned().map(OirExpr::string).collect(),
                                ),
                            );
                        }
                        OirExpr::object(fields)
                    })
                    .collect(),
            ),
            OirBindingRoot::CurrentTheme => OirExpr::string(theme_key.to_string()),
            _ => OirExpr::binding(binding),
        },
        OirExpr::Array { items } => OirExpr::array(items.into_iter().map(sub).collect()),
        OirExpr::Object { fields } => {
            OirExpr::object(fields.into_iter().map(|(k, v)| (k, sub(v))).collect())
        }
        OirExpr::BinaryOp { op, left, right } => OirExpr::BinaryOp {
            op,
            left: Box::new(sub(*left)),
            right: Box::new(sub(*right)),
        },
        OirExpr::UnaryOp { op, operand } => OirExpr::UnaryOp {
            op,
            operand: Box::new(sub(*operand)),
        },
        OirExpr::Conditional {
            condition,
            then_branch,
            else_branch,
        } => OirExpr::Conditional {
            condition: Box::new(sub(*condition)),
            then_branch: Box::new(sub(*then_branch)),
            else_branch: Box::new(sub(*else_branch)),
        },
        OirExpr::Call { function, args } => OirExpr::Call {
            function,
            args: args.into_iter().map(sub).collect(),
        },
        OirExpr::Map {
            source,
            variable,
            body,
        } => OirExpr::Map {
            source: Box::new(sub(*source)),
            variable,
            body: Box::new(sub(*body)),
        },
        OirExpr::Filter {
            source,
            variable,
            predicate,
        } => OirExpr::Filter {
            source: Box::new(sub(*source)),
            variable,
            predicate: Box::new(sub(*predicate)),
        },
        OirExpr::Reduce {
            source,
            variable,
            accumulator,
            initial,
            body,
        } => OirExpr::Reduce {
            source: Box::new(sub(*source)),
            variable,
            accumulator,
            initial: Box::new(sub(*initial)),
            body: Box::new(sub(*body)),
        },
        OirExpr::Lambda { params, body } => OirExpr::Lambda {
            params,
            body: Box::new(sub(*body)),
        },
        OirExpr::Effect { effect } => {
            // render-ui carries its props (navItems, data-theme, …) as
            // `OirValue`s inside `OirPattern.config`; substitute there.
            let mut eff = *effect;
            if let OirEffect::RenderUi { pattern, .. } = &mut eff {
                for v in pattern.config.values_mut() {
                    *v = substitute_value_sigils(v.clone(), pages, theme_key);
                }
            }
            OirExpr::Effect {
                effect: Box::new(eff),
            }
        }
        // Literals — pass through.
        other => other,
    }
}

/// The `OirValue` twin of `substitute_schema_sigils` — render-ui props are
/// stored as `OirValue`s, so `@pages`/`@currentTheme` appearing there need
/// their own recursive substitution.
fn substitute_value_sigils(value: OirValue, pages: &[NavEntry], theme_key: &str) -> OirValue {
    match value {
        OirValue::String(s) if s == "@pages" => OirValue::Array(
            pages
                .iter()
                .map(|entry| {
                    let mut fields: Vec<(String, OirValue)> = vec![
                        ("href".to_string(), OirValue::String(entry.href.clone())),
                        ("label".to_string(), OirValue::String(entry.label.clone())),
                    ];
                    if let Some(ic) = &entry.icon {
                        fields.push(("icon".to_string(), OirValue::String(ic.clone())));
                    }
                    if let Some(roles) = &entry.roles {
                        fields.push((
                            "roles".to_string(),
                            OirValue::Array(roles.iter().cloned().map(OirValue::String).collect()),
                        ));
                    }
                    OirValue::Object(fields.into_iter().collect())
                })
                .collect(),
        ),
        OirValue::String(s) if s == "@currentTheme" => OirValue::String(theme_key.to_string()),
        OirValue::Array(items) => OirValue::Array(
            items
                .into_iter()
                .map(|v| substitute_value_sigils(v, pages, theme_key))
                .collect(),
        ),
        OirValue::Object(fields) => OirValue::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k, substitute_value_sigils(v, pages, theme_key)))
                .collect(),
        ),
        other => other,
    }
}

impl<'a> Resolver<'a> {
    fn resolve_entity(&self, entity: &EntityDefinition) -> Result<OirEntity, ResolveError> {
        let mut oir_entity = OirEntity::new(&entity.name);
        oir_entity.id = entity.id.clone();

        if let Some(collection) = &entity.collection {
            oir_entity.collection = collection.clone();
        }

        if let Some(persistence) = &entity.persistence {
            oir_entity.persistence = match persistence {
                orbital_core_base::schema::Persistence::Persistent => OirPersistence::Persistent,
                orbital_core_base::schema::Persistence::Runtime => OirPersistence::Runtime,
            };
        }

        oir_entity.shared = entity.shared;
        oir_entity.identity = entity.identity;
        oir_entity.local = entity.local;
        oir_entity.seed_mock = entity.seed_mock;

        oir_entity.read_policy = entity
            .read_policy
            .as_ref()
            .map(|e| self.resolve_sexpr(e))
            .transpose()?;
        oir_entity.create_policy = entity
            .create_policy
            .as_ref()
            .map(|e| self.resolve_sexpr(e))
            .transpose()?;
        oir_entity.update_policy = entity
            .update_policy
            .as_ref()
            .map(|e| self.resolve_sexpr(e))
            .transpose()?;
        oir_entity.delete_policy = entity
            .delete_policy
            .as_ref()
            .map(|e| self.resolve_sexpr(e))
            .transpose()?;

        for field in &entity.fields {
            oir_entity.fields.push(self.resolve_field(field)?);
        }

        // Copy pre-authored instances, converting SExpression values to OirValue
        if let Some(instances) = &entity.instances {
            for instance in instances {
                let mut oir_instance = orbital_core_base::ir::OirObject::new();
                for (key, value) in instance {
                    oir_instance.insert(key.clone(), self.sexpr_to_value(value));
                }
                oir_entity.instances.push(oir_instance);
            }
        }

        Ok(oir_entity)
    }

    fn resolve_field(&self, field: &FieldDefinition) -> Result<OirField, ResolveError> {
        let field_type = self.resolve_field_shape(field)?;
        let field_name = field.name.clone().unwrap_or_default();

        // `None` means the `.lolo` field carried no `!` — lowering
        // (`orbital-lolo/src/lower/entity.rs`) already documents the
        // contract for the object-type-field sibling of this same lowering:
        // "`name : T!` lowers to `required: Some(true)`, unmarked stays
        // `Some(false)` so codegen emits optional-by-default TS fields
        // matching lolo authoring semantics." The entity-field lowering
        // (same file, entity body) instead leaves unmarked fields `None`,
        // and this `unwrap_or` was defaulting that `None` to REQUIRED —
        // the opposite of the documented contract, and inconsistent with
        // this crate's own `resolve_emit` (payload fields: `unwrap_or(false)`).
        // Net effect measured on the corpus: an unmarked (non-`!`) entity
        // field NEVER emitted as TS-optional (`field?:`) — 0 of 203 fields
        // across std-iot-dashboard's 20 entities, verified before this fix —
        // so an honest `(set @entity.X @config.Y)` with no resolvable value
        // (ORB_X_SET_REQUIRED_FIELD_FROM_OPTIONAL_KNOB's target class) hit a
        // bogus TS2352 on every field the author never marked required.
        let required = field.required.unwrap_or(false);
        let mut oir_field = if required {
            OirField::new(&field_name, field_type)
        } else {
            OirField::optional(&field_name, field_type)
        };

        oir_field.intrinsic = field.intrinsic.unwrap_or(false);
        oir_field.merged_from = field.merged_from.clone();
        oir_field.declared = serde_json::to_value(field).ok();

        // Add enum values
        oir_field.enum_values = field.values.clone();

        // Carry the field's @description into the OIR — codegen turns it
        // into the form input's help text (`hint`), the compiled twin of
        // the runtime path's enrichFormFields injection.
        oir_field.description = field.description.clone();

        // Attach the constraint the declared domain implies. `OirConstraint::{Email,
        // Url,Uuid}` and the zod `.email()`/`.url()`/`.uuid()` emitter were both
        // already built and never fed — nothing ever populated `constraints`, and a
        // dead `let _ = OirConstraint::Email` keepalive existed to prove it. This is
        // the line that lights the pipeline up.
        match field.field_type {
            FieldType::Email => oir_field.constraints.push(OirConstraint::Email),
            FieldType::Url | FieldType::Image => oir_field.constraints.push(OirConstraint::Url),
            FieldType::Uuid => oir_field.constraints.push(OirConstraint::Uuid),
            FieldType::Phone => oir_field.constraints.push(OirConstraint::Phone),
            _ => {}
        }

        // Add relation
        if let Some(relation) = &field.relation {
            oir_field.relation = Some(OirRelation {
                target_entity: relation.entity.clone(),
                target_entity_id: relation.entity_id.clone(),
                cardinality: relation
                    .cardinality
                    .as_ref()
                    .map(|c| match c {
                        Cardinality::One => OirCardinality::ManyToOne,
                        Cardinality::Many => OirCardinality::OneToMany,
                        Cardinality::OneToMany => OirCardinality::OneToMany,
                        Cardinality::ManyToOne => OirCardinality::ManyToOne,
                        Cardinality::ManyToMany => OirCardinality::ManyToMany,
                    })
                    .unwrap_or(OirCardinality::ManyToOne),
                foreign_key: None,
            });
        }

        // Resolve default value
        if let Some(default) = &field.default {
            oir_field.default = Some(self.sexpr_to_value(default));
        }

        Ok(oir_field)
    }

    /// Reads `field.items`/`field.properties` so arrays-of-structs and inline
    /// objects keep their shape in OIR (`OirType::Struct { fields }`).
    fn resolve_field_shape(&self, field: &FieldDefinition) -> Result<OirType, ResolveError> {
        let shape = self.resolve_field_shape_inner(field)?;
        Ok(if field.nullable == Some(true) {
            OirType::optional(shape)
        } else {
            shape
        })
    }

    fn resolve_field_shape_inner(&self, field: &FieldDefinition) -> Result<OirType, ResolveError> {
        if field.field_type == FieldType::Tuple {
            let items = field
                .tuple_slots()
                .into_iter()
                .map(|slot| self.resolve_field_shape(slot))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(OirType::Tuple { items });
        }
        match &field.field_type {
            // Tagged union: `values` holds the variant names in DECLARATION
            // order, `properties` their shapes. Iterating `values` (not the
            // HashMap) is what keeps variant order stable across runs, which
            // codegen output depends on. A by-name reference
            // (`FieldDefinition::by_name_reference`: the recursive
            // back-reference `lower/entity.rs`'s RESOLVING cycle guard leaves
            // where a self-referential alias would expand forever) resolves
            // to a `TypeRef` naming the type, so a backend with a named-type
            // facility (the TS shell) emits a real recursive reference.
            FieldType::Union => {
                if let Some(name) = field.by_name_reference() {
                    return Ok(OirType::TypeRef {
                        name: name.to_string(),
                    });
                }
                let (Some(names), Some(props)) = (&field.values, &field.properties) else {
                    return Err(ResolveError::InvalidType(
                        "a union type declares its variants (values + properties) or names one type".to_string(),
                    ));
                };
                let mut variants: Vec<OirField> = Vec::new();
                for name in names {
                    let Some(def) = props.get(name) else { continue };
                    let mut v = self.resolve_field(def)?;
                    if v.name.is_empty() {
                        v.name = name.clone();
                    }
                    // A member is never optional: the value IS one member or another.
                    if def.nullable != Some(true) || !v.required {
                        if let OirType::Optional { inner } = &v.field_type {
                            v.field_type = (**inner).clone();
                        }
                    }
                    v.required = true;
                    variants.push(v);
                }
                if variants.is_empty() {
                    return Err(ResolveError::InvalidType(
                        "a union type names no variant that has a shape".to_string(),
                    ));
                }
                Ok(OirType::Union { variants })
            }
            // Recurses via `self` (not the free `field_shape`): an item that
            // is Object-with-`properties` or a Union back-reference needs
            // `&self` to fully expand.
            FieldType::Array => {
                let inner = if let Some(items) = &field.items {
                    self.resolve_field_shape(items)?
                } else {
                    OirType::Json
                };
                Ok(OirType::array(inner))
            }
            FieldType::Object => {
                if field.properties.is_some() || field.additional_properties.is_some() {
                    let mut struct_fields: Vec<OirField> = Vec::new();
                    for (name, def) in field.properties.iter().flatten() {
                        let mut nested = self.resolve_field(def)?;
                        if nested.name.is_empty() {
                            nested.name = name.clone();
                        }
                        struct_fields.push(nested);
                    }
                    struct_fields.sort_by(|a, b| a.name.cmp(&b.name));
                    // Named-type registration — see `register_named_type_children`'s doc.
                    register_named_type_children(&struct_fields, &self.named_type_ctx());
                    Ok(OirType::Struct {
                        fields: struct_fields,
                        additional: field.additional_properties.as_deref()
                            .map(|value| self.resolve_field_shape(value).map(Box::new))
                            .transpose()?,
                    })
                } else if let Some(items) = &field.items {
                    // A `Map K V` field nested inside a struct/alias lowers to
                    // `FieldType::Object` with the VALUE shape carried in `items`
                    // (no `properties`) — the same carrier the entity lowerer
                    // uses for maps. Emit `Record<string, V>`, not `Json`.
                    // (Top-level config-field maps are handled string-side in
                    // `resolve_config_field_shape`; this covers maps reached via
                    // a struct property / array element / map value.)
                    Ok(OirType::map(
                        OirType::String,
                        self.resolve_field_shape(items)?,
                    ))
                } else {
                    Ok(OirType::Json)
                }
            }
            FieldType::Relation => match &field.relation {
                Some(_) => Ok(orbital_core_base::ir::types::field_shape(field)),
                None => Err(ResolveError::InvalidType(
                    "Relation type requires relation definition".to_string(),
                )),
            },
            _ => Ok(orbital_core_base::ir::types::field_shape(field)),
        }
    }

    /// Resolve a `ConfigField` (trait config slot) into a structurally-rich
    /// `OirType`. Prefers the new `items` / `properties` carriers populated
    /// by the lolo lowerer for named-type slots (e.g. `metrics : [MetricSpec]`
    /// — `items` carries the MetricSpec field-by-field shape). Falls back to
    /// the string-based `type_string_to_oir` only when neither carrier is
    /// present (legacy primitives like `"string"`, `"number"`, `"boolean"`,
    /// closed-string enums where `values` already encodes the union).
    ///
    /// Mirrors `resolve_field_shape` on `FieldDefinition` — same recursion
    /// rules so a typed config slot's TS shape parallels what entity fields
    /// emit.
    fn resolve_config_field_shape(
        &self,
        field: &orbital_core_base::schema::ConfigField,
    ) -> OirType {
        let shape = self.resolve_config_field_shape_inner(field);
        if field.nullable == Some(true) {
            OirType::optional(shape)
        } else {
            shape
        }
    }

    fn resolve_config_field_shape_inner(
        &self,
        field: &orbital_core_base::schema::ConfigField,
    ) -> OirType {
        // Tuple slot: `[T1, T2, …]` with each slot's shape riding `properties` by index.
        if orbital_core_base::type_compat::tuple_type_slots(&field.field_type).is_some() {
            let slots = orbital_core_base::schema::tuple_slots_of(field.properties.as_ref());
            if !slots.is_empty() {
                return OirType::Tuple {
                    items: slots
                        .into_iter()
                        .map(|slot| self.resolve_field_shape(slot).unwrap_or(OirType::Json))
                        .collect(),
                };
            }
            return self.type_string_to_oir(&field.field_type);
        }
        // Map slot: `params : Map string SimParam` lowers to
        // `field_type: "Map<string,SimParam>"` plus `items: FieldDefinition`
        // carrying the VALUE's resolved shape (reusing the array `items`
        // carrier; the `Map<…>` field-type string disambiguates from arrays).
        // Inline the value struct so the emitted `Record<string, …>` is
        // self-contained (no undefined alias name). Key is always `string`.
        if let Some(inner) = field
            .field_type
            .trim_start()
            .strip_prefix("Map<")
            .and_then(|s| s.strip_suffix('>'))
        {
            // The `items` FieldDefinition carrier holds the fully-resolved,
            // inlined value shape (named structs inlined, arrays preserved) and
            // is authoritative for every value type EXCEPT the `SExpr` brand:
            // entity.rs lowers `SExpr` lossily to `FieldType::Object`, so the
            // carrier would degrade it to `Json`. For a bare `SExpr` value, the
            // field-type string is authoritative — emit the real `SExpr` brand.
            let value_str = inner.splitn(2, ',').nth(1).map(str::trim).unwrap_or("");
            let value = if value_str.eq_ignore_ascii_case("SExpr") {
                OirType::SExpr
            } else {
                field
                    .items
                    .as_ref()
                    .map(|it| self.resolve_field_shape(it).unwrap_or(OirType::Json))
                    .unwrap_or(OirType::Json)
            };
            return OirType::map(OirType::String, value);
        }
        // Array slot with a typed element shape: `metrics : [MetricSpec]`
        // lowers to `field_type: "[MetricSpec]"` plus `items: FieldDefinition`
        // carrying MetricSpec's fields. Walk items via the existing
        // FieldDefinition-shaped resolver.
        if let Some(items) = &field.items {
            let inner = self.resolve_field_shape(items).unwrap_or(OirType::Json);
            return OirType::array(inner);
        }
        // Object slot with inline / named struct: `prefs : { theme: string }`
        // lowers to `field_type: "object"` plus `properties: {...}`. Walk each
        // property via the existing resolver.
        if field.properties.is_some() || field.additional_properties.is_some() {
            let mut struct_fields: Vec<OirField> = Vec::new();
            for (name, def) in field.properties.iter().flatten() {
                let nested = match self.resolve_field(def) {
                    Ok(mut f) => {
                        if f.name.is_empty() {
                            f.name = name.clone();
                        }
                        f
                    }
                    Err(_) => OirField::new(name, OirType::Json),
                };
                struct_fields.push(nested);
            }
            struct_fields.sort_by(|a, b| a.name.cmp(&b.name));
            return OirType::Struct {
                fields: struct_fields,
                additional: field.additional_properties.as_deref()
                    .map(orbital_core_base::ir::types::field_shape)
                    .map(Box::new),
            };
        }
        // No structural carrier — primitives / closed-string enums.
        self.type_string_to_oir(&field.field_type)
    }

    /// Convert a string type name to OirType (for payload fields and
    /// declared config schema). Handles array form `T[]` by recursing on
    /// the inner base type, and `[T]` form (lolo array notation).
    fn type_string_to_oir(&self, type_str: &str) -> OirType {
        let trimmed = type_str.trim();
        if let Some(slots) = orbital_core_base::type_compat::tuple_type_slots(trimmed) {
            return OirType::Tuple {
                items: slots
                    .into_iter()
                    .map(|t| self.type_string_to_oir(t))
                    .collect(),
            };
        }
        if let Some(base) = trimmed.strip_suffix("[]") {
            return OirType::array(self.type_string_to_oir(base));
        }
        if let Some(stripped) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            return OirType::array(self.type_string_to_oir(stripped));
        }
        // `Map<K,V>` config-knob type. Delegate to the free
        // `parse_payload_field_type`, which (unlike this method's `_ => String`
        // default) resolves a Named value type to `Reference` — so the value
        // emits as `Record<string, SimParam>`, not `Record<string, string>`.
        if trimmed.starts_with("Map<") {
            return parse_payload_field_type(trimmed);
        }
        match trimmed.to_lowercase().as_str() {
            "string" => OirType::String,
            // Keep the numeric/collection arms converged with
            // `parse_payload_field_type` — a `float` config knob emitted
            // `string` when this arm lacked the aliases (TS2352 on the
            // DEFAULT_CONFIG cast).
            "number" | "float" | "double" => OirType::Number,
            "integer" | "int" => OirType::Integer,
            "boolean" | "bool" => OirType::Boolean,
            "date" => OirType::Date,
            "datetime" | "timestamp" => OirType::DateTime,
            "array" | "list" => OirType::array(OirType::Json),
            "object" | "json" | "tensor" => OirType::Json,
            // Renderable content — keeps its tag so codegen emits the
            // canonical pattern-tree type (type-integrity campaign).
            "node" => OirType::Node,
            // Layer 3 — render-ui SExpr tree carried as a config knob value.
            // Lowered to Json so codegen emits a JSON-shaped TS type
            // (the variant author supplies any tree; runtime substitutes).
            "render-ui" => OirType::Json,
            // `SExpr` brand config knob — emits the recursive `SExpr` TS type.
            "sexpr" => OirType::SExpr,
            _ => OirType::String, // Default to string for unknown types
        }
    }

    fn resolve_trait(&self, trait_def: &TraitDefinition) -> Result<OirTrait, ResolveError> {
        let mut oir_trait = OirTrait::new(&trait_def.name);
        oir_trait.id = trait_def.id.clone();
        oir_trait.description = trait_def.description.clone();
        oir_trait.linked_entity = trait_def.linked_entity.clone();
        oir_trait.linked_entity_id = trait_def.linked_entity_id.clone();
        oir_trait.entity_ref_ids = trait_def.entity_ref_ids.as_ref().map(sorted_id_map);
        oir_trait.trait_embed_ids = trait_def.trait_embed_ids.as_ref().map(sorted_id_map);
        oir_trait.source = OirTraitSource::Schema;
        oir_trait.local = trait_def.local;

        if let Some(category) = &trait_def.category {
            oir_trait.category = Some(format!("{:?}", category).to_lowercase());
        }

        // Find initial state from states with is_initial = true
        let sm = &trait_def.state_machine;
        let initial_state_name = sm
            .states
            .iter()
            .find(|s| s.is_initial == Some(true))
            .map(|s| &s.name)
            .or_else(|| sm.states.first().map(|s| &s.name));

        for state in &sm.states {
            let is_initial = initial_state_name == Some(&state.name);
            let mut oir_state = if is_initial {
                OirState::initial(&state.name)
            } else {
                OirState::new(&state.name)
            };
            oir_state.time_bound_ms = state.time_bound_ms;
            oir_trait.states.push(oir_state);
        }

        // Resolve events from state machine
        if let Some(events) = &sm.events {
            for event in events {
                // EventDefinition has key and name (optional)
                let event_key = &event.key;
                // A config-forward event (`@config.<knob> -> external { ... }`)
                // resolves its key to the knob's effective value earlier in the
                // inline phase (`resolve_config_ref_emit_names`). An unset knob
                // legitimately resolves to its declared default — which can be
                // `""` (the "this optional event slot is unfulfilled" contract,
                // e.g. `extraAddonEvent : string = ""`). Such a slot has no
                // event to carry, so it must not become an OirEvent at all —
                // every codegen backend would otherwise emit an empty-name row.
                if event_key.is_empty() {
                    continue;
                }
                let mut oir_event = OirEvent::new(event_key);
                oir_event.description = event.name.clone();
                // The owning orbital is stamped with `orbital_name` after this returns.
                oir_event.external = orbital_core_base::external_inputs::external_input_of(
                    "",
                    &trait_def.name,
                    event,
                );

                // Populate payload_schema from event's payload field.
                // `field_type` is the string produced by the lolo lowerer
                // (`type_expr_to_string`). It can be a primitive keyword
                // ("string", "number"), the `"object"` / `"json"` fallback,
                // or a structural expression:
                //   - `"[X]"` → array of X (X is usually an entity name,
                //     which we represent as `OirType::Reference`).
                //   - `"X"` (any PascalCase identifier that isn't a primitive)
                //     → reference to entity X.
                // Preserving the reference is what lets `data : [CartItem]`
                // reach the TypeScript codegen as `data: CartItem[]` instead
                // of collapsing to `EventPayload`.
                if let Some(payload_fields) = &event.payload_schema {
                    let mut schema = orbital_core_base::ir::OirTypeMap::new();
                    let mut required = std::collections::BTreeSet::new();
                    for field in payload_fields {
                        let oir_type = payload_field_to_oir_type(field, &self.named_type_ctx());
                        schema.insert(field.name.clone(), oir_type);
                        if field.required.unwrap_or(false) {
                            required.insert(field.name.clone());
                        }
                    }
                    oir_event.payload_schema = Some(schema);
                    oir_event.payload_required_fields = required;
                }

                // `sm.events` can carry the same key twice (a bare machine
                // event plus a payload-schema'd declaration). One OirEvent per
                // key — the payload-typed entry wins — or every downstream
                // keyed emission (payload maps, action creators, useUIEvents
                // lists) duplicates the key (TS1117/TS2300).
                match oir_trait.events.iter_mut().find(|e| e.key == oir_event.key) {
                    Some(existing) => {
                        let external = oir_event
                            .external
                            .clone()
                            .or_else(|| existing.external.take());
                        if existing.payload_schema.is_none() && oir_event.payload_schema.is_some() {
                            *existing = oir_event;
                        }
                        existing.external = external;
                    }
                    None => oir_trait.events.push(oir_event),
                }
            }
        }

        // Also add events from transitions if not already added
        for trans in &sm.transitions {
            if trans.event.is_empty() {
                continue;
            }
            if !oir_trait.events.iter().any(|e| e.key == trans.event) {
                oir_trait.events.push(OirEvent::new(&trans.event));
            }
        }

        // Resolve transitions
        for trans in &sm.transitions {
            oir_trait.transitions.push(self.resolve_transition(trans)?);
        }

        // Collect slots used from all transitions' render-ui effects
        let mut slots_used: std::collections::HashSet<String> = std::collections::HashSet::new();
        for trans in &oir_trait.transitions {
            for effect in &trans.effects {
                Self::collect_slots_from_effect(effect, &mut slots_used);
            }
        }
        oir_trait.slots_used = slots_used.into_iter().collect();

        // Resolve ticks
        if let Some(ticks) = &trait_def.ticks {
            for tick in ticks {
                oir_trait.ticks.push(self.resolve_tick(tick)?);
            }
        }

        // Resolve listeners
        if let Some(listens) = &trait_def.listens {
            for listen in listens {
                // triggers defaults to the event name if not specified
                let triggers = listen.triggers.as_deref().unwrap_or(&listen.event);
                let mut oir = OirListener::new(&listen.event, triggers);
                // Map the structured ListenSource into the OIR's source_* fields
                // so the codegen can emit source-filtered `eventBus.on(…)` hooks.
                if let Some(src) = &listen.source {
                    match src {
                        orbital_core_base::schema::ListenSource::Any => {
                            oir.source_any = true;
                        }
                        orbital_core_base::schema::ListenSource::Trait {
                            trait_name,
                            trait_id,
                        } => {
                            oir.source_trait = Some(trait_name.clone());
                            oir.source_trait_id = trait_id.clone();
                        }
                        orbital_core_base::schema::ListenSource::Orbital {
                            orbital,
                            orbital_id,
                            trait_name,
                            trait_id,
                        } => {
                            oir.source_orbital = Some(orbital.clone());
                            oir.source_orbital_id = orbital_id.clone();
                            oir.source_trait = Some(trait_name.clone());
                            oir.source_trait_id = trait_id.clone();
                        }
                    }
                }
                // Carry the listen refinement surface (`when` guard + `with{}`
                // projection) into OIR — dropping it here silently stripped it
                // from every compiled shell (C-OIR-LISTENER-DROPS-WITH-AND-GUARD).
                if let Some(guard) = &listen.guard {
                    oir.guard = Some(self.resolve_sexpr(guard)?);
                }
                if let Some(mapping) = &listen.payload_mapping {
                    let mut resolved = orbital_core_base::ir::OirFields::new();
                    for (field, expr) in mapping {
                        resolved.insert(field.clone(), self.resolve_sexpr(expr)?);
                    }
                    oir.payload_mapping = Some(resolved);
                }
                oir_trait.listeners.push(oir);
            }
        }

        // Resolve emits (events this trait can emit to other traits)
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                oir_trait.emits.push(self.resolve_emit(emit)?);
            }
        }

        // Phase VG8-C: capture the atom's declared config schema so the
        // TypeScript backend can emit a strongly-typed `<TraitName>Config`
        // interface. Each ConfigField.field_type is mapped via the same
        // rules as payload fields.
        //
        // G24-followup (Solution 1): also capture per-field default values
        // so the codegen can emit a `DEFAULT_CONFIG` constant in the trait
        // logic hook. This makes atoms render correctly when verified
        // standalone (no consumer call site) — the defaults declared in the
        // .lolo's `config { ... }` block flow through to the runtime.
        if let Some(cfg) = &trait_def.config {
            if !cfg.is_empty() {
                let mut schema = orbital_core_base::ir::OirConfigSchema::new();
                let mut defaults = orbital_core_base::ir::OirObject::new();
                for (key, field) in cfg {
                    schema.insert(
                        key.clone(),
                        OirConfigFieldType {
                            field_type: self.resolve_config_field_shape(field),
                            values: field.values.clone(),
                        },
                    );
                    if let Some(default_expr) = &field.default {
                        defaults.insert(key.clone(), self.sexpr_to_value(default_expr));
                    }
                }
                oir_trait.declared_config_schema = Some(schema);
                if !defaults.is_empty() {
                    oir_trait.declared_config_defaults = Some(defaults);
                }
            }
        }

        Ok(oir_trait)
    }

    fn resolve_emit(
        &self,
        emit: &orbital_core_base::schema::EmitDefinition,
    ) -> Result<OirEmit, ResolveError> {
        let mut oir_emit = OirEmit::new(&emit.event);
        oir_emit.event_id = emit.event_id.clone();
        oir_emit.description = emit.description.clone();

        // Convert scope
        if let Some(scope) = &emit.scope {
            oir_emit.scope = match scope {
                orbital_core_base::schema::EventScope::Internal => OirEventScope::Internal,
                orbital_core_base::schema::EventScope::External => OirEventScope::External,
            };
        }

        // Convert payload fields
        if let Some(payload) = &emit.payload_schema {
            for field in payload {
                oir_emit.payload.push(OirPayloadField {
                    name: field.name.clone(),
                    field_type: self.type_string_to_oir(&field.field_type),
                    required: field.required.unwrap_or(false),
                    description: field.description.clone(),
                });
            }
        }

        Ok(oir_emit)
    }

    fn resolve_transition(
        &self,
        trans: &TransitionDefinition,
    ) -> Result<OirTransition, ResolveError> {
        let from = match &trans.from {
            StringOrArray::Single(s) if s == "*" => OirTransitionFrom::Any,
            StringOrArray::Single(s) => OirTransitionFrom::Single(s.clone()),
            StringOrArray::Multiple(states) => OirTransitionFrom::Multiple(states.clone()),
        };

        let mut oir_trans = OirTransition {
            from,
            to: trans.to.clone(),
            event: trans.event.clone(),
            guard: None,
            effects: vec![],
            await_render: vec![],
        };

        // Resolve guard
        if let Some(guard) = &trans.guard {
            oir_trans.guard = Some(self.resolve_sexpr(guard)?);
        }

        // Resolve effects
        if let Some(effects) = &trans.effects {
            for effect in effects {
                oir_trans.effects.push(self.resolve_sexpr(effect)?);
            }
        }

        Ok(oir_trans)
    }

    fn resolve_tick(
        &self,
        tick: &orbital_core_base::schema::TickDefinition,
    ) -> Result<OirTick, ResolveError> {
        let interval = match &tick.interval {
            orbital_core_base::schema::StringOrNumber::String(s) => {
                // Order matters: check "ms" before "s" (a suffix of "ms").
                if s == "frame" {
                    OirInterval::Frame
                } else if let Some(digits) = s.strip_suffix("ms") {
                    let ms: u64 = digits
                        .parse()
                        .map_err(|_| ResolveError::InvalidTickInterval(s.clone()))?;
                    OirInterval::Milliseconds { value: ms }
                } else if let Some(digits) = s.strip_suffix("s") {
                    let value: u64 = digits
                        .parse()
                        .map_err(|_| ResolveError::InvalidTickInterval(s.clone()))?;
                    OirInterval::Seconds { value }
                } else if let Some(digits) = s.strip_suffix("m") {
                    let value: u64 = digits
                        .parse()
                        .map_err(|_| ResolveError::InvalidTickInterval(s.clone()))?;
                    OirInterval::Minutes { value }
                } else if let Some(digits) = s.strip_suffix("h") {
                    let hours: u64 = digits
                        .parse()
                        .map_err(|_| ResolveError::InvalidTickInterval(s.clone()))?;
                    OirInterval::Milliseconds {
                        value: hours * 60 * 60 * 1000,
                    }
                } else if let Some(digits) = s.strip_suffix("d") {
                    let days: u64 = digits
                        .parse()
                        .map_err(|_| ResolveError::InvalidTickInterval(s.clone()))?;
                    OirInterval::Milliseconds {
                        value: days * 24 * 60 * 60 * 1000,
                    }
                } else if orbital_core_base::cron::is_valid_cron_expression(s) {
                    // A real 5-field cron expression (e.g. "0 9 * * *") —
                    // previously silently collapsed to `Seconds { value: 1 }`.
                    OirInterval::Cron {
                        expression: s.clone(),
                    }
                } else {
                    return Err(ResolveError::InvalidTickInterval(s.clone()));
                }
            }
            orbital_core_base::schema::StringOrNumber::Number(n) => {
                OirInterval::Milliseconds { value: *n as u64 }
            }
        };

        let mut oir_tick = OirTick::new(&tick.name, interval);
        oir_tick.background = tick.background;
        oir_tick.while_visible = tick.while_visible;
        oir_tick.applies_to = tick.applies_to.clone().unwrap_or_default();

        if let Some(guard) = &tick.guard {
            oir_tick.guard = Some(self.resolve_sexpr(guard)?);
        }

        // effects is Vec<SExpression>, not Option<Vec>
        for effect in &tick.effects {
            oir_tick.effects.push(self.resolve_sexpr(effect)?);
        }

        Ok(oir_tick)
    }

    fn resolve_page(&self, page: &PageDefinition) -> Result<OirPage, ResolveError> {
        let mut oir_page = OirPage::new(&page.name, &page.path);
        oir_page.id = page.id.clone();
        oir_page.label = page.label.clone();
        oir_page.access = page.access;
        oir_page.indexing = page.indexing;
        oir_page.source_page = page.source_page.clone();
        oir_page.translation_of = page.translation_of.clone();
        oir_page.title = page.title.clone();
        oir_page.description = page.description.clone();

        // Resolve sections if present
        if let Some(sections) = &page.sections {
            for section in sections {
                oir_page.sections.push(self.resolve_section(section)?);
            }
        }

        // Resolve trait bindings. Prefer the CURRENT name of the trait the
        // ref_id points at over the page ref's own `reference` name: after a
        // cross-orbital dedup rename the ref may carry a stale name while its
        // ref_id stays stable, and codegen/render key off `binding.trait_name`.
        // Resolving by id here (the OIR-construction seam) means no page-ref
        // token rewrite is needed downstream. Falls back to the ref name when no
        // id (unstamped input).
        if let Some(traits) = &page.traits {
            for trait_ref in traits {
                let trait_id = trait_ref.trait_ref_id();
                let current_name = trait_id
                    .and_then(|id| self.trait_name_by_id(id))
                    .or_else(|| trait_ref.trait_name());
                if let Some(name) = current_name {
                    let mut binding = OirTraitBinding::new(name.to_string());
                    binding.trait_id = trait_id.map(str::to_string);
                    oir_page.traits.push(binding);
                }
            }
        }

        Ok(oir_page)
    }

    /// Current name of the inline trait carrying `id`, scanning every orbital
    /// (a page ref_id may point across orbital boundaries). Backs the page
    /// binding's id-first name resolution.
    fn trait_name_by_id(&self, id: &str) -> Option<&'a str> {
        self.schema
            .orbitals
            .iter()
            .flat_map(|o| o.traits.iter())
            .find_map(|tr| match tr {
                TraitReference::Inline(td) if td.id.as_deref() == Some(id) => {
                    Some(td.name.as_str())
                }
                _ => None,
            })
    }

    fn resolve_section(&self, section: &SectionDefinition) -> Result<OirSection, ResolveError> {
        let pattern_type = section
            .pattern
            .as_ref()
            .map(|p| p.pattern_type.clone())
            .unwrap_or_else(|| "custom".to_string());

        let mut pattern = OirPattern::new(&pattern_type);

        // Copy props to pattern config
        if let Some(props) = &section.props {
            for (key, value) in props {
                pattern
                    .config
                    .insert(key.clone(), self.sexpr_to_value(value));
            }
        }

        // Use slot as section id, or generate one
        let section_id = section.slot.clone().unwrap_or_else(|| "main".to_string());
        let oir_section = OirSection::new(&section_id, pattern);

        Ok(oir_section)
    }

    pub fn resolve_sexpr(&self, sexpr: &SExpression) -> Result<OirExpr, ResolveError> {
        Ok(ExprLowerer.resolve_sexpr(sexpr)?)
    }

    fn sexpr_to_value(&self, sexpr: &SExpression) -> OirValue {
        ExprLowerer.sexpr_to_value(sexpr)
    }
    /// Collect every slot a render-ui effect reaches, including branches of
    /// an `(if …)` — a config-driven switch (`std-board`'s boardLook,
    /// `std-browse`'s browseLook) still targets a real slot from EITHER
    /// arm. A top-level-only check here (this used to return at most one
    /// `Option<String>` and never looked inside `OirExpr::Conditional`)
    /// under-reports `OirTrait.slots_used` for any trait whose render-ui is
    /// entirely conditional — `slots_used` gates whether the TS backend
    /// even imports/mounts `ModalSlot`/`DrawerSlot`/`ToastSlot` on the page
    /// (backend/pages.rs), so a trait whose only `modal`/`drawer` render-ui
    /// sits behind a config switch got NO portal host on the page at all:
    /// the trait's own view computes the right JSX, but it has nowhere to
    /// portal into, so nothing visibly happens (C-SLOTS-USED-CONDITIONAL-
    /// BLIND — plausible root cause of "modal doesn't open" reports on any
    /// look-switched trait).
    fn collect_slots_from_effect(effect: &OirExpr, out: &mut std::collections::HashSet<String>) {
        match effect {
            OirExpr::Effect { effect } => {
                if let OirEffect::RenderUi { slot, .. } = effect.as_ref() {
                    out.insert(slot.clone());
                }
            }
            OirExpr::Conditional {
                then_branch,
                else_branch,
                ..
            } => {
                Self::collect_slots_from_effect(then_branch, out);
                Self::collect_slots_from_effect(else_branch, out);
            }
            _ => {}
        }
    }
}

/// Parse the `field_type` string produced by the lolo lowerer
/// (`type_expr_to_string`) into a narrow `OirType`. Handles:
///
/// | Input          | Output                                       |
/// |----------------|----------------------------------------------|
/// | `"string"`     | `OirType::String`                            |
/// | `"int"`        | `OirType::Integer`                           |
/// | `"[X]"`        | `OirType::Array { inner: parse_type(X) }`    |
/// | `"X"` (other)  | `OirType::Reference { entity: X }`           |
/// | `"object"`     | `OirType::Json` (fallback; authors should narrow) |
///
/// Recursive on arrays so `[[X]]` (unlikely in practice) still resolves.
/// Resolve a `PayloadField` to an `OirType`. A field carrying a nested
/// `properties` shape (an anonymous struct payload — inline `{ … }` or a
/// `type X = { … }` alias) becomes an `OirType::Struct` so codegen emits a
/// strict inline object type; everything else uses the flat `field_type`
/// string parse.
/// Bundles what the named-type registration path needs, so
/// `payload_field_to_oir_type` / `register_named_type_children` /
/// `lookup_named_type_shape` thread one reference instead of four.
struct NamedTypeCtx<'a> {
    schema: &'a OrbitalSchema,
    named_types: &'a RefCell<BTreeMap<String, OirType>>,
    /// See `Resolver::named_types_in_progress`'s doc — the mutual-recursion
    /// guard for `lookup_named_type_shape`.
    in_progress: &'a RefCell<HashSet<String>>,
    /// See `Resolver::entity_names`'s doc.
    entity_names: &'a HashSet<String>,
}

/// The `OirType` a declared trait config knob has.
pub(crate) fn config_field_oir_type(
    field: &orbital_core_base::schema::ConfigField,
    schema: &OrbitalSchema,
) -> OirType {
    Resolver::new(schema).resolve_config_field_shape(field)
}

/// The `OirType` of a config knob whose shape its own `items` / `properties` carriers spell out, read where no program
/// is in hand (the inline pass): every named type in a carrier is already inlined.
pub(crate) fn config_field_carried_type(field: &orbital_core_base::schema::ConfigField) -> OirType {
    thread_local! {
        static NO_PROGRAM: OrbitalSchema =
            serde_json::from_value(serde_json::json!({ "name": "", "orbitals": [] })).expect("the empty program");
    }
    NO_PROGRAM.with(|schema| Resolver::new(schema).resolve_config_field_shape(field))
}

/// The `OirType` a declared payload field has, named types resolved against the schema.
pub(crate) fn payload_field_oir_type(
    field: &orbital_core_base::schema::PayloadField,
    schema: &OrbitalSchema,
) -> OirType {
    let named_types = RefCell::new(BTreeMap::new());
    let in_progress = RefCell::new(HashSet::new());
    let entity_names = declared_entity_names(schema);
    let ctx = NamedTypeCtx {
        schema,
        named_types: &named_types,
        in_progress: &in_progress,
        entity_names: &entity_names,
    };
    payload_field_to_oir_type(field, &ctx)
}

/// The struct a declared alias (`type X = { … }`) names, as an `OirType`, else the installed language's program type
/// of that name (`Viewer`, `BehaviorRef`, …); `None` when neither declares it.
pub(crate) fn declared_struct_type(schema: &OrbitalSchema, name: &str) -> Option<OirType> {
    schema
        .orbitals
        .iter()
        .find_map(|orbital| {
            orbital.types.as_ref().and_then(|aliases| {
                orbital_core_base::ir::types::DeclaredShape::of_alias(name, aliases)
                    .map(|shape| shape.ty)
            })
        })
        .or_else(|| {
            schema.types.as_ref().and_then(|aliases| {
                orbital_core_base::ir::types::DeclaredShape::of_alias(name, aliases)
                    .map(|shape| shape.ty)
            })
        })
        .or_else(|| orbital_core_base::language::program_types::program_type(name).cloned())
}

/// Every entity name `resolve()` will push into `OirModule.entities`: each
/// orbital's own primary entity plus its auxiliary entities (Gap #22). One
/// owner, shared by `Resolver::new` (the real run) and the `payload_field_to_oir_type`
/// tests below (a fresh `HashSet` built the same way from a fixture schema) —
/// re-deriving this list a second way would let the two silently drift.
fn declared_entity_names(schema: &OrbitalSchema) -> HashSet<String> {
    let mut names = HashSet::new();
    for orbital in &schema.orbitals {
        if let EntityRef::Inline(entity) = &orbital.entity {
            names.insert(entity.name.clone());
        }
        if let Some(aux_list) = &orbital.auxiliary_entities {
            for aux_ref in aux_list {
                if let EntityRef::Inline(entity) = aux_ref {
                    names.insert(entity.name.clone());
                }
            }
        }
    }
    names
}

fn payload_field_to_oir_type(
    field: &orbital_core_base::schema::PayloadField,
    ctx: &NamedTypeCtx,
) -> OirType {
    let shape = orbital_core_base::ir::types::payload_field_shape_with_entity(
        field,
        &|entity| ctx.entity_names.contains(entity),
    );
    if let OirType::Struct { fields, .. } = &shape {
        register_named_type_children(fields, ctx);
    }
    let declared = orbital_core_base::ir::types::DeclaredShape::with_resolver(
        None,
        shape,
        |name| lookup_named_type_shape(name, ctx),
    );
    for (name, shape) in declared.named {
        ctx.named_types.borrow_mut().entry(name).or_insert(shape);
    }
    declared.ty
}

/// The alias name a resolved `OirType` self-references, if it is (or wraps,
/// one Array/Optional layer deep, since a recursive field is almost always
/// `[Name]`/`Name?`) a `TypeRef`.
fn typeref_name(ty: &OirType) -> Option<&str> {
    match ty {
        OirType::TypeRef { name } => Some(name),
        OirType::Array { inner } | OirType::Optional { inner } => typeref_name(inner),
        _ => None,
    }
}

/// For every name a `TypeRef` among `fields` (this struct's OWN, possibly
/// Array/Optional-wrapped, direct children) self-references, register that
/// name's definition — looked up by NAME in `schema.orbitals[].types` (Gap
/// A's alias table, the authoritative name→shape carrier every struct alias
/// is keyed under regardless of where a `TypeRef` to it happens to occur).
/// `fields` itself (the CONTAINING struct) is only a fallback for a name Gap
/// A doesn't carry (a pre-Gap-A `.orb` authored/emitted before that table
/// existed) — using it as the primary source was the bug: for DIRECT
/// self-reference the containing struct genuinely IS the definition, but
/// under MUTUAL recursion (`type A = { b : B }`, `type B = { a : A }`) the
/// struct that CONTAINS the back-reference to "A" is B's, not A's.
///
/// Shared by `resolve_field_shape`'s Object branch (entity/config fields)
/// and `payload_field_to_oir_type` (emit/listen payload fields) — one rule,
/// both wire shapes. First registration for a name wins; a name is stable
/// across the module once it's been looked up once.
fn register_named_type_children(fields: &[OirField], ctx: &NamedTypeCtx) {
    for f in fields {
        let Some(name) = typeref_name(&f.field_type) else {
            continue;
        };
        if ctx.named_types.borrow().contains_key(name) {
            continue;
        }
        if !ctx.in_progress.borrow_mut().insert(name.to_string()) {
            // Already being computed higher up this same call stack (a
            // mutual-recursion cycle) — that frame owns the eventual
            // registration; recursing again here would never terminate.
            continue;
        }
        let shape = lookup_named_type_shape(name, ctx).unwrap_or_else(|| OirType::Struct {
            additional: None,
            fields: fields.to_vec(),
        });
        ctx.named_types
            .borrow_mut()
            .entry(name.to_string())
            .or_insert(shape);
        ctx.in_progress.borrow_mut().remove(name);
    }
}

/// Look up a named struct alias's own shape by NAME in `schema.orbitals[].
/// types` (Gap A) — every struct alias in an orbital's file, and (via the
/// inline pre-pass) imported ones, keyed under its OWN declared name.
/// Searches every orbital's map; first hit wins. `None` when no orbital
/// carries the name (the pre-Gap-A fallback case).
///
/// The looked-up `Vec<PayloadField>` may carry one extra expansion level
/// before hitting the by-name back-reference marker, versus the shape a
/// `TypeRef` OCCURRENCE site produces — the map is built by lowering from
/// the alias's own BODY (not from a call site re-entering it), so the cycle
/// guard triggers one level deeper. Structurally equivalent; not normalized.
fn lookup_named_type_shape(name: &str, ctx: &NamedTypeCtx) -> Option<OirType> {
    ctx.schema.orbitals.iter().filter_map(|orbital| orbital.types.as_ref())
        .chain(ctx.schema.types.iter())
        .find_map(|types| types.get(name))
        .map(|members| orbital_core_base::ir::types::declared_alias_shape(members))
}

fn parse_payload_field_type(field_type: &str) -> OirType {
    orbital_core_base::ir::types::payload_type_shape(field_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::ir::{OirBinaryOp, OirBinding};

    #[test]
    fn native_contract_nested_explicit_entity_markers_keep_nominal_identity() {
        let schema = empty_schema();
        let named_types = RefCell::new(BTreeMap::new());
        let in_progress = RefCell::new(HashSet::new());
        let entity_names: HashSet<String> = ["Note".to_string()].into_iter().collect();
        let ctx = NamedTypeCtx { schema: &schema, named_types: &named_types, in_progress: &in_progress, entity_names: &entity_names };
        for entity in ["Note", "Ghost"] {
            let field: orbital_core_base::schema::PayloadField = serde_json::from_value(serde_json::json!({
                "name": "envelope", "type": "object",
                "properties": [{ "name": "rows", "type": "[object]", "nullable": true, "entity": entity,
                    "properties": [{ "name": "id", "type": "string", "required": true }] }],
                "additionalProperties": { "name": "", "type": "object", "entity": entity,
                    "properties": [{ "name": "id", "type": "string", "required": true }] }
            })).unwrap();
            let OirType::Struct { fields, additional: Some(additional) } = payload_field_to_oir_type(&field, &ctx) else { panic!("envelope must retain its catchall") };
            let OirType::Optional { inner } = &fields[0].field_type else { panic!("nullable nested list must retain nullability") };
            let OirType::Array { inner } = inner.as_ref() else { panic!("nested rows must remain a list") };
            if entity == "Note" {
                assert!(matches!(inner.as_ref(), OirType::Reference { entity } if entity == "Note"));
                assert!(matches!(additional.as_ref(), OirType::Reference { entity } if entity == "Note"));
            } else {
                assert!(matches!(inner.as_ref(), OirType::Struct { .. }));
                assert!(matches!(additional.as_ref(), OirType::Struct { .. }));
            }
        }
    }

    #[test]
    fn collect_slots_from_effect_reaches_a_render_ui_nested_inside_a_conditional() {
        let effect = OirExpr::Conditional {
            condition: Box::new(OirExpr::BinaryOp {
                op: OirBinaryOp::Eq,
                left: Box::new(OirExpr::Binding {
                    binding: OirBinding::new(OirBindingRoot::Config, vec!["look".to_string()]),
                }),
                right: Box::new(OirExpr::String {
                    value: "compact".to_string(),
                }),
            }),
            then_branch: Box::new(OirExpr::Effect {
                effect: Box::new(OirEffect::RenderUi {
                    slot: "modal".to_string(),
                    pattern: OirPattern::new("form-section"),
                }),
            }),
            else_branch: Box::new(OirExpr::Effect {
                effect: Box::new(OirEffect::RenderUi {
                    slot: "modal".to_string(),
                    pattern: OirPattern::new("form-section-wide"),
                }),
            }),
        };

        let mut slots = std::collections::HashSet::new();
        Resolver::collect_slots_from_effect(&effect, &mut slots);
        assert!(
            slots.contains("modal"),
            "a render-ui nested inside a conditional must still register its slot, got: {slots:?}"
        );
    }

    fn resolved_call_service(json: &str) -> OirEffect {
        let sexpr: SExpression = serde_json::from_str(json).expect("sexpr");
        match resolve_sexpr_standalone(&sexpr).expect("resolve") {
            OirExpr::Effect { effect } => *effect,
            other => panic!("expected an effect, got {other:?}"),
        }
    }

    #[test]
    fn call_service_options_resolve_key_and_cancelled() {
        let effect = resolved_call_service(
            r#"["call-service", "test-svc", "run", {"items": "@entity.items"},
                {"key": "@entity.batchId", "emit": {"success": "OK", "failure": "BAD", "cancelled": "STOPPED"}}]"#,
        );
        let OirEffect::CallService {
            key, emit, args, ..
        } = effect
        else {
            panic!("expected CallService");
        };
        assert!(matches!(key, Some(OirExpr::Binding { .. })), "{key:?}");
        assert!(matches!(args, OirExpr::Object { .. }));
        let emit = emit.expect("emit");
        assert_eq!(emit.cancelled.as_deref(), Some("STOPPED"));
        assert_eq!(emit.success.as_deref(), Some("OK"));
    }

    #[test]
    fn call_service_key_without_emit_is_still_options_not_args() {
        let effect = resolved_call_service(
            r#"["call-service", "test-svc", "run", {"userPrompt": "hi"}, {"key": "k1"}]"#,
        );
        let OirEffect::CallService {
            key, emit, args, ..
        } = effect
        else {
            panic!("expected CallService");
        };
        assert!(matches!(key, Some(OirExpr::String { .. })), "{key:?}");
        assert!(emit.is_none());
        let OirExpr::Object { fields } = args else {
            panic!("args must stay the payload");
        };
        assert!(fields.contains_key("userPrompt") && !fields.contains_key("key"));
    }

    #[test]
    fn call_service_three_arg_object_with_key_field_stays_the_payload() {
        let effect =
            resolved_call_service(r#"["call-service", "test-svc", "get", {"key": "@entity.key"}]"#);
        let OirEffect::CallService { key, args, .. } = effect else {
            panic!("expected CallService");
        };
        assert!(key.is_none());
        let OirExpr::Object { fields } = args else {
            panic!("expected object args");
        };
        assert!(fields.contains_key("key"));
    }

    #[test]
    fn cancel_call_resolves_its_key_expression() {
        let effect = resolved_call_service(r#"["cancel-call", "@entity.batchId"]"#);
        let OirEffect::CancelCall { key } = effect else {
            panic!("expected CancelCall");
        };
        assert!(matches!(key, OirExpr::Binding { .. }), "{key:?}");
    }

    #[test]
    fn cancel_call_requires_exactly_one_argument() {
        let none: SExpression = serde_json::from_str(r#"["cancel-call"]"#).unwrap();
        assert!(resolve_sexpr_standalone(&none).is_err());
        let two: SExpression = serde_json::from_str(r#"["cancel-call", "a", "b"]"#).unwrap();
        assert!(resolve_sexpr_standalone(&two).is_err());
    }

    /// A schema with no orbitals — every named-type test that doesn't
    /// exercise Gap A's `orbital.types` lookup itself uses this, so
    /// `lookup_named_type_shape` finds nothing and `register_named_type_children`
    /// falls back to the containing-struct inference (the pre-Gap-A path).
    fn empty_schema() -> OrbitalSchema {
        OrbitalSchema {
            type_contract_version: None,
            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            name: String::new(),
            version: None,
            description: None,
            theme: None,
            orbitals: Vec::new(),
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
            egress: None,
            inbound: None,
            os_grants: None,
            deployment: None,
        }
    }

    /// A payload field tagged `union` carries VARIANTS in `properties`, not the
    /// members of one object. Resolving it as a struct emitted
    /// `{ string?: string; number?: number; … }` — an object of optional variant
    /// keys that the value a control actually sends (`"hello"`) does not satisfy.
    #[test]
    fn union_payload_field_resolves_to_a_union_not_a_struct() {
        let variant = |name: &str, ty: &str| orbital_core_base::schema::PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: name.to_string(),
            field_type: ty.to_string(),
            required: None,
            description: None,
            properties: None,
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        };
        let field = orbital_core_base::schema::PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "value".to_string(),
            field_type: "union".to_string(),
            required: Some(true),
            description: None,
            properties: Some(vec![
                variant("string", "string"),
                variant("number", "number"),
                variant("array", "[string]"),
            ]),
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        };
        let schema = empty_schema();
        let named_types = RefCell::new(BTreeMap::new());
        let in_progress = RefCell::new(HashSet::new());
        let entity_names = HashSet::new();
        let ctx = NamedTypeCtx {
            schema: &schema,
            named_types: &named_types,
            in_progress: &in_progress,
            entity_names: &entity_names,
        };
        match payload_field_to_oir_type(&field, &ctx) {
            OirType::Union { variants } => {
                assert_eq!(variants.len(), 3);
                assert!(matches!(variants[0].field_type, OirType::String));
                assert!(matches!(variants[1].field_type, OirType::Number));
                assert!(matches!(variants[2].field_type, OirType::Array { .. }));
            }
            other => panic!("expected a union, got {other:?}"),
        }
    }

    /// A tagged-union entity field's members are never optional: the value IS
    /// one member or another. A member shape declared without `required`
    /// resolved through `resolve_field`'s optional wrapper, so every backend
    /// rendered `A | null | B | null` (TS) / `.optional()` per member (Zod).
    #[test]
    fn tagged_union_entity_field_members_are_never_optional() {
        let field: FieldDefinition = serde_json::from_value(serde_json::json!({
            "name": "entry",
            "type": "union",
            "values": ["MessageEntry", "ErrorEntry"],
            "properties": {
                "MessageEntry": { "type": "object", "properties": { "type": { "type": "string", "required": true } } },
                "ErrorEntry": { "type": "object", "required": true, "properties": { "type": { "type": "string", "required": true } } }
            }
        }))
        .expect("field fixture must deserialize");
        let schema = empty_schema();
        let resolved = Resolver::new(&schema)
            .resolve_field_shape(&field)
            .expect("field resolves");
        let OirType::Union { variants } = &resolved else {
            panic!("expected a union, got {resolved:?}");
        };
        assert_eq!(variants.len(), 2);
        for v in variants {
            assert!(v.required, "member {} must not be optional", v.name);
            assert!(
                matches!(v.field_type, OirType::Struct { .. }),
                "member {} must be its struct, got {:?}",
                v.name,
                v.field_type
            );
        }
    }

    /// The safety half: a plain `object` payload field with `properties` is a
    /// STRUCT and must stay one — the union arm keys strictly off `"union"`.
    #[test]
    fn object_payload_field_with_properties_still_resolves_to_a_struct() {
        let field = orbital_core_base::schema::PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "change".to_string(),
            field_type: "object".to_string(),
            required: Some(true),
            description: None,
            properties: Some(vec![orbital_core_base::schema::PayloadField {
                nullable: None,
                additional_properties: None,
                values: None,
                name: "fieldId".to_string(),
                field_type: "string".to_string(),
                required: Some(true),
                description: None,
                properties: None,
                type_when: None,
                entity: None,
                projected_from: None,
                items: None,
            }]),
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        };
        let schema = empty_schema();
        let named_types = RefCell::new(BTreeMap::new());
        let in_progress = RefCell::new(HashSet::new());
        let entity_names = HashSet::new();
        let ctx = NamedTypeCtx {
            schema: &schema,
            named_types: &named_types,
            in_progress: &in_progress,
            entity_names: &entity_names,
        };
        assert!(matches!(
            payload_field_to_oir_type(&field, &ctx),
            OirType::Struct { .. }
        ));
    }

    /// The `PayloadField.entity` marker's whole reason to exist: an
    /// entity-typed payload field (`rows : [Note]`) is flattened to
    /// `[object]` + `properties` by the lolo lowerer, but when `entity`
    /// names a REAL declared entity, `payload_field_to_oir_type` must hand
    /// codegen `OirType::Reference` (array-wrapped), not the flattened
    /// struct — the struct loses the entity's identity, and an inferred
    /// client read of the field (e.g. a filtered collection bound into a
    /// component's `entity:` prop) then emits an inline literal that fails
    /// TS2322 against `readonly EntityRow[]` (C-COMPILED-PATH-RED, std-notes
    /// `NoteTagNotesOverlay`).
    #[test]
    fn entity_marked_payload_field_resolves_to_reference_when_entity_is_declared() {
        let field = orbital_core_base::schema::PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "data".to_string(),
            field_type: "[object]".to_string(),
            required: None,
            description: None,
            properties: Some(vec![orbital_core_base::schema::PayloadField {
                nullable: None,
                additional_properties: None,
                values: None,
                name: "id".to_string(),
                field_type: "string".to_string(),
                required: Some(true),
                description: None,
                properties: None,
                type_when: None,
                entity: None,
                projected_from: None,
                items: None,
            }]),
            type_when: None,
            entity: Some("Note".to_string()),
            projected_from: None,
            items: None,
        };
        let schema = empty_schema();
        let named_types = RefCell::new(BTreeMap::new());
        let in_progress = RefCell::new(HashSet::new());
        let entity_names: HashSet<String> = ["Note".to_string()].into_iter().collect();
        let ctx = NamedTypeCtx {
            schema: &schema,
            named_types: &named_types,
            in_progress: &in_progress,
            entity_names: &entity_names,
        };
        match payload_field_to_oir_type(&field, &ctx) {
            OirType::Array { inner } => {
                assert!(
                    matches!(inner.as_ref(), OirType::Reference { entity } if entity == "Note"),
                    "expected Array<Reference(Note)>, got Array<{:?}>",
                    inner
                );
            }
            other => panic!("expected an array of Reference, got {other:?}"),
        }
    }

    /// The safety fallback: when `entity` names something this schema does
    /// NOT declare (a foreign/out-of-scope name), there is no local
    /// interface to reference — keep the existing flattened-struct
    /// behavior rather than emitting a `Reference` to an entity codegen
    /// cannot import.
    #[test]
    fn entity_marked_payload_field_falls_back_to_struct_when_entity_is_not_declared() {
        let field = orbital_core_base::schema::PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "data".to_string(),
            field_type: "[object]".to_string(),
            required: None,
            description: None,
            properties: Some(vec![orbital_core_base::schema::PayloadField {
                nullable: None,
                additional_properties: None,
                values: None,
                name: "id".to_string(),
                field_type: "string".to_string(),
                required: Some(true),
                description: None,
                properties: None,
                type_when: None,
                entity: None,
                projected_from: None,
                items: None,
            }]),
            type_when: None,
            entity: Some("Ghost".to_string()),
            projected_from: None,
            items: None,
        };
        let schema = empty_schema();
        let named_types = RefCell::new(BTreeMap::new());
        let in_progress = RefCell::new(HashSet::new());
        let entity_names: HashSet<String> = HashSet::new();
        let ctx = NamedTypeCtx {
            schema: &schema,
            named_types: &named_types,
            in_progress: &in_progress,
            entity_names: &entity_names,
        };
        match payload_field_to_oir_type(&field, &ctx) {
            OirType::Array { inner } => {
                assert!(
                    matches!(inner.as_ref(), OirType::Struct { .. }),
                    "expected the flattened-struct fallback, got Array<{:?}>",
                    inner
                );
            }
            other => panic!("expected an array of Struct, got {other:?}"),
        }
    }

    /// Recursive named struct type (test 1 of the spec's required suite):
    /// a payload field shaped like the lolo lowerer's cycle-guard output for
    /// `type MenuItem = { label: string!, subMenu: [MenuItem] }` — `subMenu`
    /// carries the empty-`properties` back-reference marker — resolves to a
    /// real recursive shape (`Array<TypeRef>`), not `Reference` (which would
    /// wrongly read it as an entity) nor an infinite/empty struct, and
    /// registers `MenuItem`'s definition in the named-types registry.
    #[test]
    fn recursive_struct_payload_field_resolves_to_type_ref_and_registers() {
        let sub_menu_backref = orbital_core_base::schema::PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "subMenu".to_string(),
            field_type: "[MenuItem]".to_string(),
            required: None,
            description: None,
            properties: Some(vec![]), // the back-reference marker
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        };
        let row = orbital_core_base::schema::PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "row".to_string(),
            field_type: "object".to_string(),
            required: Some(true),
            description: None,
            properties: Some(vec![
                orbital_core_base::schema::PayloadField {
                    nullable: None,
                    additional_properties: None,
                    values: None,
                    name: "label".to_string(),
                    field_type: "string".to_string(),
                    required: Some(true),
                    description: None,
                    properties: None,
                    type_when: None,
                    entity: None,
                    projected_from: None,
                    items: None,
                },
                sub_menu_backref,
            ]),
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
        };
        let schema = empty_schema();
        let named_types = RefCell::new(BTreeMap::new());
        let in_progress = RefCell::new(HashSet::new());
        let entity_names = HashSet::new();
        let ctx = NamedTypeCtx {
            schema: &schema,
            named_types: &named_types,
            in_progress: &in_progress,
            entity_names: &entity_names,
        };
        let resolved = payload_field_to_oir_type(&row, &ctx);
        let OirType::Struct { fields, .. } = &resolved else {
            panic!("expected a struct, got {resolved:?}");
        };
        let sub_menu = fields
            .iter()
            .find(|f| f.name == "subMenu")
            .expect("subMenu field present");
        match &sub_menu.field_type {
            OirType::Array { inner } => {
                assert!(
                    matches!(inner.as_ref(), OirType::TypeRef { name } if name == "MenuItem"),
                    "subMenu[] element must be a TypeRef to MenuItem, got {:?}",
                    inner
                );
            }
            other => panic!("expected subMenu to be an array, got {other:?}"),
        }
        let registered = named_types
            .borrow()
            .get("MenuItem")
            .cloned()
            .expect("MenuItem must be registered");
        assert_eq!(
            registered, resolved,
            "MenuItem's registered definition must be row's own struct"
        );
    }

    /// Feature test 1's ENTITY half: a `.orb` entity field shaped like
    /// `lower/entity.rs`'s cycle-guard output for `field x : MenuItem` (a
    /// direct self-referential struct field) resolves through
    /// `resolve_field_shape` to a real recursive `OirType` — the array
    /// element is `TypeRef { name: "MenuItem" }`, not `Json` — and registers
    /// `MenuItem`'s definition.
    #[test]
    fn recursive_struct_entity_field_resolves_to_type_ref_and_registers() {
        let field: FieldDefinition = serde_json::from_value(serde_json::json!({
            "name": "x",
            "type": "object",
            "properties": {
                "label": { "type": "string", "required": true },
                "subMenu": {
                    "type": "array",
                    "items": { "type": "union", "values": ["MenuItem"] }
                }
            }
        }))
        .expect("field fixture must deserialize");
        let schema = OrbitalSchema {
            type_contract_version: None,
            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            name: String::new(),
            version: None,
            description: None,
            theme: None,
            orbitals: Vec::new(),
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
            egress: None,
            inbound: None,
            os_grants: None,
            deployment: None,
        };
        let resolver = Resolver::new(&schema);
        let resolved = resolver
            .resolve_field_shape(&field)
            .expect("field resolves");
        let OirType::Struct { fields, .. } = &resolved else {
            panic!("expected a struct, got {resolved:?}");
        };
        let sub_menu = fields
            .iter()
            .find(|f| f.name == "subMenu")
            .expect("subMenu field present");
        // `subMenu` carries no `required` key in the fixture — the honest
        // read (`resolve_field`'s `unwrap_or(false)`) is optional, so
        // `OirField::optional` wraps the shape in `Optional`; unwrap it the
        // same way entity-interface codegen does (`!field.required` arm)
        // before checking the recursive element type this test cares about.
        assert!(!sub_menu.required, "unmarked field must resolve optional");
        let sub_menu_type = match &sub_menu.field_type {
            OirType::Optional { inner } => inner.as_ref(),
            other => other,
        };
        match sub_menu_type {
            OirType::Array { inner } => {
                assert!(
                    matches!(inner.as_ref(), OirType::TypeRef { name } if name == "MenuItem"),
                    "subMenu[] element must be a TypeRef to MenuItem, got {:?}",
                    inner
                );
            }
            other => panic!("expected subMenu to be an array, got {other:?}"),
        }
        let registered = resolver
            .named_types
            .borrow()
            .get("MenuItem")
            .cloned()
            .expect("MenuItem must be registered");
        assert_eq!(registered, resolved);
    }

    /// Mutual recursion (coordinator review 2026-09-02): `type A = { b : B
    /// }`, `type B = { a : A }` — the containing-struct inference
    /// (`register_named_type_children`'s old, buggy sole source) would
    /// register B's struct (`{ a: TypeRef A }`) as "A"'s definition, since B
    /// is what directly contains the back-reference to A. The fix looks
    /// "A" up BY NAME in `schema.orbitals[].types` (Gap A) instead, which
    /// carries A's own true shape (`{ b: … }`) independent of where the
    /// `TypeRef` occurrence happens to sit.
    #[test]
    fn mutual_recursion_registers_each_name_under_its_own_shape_not_the_containing_struct() {
        // `Orbital.types`, as `lower/mod.rs`'s Gap-A pre-pass would populate
        // it: each alias keyed under its OWN name, body-first (one extra
        // expansion level before the by-name marker — see
        // `lookup_named_type_shape`'s doc; not normalized).
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "MutualApp",
            "version": "1.0.0",
            "orbitals": [{
                "name": "MutualOrbital",
                "entity": { "name": "Container", "fields": [] },
                "traits": [],
                "pages": [],
                "types": {
                    "A": [{
                        "name": "b", "type": "object", "properties": [
                            { "name": "a", "type": "object", "properties": [
                                { "name": "b", "type": "B", "properties": [] }
                            ]}
                        ]
                    }],
                    "B": [{
                        "name": "a", "type": "object", "properties": [
                            { "name": "b", "type": "object", "properties": [
                                { "name": "a", "type": "A", "properties": [] }
                            ]}
                        ]
                    }]
                }
            }]
        }))
        .expect("schema fixture must deserialize");

        // An entity field `x : A` — `lower/entity.rs`'s lowered shape for a
        // mutually-recursive struct field: full expansion down to the FIRST
        // re-entrant occurrence (`x.b.a` re-enters "A").
        let field: FieldDefinition = serde_json::from_value(serde_json::json!({
            "name": "x",
            "type": "object",
            "properties": {
                "b": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "union", "values": ["A"] }
                    }
                }
            }
        }))
        .expect("field fixture must deserialize");

        let resolver = Resolver::new(&schema);
        resolver
            .resolve_field_shape(&field)
            .expect("field resolves");

        let registered = resolver
            .named_types
            .borrow()
            .get("A")
            .cloned()
            .expect("A must be registered");
        let OirType::Struct { fields, .. } = &registered else {
            panic!("expected a struct, got {registered:?}");
        };
        assert_eq!(
            fields.len(),
            1,
            "A's registered shape must be its OWN single field, not the containing B-shaped struct"
        );
        assert_eq!(
            fields[0].name, "b",
            "A's registered shape must start with field 'b' (its own field) — 'a' would mean the \
             containing struct (B's) was registered instead, the mutual-recursion bug"
        );
    }

    /// Safety test 3's L2 half: a single-member back-reference resolves to a
    /// `TypeRef`, never something a downstream reader could mistake for an
    /// enum's declared-value list (the field carries no `Enum`/values at
    /// all — `TypeRef` is a distinct variant).
    #[test]
    fn single_member_backref_is_not_an_enum_shape() {
        let field: FieldDefinition = serde_json::from_value(serde_json::json!({
            "type": "union",
            "values": ["MenuItem"]
        }))
        .expect("field fixture must deserialize");
        let schema = OrbitalSchema {
            type_contract_version: None,
            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            name: String::new(),
            version: None,
            description: None,
            theme: None,
            orbitals: Vec::new(),
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
            egress: None,
            inbound: None,
            os_grants: None,
            deployment: None,
        };
        let resolver = Resolver::new(&schema);
        let resolved = resolver.resolve_field_shape(&field).expect("resolves");
        assert!(
            matches!(&resolved, OirType::TypeRef { name } if name == "MenuItem"),
            "expected TypeRef, got {resolved:?}"
        );
        assert!(
            !matches!(resolved, OirType::Enum { .. }),
            "a back-reference must never resolve to Enum"
        );
    }

    /// A union that names a list of types without a shape for them is not a type: resolution refuses it
    /// rather than inventing an untyped value. Control: the single-name form is the by-name reference.
    #[test]
    fn a_union_without_variant_shapes_is_refused_not_degraded() {
        let schema = OrbitalSchema {
            type_contract_version: None,
            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            name: String::new(),
            version: None,
            description: None,
            theme: None,
            orbitals: Vec::new(),
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
            egress: None,
            inbound: None,
            os_grants: None,
            deployment: None,
        };
        let resolver = Resolver::new(&schema);
        for malformed in [
            serde_json::json!({ "type": "union" }),
            serde_json::json!({ "type": "union", "values": ["A", "B"] }),
            serde_json::json!({ "type": "union", "values": ["A"], "properties": {} }),
        ] {
            let field: FieldDefinition = serde_json::from_value(malformed.clone()).unwrap();
            assert!(resolver.resolve_field_shape(&field).is_err(), "{malformed}");
        }
        let by_name: FieldDefinition =
            serde_json::from_value(serde_json::json!({ "type": "union", "values": ["Node"] }))
                .unwrap();
        assert!(matches!(
            resolver.resolve_field_shape(&by_name),
            Ok(OirType::TypeRef { .. })
        ));
    }

    #[test]
    fn theme_key_precedence_orbital_over_app_over_default() {
        let orb = ThemeRef::Reference("clay-light".to_string());
        let app = ThemeRef::Reference("linear-clean-light".to_string());
        assert_eq!(resolve_theme_key(Some(&orb), Some(&app)), "clay-light");
        assert_eq!(resolve_theme_key(None, Some(&app)), "linear-clean-light");
        assert_eq!(resolve_theme_key(None, None), DEFAULT_THEME_KEY);
        let empty = ThemeRef::Reference(String::new());
        assert_eq!(
            resolve_theme_key(Some(&empty), Some(&app)),
            "linear-clean-light"
        );
    }

    /// `.lolo` `[number, number, number]` is a tuple in both type-string parsers (payload
    /// fields and declared config/struct types); `[T]` stays an array.
    #[test]
    fn a_tuple_type_string_resolves_to_a_tuple() {
        let n3 = OirType::Tuple {
            items: vec![OirType::Number, OirType::Number, OirType::Number],
        };
        assert_eq!(parse_payload_field_type("[number, number, number]"), n3);
        assert_eq!(
            parse_payload_field_type("[number]"),
            OirType::array(OirType::Number)
        );
        let schema: OrbitalSchema =
            serde_json::from_value(serde_json::json!({ "name": "T", "orbitals": [] })).unwrap();
        let resolver = Resolver::new(&schema);
        assert_eq!(resolver.type_string_to_oir("[number, number, number]"), n3);
        assert_eq!(
            resolver.type_string_to_oir("[string, number]"),
            OirType::Tuple {
                items: vec![OirType::String, OirType::Number]
            }
        );
        assert_eq!(
            resolver.type_string_to_oir("[number]"),
            OirType::array(OirType::Number)
        );
    }

    /// A tuple's slots ride `properties` by index; resolution must read them as slots, not
    /// as a struct with fields `0`, `1`, `2` (member and knob alike).
    #[test]
    fn a_tuple_field_and_knob_resolve_to_tuples() {
        let n3 = OirType::Tuple {
            items: vec![OirType::Number, OirType::Number, OirType::Number],
        };
        let slots = serde_json::json!({ "0": { "type": "number" }, "1": { "type": "number" }, "2": { "type": "number" } });
        let schema: OrbitalSchema =
            serde_json::from_value(serde_json::json!({ "name": "T", "orbitals": [] })).unwrap();
        let resolver = Resolver::new(&schema);
        let member: FieldDefinition = serde_json::from_value(
            serde_json::json!({ "name": "position", "type": "tuple", "properties": slots }),
        )
        .unwrap();
        assert_eq!(resolver.resolve_field_shape(&member).expect("resolves"), n3);
        let knob: orbital_core_base::schema::ConfigField = serde_json::from_value(
            serde_json::json!({ "type": "[number, number, number]", "properties": slots }),
        )
        .unwrap();
        assert_eq!(resolver.resolve_config_field_shape(&knob), n3);
    }

    #[test]
    fn a_row_of_the_entity_an_unset_knob_names_is_an_empty_record() {
        assert_eq!(
            parse_payload_field_type("@entity(captureEntity)"),
            OirType::Struct { additional: None, fields: Vec::new() }
        );
        assert_eq!(
            parse_payload_field_type("$t"),
            OirType::Var { name: "t".into() }
        );
        assert_eq!(
            parse_payload_field_type("[$t]"),
            OirType::array(OirType::Var { name: "t".into() })
        );
        assert_eq!(
            parse_payload_field_type("$"),
            OirType::Reference { entity: "$".into() },
            "control: no name"
        );
        assert_eq!(
            parse_payload_field_type("[@entity(captureEntity)]"),
            OirType::array(OirType::Struct { additional: None, fields: Vec::new() })
        );
        // control: a bare name stays a reference to the entity it names.
        assert_eq!(
            parse_payload_field_type("Order"),
            OirType::Reference {
                entity: "Order".to_string()
            }
        );
    }

    #[test]
    fn parse_payload_field_type_primitives() {
        assert_eq!(parse_payload_field_type("string"), OirType::String);
        assert_eq!(parse_payload_field_type("int"), OirType::Integer);
        assert_eq!(parse_payload_field_type("boolean"), OirType::Boolean);
        assert_eq!(
            parse_payload_field_type("object"),
            OirType::Struct { additional: None, fields: Vec::new() }
        );
    }

    #[test]
    fn parse_payload_field_type_semantic_scalars() {
        // @entity payload expansion feeds entity field-type strings verbatim —
        // every FieldType keyword must resolve, never fall through to Reference
        assert_eq!(parse_payload_field_type("email"), OirType::Email);
        assert_eq!(parse_payload_field_type("phone"), OirType::Phone);
        assert_eq!(parse_payload_field_type("money"), OirType::Money);
        assert_eq!(parse_payload_field_type("file"), OirType::File);
        assert_eq!(parse_payload_field_type("timestamp"), OirType::DateTime);
        assert_eq!(parse_payload_field_type("relation"), OirType::String);
        assert_eq!(parse_payload_field_type("enum"), OirType::String);
    }

    #[test]
    fn parse_payload_field_type_array_of_reference() {
        let parsed = parse_payload_field_type("[CartItem]");
        match parsed {
            OirType::Array { inner } => match *inner {
                OirType::Reference { entity } => assert_eq!(entity, "CartItem"),
                other => panic!("expected Reference inside Array, got {:?}", other),
            },
            other => panic!("expected Array, got {:?}", other),
        }
    }

    #[test]
    fn parse_payload_field_type_map() {
        let parsed = parse_payload_field_type("Map<string,SimulationParameter>");
        match parsed {
            OirType::Map { key, value } => {
                assert_eq!(*key, OirType::String);
                match *value {
                    OirType::Reference { entity } => assert_eq!(entity, "SimulationParameter"),
                    other => panic!("expected Reference value, got {:?}", other),
                }
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn parse_payload_field_type_map_nested_value() {
        // `Map string (Map string Foo)` — value is itself a map.
        let parsed = parse_payload_field_type("Map<string,Map<string,Foo>>");
        match parsed {
            OirType::Map { key, value } => {
                assert_eq!(*key, OirType::String);
                match *value {
                    OirType::Map { value: inner, .. } => match *inner {
                        OirType::Reference { entity } => assert_eq!(entity, "Foo"),
                        other => panic!("expected nested Reference, got {:?}", other),
                    },
                    other => panic!("expected nested Map, got {:?}", other),
                }
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn parse_payload_field_type_bare_reference() {
        match parse_payload_field_type("User") {
            OirType::Reference { entity } => assert_eq!(entity, "User"),
            other => panic!("expected Reference, got {:?}", other),
        }
    }

    /// `(navigate path payload)` must resolve the payload arg into
    /// `OirEffect::Navigate.params` — §48 closure: the JS runtime path
    /// already merges route params + navigate payload into INIT, and the
    /// compiled path was silently dropping this arg.
    #[test]
    fn navigate_with_payload_resolves_params() {
        let schema: OrbitalSchema = serde_json::from_str(
            r#"{
              "name": "t", "version": "1.0.0",
              "orbitals": [{
                "name": "O",
                "entity": { "name": "W", "fields": [] },
                "traits": [{
                  "name": "Panel",
                  "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{
                      "from": "idle", "to": "idle", "event": "GO",
                      "effects": [["navigate", "/detail", { "foo": "bar" }]]
                    }]
                  }
                }]
              }]
            }"#,
        )
        .expect("fixture parses");

        let module = resolve_to_oir(&schema).expect("resolves");
        let effect = module.traits[0].transitions[0].effects[0]
            .as_effect()
            .expect("navigate resolves to an effect expr");
        match effect {
            OirEffect::Navigate { params, .. } => {
                assert!(params.is_some(), "payload arg must resolve into params");
            }
            other => panic!("expected Navigate, got {:?}", other),
        }
    }

    /// `(navigate path)` with no payload arg must serialize byte-identically
    /// to before — `params` stays `None` and, via `skip_serializing_if`,
    /// contributes no `"params"` key to the OIR JSON at all.
    #[test]
    fn navigate_without_payload_omits_params_key_from_oir_json() {
        let schema: OrbitalSchema = serde_json::from_str(
            r#"{
              "name": "t", "version": "1.0.0",
              "orbitals": [{
                "name": "O",
                "entity": { "name": "W", "fields": [] },
                "traits": [{
                  "name": "Panel",
                  "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{
                      "from": "idle", "to": "idle", "event": "GO",
                      "effects": [["navigate", "/detail"]]
                    }]
                  }
                }]
              }]
            }"#,
        )
        .expect("fixture parses");

        let module = resolve_to_oir(&schema).expect("resolves");
        let effect = module.traits[0].transitions[0].effects[0]
            .as_effect()
            .expect("navigate resolves to an effect expr");
        match effect {
            OirEffect::Navigate { params, .. } => assert!(params.is_none()),
            other => panic!("expected Navigate, got {:?}", other),
        }

        let json = serde_json::to_string(&module).expect("module serializes");
        assert!(
            !json.contains("\"params\""),
            "a payload-less navigate must not serialize a params key:\n{json}"
        );
    }

    #[test]
    fn page_binding_takes_current_trait_name_via_ref_id() {
        // V4 codegen surface: a page trait ref carrying a STALE `reference` name
        // but a stable `refId` must resolve to the trait's CURRENT name in the
        // OIR binding — this is what replaced the deleted `rewrite_page_trait_refs`
        // token rewrite for codegen. The inline trait is named "RenamedTrait" and
        // carries id "trt_x"; the page ref says "OldName" + refId "trt_x".
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "fields": [{ "name": "title", "type": "string" }] },
                "traits": [{
                    "name": "RenamedTrait",
                    "id": "trt_x00000000000000000000000",
                    "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] }
                }],
                "pages": [{
                    "name": "P",
                    "path": "/p",
                    "traits": [{ "ref": "OldName", "refId": "trt_x00000000000000000000000" }]
                }]
            }]
        }))
        .expect("fixture parses");

        let oir = resolve_to_oir(&schema).expect("resolves");
        let page = oir.pages.first().expect("one page");
        let binding = page.traits.first().expect("one binding");
        assert_eq!(
            binding.trait_name, "RenamedTrait",
            "page binding must take the current name resolved via ref_id, not the stale ref name"
        );
        assert_eq!(
            binding.trait_id.as_deref(),
            Some("trt_x00000000000000000000000")
        );
    }

    #[test]
    fn config_forward_event_resolved_empty_is_omitted_from_oir() {
        // Mirrors what the inline phase leaves behind for a config-forward
        // event (`@config.extraAddonEvent -> external {...}`) whose knob is
        // unset at every call site: `resolve_config_ref_event_name` resolves
        // it to the knob's declared default, `""` — a real, deliberate value
        // meaning "this optional event slot is unfulfilled", not a
        // substitution failure. Resolve must not turn that into an OirEvent.
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "fields": [{ "name": "title", "type": "string" }] },
                "traits": [{
                    "name": "Board",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [],
                        "events": [{
                            "key": "",
                            "name": "extra addon event",
                            "payloadSchema": [{ "name": "id", "type": "string" }]
                        }]
                    }
                }]
            }]
        }))
        .expect("fixture parses");

        let oir = resolve_to_oir(&schema).expect("resolves");
        let board = oir
            .traits
            .iter()
            .find(|t| t.name == "Board")
            .expect("trait present");
        assert!(
            board.events.iter().all(|e| !e.key.is_empty()),
            "an unfulfilled config-forward event slot must not become an OirEvent: {:?}",
            board.events
        );
        assert!(board.events.is_empty());
    }

    #[test]
    fn config_forward_event_resolved_to_a_name_survives_with_payload_schema() {
        // Same slot, but the knob resolved to a concrete literal (what the
        // inline phase leaves behind when a call site sets it, e.g.
        // `extraAddonEvent: "PRODUCE"`) — the event must reach OIR intact.
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "E", "fields": [{ "name": "title", "type": "string" }] },
                "traits": [{
                    "name": "Board",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [],
                        "events": [{
                            "key": "PRODUCE",
                            "name": "extra addon event",
                            "payloadSchema": [{ "name": "id", "type": "string" }]
                        }]
                    }
                }]
            }]
        }))
        .expect("fixture parses");

        let oir = resolve_to_oir(&schema).expect("resolves");
        let board = oir
            .traits
            .iter()
            .find(|t| t.name == "Board")
            .expect("trait present");
        let event = board
            .events
            .iter()
            .find(|e| e.key == "PRODUCE")
            .expect("named config-forward event survives");
        assert!(event
            .payload_schema
            .as_ref()
            .expect("payload schema kept")
            .contains_key("id"));
    }

    #[test]
    fn test_resolve_minimal_schema() {
        let schema = OrbitalSchema {
            type_contract_version: None,
            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
            egress: None,
            inbound: None,
            os_grants: None,
            deployment: None,
        };

        let result = resolve_to_oir(&schema);
        assert!(result.is_ok());

        let oir = result.unwrap();
        assert_eq!(oir.name, "TestApp");
        assert_eq!(oir.version, "1.0.0");
    }

    fn tick_with_interval(
        interval: orbital_core_base::schema::StringOrNumber,
    ) -> orbital_core_base::schema::TickDefinition {
        orbital_core_base::schema::TickDefinition {
            name: "testTick".to_string(),
            interval,
            guard: None,
            effects: vec![],
            applies_to: None,
            description: None,
            background: false,
            while_visible: false,
        }
    }

    fn empty_resolver() -> Resolver<'static> {
        static SCHEMA: std::sync::OnceLock<OrbitalSchema> = std::sync::OnceLock::new();
        let schema = SCHEMA.get_or_init(|| OrbitalSchema {
            type_contract_version: None,
            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "Test".to_string(),
            version: None,
            description: None,
            orbitals: vec![],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
            egress: None,
            inbound: None,
            os_grants: None,
            deployment: None,
        });
        Resolver::new(schema)
    }

    fn relation_field_json(cardinality: &str) -> FieldDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "revisions",
            "type": "relation",
            "relation": { "entity": "Revision", "cardinality": cardinality }
        }))
        .expect("relation field")
    }

    #[test]
    fn resolve_field_shape_to_many_relation_is_array_of_reference() {
        let resolver = empty_resolver();
        for c in ["many", "one-to-many", "many-to-many"] {
            assert_eq!(
                resolver
                    .resolve_field_shape(&relation_field_json(c))
                    .unwrap(),
                OirType::array(OirType::reference("Revision")),
                "{c}"
            );
        }
    }

    #[test]
    fn resolve_field_shape_intrinsic_to_many_relation_is_array_of_row() {
        let resolver = empty_resolver();
        for c in ["many", "one-to-many", "many-to-many"] {
            let mut field = relation_field_json(c);
            field.intrinsic = Some(true);
            assert_eq!(
                resolver.resolve_field_shape(&field).unwrap(),
                OirType::array(OirType::row("Revision")),
                "{c}"
            );
        }
    }

    /// `loadedRow : RecordItem @intrinsic` holds the fetched row, not its id (core `field_shape`).
    #[test]
    fn resolve_field_shape_intrinsic_to_one_relation_is_a_row() {
        let resolver = empty_resolver();
        let mut field = relation_field_json("many-to-one");
        field.intrinsic = Some(true);
        assert_eq!(
            resolver.resolve_field_shape(&field).unwrap(),
            OirType::row("Revision")
        );
    }

    #[test]
    fn resolve_field_shape_to_one_relation_is_reference() {
        let resolver = empty_resolver();
        for c in ["one", "many-to-one"] {
            assert_eq!(
                resolver
                    .resolve_field_shape(&relation_field_json(c))
                    .unwrap(),
                OirType::reference("Revision"),
                "{c}"
            );
        }
    }

    #[test]
    fn resolve_tick_keeps_the_states_it_applies_to() {
        let resolver = empty_resolver();
        let mut tick = tick_with_interval(orbital_core_base::schema::StringOrNumber::String(
            "1s".to_string(),
        ));
        tick.applies_to = Some(vec!["editing".to_string(), "review".to_string()]);
        let resolved = resolver.resolve_tick(&tick).expect("tick resolves");
        assert_eq!(
            resolved.applies_to,
            vec!["editing".to_string(), "review".to_string()]
        );
    }

    #[test]
    fn control_a_tick_without_applies_to_runs_in_every_state() {
        let resolver = empty_resolver();
        let tick = tick_with_interval(orbital_core_base::schema::StringOrNumber::String(
            "1s".to_string(),
        ));
        assert!(resolver
            .resolve_tick(&tick)
            .expect("tick resolves")
            .applies_to
            .is_empty());
    }

    #[test]
    fn resolve_tick_cron_expression_becomes_oir_cron() {
        let resolver = empty_resolver();
        let tick = tick_with_interval(orbital_core_base::schema::StringOrNumber::String(
            "0 9 * * *".to_string(),
        ));
        let resolved = resolver
            .resolve_tick(&tick)
            .expect("valid cron should resolve");
        assert_eq!(
            resolved.interval,
            OirInterval::Cron {
                expression: "0 9 * * *".to_string()
            }
        );
    }

    #[test]
    fn resolve_tick_garbage_string_is_rejected_not_silently_1s() {
        let resolver = empty_resolver();
        let tick = tick_with_interval(orbital_core_base::schema::StringOrNumber::String(
            "not a valid interval".to_string(),
        ));
        let err = resolver
            .resolve_tick(&tick)
            .expect_err("garbage interval must be rejected, not silently default to 1s");
        assert!(matches!(err, ResolveError::InvalidTickInterval(_)));
    }

    #[test]
    fn resolve_tick_duration_suffixes_all_resolve() {
        let resolver = empty_resolver();
        let cases: &[(&str, OirInterval)] = &[
            ("500ms", OirInterval::Milliseconds { value: 500 }),
            ("30s", OirInterval::Seconds { value: 30 }),
            ("5m", OirInterval::Minutes { value: 5 }),
            (
                "2h",
                OirInterval::Milliseconds {
                    value: 2 * 60 * 60 * 1000,
                },
            ),
            (
                "1d",
                OirInterval::Milliseconds {
                    value: 24 * 60 * 60 * 1000,
                },
            ),
            ("frame", OirInterval::Frame),
        ];
        for (input, expected) in cases {
            let tick = tick_with_interval(orbital_core_base::schema::StringOrNumber::String(
                input.to_string(),
            ));
            let resolved = resolver
                .resolve_tick(&tick)
                .unwrap_or_else(|e| panic!("expected '{input}' to resolve, got error: {e}"));
            assert_eq!(&resolved.interval, expected, "mismatch for input '{input}'");
        }
    }

    // helper: build an s-expr call `[op, ...args]` from string args.
    fn sexpr_call(op: &str, args: &[&str]) -> SExpression {
        let mut elems = vec![SExpression::String(op.to_string())];
        elems.extend(args.iter().map(|a| SExpression::String(a.to_string())));
        SExpression::Array(elems)
    }

    #[test]
    fn max_arity_rejects_excess_args_namespaced_op() {
        // math/cos: minArity 1, maxArity 1. Two args must error.
        let sexpr = sexpr_call("math/cos", &["@x", "@y"]);
        let err = resolve_sexpr_standalone(&sexpr)
            .expect_err("math/cos with 2 args must be rejected, not silently drop one");
        assert!(matches!(err, ResolveError::InvalidSExpr(_)), "got {err:?}");
        let msg = format!("{err}");
        assert!(
            msg.contains("math/cos") && msg.contains("at most 1"),
            "msg: {msg}"
        );
    }

    #[test]
    fn max_arity_rejects_excess_args_binary_op() {
        // vec/add: maxArity 2. Three args must error (the C-EMIT silent-drop case:
        // binary_op used to keep the first two and discard the third).
        let sexpr = sexpr_call("vec/add", &["@x", "@y", "@z"]);
        let err = resolve_sexpr_standalone(&sexpr)
            .expect_err("vec/add with 3 args must be rejected, not silently drop @z");
        assert!(matches!(err, ResolveError::InvalidSExpr(_)));
        let msg = format!("{err}");
        assert!(
            msg.contains("vec/add") && msg.contains("at most 2"),
            "msg: {msg}"
        );
    }

    /// A `let` pair's name is a binding, never a call — even when it spells an
    /// operator (`swap`, `max`) — and its value still resolves as an expression.
    #[test]
    fn let_binding_names_never_resolve_as_calls() {
        for name in ["swap", "max", "total"] {
            let sexpr: SExpression = serde_json::from_value(serde_json::json!([
                "let",
                [[name, [">", "@a", "@b"]]],
                ["+", format!("@{name}"), 1]
            ]))
            .unwrap();
            let resolved = resolve_sexpr_standalone(&sexpr)
                .unwrap_or_else(|e| panic!("`{name}` binding must resolve: {e}"));
            let OirExpr::Call { function, args } = &resolved else {
                panic!("let must stay a call, got {resolved:?}");
            };
            assert_eq!(function, "let");
            assert_eq!(args.len(), 2, "{name}");
            let OirExpr::Array { items } = &args[0] else {
                panic!("bindings must stay an array, got {:?}", args[0]);
            };
            let OirExpr::Array { items: pair } = &items[0] else {
                panic!("`{name}` pair must stay [name, value], got {:?}", items[0]);
            };
            assert!(
                matches!(&pair[0], OirExpr::String { value } if value == name),
                "{name}: {:?}",
                pair[0]
            );
            assert!(
                matches!(&pair[1], OirExpr::BinaryOp { .. }),
                "{name} value: {:?}",
                pair[1]
            );
            assert!(
                matches!(&args[1], OirExpr::BinaryOp { .. }),
                "{name} body: {:?}",
                args[1]
            );
        }
    }

    #[test]
    fn variadic_ops_accept_many_args() {
        // array/concat: maxArity null (variadic). Five args must NOT error.
        let sexpr = sexpr_call("array/concat", &["@a", "@b", "@c", "@d", "@e"]);
        assert!(
            resolve_sexpr_standalone(&sexpr).is_ok(),
            "variadic op must accept any number of args"
        );
    }

    #[test]
    fn exact_max_arity_passes() {
        // vec/add with exactly 2 args (== maxArity) must resolve cleanly.
        let sexpr = sexpr_call("vec/add", &["@x", "@y"]);
        assert!(resolve_sexpr_standalone(&sexpr).is_ok());
    }

    #[test]
    fn variadic_or_keeps_every_branch() {
        // `(or A B C)` must lower to `Or(Or(A,B), C)`. Before the fold, args[2..]
        // were dropped: std-helpdesk's three-role @read silently lost its
        // customer branch, so no customer could ever read their own ticket.
        let expr = SExpression::Array(vec![
            SExpression::String("or".to_string()),
            SExpression::Bool(false),
            SExpression::Bool(false),
            SExpression::Bool(true),
        ]);
        let lowered = resolve_sexpr_standalone(&expr).expect("lowers");
        let round_tripped = lowered.to_sexpression().expect("has a guard form");
        let json = serde_json::to_string(&round_tripped.to_canonical_json()).expect("serializes");
        assert_eq!(
            json, r#"["or",["or",false,false],true]"#,
            "every branch of a variadic or must survive lowering"
        );
    }

    #[test]
    fn variadic_and_keeps_every_branch() {
        let expr = SExpression::Array(vec![
            SExpression::String("and".to_string()),
            SExpression::Bool(true),
            SExpression::Bool(true),
            SExpression::Bool(false),
        ]);
        let lowered = resolve_sexpr_standalone(&expr).expect("lowers");
        let round_tripped = lowered.to_sexpression().expect("has a guard form");
        let json = serde_json::to_string(&round_tripped.to_canonical_json()).expect("serializes");
        assert_eq!(json, r#"["and",["and",true,true],false]"#);
    }

    /// `FieldDefinition.intrinsic` (schema) must survive `resolve_field` into
    /// `OirField.intrinsic` — it is the ONLY frame allow-list downstream
    /// (`OirEntity::frame_fields`), so a dropped flag would silently make an
    /// intrinsic field look like ordinary persisted data to every later phase.
    #[test]
    fn resolve_field_carries_intrinsic_true() {
        let schema = empty_schema();
        let resolver = Resolver::new(&schema);
        let field: orbital_core_base::schema::FieldDefinition =
            serde_json::from_value(serde_json::json!({
                "name": "currentStepIndex",
                "type": "number",
                "intrinsic": true
            }))
            .expect("valid field definition");
        let oir_field = resolver.resolve_field(&field).expect("resolves");
        assert!(oir_field.intrinsic, "intrinsic must survive resolve_field");
    }

    #[test]
    fn resolve_field_defaults_intrinsic_false_when_absent() {
        let schema = empty_schema();
        let resolver = Resolver::new(&schema);
        let field: orbital_core_base::schema::FieldDefinition =
            serde_json::from_value(serde_json::json!({
                "name": "title",
                "type": "string"
            }))
            .expect("valid field definition");
        let oir_field = resolver.resolve_field(&field).expect("resolves");
        assert!(!oir_field.intrinsic);
    }

    /// `TraitDefinition.local` must survive `resolve_trait` into
    /// `OirTrait.local` — `OirTrait::dispatch_mode` reads it directly, so a
    /// dropped flag would silently flip a client-only trait back to
    /// server-gated dispatch.
    #[test]
    fn resolve_trait_carries_local_true() {
        let schema = empty_schema();
        let resolver = Resolver::new(&schema);
        let trait_def: orbital_core_base::schema::TraitDefinition =
            serde_json::from_value(serde_json::json!({
                "name": "DraftEditor",
                "local": true,
                "stateMachine": {
                    "states": [{"name": "idle", "isInitial": true}],
                    "transitions": []
                }
            }))
            .expect("valid trait definition");
        let oir_trait = resolver.resolve_trait(&trait_def).expect("resolves");
        assert!(oir_trait.local, "local must survive resolve_trait");
    }

    #[test]
    fn resolve_trait_defaults_local_false_when_absent() {
        let schema = empty_schema();
        let resolver = Resolver::new(&schema);
        let trait_def: orbital_core_base::schema::TraitDefinition =
            serde_json::from_value(serde_json::json!({
                "name": "ServerTrait",
                "stateMachine": {
                    "states": [{"name": "idle", "isInitial": true}],
                    "transitions": []
                }
            }))
            .expect("valid trait definition");
        let oir_trait = resolver.resolve_trait(&trait_def).expect("resolves");
        assert!(!oir_trait.local);
    }

    fn resolve_json(value: serde_json::Value) -> OirExpr {
        let sexpr: SExpression = serde_json::from_value(value).unwrap();
        resolve_sexpr_standalone(&sexpr).unwrap()
    }

    fn callback_arg(resolved: &OirExpr, op: &str, pos: usize) -> OirExpr {
        let OirExpr::Call { function, args } = resolved else {
            panic!("{op} must stay a call, got {resolved:?}");
        };
        assert_eq!(function, op);
        args[pos].clone()
    }

    fn lambda_params(expr: &OirExpr) -> Option<Vec<String>> {
        match expr {
            OirExpr::Lambda { params, .. } => Some(params.clone()),
            _ => None,
        }
    }

    /// A callback written as a raw body (no `(fn …)`) is evaluated with `@item`/`@index`
    /// (`@acc`/`@item` for reduce) bound. Resolve lowers it to that same lambda, so a
    /// backend never sees a body whose `@item` it has no binding for (std-algo-gameoflife:
    /// the compiled page threw `item is not defined`).
    #[test]
    fn raw_body_callbacks_lower_to_the_lambda_the_evaluator_binds() {
        let filter = resolve_json(serde_json::json!([
            "array/filter",
            "@entity.cells",
            ["==", ["object/get", "@item", "value"], 1]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&filter, "array/filter", 1)),
            Some(vec!["item".to_string(), "index".to_string()])
        );

        let map = resolve_json(serde_json::json!([
            "array/map",
            "@entity.cells",
            [
                "let",
                [["r", ["object/get", "@item", "row"]]],
                ["+", "@r", "@index"]
            ]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&map, "array/map", 1)),
            Some(vec!["item".to_string(), "index".to_string()])
        );

        let reduce = resolve_json(serde_json::json!([
            "array/reduce",
            "@entity.cells",
            0,
            ["+", "@acc", "@item"]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&reduce, "array/reduce", 2)),
            Some(vec!["acc".to_string(), "item".to_string()])
        );

        let count = resolve_json(serde_json::json!([
            "array/count",
            "@entity.cells",
            [">", "@index", 0]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&count, "array/count", 1)),
            Some(vec!["item".to_string(), "index".to_string()])
        );
    }

    #[test]
    fn an_explicit_lambda_keeps_its_own_params() {
        let filter = resolve_json(serde_json::json!([
            "array/filter",
            "@entity.cells",
            ["fn", "cell", ["==", "@cell.value", 1]]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&filter, "array/filter", 1)),
            Some(vec!["cell".to_string()])
        );
    }

    /// A raw value that binds no per-element name is evaluated once and applied when it
    /// yields a function (std-stats' configurable predicate slot), so it stays a value.
    #[test]
    fn a_raw_callback_value_that_binds_no_element_name_stays_a_value() {
        let filter = resolve_json(serde_json::json!([
            "array/filter",
            "@entity.rows",
            ["object/get", "@config", "pred"]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&filter, "array/filter", 1)),
            None
        );
    }

    #[test]
    fn a_raw_body_is_lowered_only_in_the_callback_slot() {
        // The array slot references @item from an enclosing lambda; it is not a callback.
        let filter = resolve_json(serde_json::json!([
            "array/filter",
            ["object/get", "@item", "children"],
            ["fn", "c", true]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&filter, "array/filter", 0)),
            None
        );
        // findIndex takes only a real lambda; its raw body is not given one.
        let find_index = resolve_json(serde_json::json!([
            "array/findIndex",
            "@entity.cells",
            [">", "@item", 1]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&find_index, "array/findIndex", 1)),
            None
        );
    }

    #[test]
    fn nested_raw_bodies_each_bind_their_own_item() {
        let map = resolve_json(serde_json::json!([
            "array/map",
            "@entity.rows",
            [
                "array/len",
                ["array/filter", "@entity.rows", ["==", "@item", 1]]
            ]
        ]));
        assert_eq!(
            lambda_params(&callback_arg(&map, "array/map", 1)),
            Some(vec!["item".to_string(), "index".to_string()])
        );
        let OirExpr::Lambda { body, .. } = callback_arg(&map, "array/map", 1) else {
            unreachable!()
        };
        let OirExpr::Call { args: len_args, .. } = *body else {
            panic!("array/len call expected")
        };
        assert_eq!(
            lambda_params(&callback_arg(&len_args[0], "array/filter", 1)),
            Some(vec!["item".to_string(), "index".to_string()])
        );
    }
}
