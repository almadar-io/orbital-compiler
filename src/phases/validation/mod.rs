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
//! - [`time_unit`] - `time/*` unit literals
//!
//! # Usage
//!
//! ```rust,ignore
//! use orbital_compiler::phases::validation::{validate_comprehensive_full, SchemaLanguage};
//!
//! let schema = parse_schema(json)?;
//! let result = validate_comprehensive_full(
//!     &schema, None, SchemaLanguage::English, installed_registry, packages,
//! );
//!
//! if !result.is_valid() {
//!     for error in &result.errors {
//!         eprintln!("{}", error);
//!     }
//! }
//! ```

// shared
pub mod binding;
pub mod error;
pub mod final_program;
pub mod id_integrity;
pub(crate) mod value_walk;

// entities — entity shape, mutation and the row store
#[path = "entities/entity.rs"]
pub mod entity;
#[path = "entities/entity_mutation.rs"]
pub mod entity_mutation;
#[path = "entities/fetch_by_id_payload.rs"]
mod fetch_by_id_payload;
#[path = "entities/fetch_deliver.rs"]
mod fetch_deliver;

// identity — identity and access
#[path = "identity/entity_access.rs"]
pub mod entity_access;
#[path = "identity/user_identity.rs"]
pub mod user_identity;

// traits
#[path = "traits/client_tick_locality.rs"]
pub(crate) mod client_tick_locality;
#[path = "traits/mounts.rs"]
pub(crate) mod mounts;
#[path = "traits/sigil_context.rs"]
pub mod sigil_context;
#[path = "traits/trait_validator.rs"]
pub mod trait_validator;

// circuit — Event → Guard → Transition → Effects → UI → Event, and the payloads it carries
#[path = "circuit/async_results.rs"]
pub(crate) mod async_results;
#[path = "circuit/closed_circuit.rs"]
pub mod closed_circuit;
#[path = "circuit/cross_emitter.rs"]
pub mod cross_emitter;
#[path = "circuit/cross_orbital/mod.rs"]
pub mod cross_orbital;
#[path = "circuit/emit_payload.rs"]
pub mod emit_payload;
#[path = "circuit/fired_events.rs"]
pub mod fired_events;
#[path = "circuit/produced_shape.rs"]
pub mod produced_shape;

// expressions — expressions and effects
#[path = "expressions/effect/mod.rs"]
pub mod effect;
#[path = "expressions/service.rs"]
pub mod service;
#[path = "expressions/sexpr/mod.rs"]
pub mod sexpr;

// types — types, type integrity and time
#[path = "types/config_type.rs"]
pub mod config_type;
#[path = "types/generic_types.rs"]
pub mod generic_types;
#[path = "types/time_unit.rs"]
pub mod time_unit;
#[path = "types/type_param_sweep.rs"]
pub mod type_param_sweep;

// ui — render-ui, patterns, slots, pages
#[path = "ui/icon.rs"]
pub mod icon;
#[path = "ui/page.rs"]
pub mod page;
#[path = "ui/render_ui/mod.rs"]
pub mod render_ui;
#[path = "ui/render_ui_binding.rs"]
pub mod render_ui_binding;
#[path = "ui/slot.rs"]
pub mod slot;
#[path = "ui/sync/mod.rs"]
pub mod sync;
#[path = "ui/theme.rs"]
pub mod theme;

// composition — call sites, config knobs and cross-program contracts
#[path = "composition/composed_wiring.rs"]
pub mod composed_wiring;
#[path = "composition/cross_registry.rs"]
pub mod cross_registry;
#[path = "composition/embed_bridge.rs"]
pub mod embed_bridge;
#[path = "composition/expectations.rs"]
pub mod expectations;
#[path = "composition/orbital_config.rs"]
pub mod orbital_config;
#[path = "composition/orbital_ref_config.rs"]
pub mod orbital_ref_config;
#[path = "composition/orbital_ref_extend.rs"]
pub mod orbital_ref_extend;
#[path = "composition/trait_ref_config.rs"]
pub mod trait_ref_config;

