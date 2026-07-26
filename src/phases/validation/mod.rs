//! Comprehensive Orbital Schema Validation
//!
//! This module provides comprehensive validation for OrbitalSchema,
//! porting all TypeScript validators from `builder/packages/shared/src/orbitals/validation/`.
//!
//! # Architecture
//!
//! The validation system is organized into specialized validators:
//!
//! - [`entity`] - Entity structure, fields, types, relations
//! - [`trait_validator`] - Trait refs, categories, state machines
//! - [`page`] - Page structure, paths, traits
//! - [`effect`] - Effect validation (call_service, set, fetch)
//! - [`render_ui`] - Render-UI effects, patterns, props
//! - [`slot`] - Slot coverage, contention, nesting
//! - [`sexpr`] - S-expression semantic validation
//! - [`service`] - Service definitions
//! - [`cross_orbital`] - Cross-orbital concerns (events, entities)
//! - [`binding`] - Binding validation (@entity, @payload)
//! - [`closed_circuit`] - UI→Event→Transition flow
//! - [`icon`] - Lucide icon names
//!
//! # Usage
//!
//! ```rust,ignore
//! use orbital_compiler::phases::validation::{validate_comprehensive, ValidationResult};
//!
//! let schema = parse_schema(json)?;
//! let result = validate_comprehensive(&schema);
//!
//! if !result.is_valid() {
//!     for error in &result.errors {
//!         eprintln!("{}", error);
//!     }
//! }
//! ```

pub mod binding;
pub mod canonical_operators;
pub mod closed_circuit;
pub mod config_type;
pub mod cross_emitter;
pub mod cross_orbital;
pub mod cross_registry;
pub mod effect;
pub mod emit_config_ref;
pub mod emit_payload;
pub mod entity;
pub mod entity_mutation;
pub mod error;
pub mod icon;
pub mod id_integrity;
pub mod literal_default;
pub mod page;
pub mod payload_field_name;
pub mod render_ui;
pub mod render_ui_binding;
pub mod service;
pub mod sexpr;
pub mod sigil_context;
pub mod slot;
pub mod sync;
pub mod theme;
pub mod trait_ref_config;
pub mod trait_validator;

// Re-export main types
pub use error::{ErrorCode, Severity, ValidationError, ValidationResult};

use orbital_core::schema::{EntityDefinition, OrbitalSchema, TraitReference};
use std::collections::HashSet;
use sync::ExtraPatterns;

/// Build a source-tagged path for a trait that names BOTH the orbital and the
/// trait (not just array indices), so validation errors point at the trait the
/// author actually wrote. Indices are retained for disambiguation (e.g. two
/// auto-pulled siblings that share a name). Shared across the validation
/// phases so every `@trait` / `linkedEntity` / `stateMachine` error row reads
/// the same way.
pub(crate) fn trait_path(
    orbital_index: usize,
    orbital_name: &str,
    trait_index: usize,
    trait_ref: &TraitReference,
) -> String {
    let name = match trait_ref {
        TraitReference::Inline(td) => td.name.as_str(),
        TraitReference::Reference {
            name, reference, ..
        } => name
            .as_deref()
            .or(reference.as_deref())
            .unwrap_or("<anonymous>"),
    };
    format!(
        "orbitals[{} ({})].traits[{} ({})]",
        orbital_index, orbital_name, trait_index, name
    )
}

/// Required payload fields for a page's INIT event
#[derive(Debug, Clone, Default)]
pub struct PageInitPayload {
    /// Required field names
    pub required_fields: Vec<String>,
    /// All field names (required + optional)
    pub all_fields: Vec<String>,
}

/// Schema language for i18n-aware validation
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SchemaLanguage {
    /// English (default) — applies ASCII naming conventions
    #[default]
    English,
    /// Non-Latin script language (Arabic, etc.) — skips ASCII naming checks
    NonLatin(String),
}

impl SchemaLanguage {
    /// Returns true if ASCII naming conventions (PascalCase, camelCase) should be enforced
    pub fn should_check_ascii_naming(&self) -> bool {
        matches!(self, SchemaLanguage::English)
    }
}

