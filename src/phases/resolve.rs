//! Resolve Phase
//!
//! Resolves all references and produces the final OIR (Orbital Intermediate Representation).

use orbital_core::ir::{
    OirBinaryOp, OirBinding, OirCardinality, OirConfigFieldType, OirEffect, OirEmit, OirEmitConfig,
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
    };
    Resolver::new(&empty).resolve_sexpr(sexpr)
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

        Ok(module)
    }

    fn resolve_entity(&self, entity: &EntityDefinition) -> Result<OirEntity, ResolveError> {
        let mut oir_entity = OirEntity::new(&entity.name);

        if let Some(collection) = &entity.collection {
            oir_entity.collection = collection.clone();
        }

        if let Some(persistence) = &entity.persistence {
            oir_entity.persistence = match persistence {
                orbital_core::schema::Persistence::Persistent => OirPersistence::Persistent,
                orbital_core::schema::Persistence::Runtime => OirPersistence::Runtime,
                orbital_core::schema::Persistence::Singleton => OirPersistence::Singleton,
                orbital_core::schema::Persistence::Instance => OirPersistence::Instance,
            };
        }

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

        // Add relation
        if let Some(relation) = &field.relation {
            oir_field.relation = Some(OirRelation {
                target_entity: relation.entity.clone(),
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
            "number" => OirType::Number,
            "integer" | "int" => OirType::Integer,
            "boolean" | "bool" => OirType::Boolean,
            "date" => OirType::Date,
            "datetime" | "timestamp" => OirType::DateTime,
            "array" => OirType::array(OirType::Json),
            "object" | "json" => OirType::Json,
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
        oir_trait.description = trait_def.description.clone();
        oir_trait.linked_entity = trait_def.linked_entity.clone();
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
            oir_trait.states.push(if is_initial {
                OirState::initial(&state.name)
            } else {
                OirState::new(&state.name)
            });
        }

        // Resolve events from state machine
        if let Some(events) = &sm.events {
            for event in events {
                // EventDefinition has key and name (optional)
                let event_key = &event.key;
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

                oir_trait.events.push(oir_event);
            }
        }

        // Also add events from transitions if not already added
        for trans in &sm.transitions {
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
                        orbital_core::schema::ListenSource::Trait { trait_name } => {
                            oir.source_trait = Some(trait_name.clone());
                        }
                        orbital_core::schema::ListenSource::Orbital {
                            orbital,
                            trait_name,
                        } => {
                            oir.source_orbital = Some(orbital.clone());
                            oir.source_trait = Some(trait_name.clone());
                        }
                    }
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
                if s == "frame" {
                    OirInterval::Frame
                } else if s.ends_with("ms") {
                    let ms: u64 = s.trim_end_matches("ms").parse().unwrap_or(1000);
                    OirInterval::Milliseconds { value: ms }
                } else if s.ends_with("s") {
                    let s_val: u64 = s.trim_end_matches("s").parse().unwrap_or(1);
                    OirInterval::Seconds { value: s_val }
                } else {
                    OirInterval::Seconds { value: 1 }
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

        // Resolve sections if present
        if let Some(sections) = &page.sections {
            for section in sections {
                oir_page.sections.push(self.resolve_section(section)?);
            }
        }

        // Resolve trait bindings
        if let Some(traits) = &page.traits {
            for trait_ref in traits {
                if let Some(trait_name) = trait_ref.trait_name() {
                    oir_page
                        .traits
                        .push(OirTraitBinding::new(trait_name.to_string()));
                }
            }
        }

        Ok(oir_page)
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
            "and" | "&&" => self.binary_op(args, OirExpr::and),
            "or" | "||" => self.binary_op(args, OirExpr::or),
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
    fn test_resolve_minimal_schema() {
        let schema = OrbitalSchema {
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![],
            metadata: None,
        };

        let result = resolve_to_oir(&schema);
        assert!(result.is_ok());

        let oir = result.unwrap();
        assert_eq!(oir.name, "TestApp");
        assert_eq!(oir.version, "1.0.0");
    }
}
