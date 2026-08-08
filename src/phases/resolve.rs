//! Resolve Phase
//!
//! Resolves all references and produces the final OIR (Orbital Intermediate Representation).

use orbital_core::ir::{
    OirBinaryOp, OirBinding, OirBindingRoot, OirCardinality, OirConfigFieldType, OirConstraint, OirEffect, OirEmit,
    OirEmitConfig,
    OirEntity, OirEvent, OirEventScope, OirExpr, OirField, OirInterval, OirListener, OirMetadata,
    OirModule, OirPage, OirPattern, OirPayloadField, OirPersistence, OirRelation, OirSection,
    OirState, OirTick, OirTrait, OirTraitBinding, OirTraitSource, OirTransition, OirTransitionFrom,
    OirType, OirValue,
};
use orbital_core::schema::{
    Cardinality, EntityDefinition, EntityRef, FieldDefinition, FieldType, OrbitalSchema,
    PageDefinition, PageRef, SExpression, SectionDefinition, ServiceRef, StringOrArray, ThemeRef,
    TraitDefinition, TraitReference, TransitionDefinition,
};
use std::collections::HashMap;
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

/// Resolve a schema to OIR
pub fn resolve_to_oir(schema: &OrbitalSchema) -> Result<OirModule, ResolveError> {
    let mut resolver = Resolver::new(schema);
    resolver.resolve()
}

/// Lower a standalone EXPRESSION `SExpression` to `OirExpr` through the ONE
/// canonical lowering — for codegen paths (render-ui operator-valued props)
/// that re-lower an already-resolved expression rather than keeping a parallel
/// emitter. Uses an empty schema: the expression arms (comparison/logic/
/// arithmetic/array/object/str/time/validate/bindings/literals/`fn`/`cond`)
/// never read it; only the effect arms (`emit`/`persist`/`fetch`/…) do, and
/// effects never appear in an expression-valued prop. Returns `Err` for shapes
/// the lowering rejects (the caller falls back to literal handling).
pub fn resolve_sexpr_standalone(sexpr: &SExpression) -> Result<OirExpr, ResolveError> {
    let empty = OrbitalSchema {
        name: String::new(),
        version: None,
        description: None,
        orbitals: Vec::new(),
        metadata: None,
        ledger: None,
        schema_version: None,
    };
    Resolver::new(&empty).resolve_sexpr(sexpr)
}

/// Substrate operators — async ops that reach outside the app via
/// `getSubstrateService()` (`llm/*`, `workspace/*`, `session/*`, `memory/*`,
/// `trace/*`, `behavior/*`, `compose/*`, `integration/*`, `validate/validate`).
/// Valid only as effects (statement position) with an `emit: { success,
/// failure }` options object; `resolve_sexpr_call` rejects them in expression
/// (value) position. The effect-resolution arm routes them to
/// `OirEffect::Custom`. Delegates to the canonical predicate in
/// `orbital_core::ir::is_substrate_operator` — the single source of truth
/// shared with the TS and Python shells (closes C-EMIT-SUBSTRATE-OPS-MARKER).
pub(crate) fn is_substrate_op(name: &str) -> bool {
    orbital_core::ir::is_substrate_operator(name)
}

/// Resolver state
pub struct Resolver<'a> {
    schema: &'a OrbitalSchema,
}