/// Validation context that accumulates errors across all validators
#[derive(Debug)]
pub struct ValidationContext {
    /// Collected validation result
    pub result: ValidationResult,
    /// All entity names for cross-reference validation
    pub entity_names: HashSet<String>,
    /// Entity definitions keyed by name (for relation field validation)
    pub entity_definitions: std::collections::HashMap<String, EntityDefinition>,
    /// All trait names (custom + std library)
    pub trait_names: HashSet<String>,
    /// All inline-trait ids (V4). A page trait ref whose stale `reference` name
    /// no longer matches `trait_names` (after a cross-orbital dedup rename) is
    /// still valid when its `ref_id` resolves here — the id-aware page check
    /// that lets `rewrite_page_trait_refs` be deleted.
    pub trait_ids: HashSet<String>,
    /// All page paths for duplicate detection
    pub page_paths: HashSet<String>,
    /// All service names for duplicate detection
    pub service_names: HashSet<String>,
    /// Domain type (business, game) for context-aware validation
    pub domain: Domain,
    /// Schema language for i18n-aware validation (GAP-V001)
    pub schema_language: SchemaLanguage,
    /// Page path -> INIT payload requirements (for navigate validation)
    pub page_init_payloads: std::collections::HashMap<String, PageInitPayload>,
    /// Extra patterns from project-local patterns.json
    pub extra_patterns: Option<ExtraPatterns>,
    /// Service action registry: service name → set of valid action names
    /// Loaded from services-registry.json for call-service validation
    pub service_actions: std::collections::HashMap<String, HashSet<String>>,
    /// Phase F: when true, the F-series validators escalate the
    /// declared-vs-actual emit drift class (`EmitPayloadMissingRequired`)
    /// from warning to hard error. Defaults to true so the validator's
    /// default mode catches the regression class that produced G31.
    pub phase_f_strict: bool,
}

/// Domain type for context-aware validation
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Domain {
    /// Business/CRUD applications
    #[default]
    Business,
    /// Game applications (allows HUD slots, etc.)
    Game,
}

impl Default for ValidationContext {
    fn default() -> Self {
        Self {
            result: ValidationResult::default(),
            entity_names: HashSet::new(),
            entity_definitions: std::collections::HashMap::new(),
            trait_names: HashSet::new(),
            trait_ids: HashSet::new(),
            page_paths: HashSet::new(),
            service_names: HashSet::new(),
            domain: Domain::default(),
            schema_language: SchemaLanguage::default(),
            page_init_payloads: std::collections::HashMap::new(),
            extra_patterns: None,
            service_actions: std::collections::HashMap::new(),
            phase_f_strict: true,
        }
    }
}

impl ValidationContext {
    /// Create a new validation context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a context for game domain
    pub fn for_game() -> Self {
        Self {
            domain: Domain::Game,
            ..Self::default()
        }
    }

    /// Add an error to the context
    pub fn add_error(&mut self, error: ValidationError) {
        self.result.add_error(error);
    }

    /// Add multiple errors
    pub fn add_errors(&mut self, errors: impl IntoIterator<Item = ValidationError>) {
        self.result.add_errors(errors);
    }

    /// Merge another result
    pub fn merge(&mut self, result: ValidationResult) {
        self.result.merge(result);
    }

    /// Check if validation is passing
    pub fn is_valid(&self) -> bool {
        self.result.is_valid()
    }

    /// Get the final result
    pub fn into_result(self) -> ValidationResult {
        self.result
    }
}

/// Standard library trait names that are always valid
pub const STD_TRAITS: &[&str] = &[
    // Interaction traits
    "std/Form",
    "std/List",
    "std/Detail",
    "std/Dashboard",
    "std/Search",
    "std/Filter",
    "std/Sort",
    "std/Pagination",
    // Integration traits
    "std/Auth",
    "std/Analytics",
    "std/Notifications",
    // Game traits
    "std/GameLoop",
    "std/Physics",
    "std/Collision",
    "std/Input",
    "std/Renderer",
];

/// Check if a trait name is a standard library trait
pub fn is_std_trait(name: &str) -> bool {
    STD_TRAITS.contains(&name) || name.starts_with("std/")
}