pub use orbital_core_base::language::canonical_operators;

// Re-export main types
pub use error::{ErrorCode, Severity, ValidationError, ValidationResult};

use orbital_core_base::schema::{EntityDefinition, OrbitalSchema, TraitReference};
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
    /// Schema language for i18n-aware validation (GAP-V001)
    pub schema_language: SchemaLanguage,
    /// Page path -> INIT payload requirements (for navigate validation)
    pub page_init_payloads: std::collections::HashMap<String, PageInitPayload>,
    /// Extra patterns from project-local patterns.json
    pub extra_patterns: Option<ExtraPatterns>,
    /// Services registry projections (action names + per-action param
    /// contracts), loaded from services-registry.json for call-service
    /// validation — one struct so the two can never drift.
    pub service_registry: crate::embedded::ServiceRegistryData,
    /// Installed behavior packages (discovered node-style, same set
    /// `CompilerOptions`/`LoaderConfig` carry). Behavior packages are
    /// installed, not baked (Almadar_Behavior_Packages.md Phase 2) — a
    /// validator that needs to read a std/io `.orb`'s content (e.g. the
    /// scope-boundary check's std-emits lookup) resolves it against this
    /// list rather than an embedded registry.
    pub packages: Vec<orbital_core_base::behavior_packages::BehaviorPackage>,
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
            schema_language: SchemaLanguage::default(),
            page_init_payloads: std::collections::HashMap::new(),
            extra_patterns: None,
            service_registry: crate::embedded::ServiceRegistryData::default(),
            packages: Vec::new(),
        }
    }
}