impl<'a> Resolver<'a> {
    fn new(schema: &'a OrbitalSchema) -> Self {
        Self { schema }
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
        module.description = self.schema.description.clone();

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
            for trait_ref in &orbital.traits {
                if let TraitReference::Inline(trait_def) = trait_ref {
                    let mut resolved = self.resolve_trait(trait_def)?;
                    resolved.orbital_name = orbital.name.clone();
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

        // Collect themes from orbitals (only inline themes, skip duplicates)
        let mut seen_themes = std::collections::HashSet::new();
        for orbital in &self.schema.orbitals {
            // Note: ThemeRef::Reference would be resolved by pre-processing
            if let Some(ThemeRef::Inline(theme)) = &orbital.theme {
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
                            orbital_core::schema::ServiceDefinition::Rest(r) => r.name.clone(),
                            orbital_core::schema::ServiceDefinition::Socket(s) => s.name.clone(),
                            orbital_core::schema::ServiceDefinition::Mcp(m) => m.name.clone(),
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
        // ALL orbitals, deduped by path. `label` = the page's `@label`, or
        // derived (strip a trailing `Page` suffix from `name`); `icon` = the
        // page's `@icon` when present.
        let page_entries: Vec<(String, String, Option<String>)> = {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut entries = Vec::new();
            for o in &self.schema.orbitals {
                if let Some(ps) = o.pages.as_ref() {
                    for pr in ps {
                        if let Some(p) = self.get_page_from_ref(pr) {
                            if p.path.contains(':') || !seen.insert(p.path.clone()) {
                                continue;
                            }
                            let label = p
                                .label
                                .clone()
                                .unwrap_or_else(|| derive_nav_label(&p.name));
                            entries.push((p.path.clone(), label, p.icon.clone()));
                        }
                    }
                }
            }
            entries
        };
        for oir_trait in &mut module.traits {
            let theme_key = self
                .schema
                .orbitals
                .iter()
                .find(|o| o.name == oir_trait.orbital_name)
                .and_then(|o| o.theme.as_ref())
                .map(theme_data_key)
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| DEFAULT_THEME_KEY.to_string());
            for trans in &mut oir_trait.transitions {
                for effect in &mut trans.effects {
                    let old = std::mem::replace(effect, OirExpr::Null);
                    *effect = substitute_schema_sigils(old, &page_entries, &theme_key);
                }
            }
        }

        Ok(module)
    }
}

/// The baseline theme `@currentTheme` falls back to when an orbital declares
/// no `theme`. Keeps standalone renders (no rabit) styled; rabit overrides
/// globally via `Orbital.theme`. A full `<base>-<variant>` registry key.
const DEFAULT_THEME_KEY: &str = "minimalist-light";

/// Derive a human nav label from a page's `name` when no `@label`
/// annotation is present: strip a trailing `Page` suffix
/// (`ContactsPage` → `Contacts`). The name is always present (the lowerer
/// derives one from the path when `as` is omitted).
fn derive_nav_label(name: &str) -> String {
    if let Some(stripped) = name.strip_suffix("Page") {
        if !stripped.is_empty() {
            return stripped.to_string();
        }
    }
    name.to_string()
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

/// Recursively substitute render-resolved schema sigils with their concrete
/// values. `@pages` → a `NavItem[]` array; `@currentTheme` → the `data-theme`
/// key string. Every other expression form is rebuilt with substituted
/// children so the sigils resolve anywhere they appear (including inside
/// conditional/computed overrides like `(if @user.prefersDark "x" @currentTheme)`).
fn substitute_schema_sigils(
    expr: OirExpr,
    pages: &[(String, String, Option<String>)],
    theme_key: &str,
) -> OirExpr {
    let sub = |e: OirExpr| substitute_schema_sigils(e, pages, theme_key);
    match expr {
        OirExpr::Binding { binding } => match binding.root {
            OirBindingRoot::Pages => OirExpr::array(
                pages
                    .iter()
                    .map(|(path, label, icon)| {
                        let mut fields = HashMap::new();
                        fields.insert("href".to_string(), OirExpr::string(path.clone()));
                        fields.insert("label".to_string(), OirExpr::string(label.clone()));
                        if let Some(ic) = icon {
                            fields.insert("icon".to_string(), OirExpr::string(ic.clone()));
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
        OirExpr::Call { function, args } => {
            OirExpr::Call { function, args: args.into_iter().map(sub).collect() }
        }
        OirExpr::Map { source, variable, body } => OirExpr::Map {
            source: Box::new(sub(*source)),
            variable,
            body: Box::new(sub(*body)),
        },
        OirExpr::Filter { source, variable, predicate } => OirExpr::Filter {
            source: Box::new(sub(*source)),
            variable,
            predicate: Box::new(sub(*predicate)),
        },
        OirExpr::Reduce { source, variable, accumulator, initial, body } => OirExpr::Reduce {
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
            OirExpr::Effect { effect: Box::new(eff) }
        }
        // Literals — pass through.
        other => other,
    }
}

/// The `OirValue` twin of `substitute_schema_sigils` — render-ui props are
/// stored as `OirValue`s, so `@pages`/`@currentTheme` appearing there need
/// their own recursive substitution.
fn substitute_value_sigils(
    value: OirValue,
    pages: &[(String, String, Option<String>)],
    theme_key: &str,
) -> OirValue {
    match value {
        OirValue::String(s) if s == "@pages" => OirValue::Array(
            pages
                .iter()
                .map(|(path, label, icon)| {
                    let mut fields: Vec<(String, OirValue)> = vec![
                        ("href".to_string(), OirValue::String(path.clone())),
                        ("label".to_string(), OirValue::String(label.clone())),
                    ];
                    if let Some(ic) = icon {
                        fields.push(("icon".to_string(), OirValue::String(ic.clone())));
                    }
                    OirValue::Object(fields.into_iter().collect())
                })
                .collect(),
        ),
        OirValue::String(s) if s == "@currentTheme" => OirValue::String(theme_key.to_string()),
        OirValue::Array(items) => OirValue::Array(
            items.into_iter().map(|v| substitute_value_sigils(v, pages, theme_key)).collect(),
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
                orbital_core::schema::Persistence::Persistent => OirPersistence::Persistent,
                orbital_core::schema::Persistence::Runtime => OirPersistence::Runtime,
            };
        }

        oir_entity.shared = entity.shared;
        oir_entity.identity = entity.identity;

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
                let mut oir_instance = HashMap::new();
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

        let required = field.required.unwrap_or(true);
        let mut oir_field = if required {
            OirField::new(&field_name, field_type)
        } else {
            OirField::optional(&field_name, field_type)
        };

        // Add enum values
        oir_field.enum_values = field.values.clone();

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

    fn resolve_field_type(
        &self,
        field_type: &FieldType,
        relation: Option<&orbital_core::schema::RelationDefinition>,
    ) -> Result<OirType, ResolveError> {
        match field_type {
            FieldType::String => Ok(OirType::String),
            FieldType::Number => Ok(OirType::Number),
            FieldType::Boolean => Ok(OirType::Boolean),
            FieldType::Date => Ok(OirType::Date),
            FieldType::Datetime | FieldType::Timestamp => Ok(OirType::DateTime),
            FieldType::Email => Ok(OirType::Email),
            FieldType::Url => Ok(OirType::Url),
            FieldType::Phone => Ok(OirType::Phone),
            FieldType::Uuid => Ok(OirType::Uuid),
            FieldType::Image => Ok(OirType::Image),
            FieldType::Array => Ok(OirType::array(OirType::Json)), // Nested type would need items
            FieldType::Object => Ok(OirType::Json),
            FieldType::Enum => Ok(OirType::enum_type("Enum")),
            FieldType::Relation => {
                if let Some(rel) = relation {
                    Ok(OirType::reference(&rel.entity))
                } else {
                    Err(ResolveError::InvalidType(
                        "Relation type requires relation definition".to_string(),
                    ))
                }
            }
            // `trait`-typed config fields hold `@trait.X` references. The OIR
            // preserves the type tag so the codegen emits the branded
            // `TraitFieldRef` TS type and substitutes `@trait.X` values with
            // embedded trait UI at render time.
            FieldType::Trait => Ok(OirType::Trait),
            // `slot` and `pattern` are validated at compile time but resolve to
            // strings in OIR — the runtime doesn't need the type tag.
            FieldType::Slot | FieldType::Pattern => Ok(OirType::String),
        }
    }

    /// Like `resolve_field_type` but reads `field.items` and `field.properties`
    /// so arrays-of-structs and inline objects keep their shape in OIR
    /// (`OirType::Struct { fields }`). Used during entity-field resolution
    /// — the legacy `resolve_field_type` is retained for callers that only
    /// have a `FieldType` enum.
    fn resolve_field_shape(&self, field: &FieldDefinition) -> Result<OirType, ResolveError> {
        match &field.field_type {
            FieldType::Array => {
                let inner = if let Some(items) = &field.items {
                    self.resolve_field_shape(items)?
                } else {
                    OirType::Json
                };
                Ok(OirType::array(inner))
            }
            FieldType::Object => {
                if let Some(props) = &field.properties {
                    let mut struct_fields: Vec<OirField> = Vec::new();
                    for (name, def) in props {
                        let mut nested = self.resolve_field(def)?;
                        if nested.name.is_empty() {
                            nested.name = name.clone();
                        }
                        struct_fields.push(nested);
                    }
                    struct_fields.sort_by(|a, b| a.name.cmp(&b.name));
                    Ok(OirType::Struct {
                        fields: struct_fields,
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
            other => self.resolve_field_type(other, field.relation.as_ref()),
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
    fn resolve_config_field_shape(&self, field: &orbital_core::schema::ConfigField) -> OirType {
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
        if let Some(props) = &field.properties {
            let mut struct_fields: Vec<OirField> = Vec::new();
            for (name, def) in props {
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
        oir_trait.entity_ref_ids = trait_def.entity_ref_ids.clone();
        oir_trait.trait_embed_ids = trait_def.trait_embed_ids.clone();
        oir_trait.source = OirTraitSource::Schema;

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
                    let mut schema = std::collections::HashMap::new();
                    let mut required = std::collections::BTreeSet::new();
                    for field in payload_fields {
                        let oir_type = payload_field_to_oir_type(field);
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
                        if existing.payload_schema.is_none() && oir_event.payload_schema.is_some() {
                            *existing = oir_event;
                        }
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
                if let Some(slot) = self.extract_slot_from_effect(effect) {
                    slots_used.insert(slot);
                }
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
                        orbital_core::schema::ListenSource::Any => {
                            oir.source_any = true;
                        }
                        orbital_core::schema::ListenSource::Trait {
                            trait_name,
                            trait_id,
                        } => {
                            oir.source_trait = Some(trait_name.clone());
                            oir.source_trait_id = trait_id.clone();
                        }
                        orbital_core::schema::ListenSource::Orbital {
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
                    let mut resolved = std::collections::HashMap::new();
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
                let mut schema = std::collections::HashMap::new();
                let mut defaults = std::collections::HashMap::new();
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
        emit: &orbital_core::schema::EmitDefinition,
    ) -> Result<OirEmit, ResolveError> {
        let mut oir_emit = OirEmit::new(&emit.event);
        oir_emit.event_id = emit.event_id.clone();
        oir_emit.description = emit.description.clone();

        // Convert scope
        if let Some(scope) = &emit.scope {
            oir_emit.scope = match scope {
                orbital_core::schema::EventScope::Internal => OirEventScope::Internal,
                orbital_core::schema::EventScope::External => OirEventScope::External,
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
        tick: &orbital_core::schema::TickDefinition,
    ) -> Result<OirTick, ResolveError> {
        let interval = match &tick.interval {
            orbital_core::schema::StringOrNumber::String(s) => {
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
                } else if orbital_core::cron::is_valid_cron_expression(s) {
                    // A real 5-field cron expression (e.g. "0 9 * * *") —
                    // previously silently collapsed to `Seconds { value: 1 }`.
                    OirInterval::Cron(s.clone())
                } else {
                    return Err(ResolveError::InvalidTickInterval(s.clone()));
                }
            }
            orbital_core::schema::StringOrNumber::Number(n) => {
                OirInterval::Milliseconds { value: *n as u64 }
            }
        };

        let mut oir_tick = OirTick::new(&tick.name, interval);

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
        match sexpr {
            SExpression::Null => Ok(OirExpr::null()),
            SExpression::Bool(b) => Ok(OirExpr::bool(*b)),
            SExpression::Number(n) => Ok(OirExpr::number(*n)),
            SExpression::String(s) => {
                // Check if it's a binding
                if s.starts_with('@') {
                    if let Some(binding) = OirBinding::parse(s) {
                        return Ok(OirExpr::binding(binding));
                    }
                }
                Ok(OirExpr::string(s))
            }
            SExpression::Array(arr) => {
                // Disambiguate Lisp call form `[op, arg1, arg2, …]` from a
                // string-array literal `["a", "b", "c"]`. Both look identical
                // in JSON. The OIR resolver matches a finite set of operator
                // names (its own match arms in resolve_sexpr_call) AND the
                // evaluator's runtime operator registry handles a wider
                // namespaced set (str/concat, math/map, agent/*, os/*, …).
                //
                // Use `orbital_core::evaluator::get_operator` as the single
                // source of truth. If the head string is recognised as an
                // operator there OR matched by resolve_sexpr_call's own
                // arms, treat as a call. Otherwise it's a string-array
                // literal.
                //
                // Prevents G44 (`["positive", "negative", "neutral"]` →
                // `positive("negative", "neutral")` and TS2304) without
                // misclassifying `str/concat`, `agent/generate`, etc.
                if let Some(SExpression::String(op)) = arr.first() {
                    // Discriminate against the CANONICAL operator registry
                    // (`@almadar/std` canonical-operators.json, baked via the sync
                    // tool) — the single source of record — first, then the
                    // evaluator/effect registries for aliases not yet declared
                    // canonical. Using only the hand-maintained evaluator registry
                    // here silently DEMOTED real canonical ops it happened to miss
                    // (`array/count`, `time/relative`) to string-array literals
                    // (G-LOLO-OP-ARRAY-DEMOTION). Corpus-verified: every
                    // canonical-headed array is a call; data lists never use a
                    // canonical head.
                    let is_known = crate::embedded::is_known_operator(op)
                        || orbital_core::evaluator::get_operator(op).is_some()
                        || orbital_core::effects::is_known_effect_name(op);
                    if is_known {
                        return self.resolve_sexpr_call(op, &arr[1..]);
                    }
                }

                // Regular array literal
                let items = arr
                    .iter()
                    .map(|item| self.resolve_sexpr(item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OirExpr::array(items))
            }
            SExpression::Object(obj) => {
                let mut fields = HashMap::new();
                for (key, value) in obj {
                    fields.insert(key.clone(), self.resolve_sexpr(value)?);
                }
                Ok(OirExpr::object(fields))
            }
        }
    }

    fn resolve_sexpr_call(&self, op: &str, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Enforce canonical maxArity (canonical-operators.json). Variadic ops
        // (maxArity null) skip. Catches the silent-drop bug in binary_op /
        // unary_op, which keep only the first 1–2 args and discard the rest
        // (C-EMIT-MAX-ARITY-SILENT-DROP). Aliases absent from the canonical
        // registry (`add`, `&&`, …) return None here and are still covered by
        // the per-arm binary/unary minimum checks below.
        if let Some(meta) = crate::embedded::get_operator(op) {
            // Effect operators are async EFFECTS — like fetch/persist
            // (EFFECT_OPERATORS) they carry a trailing `{ emit }` envelope
            // that is not part of the operator's logical arity and have their
            // own validation, so they skip the static value-operator
            // max-arity check. Covers substrate ops (llm/*, workspace/*, …)
            // and every registry op flagged `has_side_effects` (os/*,
            // browser/*, set, …). Consistent with `get_arity_rule` (sexpr
            // validator), which returns None for them (C-EMIT-SUBSTRATE).
            if !is_substrate_op(op) && !meta.has_side_effects {
                if let Some(max) = meta.max_arity {
                    if (args.len() as u32) > max {
                        return Err(ResolveError::InvalidSExpr(format!(
                            "Operator '{op}' takes at most {max} argument(s), got {}",
                            args.len()
                        )));
                    }
                }
            }
        }

        match op {
            // Comparison operators
            "=" | "==" | "eq" => self.binary_op(args, OirExpr::eq),
            "!=" | "ne" => self.binary_op(args, OirExpr::ne),
            "<" | "lt" => self.binary_op(args, OirExpr::lt),
            ">" | "gt" => self.binary_op(args, OirExpr::gt),
            "<=" | "le" => self.binary_op(args, |l, r| OirExpr::BinaryOp {
                op: OirBinaryOp::Le,
                left: Box::new(l),
                right: Box::new(r),
            }),
            ">=" | "ge" => self.binary_op(args, |l, r| OirExpr::BinaryOp {
                op: OirBinaryOp::Ge,
                left: Box::new(l),
                right: Box::new(r),
            }),

            // Logical operators
            "and" | "&&" => self.variadic_binary_op(args, OirExpr::and),
            "or" | "||" => self.variadic_binary_op(args, OirExpr::or),
            "not" | "!" => self.unary_op(args, OirExpr::not),

            // Arithmetic operators
            "+" | "add" => self.binary_op(args, OirExpr::add),
            "-" | "sub" => self.binary_op(args, OirExpr::sub),
            "*" | "mul" => self.binary_op(args, |l, r| OirExpr::BinaryOp {
                op: OirBinaryOp::Mul,
                left: Box::new(l),
                right: Box::new(r),
            }),
            "/" | "div" => self.binary_op(args, |l, r| OirExpr::BinaryOp {
                op: OirBinaryOp::Div,
                left: Box::new(l),
                right: Box::new(r),
            }),
            "%" | "mod" => self.binary_op(args, |l, r| OirExpr::BinaryOp {
                op: OirBinaryOp::Mod,
                left: Box::new(l),
                right: Box::new(r),
            }),

            // Conditional
            "if" => {
                if args.len() >= 3 {
                    let cond = self.resolve_sexpr(&args[0])?;
                    let then_branch = self.resolve_sexpr(&args[1])?;
                    let else_branch = self.resolve_sexpr(&args[2])?;
                    Ok(OirExpr::if_then_else(cond, then_branch, else_branch))
                } else {
                    Err(ResolveError::InvalidSExpr(
                        "if requires 3 arguments".to_string(),
                    ))
                }
            }

            // Effects
            "emit" => self.resolve_emit_effect(args),
            "persist" => self.resolve_persist_effect(args),
            "fetch" => self.resolve_fetch_effect(args),
            "fetch-stream" => self.resolve_fetch_stream_effect(args),
            "send-server" => self.resolve_send_server_effect(args),
            "set" => self.resolve_set_effect(args),
            "navigate" => self.resolve_navigate_effect(args),
            "render-ui" => self.resolve_render_ui_effect(args),
            "notify" => self.resolve_notify_effect(args),
            "log" => self.resolve_log_effect(args),
            "call-service" | "call_service" => self.resolve_call_service_effect(args),

            // Resource operators
            "ref" => self.resolve_ref_effect(args),
            "deref" => self.resolve_deref_effect(args),
            "swap!" => self.resolve_swap_effect(args),
            "watch" => self.resolve_watch_effect(args),
            "atomic" => self.resolve_atomic_effect(args),

            // OS trigger effects (server-side only)
            //
            // Each may accept a trailing options object carrying `emit:`:
            //   ["os/watch-http", urlPattern, method?, { emit: {...} }]
            //   ["os/watch-cron", expr,               { emit: {...} }]
            //
            // The options object is detected by being an `Object` *and*
            // carrying an `emit` key, so existing 2-arg forms where arg[1]
            // is a non-emit options map keep working unchanged.
            "os/watch-files" => {
                let glob = self.resolve_sexpr(&args[0])?;
                let (options, emit_cfg) = self.split_watch_trailing(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::OsWatchFiles {
                    glob,
                    options,
                    emit: emit_cfg,
                }))
            }
            "os/watch-process" => {
                let name = self.resolve_sexpr(&args[0])?;
                let (subcommand, emit_cfg) = self.split_watch_trailing(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::OsWatchProcess {
                    name,
                    subcommand,
                    emit: emit_cfg,
                }))
            }
            "os/watch-port" => {
                let port = self.resolve_sexpr(&args[0])?;
                let (protocol, emit_cfg) = self.split_watch_trailing(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::OsWatchPort {
                    port,
                    protocol,
                    emit: emit_cfg,
                }))
            }
            "os/watch-http" => {
                let url_pattern = self.resolve_sexpr(&args[0])?;
                let (method, emit_cfg) = self.split_watch_trailing(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::OsWatchHttp {
                    url_pattern,
                    method,
                    emit: emit_cfg,
                }))
            }
            "os/watch-cron" => {
                let expression = self.resolve_sexpr(&args[0])?;
                let emit_cfg = self.extract_watch_only_emit(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::OsWatchCron {
                    expression,
                    emit: emit_cfg,
                }))
            }
            "os/watch-signal" => {
                let signal = self.resolve_sexpr(&args[0])?;
                let emit_cfg = self.extract_watch_only_emit(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::OsWatchSignal {
                    signal,
                    emit: emit_cfg,
                }))
            }
            "os/watch-env" => {
                let variable = self.resolve_sexpr(&args[0])?;
                let emit_cfg = self.extract_watch_only_emit(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::OsWatchEnv {
                    variable,
                    emit: emit_cfg,
                }))
            }
            "os/debounce" => {
                let ms = self.resolve_sexpr(&args[0])?;
                let event_type = self.resolve_sexpr(&args[1])?;
                Ok(OirExpr::effect(OirEffect::OsDebounce { ms, event_type }))
            }

            // Browser device effects (client host path)
            //
            // User-initiated, async device APIs. Each accepts a trailing
            // `{ emit: { success, failure } }` envelope (mirroring os/watch-*):
            //   ["browser/open-file-picker", options?, { emit? }]
            //   ["browser/clipboard-read",            { emit? }]
            //   ["browser/clipboard-write", text,     { emit? }]
            //   ["browser/geolocation-current", options?, { emit? }]
            "browser/open-file-picker" => {
                let (options, emit_cfg) = self.split_watch_trailing(args)?;
                Ok(OirExpr::effect(OirEffect::BrowserOpenFilePicker {
                    options,
                    emit: emit_cfg,
                }))
            }
            "browser/clipboard-read" => {
                let emit_cfg = self.extract_watch_only_emit(args)?;
                Ok(OirExpr::effect(OirEffect::BrowserClipboardRead {
                    emit: emit_cfg,
                }))
            }
            "browser/clipboard-write" => {
                if args.is_empty() {
                    return Err(ResolveError::InvalidSExpr(
                        "browser/clipboard-write requires text".to_string(),
                    ));
                }
                let text = self.resolve_sexpr(&args[0])?;
                let emit_cfg = self.extract_watch_only_emit(&args[1..])?;
                Ok(OirExpr::effect(OirEffect::BrowserClipboardWrite {
                    text,
                    emit: emit_cfg,
                }))
            }
            "browser/geolocation-current" => {
                let (options, emit_cfg) = self.split_watch_trailing(args)?;
                Ok(OirExpr::effect(OirEffect::BrowserGeolocationCurrent {
                    options,
                    emit: emit_cfg,
                }))
            }

            // Agent operators — lowered as Custom effects so backend can codegen TypeScript
            op_name if op_name.starts_with("agent/") => {
                let resolved_args: Vec<OirExpr> = args
                    .iter()
                    .map(|a| self.resolve_sexpr(a))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut args_map = std::collections::HashMap::new();
                for (i, arg) in resolved_args.into_iter().enumerate() {
                    args_map.insert(format!("arg{}", i), arg);
                }
                Ok(OirExpr::effect(OirEffect::Custom {
                    name: op_name.to_string(),
                    args: args_map,
                    emit: None,
                }))
            }

            // Substrate operators — same pattern as agent/*: lowered as
            // Custom effects so the codegen backend can emit TypeScript
            // service-accessor calls. Without this routing, these would
            // fall through to `OirExpr::Call` and produce invalid TS
            // (bare `llm/generate(...)` with a `/` in the function name).
            op_name if is_substrate_op(op_name) => {
                // A trailing `{ emit: { success, failure } }` options object
                // (mirroring fetch/persist) carries the completion events.
                // The op fires `getSubstrateService().invoke(name, args)` and
                // dispatches the success/failure event on completion — the
                // `.lolo` async model. Without it the op is fire-and-forget.
                // Only treated as options when the object has an `emit` key,
                // so a plain object argument is still passed positionally
                // (C-EMIT-SUBSTRATE-OPS-MARKER).
                let (positional, emit_cfg) = match args.last() {
                    Some(SExpression::Object(obj)) if obj.contains_key("emit") => {
                        (&args[..args.len() - 1], self.extract_emit_config(obj)?)
                    }
                    _ => (args, None),
                };
                let resolved_args: Vec<OirExpr> = positional
                    .iter()
                    .map(|a| self.resolve_sexpr(a))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut args_map = std::collections::HashMap::new();
                for (i, arg) in resolved_args.into_iter().enumerate() {
                    args_map.insert(format!("arg{}", i), arg);
                }
                Ok(OirExpr::effect(OirEffect::Custom {
                    name: op_name.to_string(),
                    args: args_map,
                    emit: emit_cfg,
                }))
            }

            // Function call (also covers namespaced operators like
            // `str/concat`, `crypto/hash`, … that downstream codegen
            // dispatches via OirExpr::Call). String-array literals are
            // diverted at the resolve_sexpr Array case BEFORE reaching
            // here, so this arm only fires for genuine call forms.
            // Lambda — the single shared representation (OirExpr::Lambda) every
            // backend consumes, replacing per-site `Call{function:"fn"}` parsing.
            // Post-normalization the shape is `[params, body]` (params = a string
            // for a single param or an array for the grouped form); the flat
            // `[p1, …, pN, body]` form is handled defensively.
            "fn" | "lambda" => {
                if args.len() < 2 {
                    return Err(ResolveError::InvalidSExpr(format!(
                        "{op} lambda requires parameters and a body"
                    )));
                }
                let (param_slice, body_sexpr) = if args.len() == 2 {
                    (std::slice::from_ref(&args[0]), &args[1])
                } else {
                    (&args[..args.len() - 1], &args[args.len() - 1])
                };
                let mut params = Vec::new();
                for p in param_slice {
                    match p {
                        SExpression::String(s) => params.push(s.clone()),
                        SExpression::Array(ps) => {
                            for pp in ps {
                                if let SExpression::String(s) = pp {
                                    params.push(s.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let body = self.resolve_sexpr(body_sexpr)?;
                Ok(OirExpr::lambda(params, body))
            }

            _ => {
                let resolved_args = args
                    .iter()
                    .map(|a| self.resolve_sexpr(a))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(OirExpr::call(op, resolved_args))
            }
        }
    }

    /// Left-fold a genuinely associative operator over ALL its arguments:
    /// `(or A B C)` → `Or(Or(A, B), C)`.
    ///
    /// `binary_op` reads only `args[0..2]`. That is correct for a fixed-arity
    /// operator, whose excess args the max-arity check above already rejects —
    /// but `and`/`or` are declared VARIADIC, so they sail past that check and
    /// then lost every argument from the third on, silently. A three-branch
    /// `(or supervisor-clause agent-clause customer-clause)` lowered to just
    /// the first two, so the third branch could never match and the guard
    /// denied a viewer the source plainly allowed.
    fn variadic_binary_op<F>(&self, args: &[SExpression], f: F) -> Result<OirExpr, ResolveError>
    where
        F: Fn(OirExpr, OirExpr) -> OirExpr,
    {
        if args.len() < 2 {
            return Err(ResolveError::InvalidSExpr(
                "Binary operation requires 2 arguments".to_string(),
            ));
        }
        let mut acc = self.resolve_sexpr(&args[0])?;
        for arg in &args[1..] {
            acc = f(acc, self.resolve_sexpr(arg)?);
        }
        Ok(acc)
    }

    fn binary_op<F>(&self, args: &[SExpression], f: F) -> Result<OirExpr, ResolveError>
    where
        F: FnOnce(OirExpr, OirExpr) -> OirExpr,
    {
        if args.len() < 2 {
            return Err(ResolveError::InvalidSExpr(
                "Binary operation requires 2 arguments".to_string(),
            ));
        }
        let left = self.resolve_sexpr(&args[0])?;
        let right = self.resolve_sexpr(&args[1])?;
        Ok(f(left, right))
    }

    fn unary_op<F>(&self, args: &[SExpression], f: F) -> Result<OirExpr, ResolveError>
    where
        F: FnOnce(OirExpr) -> OirExpr,
    {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "Unary operation requires 1 argument".to_string(),
            ));
        }
        let operand = self.resolve_sexpr(&args[0])?;
        Ok(f(operand))
    }

    // Effect resolution helpers
    fn resolve_emit_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "emit requires event name".to_string(),
            ));
        }
        let event = self.sexpr_as_str(&args[0])?;
        let payload = if args.len() > 1 {
            Some(self.resolve_sexpr(&args[1])?)
        } else {
            None
        };
        Ok(OirExpr::effect(if let Some(p) = payload {
            OirEffect::emit_with_payload(event, p)
        } else {
            OirEffect::emit(event)
        }))
    }

    fn resolve_persist_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Format: ["persist", operation, entity, ...args]
        // Examples:
        //   ["persist", "create", "Task", "@payload.data"]
        //   ["persist", "update", "Task", "@payload.id", "@payload.data"]
        //   ["persist", "delete", "Task", "@payload.row.id"]
        //   ["persist", "update", "Task", "@entity", { "emit": {...} }]
        if args.len() < 2 {
            return Err(ResolveError::InvalidSExpr(
                "persist requires operation and entity".to_string(),
            ));
        }
        let operation = self.sexpr_as_str(&args[0])?;
        let entity = self.sexpr_as_str(&args[1])?;

        // The trailing arg may be an options object carrying `emit:`. When
        // present, the data arg shifts one position back. Options objects
        // are detected by presence of `emit:` (or being an empty object
        // that is not a data payload). A plain object with no `emit:` is
        // still treated as data (e.g. inline record for create/update).
        let mut data_end = args.len();
        let mut emit_cfg: Option<OirEmitConfig> = None;

        if args.len() > 3 {
            if let SExpression::Object(obj) = &args[args.len() - 1] {
                if obj.contains_key("emit") {
                    emit_cfg = self.extract_emit_config(obj)?;
                    data_end -= 1;
                }
            }
        }

        // Get data argument (may be at different positions based on operation)
        let data = if data_end > 2 {
            self.resolve_sexpr(&args[data_end - 1])?
        } else {
            OirExpr::object(std::collections::HashMap::new())
        };

        let mut effect = match operation.as_str() {
            "create" => OirEffect::create(&entity, data),
            "update" => OirEffect::update(&entity, data),
            "delete" => OirEffect::delete(&entity, data),
            "read" | "get" => OirEffect::read(&entity, data),
            "list" => OirEffect::list(&entity, data),
            "query" => OirEffect::query(&entity, data),
            _ => OirEffect::update(&entity, data), // default to update
        };

        if let OirEffect::Persist {
            emit: ref mut em, ..
        } = effect
        {
            *em = emit_cfg;
        }

        Ok(OirExpr::effect(effect))
    }

    fn resolve_fetch_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "fetch requires entity name".to_string(),
            ));
        }
        let entity = self.sexpr_as_str(&args[0])?;

        // Check if args[1] is an options object with { id, include, filter, ... }
        // This supports: ["fetch", "Entity", { "id": "@payload.id" }]
        //                 ["fetch", "Entity", { "filter": ..., "include": [...] }]
        if args.len() > 1 {
            if let SExpression::Object(obj) = &args[1] {
                // Extract id (single-entity fetch)
                let id = if let Some(id_expr) = obj.get("id") {
                    match id_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(id_expr)?),
                    }
                } else {
                    None
                };

                // Extract include array
                let include = if let Some(SExpression::Array(arr)) = obj.get("include") {
                    let fields: Vec<String> = arr
                        .iter()
                        .filter_map(|v| {
                            if let SExpression::String(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if fields.is_empty() {
                        None
                    } else {
                        Some(fields)
                    }
                } else {
                    None
                };

                // Extract filter from options object
                let filter = if let Some(filter_expr) = obj.get("filter") {
                    match filter_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(filter_expr)?),
                    }
                } else {
                    None
                };

                // Extract order_by from options object. Same Null-skip
                // shape as filter; the codegen accepts both static
                // "field:dir" string literals and dynamic SExpressions.
                let order_by = if let Some(order_expr) = obj.get("orderBy") {
                    match order_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(order_expr)?),
                    }
                } else {
                    None
                };

                // Extract offset (pagination start). Captured as OirExpr
                // because schema authors compute it dynamically — e.g.
                // `(* (- @payload.page 1) @config.pageSize)`.
                let offset = if let Some(offset_expr) = obj.get("offset") {
                    match offset_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(offset_expr)?),
                    }
                } else {
                    None
                };

                // Extract limit (pagination size). Same OirExpr capture as
                // offset — `@config.pageSize` is the canonical form.
                let limit = if let Some(limit_expr) = obj.get("limit") {
                    match limit_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(limit_expr)?),
                    }
                } else {
                    None
                };

                let emit_cfg = self.extract_emit_config(obj)?;

                if let Some(id) = id {
                    // Fetch by ID: ["fetch", "Entity", { "id": "@payload.id" }]
                    let mut effect = OirEffect::fetch_by_id(&entity, id);
                    if let OirEffect::Fetch {
                        include: ref mut inc,
                        emit: ref mut em,
                        ..
                    } = effect
                    {
                        *inc = include;
                        *em = emit_cfg;
                    }
                    return Ok(OirExpr::effect(effect));
                }

                let mut effect = OirEffect::fetch_with_options(entity, filter, order_by, limit);
                if let OirEffect::Fetch {
                    offset: ref mut off,
                    include: ref mut inc,
                    emit: ref mut em,
                    ..
                } = effect
                {
                    *off = offset;
                    *inc = include;
                    *em = emit_cfg;
                }
                return Ok(OirExpr::effect(effect));
            }

            // Legacy shorthand: ["fetch", "Entity", "@payload.id"]
            // A string binding as the second arg is treated as an ID, not a filter.
            if let SExpression::String(s) = &args[1] {
                if s.starts_with('@') && s.contains('.') && s.ends_with("id") {
                    let id_expr = self.resolve_sexpr(&args[1])?;
                    return Ok(OirExpr::effect(OirEffect::fetch_by_id(entity, id_expr)));
                }
            }
        }

        // Legacy positional args: ["fetch", "Entity", filterExpr, orderByExpr, limit]
        let filter = if args.len() > 1 {
            match &args[1] {
                SExpression::Null => None,
                _ => Some(self.resolve_sexpr(&args[1])?),
            }
        } else {
            None
        };

        // args[2]: order_by expression
        let order_by = if args.len() > 2 {
            match &args[2] {
                SExpression::Null => None,
                _ => Some(self.resolve_sexpr(&args[2])?),
            }
        } else {
            None
        };

        // args[3]: limit expression. Captured as OirExpr so dynamic
        // bindings like `@config.pageSize` survive lowering. Static numeric
        // literals also flow through this path as `OirExpr::number`.
        let limit = if args.len() > 3 {
            match &args[3] {
                SExpression::Null => None,
                _ => Some(self.resolve_sexpr(&args[3])?),
            }
        } else {
            None
        };

        Ok(OirExpr::effect(OirEffect::fetch_with_options(
            entity, filter, order_by, limit,
        )))
    }

    fn resolve_fetch_stream_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "fetch-stream requires entity name".to_string(),
            ));
        }
        let entity = self.sexpr_as_str(&args[0])?;

        if args.len() > 1 {
            if let SExpression::Object(obj) = &args[1] {
                let id = if let Some(id_expr) = obj.get("id") {
                    match id_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(id_expr)?),
                    }
                } else {
                    None
                };
                let filter = if let Some(filter_expr) = obj.get("filter") {
                    match filter_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(filter_expr)?),
                    }
                } else {
                    None
                };
                let emit_cfg = self.extract_emit_config(obj)?;
                return Ok(OirExpr::effect(OirEffect::FetchStream {
                    entity_type: entity,
                    id,
                    filter,
                    emit: emit_cfg,
                }));
            }
        }

        Ok(OirExpr::effect(OirEffect::FetchStream {
            entity_type: entity,
            id: None,
            filter: None,
            emit: None,
        }))
    }

    fn resolve_send_server_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "send-server requires event name".to_string(),
            ));
        }
        let event = self.resolve_sexpr(&args[0])?;
        let payload = if args.len() > 1 {
            Some(self.resolve_sexpr(&args[1])?)
        } else {
            None
        };
        Ok(OirExpr::effect(OirEffect::SendServer { event, payload }))
    }

    fn resolve_set_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.len() < 2 {
            return Err(ResolveError::InvalidSExpr(
                "set requires target and value".to_string(),
            ));
        }

        // Optional trailing options object: ["set", target, value, { emit: {...} }]
        let emit_cfg = if args.len() > 2 {
            if let SExpression::Object(obj) = &args[2] {
                self.extract_emit_config(obj)?
            } else {
                None
            }
        } else {
            None
        };

        // Handle dynamic path expressions like ["path", "formValues", "@payload.fieldId"]
        // In this case, we resolve the path expression and use it as a dynamic binding
        if let SExpression::Array(arr) = &args[0] {
            if arr
                .first()
                .map(|v| matches!(v, SExpression::String(s) if s == "path"))
                .unwrap_or(false)
            {
                // Dynamic path - resolve the path expression and create a set with dynamic target
                let path_expr = self.resolve_sexpr(&args[0])?;
                let value = self.resolve_sexpr(&args[1])?;
                if matches!(value, OirExpr::Effect { .. }) {
                    return Err(ResolveError::InvalidSExpr(
                        "set cannot take an effect as a value — substrate/async operators (llm/generate, memory/store, …) are async and reach outside the app. Use them as a top-level effect with an emit options object, e.g. (llm/generate ... { emit: { success: E, failure: F } }), and capture the result via a transition on the success event (C-EMIT-SUBSTRATE-OPS-MARKER).".to_string()
                    ));
                }
                let mut effect = OirEffect::set_dynamic(path_expr, value);
                if let OirEffect::SetDynamic {
                    emit: ref mut em, ..
                } = effect
                {
                    *em = emit_cfg;
                }
                return Ok(OirExpr::effect(effect));
            }
        }

        let target_str = self.sexpr_as_str(&args[0])?;
        let target = OirBinding::parse(&target_str).ok_or_else(|| {
            ResolveError::InvalidSExpr(format!("Invalid binding: {}", target_str))
        })?;
        let value = self.resolve_sexpr(&args[1])?;
        if matches!(value, OirExpr::Effect { .. }) {
            return Err(ResolveError::InvalidSExpr(
                "set cannot take an effect as a value — substrate/async operators (llm/generate, memory/store, …) are async and reach outside the app. Use them as a top-level effect with an emit options object, e.g. (llm/generate ... { emit: { success: E, failure: F } }), and capture the result via a transition on the success event (C-EMIT-SUBSTRATE-OPS-MARKER).".to_string()
            ));
        }
        let mut effect = OirEffect::set(target, value);
        if let OirEffect::Set {
            emit: ref mut em, ..
        } = effect
        {
            *em = emit_cfg;
        }
        Ok(OirExpr::effect(effect))
    }

    fn resolve_navigate_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "navigate requires path".to_string(),
            ));
        }
        let path = self.resolve_sexpr(&args[0])?;
        Ok(OirExpr::effect(OirEffect::navigate(path)))
    }

    fn resolve_render_ui_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.len() < 2 {
            return Err(ResolveError::InvalidSExpr(
                "render-ui requires slot and pattern".to_string(),
            ));
        }
        let slot = self.sexpr_as_str(&args[0])?;

        // Handle null pattern - used to clear a slot
        let pattern = if matches!(&args[1], SExpression::Null) {
            OirPattern::new("clear")
        } else if let Ok(pattern_type) = self.sexpr_as_str(&args[1]) {
            OirPattern::new(pattern_type)
        } else if let SExpression::Object(obj) = &args[1] {
            let pattern_type = obj
                .get("type")
                .and_then(|v| {
                    if let SExpression::String(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("custom");
            let mut pattern = OirPattern::new(pattern_type);
            for (key, value) in obj {
                if key != "type" {
                    pattern
                        .config
                        .insert(key.clone(), self.sexpr_to_value(value));
                }
            }
            pattern
        } else {
            return Err(ResolveError::InvalidSExpr(
                "render-ui pattern must be string or object".to_string(),
            ));
        };

        Ok(OirExpr::effect(OirEffect::render_ui(slot, pattern)))
    }

    fn resolve_notify_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "notify requires message".to_string(),
            ));
        }
        let message = self.resolve_sexpr(&args[0])?;
        Ok(OirExpr::effect(OirEffect::notify(message)))
    }

    fn resolve_log_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "log requires message".to_string(),
            ));
        }
        let message = self.resolve_sexpr(&args[0])?;
        Ok(OirExpr::effect(OirEffect::log(
            orbital_core::ir::OirLogLevel::Info,
            message,
        )))
    }

    fn resolve_call_service_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Format: ["call-service", "serviceName", "action", { ...params }]
        // With emit: ["call-service", "svc", "action", argsExpr, { emit: {...} }]
        if args.len() < 2 {
            return Err(ResolveError::InvalidSExpr(
                "call-service requires service name and action".to_string(),
            ));
        }
        let service = self.sexpr_as_str(&args[0])?;
        let method = self.sexpr_as_str(&args[1])?;

        // Detect trailing options object carrying `emit:`. Only pulled out
        // of the args list if it actually carries an `emit:` key — a plain
        // trailing object is still treated as the call's args payload.
        let mut args_end = args.len();
        let mut emit_cfg: Option<OirEmitConfig> = None;

        if args.len() > 3 {
            if let SExpression::Object(obj) = &args[args.len() - 1] {
                if obj.contains_key("emit") {
                    emit_cfg = self.extract_emit_config(obj)?;
                    args_end -= 1;
                }
            }
        }

        let call_args = if args_end > 2 {
            self.resolve_sexpr(&args[args_end - 1])?
        } else {
            OirExpr::object(std::collections::HashMap::new())
        };

        let mut effect = OirEffect::call_service(service, method, call_args);
        if let OirEffect::CallService {
            emit: ref mut em, ..
        } = effect
        {
            *em = emit_cfg;
        }
        Ok(OirExpr::effect(effect))
    }

    fn resolve_ref_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Format: ["ref", "EntityType"]
        //         ["ref", "EntityType", { "id": "@payload.id", "filter": ..., "include": [...] }]
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "ref requires entity type".to_string(),
            ));
        }
        let entity_type = self.sexpr_as_str(&args[0])?;

        if args.len() > 1 {
            if let SExpression::Object(obj) = &args[1] {
                // Extract id (single-entity ref)
                let id = if let Some(id_expr) = obj.get("id") {
                    match id_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(id_expr)?),
                    }
                } else {
                    None
                };

                // Extract filter
                let filter = if let Some(filter_expr) = obj.get("filter") {
                    match filter_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(filter_expr)?),
                    }
                } else {
                    None
                };

                // Extract order_by
                let order_by = if let Some(order_expr) = obj.get("order_by") {
                    match order_expr {
                        SExpression::Null => None,
                        _ => Some(self.resolve_sexpr(order_expr)?),
                    }
                } else {
                    None
                };

                // Extract limit
                let limit = if let Some(SExpression::Number(n)) = obj.get("limit") {
                    Some(*n as usize)
                } else {
                    None
                };

                // Extract include array
                let include = if let Some(SExpression::Array(arr)) = obj.get("include") {
                    let fields: Vec<String> = arr
                        .iter()
                        .filter_map(|v| {
                            if let SExpression::String(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if fields.is_empty() {
                        None
                    } else {
                        Some(fields)
                    }
                } else {
                    None
                };

                let emit_cfg = self.extract_emit_config(obj)?;

                let mut effect = OirEffect::ref_entity(&entity_type);
                if let OirEffect::Ref {
                    id: ref mut i,
                    filter: ref mut f,
                    order_by: ref mut ob,
                    limit: ref mut l,
                    include: ref mut inc,
                    emit: ref mut em,
                    ..
                } = effect
                {
                    *i = id;
                    *f = filter;
                    *ob = order_by;
                    *l = limit;
                    *inc = include;
                    *em = emit_cfg;
                }
                return Ok(OirExpr::effect(effect));
            }

            // Legacy shorthand: ["ref", "Entity", "@payload.id"]
            if let SExpression::String(s) = &args[1] {
                if s.starts_with('@') && s.contains('.') && s.ends_with("id") {
                    let id_expr = self.resolve_sexpr(&args[1])?;
                    let mut effect = OirEffect::ref_entity(&entity_type);
                    if let OirEffect::Ref { id: ref mut i, .. } = effect {
                        *i = Some(id_expr);
                    }
                    return Ok(OirExpr::effect(effect));
                }
            }
        }

        Ok(OirExpr::effect(OirEffect::ref_entity(entity_type)))
    }

    /// Extract the `emit:` config from an options object, if present.
    ///
    /// Accepts both snake_case (`on_change`, `on_message`) and camelCase
    /// (`onChange`, `onMessage`) keys — lolo-authored source uses the former;
    /// JSON-authored `.orb` uses the latter. All four keys are optional; the
    /// validator enforces which ones are allowed per operator.
    ///
    /// Returns `None` if no `emit:` key is present, so the caller stores
    /// `emit: None` on the OIR and the field is skipped in serialization.
    fn extract_emit_config(
        &self,
        obj: &std::collections::HashMap<String, SExpression>,
    ) -> Result<Option<OirEmitConfig>, ResolveError> {
        let emit_entry = match obj.get("emit") {
            Some(SExpression::Object(inner)) => inner,
            Some(SExpression::Null) | None => return Ok(None),
            Some(other) => {
                return Err(ResolveError::InvalidSExpr(format!(
                    "emit: must be an object, got {:?}",
                    other
                )));
            }
        };

        let read_str = |key: &str| -> Result<Option<String>, ResolveError> {
            match emit_entry.get(key) {
                None | Some(SExpression::Null) => Ok(None),
                Some(SExpression::String(s)) => Ok(Some(s.clone())),
                Some(other) => Err(ResolveError::InvalidSExpr(format!(
                    "emit.{} must be a string event name, got {:?}",
                    key, other
                ))),
            }
        };

        let on_change = read_str("on_change")?.or(read_str("onChange")?);
        let on_message = read_str("on_message")?.or(read_str("onMessage")?);

        let cfg = OirEmitConfig {
            success: read_str("success")?,
            failure: read_str("failure")?,
            on_change,
            on_message,
        };

        if cfg.is_empty() {
            Ok(None)
        } else {
            Ok(Some(cfg))
        }
    }

    /// Split the trailing args of an `os/watch-*` effect into (options, emit).
    ///
    /// `os/watch-files`, `-process`, `-port`, `-http` take a positional
    /// second arg that is the watcher's own options (glob options,
    /// subcommand, protocol, method). This helper distinguishes:
    /// - `[]`                          → (None, None)
    /// - `[optsObj_with_emit]`         → (None, Some(cfg))
    /// - `[optsObj_without_emit]`      → (Some(opts), None)
    /// - `[positional, emitObj]`       → (Some(positional), Some(cfg))
    fn split_watch_trailing(
        &self,
        tail: &[SExpression],
    ) -> Result<(Option<OirExpr>, Option<OirEmitConfig>), ResolveError> {
        match tail.len() {
            0 => Ok((None, None)),
            1 => {
                if let SExpression::Object(obj) = &tail[0] {
                    if obj.contains_key("emit") {
                        return Ok((None, self.extract_emit_config(obj)?));
                    }
                }
                Ok((Some(self.resolve_sexpr(&tail[0])?), None))
            }
            _ => {
                let positional = Some(self.resolve_sexpr(&tail[0])?);
                let emit_cfg = if let SExpression::Object(obj) = &tail[1] {
                    self.extract_emit_config(obj)?
                } else {
                    None
                };
                Ok((positional, emit_cfg))
            }
        }
    }

    /// Extract emit config from a single trailing options object for
    /// `os/watch-*` effects that take no positional secondary args
    /// (cron, signal, env).
    fn extract_watch_only_emit(
        &self,
        tail: &[SExpression],
    ) -> Result<Option<OirEmitConfig>, ResolveError> {
        if tail.is_empty() {
            return Ok(None);
        }
        if let SExpression::Object(obj) = &tail[0] {
            return self.extract_emit_config(obj);
        }
        Ok(None)
    }

    fn resolve_deref_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Format: ["deref", "EntityType"] or ["deref", "EntityType", idExpr]
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "deref requires entity type".to_string(),
            ));
        }
        let entity_type = self.sexpr_as_str(&args[0])?;

        if args.len() > 1 {
            let id = self.resolve_sexpr(&args[1])?;
            Ok(OirExpr::effect(OirEffect::deref_by_id(entity_type, id)))
        } else {
            Ok(OirExpr::effect(OirEffect::deref_entity(entity_type)))
        }
    }

    fn resolve_swap_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Format: ["swap!", "EntityType", idExpr, transformExpr]
        if args.len() < 3 {
            return Err(ResolveError::InvalidSExpr(
                "swap! requires entity type, id, and transform".to_string(),
            ));
        }
        let entity_type = self.sexpr_as_str(&args[0])?;
        let id = self.resolve_sexpr(&args[1])?;
        let transform = self.resolve_sexpr(&args[2])?;
        Ok(OirExpr::effect(OirEffect::swap(entity_type, id, transform)))
    }

    fn resolve_watch_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Format: ["watch", "EntityType", [effect1, effect2, ...]]
        if args.len() < 2 {
            return Err(ResolveError::InvalidSExpr(
                "watch requires entity type and effects array".to_string(),
            ));
        }
        let entity_type = self.sexpr_as_str(&args[0])?;

        let effects = if let SExpression::Array(arr) = &args[1] {
            arr.iter()
                .map(|e| self.resolve_sexpr(e))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            return Err(ResolveError::InvalidSExpr(
                "watch effects must be an array".to_string(),
            ));
        };

        Ok(OirExpr::effect(OirEffect::watch(entity_type, effects)))
    }

    fn resolve_atomic_effect(&self, args: &[SExpression]) -> Result<OirExpr, ResolveError> {
        // Format: ["atomic", [effect1, effect2, ...]]
        if args.is_empty() {
            return Err(ResolveError::InvalidSExpr(
                "atomic requires effects array".to_string(),
            ));
        }

        let effects = if let SExpression::Array(arr) = &args[0] {
            arr.iter()
                .map(|e| self.resolve_sexpr(e))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            return Err(ResolveError::InvalidSExpr(
                "atomic effects must be an array".to_string(),
            ));
        };

        Ok(OirExpr::effect(OirEffect::atomic(effects)))
    }

    fn sexpr_as_str(&self, sexpr: &SExpression) -> Result<String, ResolveError> {
        match sexpr {
            SExpression::String(s) => Ok(s.clone()),
            _ => Err(ResolveError::InvalidSExpr("Expected string".to_string())),
        }
    }

    fn sexpr_to_value(&self, sexpr: &SExpression) -> OirValue {
        match sexpr {
            SExpression::Null => OirValue::Null,
            SExpression::Bool(b) => OirValue::Bool(*b),
            SExpression::Number(n) => OirValue::Number(*n),
            SExpression::String(s) => OirValue::String(s.clone()),
            SExpression::Array(arr) => {
                OirValue::Array(arr.iter().map(|v| self.sexpr_to_value(v)).collect())
            }
            SExpression::Object(obj) => OirValue::Object(
                obj.iter()
                    .map(|(k, v)| (k.clone(), self.sexpr_to_value(v)))
                    .collect(),
            ),
        }
    }

    /// Extract slot name from a render-ui effect (stored as OirExpr::Effect)
    fn extract_slot_from_effect(&self, effect: &OirExpr) -> Option<String> {
        // Effects are wrapped in OirExpr::Effect { effect: OirEffect }
        if let Some(OirEffect::RenderUi { slot, .. }) = effect.as_effect() {
            return Some(slot.clone());
        }
        None
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
fn payload_field_to_oir_type(field: &orbital_core::schema::PayloadField) -> OirType {
    if let Some(props) = &field.properties {
        let fields = props
            .iter()
            .map(|p| OirField {
                name: p.name.clone(),
                field_type: payload_field_to_oir_type(p),
                description: p.description.clone(),
                required: p.required.unwrap_or(false),
                default: None,
                constraints: Vec::new(),
                enum_values: None,
                relation: None,
            })
            .collect();
        let struct_ty = OirType::Struct { fields };
        // `field_type` "[object]" signals an array-of-struct: `properties` holds
        // the ELEMENT shape, so wrap in an array. Plain "object" is a direct
        // struct field.
        if field.field_type.trim_start().starts_with('[') {
            return OirType::array(struct_ty);
        }
        return struct_ty;
    }
    parse_payload_field_type(&field.field_type)
}

fn parse_payload_field_type(field_type: &str) -> OirType {
    let trimmed = field_type.trim();
    // Array: `[X]` where X may itself be a compound expression.
    if let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return OirType::array(parse_payload_field_type(inner));
    }
    // Map: `Map<K,V>` — dynamic-key uniform-value map. The key is a comma-free
    // atom, so split on the FIRST comma (value may itself be `Map<…>`/`[…]`).
    if let Some(inner) = trimmed
        .strip_prefix("Map<")
        .and_then(|s| s.strip_suffix('>'))
    {
        if let Some(comma) = inner.find(',') {
            let key = &inner[..comma];
            let value = &inner[comma + 1..];
            return OirType::map(
                parse_payload_field_type(key),
                parse_payload_field_type(value),
            );
        }
    }
    // Primitive keywords (case-insensitive match).
    match trimmed.to_lowercase().as_str() {
        "string" => return OirType::String,
        "number" | "float" | "double" => return OirType::Number,
        "integer" | "int" => return OirType::Integer,
        "boolean" | "bool" => return OirType::Boolean,
        "date" => return OirType::Date,
        "datetime" => return OirType::DateTime,
        "array" | "list" => return OirType::array(OirType::Json),
        "object" | "json" | "tensor" => return OirType::Json,
        // `SExpr` brand — a compiled s-expression value (config-only; e.g. a
        // `conditionalFields` map value). Emits the recursive `SExpr` TS type.
        "sexpr" => return OirType::SExpr,
        // UI scalar types (`node`/`slot`/`icon`/`asset`/`component`/`pattern`)
        // carry no serializable OIR shape — in a data/payload/struct-field context
        // they are represented as strings (an icon/component name, an asset URL, or
        // a serialized node), matching the config-knob emitter (`node`-typed knobs
        // emit `string`) and the Zod payload validator (`z.string()`). Without this
        // they fell through to `Reference { entity: "node" }` and emitted the
        // bare identifier `node` — TS2552 "Cannot find name 'node'".
        "node" | "slot" | "icon" | "asset" | "component" | "pattern" => return OirType::String,
        _ => {}
    }
    // Any remaining non-empty identifier: treat as an entity reference.
    // `type_expr_to_string` on a `TypeExpr::Named` emits the bare identifier
    // for user-defined types, so `data : User` reaches us as `"User"` here.
    // Empty strings would be malformed; fall back to Json to stay safe.
    if trimmed.is_empty() {
        return OirType::Json;
    }
    OirType::Reference {
        entity: trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_payload_field_type_primitives() {
        assert_eq!(parse_payload_field_type("string"), OirType::String);
        assert_eq!(parse_payload_field_type("int"), OirType::Integer);
        assert_eq!(parse_payload_field_type("boolean"), OirType::Boolean);
        assert_eq!(parse_payload_field_type("object"), OirType::Json);
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
        assert_eq!(binding.trait_id.as_deref(), Some("trt_x00000000000000000000000"));
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
        let board = oir.traits.iter().find(|t| t.name == "Board").expect("trait present");
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
        let board = oir.traits.iter().find(|t| t.name == "Board").expect("trait present");
        let event = board
            .events
            .iter()
            .find(|e| e.key == "PRODUCE")
            .expect("named config-forward event survives");
        assert!(event.payload_schema.as_ref().expect("payload schema kept").contains_key("id"));
    }

    #[test]
    fn test_resolve_minimal_schema() {
        let schema = OrbitalSchema {
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![],
            metadata: None,
            ledger: None,
            schema_version: None,
        };

        let result = resolve_to_oir(&schema);
        assert!(result.is_ok());

        let oir = result.unwrap();
        assert_eq!(oir.name, "TestApp");
        assert_eq!(oir.version, "1.0.0");
    }

    fn tick_with_interval(
        interval: orbital_core::schema::StringOrNumber,
    ) -> orbital_core::schema::TickDefinition {
        orbital_core::schema::TickDefinition {
            name: "testTick".to_string(),
            interval,
            guard: None,
            effects: vec![],
            applies_to: None,
            description: None,
        }
    }

    fn empty_resolver() -> Resolver<'static> {
        static SCHEMA: std::sync::OnceLock<OrbitalSchema> = std::sync::OnceLock::new();
        let schema = SCHEMA.get_or_init(|| OrbitalSchema {
            name: "Test".to_string(),
            version: None,
            description: None,
            orbitals: vec![],
            metadata: None,
            ledger: None,
            schema_version: None,
        });
        Resolver::new(schema)
    }

    #[test]
    fn resolve_tick_cron_expression_becomes_oir_cron() {
        let resolver = empty_resolver();
        let tick = tick_with_interval(orbital_core::schema::StringOrNumber::String(
            "0 9 * * *".to_string(),
        ));
        let resolved = resolver
            .resolve_tick(&tick)
            .expect("valid cron should resolve");
        assert_eq!(
            resolved.interval,
            OirInterval::Cron("0 9 * * *".to_string())
        );
    }

    #[test]
    fn resolve_tick_garbage_string_is_rejected_not_silently_1s() {
        let resolver = empty_resolver();
        let tick = tick_with_interval(orbital_core::schema::StringOrNumber::String(
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
            let tick = tick_with_interval(orbital_core::schema::StringOrNumber::String(
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
}