/// Iterate every inline trait definition in the schema, yielding the trait and
/// its canonical validation path (`orbitals[i].traits[j]`).
///
/// Reference traits are skipped — they are resolved by the inline phase before
/// validation runs, so per-trait validators only see `TraitReference::Inline`.
/// This collapses the `for orbital { for trait { if let Inline } }` boilerplate
/// repeated across the per-trait validators.
pub fn inline_traits(
    schema: &OrbitalSchema,
) -> impl Iterator<Item = (&orbital_core::schema::TraitDefinition, String)> {
    use orbital_core::schema::TraitReference;
    schema.orbitals.iter().enumerate().flat_map(|(i, orbital)| {
        orbital
            .traits
            .iter()
            .enumerate()
            .filter_map(move |(j, trait_ref)| match trait_ref {
                TraitReference::Inline(trait_def) => {
                    Some((trait_def, trait_path(i, &orbital.name, j, trait_ref)))
                }
                TraitReference::Reference { .. } => None,
            })
    })
}

/// Valid UI slots. Mirrors `UI_SLOTS` in `@almadar/core`
/// (`packages/almadar-core/src/types/effect.ts`) exactly — the TS array is
/// canonical; this list must stay in lockstep with it (see
/// `valid_slots_matches_canonical_ts_source` in this module's tests).
pub const VALID_SLOTS: &[&str] = &[
    // App slots
    "main",
    "sidebar",
    "modal",
    "drawer",
    "overlay",
    "center",
    "toast",
    "floating",
    "system",
    "content",
    "screen",
    // Game HUD slots
    "hud",
    "hud-top",
    "hud-bottom",
    "hud.health",
    "hud.score",
    "hud.inventory",
    "hud.stamina",
    // Game overlay slots
    "overlay.inventory",
    "overlay.dialogue",
    "overlay.menu",
    "overlay.pause",
];

/// Valid field types
pub const VALID_FIELD_TYPES: &[&str] = &[
    "string",
    "number",
    "boolean",
    "date",
    "datetime",
    "timestamp",
    // Semantic string domains.
    "email",
    "url",
    "phone",
    "uuid",
    "image",
    "array",
    "object",
    "enum",
    "relation",
    // `trait` was missing here while being a real `FieldType` variant — a latent
    // bug that would reject any trait-typed field reaching this check.
    "trait",
    "slot",
    "pattern",
];

/// Valid trait categories
pub const VALID_TRAIT_CATEGORIES: &[&str] = &[
    "interaction",
    "integration",
    "lifecycle",
    "game-core",
    "game-entity",
    "game-ui",
];

/// Valid page view types
pub const VALID_VIEW_TYPES: &[&str] = &["list", "detail", "create", "edit", "dashboard", "custom"];

/// Reserved socket event names
pub const RESERVED_SOCKET_EVENTS: &[&str] = &[
    "INIT",
    "ERROR",
    "CONNECT",
    "DISCONNECT",
    "RECONNECT",
    "HEARTBEAT",
    "connection",
    "disconnect",
    "error",
];

/// Run comprehensive validation on a schema
///
/// This runs all validators in sequence and accumulates all errors.
/// Use `validate_comprehensive_with_patterns` to pass project-local patterns.
pub fn validate_comprehensive(schema: &OrbitalSchema) -> ValidationResult {
    validate_comprehensive_with_options(schema, None, SchemaLanguage::English)
}

/// Run comprehensive validation with optional extra patterns from a project's patterns.json.
pub fn validate_comprehensive_with_patterns(
    schema: &OrbitalSchema,
    extra_patterns: Option<ExtraPatterns>,
) -> ValidationResult {
    validate_comprehensive_with_options(schema, extra_patterns, SchemaLanguage::English)
}

/// Run comprehensive validation with extra patterns and language awareness.
pub fn validate_comprehensive_with_options(
    schema: &OrbitalSchema,
    extra_patterns: Option<ExtraPatterns>,
    schema_language: SchemaLanguage,
) -> ValidationResult {
    validate_comprehensive_full(
        schema,
        extra_patterns,
        schema_language,
        std::collections::HashMap::new(),
    )
}

/// Run comprehensive validation with all options including service action registry.
pub fn validate_comprehensive_full(
    schema: &OrbitalSchema,
    extra_patterns: Option<ExtraPatterns>,
    schema_language: SchemaLanguage,
    service_actions: std::collections::HashMap<String, HashSet<String>>,
) -> ValidationResult {
    validate_comprehensive_full_with_phase_f(
        schema,
        extra_patterns,
        schema_language,
        service_actions,
        true,
    )
}