impl ValidationContext {
    /// Create a new validation context
    pub fn new() -> Self {
        Self::default()
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

/// Field type in the lowercase kind vocabulary `set_value_type_compatible`
/// speaks (the serde spelling IS that vocabulary).
pub(crate) fn field_type_str(field: &orbital_core_base::schema::FieldDefinition) -> String {
    serde_json::to_value(&field.field_type)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
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
) -> impl Iterator<Item = (&orbital_core_base::schema::TraitDefinition, String)> {
    use orbital_core_base::schema::TraitReference;
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
    "dock",
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
    "int",
    "boolean",
    "date",
    "datetime",
    "timestamp",
    "instant",
    // Semantic string domains.
    "email",
    "url",
    "phone",
    "uuid",
    "image",
    // Promoted value types (2026-08-20): money = numeric at rest; file = the
    // canonical {name, url, mimeType, sizeBytes} struct value.
    "money",
    "file",
    "bytes",
    // A nominal string: only `(as <string> CallKey)` makes one.
    "CallKey",
    "array",
    "object",
    "enum",
    "relation",
    // `trait` was missing here while being a real `FieldType` variant — a latent
    // bug that would reject any trait-typed field reaching this check.
    "trait",
    "orbital",
    "behavior",
    "slot",
    "pattern",
    // Renderable UI content (type-integrity campaign).
    "node",
    // Event-name reference (Phase B / SCAN-EVENT-KNOB-2 struct-field carrier).
    "event",
    // A union of type variants (`string | FileItem`). Its `values` carry the
    // VARIANT NAMES, not enum members — see the enum-member check, which skips
    // this type for exactly that reason.
    "union",
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

/// Run comprehensive validation with an EMPTY service registry: no keyed
/// call-service payload shape, no `ORB_CALL_SERVICE_RESULT_HAS_KEY`. For tests
/// whose fixtures declare no services; production goes through
/// [`validate_comprehensive_full`] with the installed registry.
pub fn validate_comprehensive_without_services(schema: &OrbitalSchema) -> ValidationResult {
    validate_comprehensive_full(
        schema,
        None,
        SchemaLanguage::English,
        crate::embedded::ServiceRegistryData::default(),
        Vec::new(),
    )
}

/// Run comprehensive validation with all options including service action
/// registry and installed behavior packages (see `ValidationContext::
/// packages`; empty when the caller has none discovered — validators that
/// consult it are best-effort and skip silently on an empty list).
// `validate_comprehensive_full_with_phase_f` (the `phase_f_strict: bool`
// toggle) was removed 2026-09-02 along with the legacy `warning()` severity
// alias: the toggle only selected between two spellings of the same blocking
// severity, so "non-strict" was never a real mode.
pub fn validate_comprehensive_full(
    schema: &OrbitalSchema,
    extra_patterns: Option<ExtraPatterns>,
    schema_language: SchemaLanguage,
    service_registry: crate::embedded::ServiceRegistryData,
    packages: Vec<orbital_core_base::behavior_packages::BehaviorPackage>,
) -> ValidationResult {
    let mut ctx = ValidationContext::new();
    ctx.extra_patterns = extra_patterns;
    ctx.schema_language = schema_language;
    ctx.service_registry = service_registry;
    ctx.packages = packages;

    // Phase 1: Collect names for cross-reference validation
    collect_names(schema, &mut ctx);

    // Phase 2: Entity validation
    entity::validate_all_entities(schema, &mut ctx);

    // Phase 3: Trait validation
    trait_validator::validate_all_traits(schema, &mut ctx);

    // Phase 3b: Config type validation (slot/pattern defaults)
    config_type::validate_all_config_types(schema, &mut ctx);

    // Phase 3c: `emits { @config.<knob> -> ... }` references left
    // unresolved by inline (unknown knob / non-string / no default).

    // Phase 4: Page validation
    page::validate_all_pages(schema, &mut ctx);

    // Phase 5: Effect validation
    effect::validate_all_effects(schema, &mut ctx);

    // Phase 5b: V2 Phase 0 P0.7 — emit-literal payload shape checks
    // (missing required, extra field, primitive type mismatch). Runs
    // after effect because it reuses emit-declaration data effect
    // collected via `trait_def.emits`. Error severity.
    emit_payload::validate_all_emit_payloads(schema, &mut ctx);

    // Phase 5c: Phase F.2 cross-emitter declared-payload consensus.
    // Compares every event's declared payload across all traits that emit
    // it; errors when two emitters disagree on the field list.
    cross_emitter::validate_cross_emitter(schema, &mut ctx);

    // Phase 5d: Listen-trigger-matched backstop. Runs POST-inline so the
    // `events: { OLD: NEW }`-rename variant is caught — a `listens { Src.EVENT
    // -> TRIGGER }` whose TRIGGER has no matching transition in the same trait
    // is a silent dead wire. Error severity.

    // Phase 5e: Gate C of the type-integrity campaign — hard error for
    // generic types (json / shapeless object / any) in declaration
    // positions.
    generic_types::validate_generic_types(schema, &mut ctx);

    // Phase 5e': a bridged embed (`behavior/apply` with `bridge`) whose events the
    // host never sends or never handles is a dead wire.
    embed_bridge::validate_embed_bridges(schema, &mut ctx);

    // Phase 5f: increment 3-I defensive sweep — a `$`-prefixed payload
    // sentinel surviving the inline phase means the carrying trait had no
    // `linkedEntity` to monomorphize its event alias's `(E : Entity)`
    // parameter against. Blocking.
    type_param_sweep::validate_type_param_sentinels(schema, &mut ctx);

    // Phase 6: Render-UI validation
    render_ui::validate_all_render_ui(schema, &mut ctx);

    // Phase 7: tick-scoped slot contention (the page slot checks are std lints).
    slot::validate_tick_slot_contention(schema, &mut ctx);

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

    // Phase 11c2: `[identity]` entity rules — the declared shape of `@user`.
    // No-ops entirely when the program declares no identity entity. Runs
    // post-inline so config knob defaults carry their call-site values.
    user_identity::validate_user_identity(schema, &mut ctx);
    user_identity::validate_user_into_required_slot(schema, &mut ctx);

    // Phase 11c3: entity access directives (`@read`/`@create`/`@update`/
    // `@delete`) — field/binding checks on the four per-row authorization
    // predicates. Runs immediately after identity for the same reason (needs
    // the resolved `[identity]` entity, post-inline).
    entity_access::validate_entity_access(schema, &mut ctx);

    // Phase 11c4: `expects` consumer-side requirement declarations —
    // persist-payload self-consistency against the consumer's own declared
    // shape (every schema), and the compose link-checks
    // (ORB_S_EXPECTATION_*/ORB_S_EXPECTED_FIELD_*) for multi-orbital schemas.
    expectations::validate_expectations(schema, &mut ctx);

    // Phase 11d: Multi-trait entity-mutation trio (SPLIT_ENTITY_MUTATION /
    // SharedEntityFieldConflict / SharedEntityRenderAuthority). Runs
    // post-inline so the fully `uses`-composed trait set and each entity's
    // `shared`/`persistence` flags are visible.
    entity_mutation::validate_all_entity_mutations(schema, &mut ctx);
    // ORB_E_MERGED_FIELD_UNREFERENCED: post-inline, catches a rebind-merged
    // entity field no trait of the consuming orbital references —
    // R-ENTITY-FIELD-UNION-ACROSS-SAME-NAMED-ENTITY drift.
    entity_mutation::validate_all_merged_field_usage(schema, &mut ctx);
    entity_mutation::validate_all_local_trait_dispatch(schema, &mut ctx);
    entity_mutation::validate_all_client_tick_server_effects(schema, &mut ctx);
    entity_mutation::validate_all_mixed_residence(schema, &mut ctx);
    entity_mutation::validate_all_set_column_without_row(schema, &mut ctx);
    entity_mutation::validate_all_guard_frame_field_authorization(schema, &mut ctx);
    entity_mutation::validate_all_auth_guard_client_only(schema, &mut ctx);

    // Phase 12: Closed circuit validation
    closed_circuit::validate_all_circuits(schema, &mut ctx);
    // ORB_CIRCUIT_VIEWER_STRANDED + ORB_PAGE_SECOND_CONTENT_BODY: page-level
    // (not per-trait) checks sharing one page-content-owner computation.
    closed_circuit::validate_page_content_ownership(schema, &mut ctx);
    // ORB_COMPOSED_STATE_UNREACHABLE + ORB_CONTROL_VALUE_DISCARDED: composed
    // placements whose inputs nothing produces or whose control values nothing keeps.
    composed_wiring::validate_composed_wiring(schema, &mut ctx);

    // Phase 13: Icon validation
    icon::validate_all_icons(schema, &mut ctx);

    // Phase 13b: `time/*` unit literals (ORB_OP_INVALID_TIME_UNIT). An unknown
    // unit throws at evaluation time, which inside a read predicate drops rows
    // fail-closed while the run still reports PASS.
    time_unit::validate_all_time_units(schema, &mut ctx);

    // Phase 14: Rabit V4 dual-carry id integrity (ORB_ID_*). No-op on legacy
    // id-free `.orb`; errors on dangling/kind-mismatched/name-drifted id edges,
    // warns on stale ledger rows.

    // Phase 15: orbital-import declared config forwarding (ORB_O_CONFIG_DEAD_KNOB,
    // Almadar_Orbital_Import.md §4.5). Post-inline so every trait in the
    // flattened schema is visible as a candidate forwarder.
    ctx.merge(orbital_config::validate_orbital_config_forwarding(schema));

    // Rung 2 (2026-09-24): client-ticked `[runtime]` frames need `[client]` traits.
    ctx.merge(client_tick_locality::validate_client_tick_locality(schema));

    ctx.into_result()
}

/// Collect all names for cross-reference validation (Phase 1)
fn collect_names(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    use orbital_core_base::schema::TraitReference;

    // Build a map of trait name -> trait definition for looking up page traits
    let mut trait_map: std::collections::HashMap<String, &orbital_core_base::schema::TraitDefinition> =
        std::collections::HashMap::new();

    // Collect entity names, trait names, page paths, and service names from orbitals
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    // PRIMARY entity names declared in MORE than one orbital (the same
    // grouping ORB_X_DUPLICATE_ENTITY reports; auxiliary entities repeat
    // legitimately — every orbital's filter pulls FilterTarget). A duplicated
    // name's definition is ambiguous (which orbital's field list?), so it is
    // removed from `entity_definitions` below: field-level binding checks
    // skip definitions they can't resolve, leaving ORB_X_DUPLICATE_ENTITY (the
    // std lint's) as the single actionable error instead of a phantom FIELD_NOT_FOUND
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

        // `expects entity X` — the consumer's promise that a sibling provides
        // X. Names only (never `entity_definitions`: an expectation carries no
        // field authority), so relation fields, persist/fetch targets, and
        // `linkedEntity` rebinds naming X resolve standalone. A genuinely
        // absent provider is caught by the compose link-check
        // (`ORB_S_EXPECTATION_UNSATISFIED`), never here.
        for name in expectations::expected_entity_names(orbital) {
            ctx.entity_names.insert(name.to_string());
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

        // `expects page "/articles/:slug"` — a route a SIBLING declares, which
        // this program may not contain (a single-orbital slice never does).
        // Registering the pattern is what makes reachability decidable without
        // the whole organism present; the INIT payload stays empty because the
        // provider owns it, so the payload-requirement branch no-ops exactly as
        // it does for a declared page that has no INIT. `entry` keeps a real
        // declaration's payload authoritative when both are present.
        for expectation in orbital.expects.iter().flatten() {
            if let orbital_core_base::schema::ExpectDeclaration::Page { path, protocol: None, .. } = expectation {
                ctx.page_paths.insert(path.clone());
                ctx.page_init_payloads.entry(path.clone()).or_default();
            }
        }

        // Service names (only for inline services, not references)
        if let Some(services) = &orbital.services {
            for service_ref in services {
                if let orbital_core_base::schema::ServiceRef::Inline(service_def) = service_ref {
                    ctx.service_names.insert(service_def.name().to_string());
                }
            }
        }
    }

    // `uses lazy` pages: declared routes whose behavior loads on navigation.
    // Like an `expects page`, the path is known and the INIT payload belongs
    // to the behavior that owns it.
    for lazy in schema.lazy_pages.iter().flatten() {
        ctx.page_paths.insert(lazy.path.clone());
        ctx.page_init_payloads.entry(lazy.path.clone()).or_default();
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
    page: &orbital_core_base::schema::PageDefinition,
    trait_map: &std::collections::HashMap<String, &orbital_core_base::schema::TraitDefinition>,
) -> PageInitPayload {
    use orbital_core_base::schema::TraitReference;

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
    /// `packages/almadar-core/src/types/effect.ts` exactly. Nothing to diff
    /// against at test time, so this pins the expected set inline — if
    /// `UI_SLOTS` changes, this test must be hand-updated to match.
    #[test]
    fn valid_slots_matches_canonical_ts_source() {
        const CANONICAL: &[&str] = &[
            "main",
            "sidebar",
            "dock",
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

#[cfg(test)]
mod valid_field_types_tests {
    use super::VALID_FIELD_TYPES;
    use orbital_core_base::schema::FieldType;

    #[test]
    fn the_behavior_value_types_are_valid_field_types() {
        for ty in [FieldType::Trait, FieldType::Orbital, FieldType::Behavior, FieldType::CallKey, FieldType::Int] {
            let name = serde_json::to_value(&ty).expect("serializes");
            let name = name.as_str().expect("a string tag");
            assert!(
                VALID_FIELD_TYPES.contains(&name),
                "{name} missing from VALID_FIELD_TYPES"
            );
        }
    }
}