/// Run comprehensive validation with explicit Phase F strict toggle.
///
/// `phase_f_strict = true` (the default) escalates the F.1
/// declared-vs-actual emit drift class to a hard error. Pass `false` from
/// tests that need to verify the legacy warn-only behavior.
pub fn validate_comprehensive_full_with_phase_f(
    schema: &OrbitalSchema,
    extra_patterns: Option<ExtraPatterns>,
    schema_language: SchemaLanguage,
    service_actions: std::collections::HashMap<String, HashSet<String>>,
    phase_f_strict: bool,
) -> ValidationResult {
    let mut ctx = ValidationContext::new();
    ctx.extra_patterns = extra_patterns;
    ctx.schema_language = schema_language;
    ctx.service_actions = service_actions;
    ctx.phase_f_strict = phase_f_strict;

    // Detect domain from schema metadata or orbital context
    if let Some(orbitals) = schema.orbitals.first() {
        if let Some(domain_ctx) = &orbitals.domain_context {
            if let Some(category) = &domain_ctx.category {
                if category.to_lowercase().contains("game") {
                    ctx.domain = Domain::Game;
                }
            }
        }
    }

    // Phase 1: Collect names for cross-reference validation
    collect_names(schema, &mut ctx);

    // Phase 2: Entity validation
    entity::validate_all_entities(schema, &mut ctx);

    // Phase 2b: Phase F.3 literal-union default check (error severity).
    // Runs after entity collection so the per-field iteration sees the
    // canonical entity definitions.
    literal_default::validate_literal_defaults(schema, &mut ctx);

    // Phase 3: Trait validation
    trait_validator::validate_all_traits(schema, &mut ctx);

    // Phase 3b: Config type validation (slot/pattern defaults)
    config_type::validate_all_config_types(schema, &mut ctx);

    // Phase 3c: `emits { @config.<knob> -> ... }` references left
    // unresolved by inline (unknown knob / non-string / no default).
    emit_config_ref::validate_all_config_ref_emits(schema, &mut ctx);

    // Phase 4: Page validation
    page::validate_all_pages(schema, &mut ctx);

    // Phase 5: Effect validation
    effect::validate_all_effects(schema, &mut ctx);

    // Phase 5b: V2 Phase 0 P0.7 — emit-literal payload shape checks
    // (missing required, extra field, primitive type mismatch). Runs
    // after effect because it reuses emit-declaration data effect
    // collected via `trait_def.emits`. Warn-only severity.
    emit_payload::validate_all_emit_payloads(schema, &mut ctx);

    // Phase 5b.1: G6 Rule 1 — every emit/event payloadSchema field name must
    // be a valid TS identifier (no dots, spaces, or leading digits). Defense-
    // in-depth for the lowering pass that flattens nested payload names.
    payload_field_name::validate_payload_field_names(schema, &mut ctx);

    // Phase 5c: Phase F.2 cross-emitter declared-payload consensus.
    // Compares every event's declared payload across all traits that emit
    // it; warn when two emitters disagree on the field list.
    cross_emitter::validate_cross_emitter(schema, &mut ctx);

    // Phase 5d: Listen-trigger-matched backstop. Runs POST-inline so the
    // `events: { OLD: NEW }`-rename variant is caught — a `listens { Src.EVENT
    // -> TRIGGER }` whose TRIGGER has no matching transition in the same trait
    // is a silent dead wire. Error severity.
    ctx.merge(cross_orbital::validate_listen_triggers(schema));

    // Phase 6: Render-UI validation
    render_ui::validate_all_render_ui(schema, &mut ctx);

    // Phase 7: Slot validation
    slot::validate_all_slots(schema, &mut ctx);

    // Phase 8: S-expression validation
    sexpr::validate_all_sexprs(schema, &mut ctx);

    // Phase 9: Service validation
    service::validate_all_services(schema, &mut ctx);

    // Phase 9b: Theme validation
    theme::validate_all_themes(schema, &mut ctx);

    // Phase 10: Cross-orbital validation
    cross_orbital::validate_cross_orbital(schema, &mut ctx);

    // Phase 11: Binding validation
    binding::validate_all_bindings(schema, &mut ctx);

    // Phase 11b: Sigil context validation (Phase 3.4 of Lolo v3 plan)
    sigil_context::validate_all_sigil_contexts(schema, &mut ctx);

    // Phase 11c: Cross-registry consistency (Phase 3.5 of Lolo v3 plan)
    cross_registry::validate_cross_registry(schema, &mut ctx);

    // Phase 11d: Multi-trait entity-mutation trio (SPLIT_ENTITY_MUTATION /
    // SharedEntityFieldConflict / SharedEntityRenderAuthority) + the non-Cron
    // tick backstop (ORB_TICK_REQUIRES_RUNTIME / ORB_TICK_ASYNC_EFFECT). Runs
    // post-inline so the fully `uses`-composed trait set and each entity's
    // `shared`/`persistence` flags are visible.
    entity_mutation::validate_all_entity_mutations(schema, &mut ctx);
    entity_mutation::validate_all_tick_guards(schema, &mut ctx);

    // Phase 12: Closed circuit validation
    closed_circuit::validate_all_circuits(schema, &mut ctx);

    // Phase 13: Icon validation
    icon::validate_all_icons(schema, &mut ctx);

    // Phase 14: Rabit V4 dual-carry id integrity (ORB_ID_*). No-op on legacy
    // id-free `.orb`; errors on dangling/kind-mismatched/name-drifted id edges,
    // warns on stale ledger rows.
    id_integrity::validate_id_integrity(schema, &mut ctx);

    ctx.into_result()
}

/// Collect all names for cross-reference validation (Phase 1)
fn collect_names(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    use orbital_core::schema::TraitReference;

    // Build a map of trait name -> trait definition for looking up page traits
    let mut trait_map: std::collections::HashMap<String, &orbital_core::schema::TraitDefinition> =
        std::collections::HashMap::new();

    // Collect entity names, trait names, page paths, and service names from orbitals
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    // PRIMARY entity names declared in MORE than one orbital (the same
    // grouping ORB_X_DUPLICATE_ENTITY reports; auxiliary entities repeat
    // legitimately — every orbital's filter pulls FilterTarget). A duplicated
    // name's definition is ambiguous (which orbital's field list?), so it is
    // removed from `entity_definitions` below: field-level binding checks
    // skip definitions they can't resolve, leaving ORB_X_DUPLICATE_ENTITY as
    // the single actionable error instead of a phantom FIELD_NOT_FOUND
    // cascade validated against whichever same-named entity happened to win
    // the map insert.
    let mut primary_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut duplicated_entities: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for orbital in &schema.orbitals {
        if let Some(entity) = orbital.entity.as_inline() {
            if !primary_names.insert(entity.name.as_str()) {
                duplicated_entities.insert(entity.name.clone());
            }
        }
    }

    for orbital in &schema.orbitals {
        // Entity name and definition (only for inline entities, not references)
        if let Some(entity) = orbital.entity.as_inline() {
            ctx.entity_names.insert(entity.name.clone());
            ctx.entity_definitions
                .insert(entity.name.clone(), entity.clone());
        }

        // Gap #22 — auxiliary entities are imported atom entities surfaced
        // when a trait reference omits the `-> Entity` rebind at the call
        // site (populated by the inline phase). Register their names AND
        // definitions so trait-validator's linkedEntity check, fetch /
        // persist effect validators, and any downstream lookup all accept
        // them as legitimate orbital-scope entities.
        if let Some(aux_list) = &orbital.auxiliary_entities {
            for aux_ref in aux_list {
                if let Some(aux_entity) = aux_ref.as_inline() {
                    ctx.entity_names.insert(aux_entity.name.clone());
                    ctx.entity_definitions
                        .insert(aux_entity.name.clone(), aux_entity.clone());
                }
            }
        }

        // Inline trait names and build trait map
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                ctx.trait_names.insert(trait_def.name.clone());
                if let Some(id) = &trait_def.id {
                    ctx.trait_ids.insert(id.clone());
                }
                trait_map.insert(trait_def.name.clone(), trait_def);
            }
        }

        // Page paths (only for inline pages, not references)
        if let Some(pages) = &orbital.pages {
            for page_ref in pages {
                if let Some(page) = page_ref.as_inline() {
                    ctx.page_paths.insert(page.path.clone());

                    // Every inline page is registered so navigate reachability
                    // (`validate_navigate`) sees the COMPLETE declared-page set —
                    // its "fires only when pages exist but none match" intent
                    // needs every page, not only payload-bearing ones. A page
                    // with no INIT payload contributes empty required/all fields,
                    // so the payload-requirement branch simply no-ops for it.
                    // (Restores navigate-typo detection after the redundant L1
                    // check was removed for RABIT-DEMOTE-EDIT-WORKSPACE-INVISIBLE.)
                    let init_payload = collect_page_init_payload(page, &trait_map);
                    ctx.page_init_payloads
                        .insert(page.path.clone(), init_payload);
                }
            }
        }

        // Service names (only for inline services, not references)
        if let Some(services) = &orbital.services {
            for service_ref in services {
                if let orbital_core::schema::ServiceRef::Inline(service_def) = service_ref {
                    ctx.service_names.insert(service_def.name().to_string());
                }
            }
        }
    }

    // Ambiguous (duplicated-primary) definitions carry no field authority —
    // see the comment above `duplicated_entities`. Names stay registered so
    // existence checks (linkedEntity, fetch targets) still pass.
    for name in &duplicated_entities {
        ctx.entity_definitions.remove(name);
    }
}

/// Collect INIT payload requirements for a page from its traits
fn collect_page_init_payload(
    page: &orbital_core::schema::PageDefinition,
    trait_map: &std::collections::HashMap<String, &orbital_core::schema::TraitDefinition>,
) -> PageInitPayload {
    use orbital_core::schema::TraitReference;

    let mut payload = PageInitPayload::default();

    if let Some(page_traits) = &page.traits {
        for trait_ref in page_traits {
            // Get the trait definition (either inline or from the map)
            let trait_def = match trait_ref {
                TraitReference::Inline(def) => Some(def),
                TraitReference::Reference {
                    reference, name, ..
                } => {
                    let trait_name = reference.as_ref().or(name.as_ref());
                    trait_name.and_then(|n| trait_map.get(n).copied())
                }
            };

            if let Some(trait_def) = trait_def {
                // Check for INIT event definitions in the state machine
                // The events[].payload defines what payload fields are expected
                if let Some(events) = &trait_def.state_machine.events {
                    for event in events {
                        if event.key == "INIT" {
                            if let Some(payload_fields) = &event.payload_schema {
                                for field in payload_fields {
                                    if !payload.all_fields.contains(&field.name) {
                                        payload.all_fields.push(field.name.clone());
                                    }
                                    if field.required.unwrap_or(false)
                                        && !payload.required_fields.contains(&field.name)
                                    {
                                        payload.required_fields.push(field.name.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_std_trait() {
        assert!(is_std_trait("std/Form"));
        assert!(is_std_trait("std/List"));
        assert!(is_std_trait("std/CustomTrait"));
        assert!(!is_std_trait("CustomTrait"));
        assert!(!is_std_trait("Form"));
    }

    #[test]
    fn test_validation_context() {
        let mut ctx = ValidationContext::new();
        assert!(ctx.is_valid());

        ctx.add_error(ValidationError::error(
            ErrorCode::EntityMissingName,
            "test",
            "test error",
        ));
        assert!(!ctx.is_valid());
    }

    /// Parity gate: `VALID_SLOTS` must match `UI_SLOTS` in
    /// `packages/almadar-core/src/types/effect.ts` exactly. There is no baked
    /// copy of `effect.ts` under `src/baked/` to diff against at test time (unlike
    /// `bindings.ts`, see `embedded_validation.rs::sigil_rules_match_ts_source`),
    /// so this pins the expected set inline — if `UI_SLOTS` changes, this test
    /// must be hand-updated to match.
    #[test]
    fn valid_slots_matches_canonical_ts_source() {
        const CANONICAL: &[&str] = &[
            "main",
            "sidebar",
            "modal",
            "drawer",
            "overlay",
            "center",
            "toast",
            "floating",
            "system",
            "content",
            "screen",
            "hud",
            "hud-top",
            "hud-bottom",
            "hud.health",
            "hud.score",
            "hud.inventory",
            "hud.stamina",
            "overlay.inventory",
            "overlay.dialogue",
            "overlay.menu",
            "overlay.pause",
        ];
        assert_eq!(
            VALID_SLOTS, CANONICAL,
            "VALID_SLOTS drifted from UI_SLOTS in \
             packages/almadar-core/src/types/effect.ts — update whichever side is stale"
        );
    }
}
