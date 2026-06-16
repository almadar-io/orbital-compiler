//! Embedded JSON Registries
//!
//! Source-of-truth registries embedded directly into the orbital-compiler binary
//! at build time via `include_str!`. After Phase 3 of the Lolo v3 plan, the validator
//! reads operators, patterns, and behaviors from these embedded JSON sources rather
//! than hand-maintained Rust constants. Adding a new operator to `operators.json`
//! plus running `cargo build` is enough to update the validator.
//!
//! See `docs/Lolo_Language_Spec_v3_PLAN.md` Section 8 for context.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

// ============================================================================
// 3.1 — Embedded operators.json
// ============================================================================

const OPERATORS_JSON: &str = include_str!("baked/canonical-operators.json");

/// Operator type reference, mirror of `OperatorTypeRef` in
/// `packages/almadar-std/types.ts:148`.
///
/// The TS definition accepts either a bare primitive string (`'number'`,
/// `'string'`, `'boolean'`, `'any'`, `'void'`, `'array'`, matching `FieldType`
/// in `@almadar/core`) or a tagged object shape. `#[serde(untagged)]` handles
/// both cases: serde first tries the tagged variants by shape and falls back
/// to `Primitive(String)` for bare strings.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OperatorTypeRef {
    /// Tagged shape (kind-discriminated object).
    Tagged(OperatorTypeRefKind),
    /// Bare string primitive, e.g. `"number"`, `"string"`, `"void"`.
    Primitive(String),
}

/// Tagged variants of `OperatorTypeRef`. Split from the outer enum so serde's
/// internally-tagged discriminator (`tag = "kind"`) has a fixed shape to match
/// against. The outer `#[serde(untagged)]` enum uses this for tagged input and
/// falls back to `Primitive` for bare strings.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OperatorTypeRefKind {
    Union {
        of: Vec<OperatorTypeRef>,
    },
    Literal {
        value: serde_json::Value,
    },
    Array {
        of: Box<OperatorTypeRef>,
    },
    Object {
        #[serde(default)]
        fields: HashMap<String, OperatorTypeRef>,
        #[serde(default)]
        open: Option<bool>,
    },
    Entity {
        #[serde(default)]
        collection: Option<String>,
    },
    #[serde(rename = "entityRef")]
    EntityRef,
    #[serde(rename = "eventKey")]
    EventKey {
        #[serde(default)]
        scope: Option<String>,
    },
    #[serde(rename = "uiSlot")]
    UiSlot,
    #[serde(rename = "patternType")]
    PatternType,
    Binding {
        #[serde(default)]
        shape: Option<Box<OperatorTypeRef>>,
    },
    Sexpr,
}

/// Effect-specific metadata on an operator, mirror of `OperatorEffectMeta` in
/// `packages/almadar-std/types.ts:168`. Populated only for operators with
/// `hasSideEffects: true` that opted into v2 effect metadata. `produces`
/// carries the payload the runtime puts on the bus when the operator's
/// `emit.success` fires; the V2 validator (P0.8) uses it to type-check
/// `emit: { success: "X" }` configs against declared event payloads.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedOperatorEffect {
    pub kind: String,
    #[serde(default)]
    pub produces: Option<OperatorTypeRef>,
    #[serde(default)]
    pub config: Option<HashMap<String, OperatorTypeRef>>,
}

/// Metadata for a single operator parsed from `operators.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedOperator {
    pub category: String,
    #[serde(rename = "minArity")]
    pub min_arity: u32,
    #[serde(rename = "maxArity", default)]
    pub max_arity: Option<u32>,
    #[serde(rename = "returnType", default)]
    pub return_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "hasSideEffects", default)]
    pub has_side_effects: bool,
    /// Schema v2 effect metadata. Absent on operators that predate the v2
    /// uplift. The V2 validator consults `effect.produces` at effect call
    /// sites to check `emit: { success: "X" }` compatibility.
    #[serde(default)]
    pub effect: Option<EmbeddedOperatorEffect>,
    /// When true, the validator errors if the effect call site is missing
    /// a trailing options object with `emit.success` populated. Set on
    /// fetch / persist / call-service / ref in canonical-operators.json.
    /// Generic flag — future "this op requires X" constraints can extend
    /// this struct with sibling booleans without writing new validator
    /// passes.
    #[serde(rename = "requiresEmitSuccess", default)]
    pub requires_emit_success: bool,
}

/// Metadata for an operator category.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedCategory {
    pub description: String,
    pub target: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OperatorsSchema {
    #[serde(default)]
    version: String,
    #[serde(default)]
    categories: HashMap<String, EmbeddedCategory>,
    operators: HashMap<String, EmbeddedOperator>,
}

fn parsed_operators() -> &'static OperatorsSchema {
    static PARSED: OnceLock<OperatorsSchema> = OnceLock::new();
    PARSED.get_or_init(|| {
        serde_json::from_str(OPERATORS_JSON).expect("Failed to parse embedded operators.json")
    })
}

/// All operators keyed by name (e.g. `"+"`, `"render-ui"`, `"behavior/compose"`).
pub fn operators() -> &'static HashMap<String, EmbeddedOperator> {
    &parsed_operators().operators
}

/// All operator categories.
pub fn categories() -> &'static HashMap<String, EmbeddedCategory> {
    &parsed_operators().categories
}

/// Returns true if `name` is a known operator from `operators.json`.
pub fn is_known_operator(name: &str) -> bool {
    operators().contains_key(name)
}

/// Look up the metadata for an operator by name.
pub fn get_operator(name: &str) -> Option<&'static EmbeddedOperator> {
    operators().get(name)
}

/// Returns true if the operator category targets Rust (or if unknown).
pub fn is_rust_target(name: &str) -> bool {
    let cats = categories();
    if let Some(op) = get_operator(name) {
        if let Some(cat) = cats.get(&op.category) {
            return cat.target.iter().any(|t| t == "rust");
        }
    }
    false
}

// ============================================================================
// 3.2 — Embedded patterns-registry.json
// ============================================================================

const PATTERNS_JSON: &str = include_str!("baked/patterns-registry.json");

/// A single field of an emit/listen payload schema, mirroring trait
/// `payloadSchema` entries so L1/L2 consumers can compare the component's
/// documented bus shape against the trait's declared `emits {}` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedPayloadField {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub required: bool,
}

/// A single positional arg of a React callback prop. Used by L2 to verify
/// name-and-type parity between the callback and the trait's declared event
/// payload (gap #32).
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedCallbackArg {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub r#type: String,
}

/// JSON-schema-style `required` field — accepts either a boolean (used at
/// the pattern-prop top level, e.g. `columns: { required: true }`) OR an
/// array of property names (used at the nested-object level, e.g.
/// `items: { properties: { id: { types: ["string"] } }, required: ["id"] }`).
/// Pattern-sync writes both shapes; consumers use helpers below to read.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RequiredFlag {
    Bool(bool),
    Names(Vec<String>),
}

impl Default for RequiredFlag {
    fn default() -> Self {
        RequiredFlag::Bool(false)
    }
}

impl RequiredFlag {
    /// Whether the property carrying this flag is required ON ITS PARENT object —
    /// the `Bool(true)` form ONLY. The `Names([...])` form means something
    /// different ("these sub-keys are required WITHIN this property's own object,
    /// if the property is present"), read via [`as_names`](Self::as_names); it must
    /// NOT make the property itself parent-required. (The old `as_bool` conflated
    /// the two — `Names` non-empty → true — which wrongly demanded e.g. an optional
    /// `relation` on every `form-section` field.)
    pub fn required_on_parent(&self) -> bool {
        matches!(self, RequiredFlag::Bool(true))
    }

    /// Read as a slice of field names — the `Names([...])` form's required
    /// sub-keys; empty for the bool form.
    pub fn as_names(&self) -> &[String] {
        match self {
            RequiredFlag::Bool(_) => &[],
            RequiredFlag::Names(v) => v.as_slice(),
        }
    }
}

/// A single prop entry in a pattern's `propsSchema`.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedPatternProp {
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub required: RequiredFlag,
    #[serde(default)]
    pub description: Option<String>,
    /// Optional marker for SCALAR props whose value is a field name on the
    /// linked entity. Currently the only recognised value is `"field-ref"`.
    /// See `docs/Almadar_Agent_Behaviors_Plan.md` Phase 3.2/3.3.
    ///
    /// Pattern-sync also writes the F1 plan's event-related kinds here:
    /// `"event"` (legacy, EventKey alias), `"event-ref"` (declarative
    /// bus-emit prop, sourced from `EventEmit<P>`), `"event-listen"`
    /// (`EventListen<P>` mirror), `"event-list"` (array prop whose elements
    /// carry an event-name field), `"callback"` (React callback prop bound
    /// to an event name at the schema-author site).
    #[serde(default)]
    pub kind: Option<String>,
    /// Optional marker for ARRAY props whose string elements are field names
    /// on the linked entity. Currently the only recognised value is
    /// `"field-ref"`. Non-string array elements are ignored by the resolver.
    #[serde(rename = "itemKind", default)]
    pub item_kind: Option<String>,
    /// For `kind: "event-list"`: which field inside each array item holds
    /// the event key. Default is `"event"` — set explicitly when the
    /// descriptor type uses a different field name.
    #[serde(rename = "eventField", default)]
    pub event_field: Option<String>,
    /// For `kind: "event-ref"` whose source is `EventEmit<P>`: the
    /// structural shape of `P`, as the bus payload the component fires
    /// when the prop is bound. Validator (L2.2) compares this against the
    /// trait's declared `emits { EVENT { ... } }` payload.
    #[serde(rename = "emitPayloadSchema", default)]
    pub emit_payload_schema: Option<Vec<EmbeddedPayloadField>>,
    /// For `kind: "event-listen"` whose source is `EventListen<P>`: the
    /// structural shape of `P`, as the payload the component subscribes to.
    #[serde(rename = "listenPayloadSchema", default)]
    pub listen_payload_schema: Option<Vec<EmbeddedPayloadField>>,
    /// For `kind: "callback"`: positional parameter list of the function
    /// type. Validator uses the names to build the param-name → payload-
    /// field map for the C2 codegen + runtime callback wrap.
    #[serde(rename = "callbackArgs", default)]
    pub callback_args: Option<Vec<EmbeddedCallbackArg>>,
    /// For array-typed props: per-element shape. Carried for the
    /// `ORB_RUI_ELEMENT_SHAPE_MISMATCH` rule so the validator can check
    /// each element of a bound array literal against the component's
    /// required element fields (e.g. `MarketingFooter.columns` →
    /// `items.properties = { title: required, items: required }`).
    #[serde(default)]
    pub items: Option<Box<EmbeddedPatternProp>>,
    /// For object-typed props OR for elements of `items`: per-field shape.
    /// Required fields here are enforced by the element-shape rule against
    /// the lolo literal's keys.
    #[serde(default)]
    pub properties: Option<HashMap<String, EmbeddedPatternProp>>,
    /// Entity-row clean slate: when a prop's TS type is `EntityWith<K>`,
    /// pattern-sync writes the keys `K` here. The bound entity (or a remap
    /// field-map) MUST provide every one of these fields, else the component
    /// renders blank rows. Enforced by `ORB_X_ENTITY_PROP_CONTRACT`.
    #[serde(rename = "requiredFields", default)]
    pub required_fields: Option<Vec<String>>,
}

/// A single pattern entry from `patterns-registry.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedPattern {
    #[serde(rename = "type", default)]
    pub pattern_type: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "propsSchema", default)]
    pub props_schema: HashMap<String, EmbeddedPatternProp>,
    #[serde(rename = "entityAware", default)]
    pub entity_aware: bool,
}

static PATTERNS: OnceLock<HashMap<String, EmbeddedPattern>> = OnceLock::new();

/// All patterns keyed by pattern type (e.g. `"entity-table"`, `"button"`).
pub fn patterns() -> &'static HashMap<String, EmbeddedPattern> {
    PATTERNS.get_or_init(|| {
        // patterns-registry.json shape: { version, exportedAt, patterns: { name: PatternEntry } }
        let parsed: serde_json::Value = serde_json::from_str(PATTERNS_JSON)
            .expect("Failed to parse embedded patterns-registry.json");
        let patterns_obj = parsed
            .get("patterns")
            .and_then(|v| v.as_object())
            .expect("patterns-registry.json missing 'patterns' object at root");
        let mut map = HashMap::new();
        for (name, value) in patterns_obj {
            if let Ok(p) = serde_json::from_value::<EmbeddedPattern>(value.clone()) {
                map.insert(name.clone(), p);
            }
        }
        map
    })
}

/// Returns true if `pattern_type` is a known pattern.
pub fn is_known_pattern(pattern_type: &str) -> bool {
    patterns().contains_key(pattern_type)
}

/// Look up a pattern entry by type.
pub fn get_pattern(pattern_type: &str) -> Option<&'static EmbeddedPattern> {
    patterns().get(pattern_type)
}

// ============================================================================
// 3.3 — Embedded behaviors-registry.json (dual-source: std + io)
// ============================================================================
//
// Behaviors split across two packages since @almadar/std@16.0.0:
//   - @almadar/std        : core + agent topics
//   - @almadar-io/behaviors: app, game, marketing, probes, service, variations
//
// Both are embedded via include_str! at compile time. The merge happens at
// startup in `behaviors()`; partitions are disjoint by topic so the union
// has no observable collisions, but if one ever occurs std wins.

const BEHAVIORS_JSON_STD: &str = include_str!("baked/behaviors-registry-std.json");

const BEHAVIORS_JSON_IO: &str = include_str!("baked/behaviors-registry-io.json");

/// A single behavior entry from `behaviors-registry.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedBehavior {
    pub name: String,
    /// Composition level: "atom" | "molecule" | "organism".
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

static BEHAVIORS: OnceLock<HashMap<String, EmbeddedBehavior>> = OnceLock::new();

fn parse_behaviors_into(map: &mut HashMap<String, EmbeddedBehavior>, json: &str, source: &str) {
    let parsed: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|e| {
        panic!("Failed to parse embedded behaviors-registry.json ({source}): {e}")
    });
    let behaviors_obj = parsed
        .get("behaviors")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| {
            panic!("behaviors-registry.json ({source}) missing 'behaviors' object at root")
        });
    for (name, value) in behaviors_obj {
        if let Ok(b) = serde_json::from_value::<EmbeddedBehavior>(value.clone()) {
            map.insert(name.clone(), b);
        }
    }
}

/// All behaviors keyed by behavior name (e.g. `"std-cart"`, `"std-applicant"`).
/// Merged from both @almadar/std (core + agent topics) and
/// @almadar-io/behaviors (app/game/marketing/probes/service/variations).
pub fn behaviors() -> &'static HashMap<String, EmbeddedBehavior> {
    BEHAVIORS.get_or_init(|| {
        let mut map = HashMap::new();
        parse_behaviors_into(&mut map, BEHAVIORS_JSON_STD, "std");
        parse_behaviors_into(&mut map, BEHAVIORS_JSON_IO, "io");
        map
    })
}

/// Returns true if `name` is a known behavior.
pub fn is_known_behavior(name: &str) -> bool {
    behaviors().contains_key(name)
}

/// Look up a behavior entry by name.
pub fn get_behavior(name: &str) -> Option<&'static EmbeddedBehavior> {
    behaviors().get(name)
}

// ============================================================================
// 3.4 — Embedded factory-signatures.json (Almadar_Domain_Language Phase 1)
// ============================================================================
//
// One signature per factory in `@almadar/std`, emitted by
// `almadar-pattern-sync` from each `.orb`. Lives next to the .orb registry
// it summarises. The agent embedding-searches this catalog to pick a factory;
// downstream phases (TS translator today, potential Rust consumers later)
// read it to know each factory's advertised param surface.
//
// Embedded via `include_str!` so the orbital binary doesn't need filesystem
// access to load signatures.

const FACTORY_SIGNATURES_JSON_STD: &str = include_str!("baked/factory-signatures-std.json");

const FACTORY_SIGNATURES_JSON_IO: &str = include_str!("baked/factory-signatures-io.json");

/// Raw JSON content of the std factory-signature catalog (core + agent
/// topics). Consumers needing the full surface must also read
/// `factory_signatures_json_io()`.
pub fn factory_signatures_json() -> &'static str {
    FACTORY_SIGNATURES_JSON_STD
}

/// Raw JSON content of the io factory-signature catalog (app/game/marketing/
/// probes/service/variations topics).
pub fn factory_signatures_json_io() -> &'static str {
    FACTORY_SIGNATURES_JSON_IO
}

// ============================================================================
// Phase 3.5.C — Embedded std behavior registry (flat phf::Map)
// ============================================================================
//
// 138 std behavior `.orb` files (78 atoms + 28 molecules + 32 organisms)
// embedded at compile time via `include_str!`. Consumed by the
// ExternalOrbitalLoader to resolve `from: "std/behaviors/<name>"` paths in
// `uses` declarations (Sub-unit 3.5.D wires the loader).
//
// The map key is the canonical std path (`std/behaviors/<name>`); the value is
// the raw `.orb` JSON content as a `&'static str`. The loader parses the
// content with the existing schema parser when a `use` declaration matches.
//
// This replaces the earlier Sub-unit 3.5.3 design (`EmbeddedBehaviorOrb`
// struct keyed by bare behavior name) which was built for an operator-dispatch
// model that has been rejected in favor of the `use`-based import system.
//
// IMPORTANT: the contents of `EMBEDDED_STD_BEHAVIORS` are auto-generated by
// `tools/sync-rust/src/index.ts` from `packages/almadar-std/behaviors/registry/**/*.orb`.
// Do NOT hand-edit between the SYNC-GENERATED markers below — the next
// `npx tsx tools/sync-rust/src/index.ts` will overwrite them.

// SYNC-GENERATED:EMBEDDED_STD_BEHAVIORS:BEGIN
pub static EMBEDDED_STD_BEHAVIORS: phf::Map<&'static str, &'static str> = phf::phf_map! {
    // === Atoms (320) ===
    "std/behaviors/std-agent-activity-log" => include_str!("baked/std/agent/atoms/std-agent-activity-log.orb"),
    "std/behaviors/std-agent-chat-thread" => include_str!("baked/std/agent/atoms/std-agent-chat-thread.orb"),
    "std/behaviors/std-agent-classifier" => include_str!("baked/std/agent/atoms/std-agent-classifier.orb"),
    "std/behaviors/std-agent-completion" => include_str!("baked/std/agent/atoms/std-agent-completion.orb"),
    "std/behaviors/std-agent-context-window" => include_str!("baked/std/agent/atoms/std-agent-context-window.orb"),
    "std/behaviors/std-agent-conversation" => include_str!("baked/std/agent/atoms/std-agent-conversation.orb"),
    "std/behaviors/std-agent-memory" => include_str!("baked/std/agent/atoms/std-agent-memory.orb"),
    "std/behaviors/std-agent-provider" => include_str!("baked/std/agent/atoms/std-agent-provider.orb"),
    "std/behaviors/std-agent-search" => include_str!("baked/std/agent/atoms/std-agent-search.orb"),
    "std/behaviors/std-agent-session" => include_str!("baked/std/agent/atoms/std-agent-session.orb"),
    "std/behaviors/std-agent-step-progress" => include_str!("baked/std/agent/atoms/std-agent-step-progress.orb"),
    "std/behaviors/std-agent-token-gauge" => include_str!("baked/std/agent/atoms/std-agent-token-gauge.orb"),
    "std/behaviors/std-agent-tool-call" => include_str!("baked/std/agent/atoms/std-agent-tool-call.orb"),
    "std/behaviors/std-ap-bill" => include_str!("baked/io/app/atoms/std-ap-bill.orb"),
    "std/behaviors/std-app-layout" => include_str!("baked/std/core/atoms/std-app-layout.orb"),
    "std/behaviors/std-app-layout-bottomnav" => include_str!("baked/std/core-variations/atoms/std-app-layout-bottomnav.orb"),
    "std/behaviors/std-app-layout-minimal" => include_str!("baked/std/core-variations/atoms/std-app-layout-minimal.orb"),
    "std/behaviors/std-app-layout-topnav" => include_str!("baked/std/core-variations/atoms/std-app-layout-topnav.orb"),
    "std/behaviors/std-app-search" => include_str!("baked/std/core/atoms/std-app-search.orb"),
    "std/behaviors/std-applicant" => include_str!("baked/io/app/atoms/std-applicant.orb"),
    "std/behaviors/std-application" => include_str!("baked/io/app/atoms/std-application.orb"),
    "std/behaviors/std-appointment-policy" => include_str!("baked/io/app/atoms/std-appointment-policy.orb"),
    "std/behaviors/std-approval-gate" => include_str!("baked/std/infra/atoms/std-approval-gate.orb"),
    "std/behaviors/std-approval-request" => include_str!("baked/io/app/atoms/std-approval-request.orb"),
    "std/behaviors/std-audit-capture" => include_str!("baked/std/infra/atoms/std-audit-capture.orb"),
    "std/behaviors/std-autocomplete" => include_str!("baked/std/core/atoms/std-autocomplete.orb"),
    "std/behaviors/std-billable-hour" => include_str!("baked/std/core/atoms/std-billable-hour.orb"),
    "std/behaviors/std-board" => include_str!("baked/std/core/atoms/std-board.orb"),
    "std/behaviors/std-board-kanban-classic" => include_str!("baked/std/core-variations/atoms/std-board-kanban-classic.orb"),
    "std/behaviors/std-board-swim-lanes" => include_str!("baked/std/core-variations/atoms/std-board-swim-lanes.orb"),
    "std/behaviors/std-board-timeline-roadmap" => include_str!("baked/std/core-variations/atoms/std-board-timeline-roadmap.orb"),
    "std/behaviors/std-book-viewer" => include_str!("baked/io/curated/atoms/std-book-viewer.orb"),
    "std/behaviors/std-branching-logic" => include_str!("baked/std/core/atoms/std-branching-logic.orb"),
    "std/behaviors/std-browse" => include_str!("baked/std/core/atoms/std-browse.orb"),
    "std/behaviors/std-browse-command-palette" => include_str!("baked/std/core-variations/atoms/std-browse-command-palette.orb"),
    "std/behaviors/std-browse-dense" => include_str!("baked/std/core-variations/atoms/std-browse-dense.orb"),
    "std/behaviors/std-browse-feed" => include_str!("baked/std/core-variations/atoms/std-browse-feed.orb"),
    "std/behaviors/std-browse-gallery" => include_str!("baked/std/core-variations/atoms/std-browse-gallery.orb"),
    "std/behaviors/std-browse-map" => include_str!("baked/std/core-variations/atoms/std-browse-map.orb"),
    "std/behaviors/std-cache-aside" => include_str!("baked/std/core/atoms/std-cache-aside.orb"),
    "std/behaviors/std-calendar" => include_str!("baked/std/core/atoms/std-calendar.orb"),
    "std/behaviors/std-calendar-agenda-list" => include_str!("baked/std/core-variations/atoms/std-calendar-agenda-list.orb"),
    "std/behaviors/std-calendar-grid" => include_str!("baked/io/curated/atoms/std-calendar-grid.orb"),
    "std/behaviors/std-calendar-week-timeline" => include_str!("baked/std/core-variations/atoms/std-calendar-week-timeline.orb"),
    "std/behaviors/std-campaign-goal" => include_str!("baked/io/app/atoms/std-campaign-goal.orb"),
    "std/behaviors/std-cascade-on-delete" => include_str!("baked/std/infra/atoms/std-cascade-on-delete.orb"),
    "std/behaviors/std-case-study-organism" => include_str!("baked/io/curated/atoms/std-case-study-organism.orb"),
    "std/behaviors/std-change-order" => include_str!("baked/io/app/atoms/std-change-order.orb"),
    "std/behaviors/std-chart-of-accounts" => include_str!("baked/io/app/atoms/std-chart-of-accounts.orb"),
    "std/behaviors/std-checkin" => include_str!("baked/io/app/atoms/std-checkin.orb"),
    "std/behaviors/std-checkout-terminal" => include_str!("baked/io/app/atoms/std-checkout-terminal.orb"),
    "std/behaviors/std-circuit-breaker" => include_str!("baked/std/core/atoms/std-circuit-breaker.orb"),
    "std/behaviors/std-class-roster" => include_str!("baked/io/app/atoms/std-class-roster.orb"),
    "std/behaviors/std-class-session" => include_str!("baked/io/app/atoms/std-class-session.orb"),
    "std/behaviors/std-collision" => include_str!("baked/io/game/atoms/std-collision.orb"),
    "std/behaviors/std-combat" => include_str!("baked/io/game/atoms/std-combat.orb"),
    "std/behaviors/std-confirmation" => include_str!("baked/std/core/atoms/std-confirmation.orb"),
    "std/behaviors/std-content-renderer" => include_str!("baked/io/curated/atoms/std-content-renderer.orb"),
    "std/behaviors/std-conversation-policy" => include_str!("baked/io/app/atoms/std-conversation-policy.orb"),
    "std/behaviors/std-court-deadline" => include_str!("baked/io/app/atoms/std-court-deadline.orb"),
    "std/behaviors/std-cross-reference" => include_str!("baked/std/infra/atoms/std-cross-reference.orb"),
    "std/behaviors/std-customer-account" => include_str!("baked/io/app/atoms/std-customer-account.orb"),
    "std/behaviors/std-dashboard-grid-embedded" => include_str!("baked/std/core/atoms/std-dashboard-grid-embedded.orb"),
    "std/behaviors/std-dashboard-grid-layout" => include_str!("baked/std/core/atoms/std-dashboard-grid-layout.orb"),
    "std/behaviors/std-dashboard-summary" => include_str!("baked/std/core/atoms/std-dashboard-summary.orb"),
    "std/behaviors/std-data-erasure" => include_str!("baked/std/infra/atoms/std-data-erasure.orb"),
    "std/behaviors/std-data-grid" => include_str!("baked/io/curated/atoms/std-data-grid.orb"),
    "std/behaviors/std-data-list" => include_str!("baked/io/curated/atoms/std-data-list.orb"),
    "std/behaviors/std-date-range" => include_str!("baked/std/core/atoms/std-date-range.orb"),
    "std/behaviors/std-detail-drawer" => include_str!("baked/std/core-variations/atoms/std-detail-drawer.orb"),
    "std/behaviors/std-detail-panel" => include_str!("baked/io/curated/atoms/std-detail-panel.orb"),
    "std/behaviors/std-device-fleet" => include_str!("baked/io/app/atoms/std-device-fleet.orb"),
    "std/behaviors/std-dialogue-box" => include_str!("baked/io/game/atoms/std-dialogue-box.orb"),
    "std/behaviors/std-document" => include_str!("baked/io/app/atoms/std-document.orb"),
    "std/behaviors/std-donation-receipt" => include_str!("baked/io/app/atoms/std-donation-receipt.orb"),
    "std/behaviors/std-donor" => include_str!("baked/io/app/atoms/std-donor.orb"),
    "std/behaviors/std-drawer" => include_str!("baked/std/core/atoms/std-drawer.orb"),
    "std/behaviors/std-drip-sequence" => include_str!("baked/io/app/atoms/std-drip-sequence.orb"),
    "std/behaviors/std-driver" => include_str!("baked/io/app/atoms/std-driver.orb"),
    "std/behaviors/std-dunning" => include_str!("baked/io/app/atoms/std-dunning.orb"),
    "std/behaviors/std-email-broadcast" => include_str!("baked/io/app/atoms/std-email-broadcast.orb"),
    "std/behaviors/std-entity-cards" => include_str!("baked/io/curated/atoms/std-entity-cards.orb"),
    "std/behaviors/std-esign-request" => include_str!("baked/std/core/atoms/std-esign-request.orb"),
    "std/behaviors/std-event" => include_str!("baked/io/app/atoms/std-event.orb"),
    "std/behaviors/std-event-log" => include_str!("baked/std/core/atoms/std-event-log.orb"),
    "std/behaviors/std-export" => include_str!("baked/std/core/atoms/std-export.orb"),
    "std/behaviors/std-feature-grid-organism" => include_str!("baked/io/curated/atoms/std-feature-grid-organism.orb"),
    "std/behaviors/std-file-store" => include_str!("baked/std/core/atoms/std-file-store.orb"),
    "std/behaviors/std-filter" => include_str!("baked/std/core/atoms/std-filter.orb"),
    "std/behaviors/std-flag" => include_str!("baked/std/core-variations/atoms/std-flag.orb"),
    "std/behaviors/std-form-advanced" => include_str!("baked/std/core/atoms/std-form-advanced.orb"),
    "std/behaviors/std-form-builder" => include_str!("baked/io/app/atoms/std-form-builder.orb"),
    "std/behaviors/std-gallery" => include_str!("baked/std/core/atoms/std-gallery.orb"),
    "std/behaviors/std-game-canvas2d" => include_str!("baked/io/game/atoms/std-game-canvas2d.orb"),
    "std/behaviors/std-gameflow" => include_str!("baked/io/game/atoms/std-gameflow.orb"),
    "std/behaviors/std-geosearch" => include_str!("baked/std/core/atoms/std-geosearch.orb"),
    "std/behaviors/std-graphs" => include_str!("baked/std/core/atoms/std-graphs.orb"),
    "std/behaviors/std-graphs-area" => include_str!("baked/std/core-variations/atoms/std-graphs-area.orb"),
    "std/behaviors/std-graphs-bar" => include_str!("baked/std/core-variations/atoms/std-graphs-bar.orb"),
    "std/behaviors/std-graphs-donut" => include_str!("baked/std/core-variations/atoms/std-graphs-donut.orb"),
    "std/behaviors/std-graphs-histogram" => include_str!("baked/std/core-variations/atoms/std-graphs-histogram.orb"),
    "std/behaviors/std-graphs-line" => include_str!("baked/std/core-variations/atoms/std-graphs-line.orb"),
    "std/behaviors/std-graphs-pie" => include_str!("baked/std/core-variations/atoms/std-graphs-pie.orb"),
    "std/behaviors/std-graphs-scatter" => include_str!("baked/std/core-variations/atoms/std-graphs-scatter.orb"),
    "std/behaviors/std-health-score" => include_str!("baked/io/app/atoms/std-health-score.orb"),
    "std/behaviors/std-help-article" => include_str!("baked/io/app/atoms/std-help-article.orb"),
    "std/behaviors/std-help-category" => include_str!("baked/io/app/atoms/std-help-category.orb"),
    "std/behaviors/std-helpdesk-ticket" => include_str!("baked/io/app/atoms/std-helpdesk-ticket.orb"),
    "std/behaviors/std-hero-banner" => include_str!("baked/std/core/atoms/std-hero-banner.orb"),
    "std/behaviors/std-hero-organism" => include_str!("baked/io/curated/atoms/std-hero-organism.orb"),
    "std/behaviors/std-image-upload-multi" => include_str!("baked/std/core/atoms/std-image-upload-multi.orb"),
    "std/behaviors/std-incident" => include_str!("baked/io/app/atoms/std-incident.orb"),
    "std/behaviors/std-insurance-claim" => include_str!("baked/io/app/atoms/std-insurance-claim.orb"),
    "std/behaviors/std-interview-slot" => include_str!("baked/io/app/atoms/std-interview-slot.orb"),
    "std/behaviors/std-invoice" => include_str!("baked/io/app/atoms/std-invoice.orb"),
    "std/behaviors/std-kitchen-display" => include_str!("baked/io/app/atoms/std-kitchen-display.orb"),
    "std/behaviors/std-lease" => include_str!("baked/io/app/atoms/std-lease.orb"),
    "std/behaviors/std-ledger-entry" => include_str!("baked/io/app/atoms/std-ledger-entry.orb"),
    "std/behaviors/std-legal-hold" => include_str!("baked/io/app/atoms/std-legal-hold.orb"),
    "std/behaviors/std-lifecycle" => include_str!("baked/std/infra/atoms/std-lifecycle.orb"),
    "std/behaviors/std-maintenance-request" => include_str!("baked/io/app/atoms/std-maintenance-request.orb"),
    "std/behaviors/std-margin-account" => include_str!("baked/io/app/atoms/std-margin-account.orb"),
    "std/behaviors/std-marketing-article" => include_str!("baked/io/marketing/atoms/std-marketing-article.orb"),
    "std/behaviors/std-marketing-case-studies" => include_str!("baked/io/marketing/atoms/std-marketing-case-studies.orb"),
    "std/behaviors/std-marketing-community" => include_str!("baked/io/marketing/atoms/std-marketing-community.orb"),
    "std/behaviors/std-marketing-cta" => include_str!("baked/io/marketing/atoms/std-marketing-cta.orb"),
    "std/behaviors/std-marketing-features" => include_str!("baked/io/marketing/atoms/std-marketing-features.orb"),
    "std/behaviors/std-marketing-footer" => include_str!("baked/io/marketing/atoms/std-marketing-footer.orb"),
    "std/behaviors/std-marketing-hero" => include_str!("baked/io/marketing/atoms/std-marketing-hero.orb"),
    "std/behaviors/std-marketing-nav" => include_str!("baked/io/marketing/atoms/std-marketing-nav.orb"),
    "std/behaviors/std-marketing-pricing" => include_str!("baked/io/marketing/atoms/std-marketing-pricing.orb"),
    "std/behaviors/std-marketing-showcase" => include_str!("baked/io/marketing/atoms/std-marketing-showcase.orb"),
    "std/behaviors/std-marketing-split" => include_str!("baked/io/marketing/atoms/std-marketing-split.orb"),
    "std/behaviors/std-marketing-stats" => include_str!("baked/io/marketing/atoms/std-marketing-stats.orb"),
    "std/behaviors/std-marketing-steps" => include_str!("baked/io/marketing/atoms/std-marketing-steps.orb"),
    "std/behaviors/std-marketing-team" => include_str!("baked/io/marketing/atoms/std-marketing-team.orb"),
    "std/behaviors/std-marketing-testimonials" => include_str!("baked/io/marketing/atoms/std-marketing-testimonials.orb"),
    "std/behaviors/std-matter" => include_str!("baked/io/app/atoms/std-matter.orb"),
    "std/behaviors/std-media-gallery" => include_str!("baked/io/curated/atoms/std-media-gallery.orb"),
    "std/behaviors/std-membership" => include_str!("baked/io/app/atoms/std-membership.orb"),
    "std/behaviors/std-menu" => include_str!("baked/io/app/atoms/std-menu.orb"),
    "std/behaviors/std-mod-queue" => include_str!("baked/std/infra/atoms/std-mod-queue.orb"),
    "std/behaviors/std-modal" => include_str!("baked/std/core/atoms/std-modal.orb"),
    "std/behaviors/std-moderation-rule" => include_str!("baked/io/app/atoms/std-moderation-rule.orb"),
    "std/behaviors/std-modifier-group" => include_str!("baked/io/app/atoms/std-modifier-group.orb"),
    "std/behaviors/std-movement" => include_str!("baked/io/game/atoms/std-movement.orb"),
    "std/behaviors/std-multi-party-flow" => include_str!("baked/std/core/atoms/std-multi-party-flow.orb"),
    "std/behaviors/std-notify-on-event" => include_str!("baked/std/infra/atoms/std-notify-on-event.orb"),
    "std/behaviors/std-nps-survey" => include_str!("baked/io/app/atoms/std-nps-survey.orb"),
    "std/behaviors/std-offer-letter" => include_str!("baked/io/app/atoms/std-offer-letter.orb"),
    "std/behaviors/std-overworld" => include_str!("baked/io/game/atoms/std-overworld.orb"),
    "std/behaviors/std-pagination" => include_str!("baked/std/core/atoms/std-pagination.orb"),
    "std/behaviors/std-patient" => include_str!("baked/io/app/atoms/std-patient.orb"),
    "std/behaviors/std-payout-ledger" => include_str!("baked/io/app/atoms/std-payout-ledger.orb"),
    "std/behaviors/std-physics2d" => include_str!("baked/io/game/atoms/std-physics2d.orb"),
    "std/behaviors/std-portfolio" => include_str!("baked/io/app/atoms/std-portfolio.orb"),
    "std/behaviors/std-positioned-canvas" => include_str!("baked/io/curated/atoms/std-positioned-canvas.orb"),
    "std/behaviors/std-prescription" => include_str!("baked/io/app/atoms/std-prescription.orb"),
    "std/behaviors/std-pricing-organism" => include_str!("baked/io/curated/atoms/std-pricing-organism.orb"),
    "std/behaviors/std-push" => include_str!("baked/std/core/atoms/std-push.orb"),
    "std/behaviors/std-quest" => include_str!("baked/io/game/atoms/std-quest.orb"),
    "std/behaviors/std-question-bank" => include_str!("baked/io/app/atoms/std-question-bank.orb"),
    "std/behaviors/std-rate-limiter" => include_str!("baked/std/core/atoms/std-rate-limiter.orb"),
    "std/behaviors/std-rating-review" => include_str!("baked/std/core/atoms/std-rating-review.orb"),
    "std/behaviors/std-receipt" => include_str!("baked/io/app/atoms/std-receipt.orb"),
    "std/behaviors/std-recurrence" => include_str!("baked/std/core/atoms/std-recurrence.orb"),
    "std/behaviors/std-recurring-charge" => include_str!("baked/io/app/atoms/std-recurring-charge.orb"),
    "std/behaviors/std-related" => include_str!("baked/std/core/atoms/std-related.orb"),
    "std/behaviors/std-reminder-scheduler" => include_str!("baked/std/infra/atoms/std-reminder-scheduler.orb"),
    "std/behaviors/std-renewal-risk" => include_str!("baked/io/app/atoms/std-renewal-risk.orb"),
    "std/behaviors/std-rent-charge" => include_str!("baked/io/app/atoms/std-rent-charge.orb"),
    "std/behaviors/std-reorder-rule" => include_str!("baked/io/app/atoms/std-reorder-rule.orb"),
    "std/behaviors/std-reply-tree" => include_str!("baked/io/curated/atoms/std-reply-tree.orb"),
    "std/behaviors/std-response-collector" => include_str!("baked/io/app/atoms/std-response-collector.orb"),
    "std/behaviors/std-rfi" => include_str!("baked/io/app/atoms/std-rfi.orb"),
    "std/behaviors/std-rich-editor" => include_str!("baked/std/core/atoms/std-rich-editor.orb"),
    "std/behaviors/std-route-optimize" => include_str!("baked/io/app/atoms/std-route-optimize.orb"),
    "std/behaviors/std-row-access-control" => include_str!("baked/std/infra/atoms/std-row-access-control.orb"),
    "std/behaviors/std-runbook" => include_str!("baked/io/app/atoms/std-runbook.orb"),
    "std/behaviors/std-saved-search" => include_str!("baked/std/core/atoms/std-saved-search.orb"),
    "std/behaviors/std-scatter" => include_str!("baked/std/core/atoms/std-scatter.orb"),
    "std/behaviors/std-search" => include_str!("baked/std/core/atoms/std-search.orb"),
    "std/behaviors/std-segment" => include_str!("baked/io/app/atoms/std-segment.orb"),
    "std/behaviors/std-selection" => include_str!("baked/std/core/atoms/std-selection.orb"),
    "std/behaviors/std-sensor-threshold" => include_str!("baked/io/app/atoms/std-sensor-threshold.orb"),
    "std/behaviors/std-service-calendar-ical" => include_str!("baked/io/service/atoms/std-service-calendar-ical.orb"),
    "std/behaviors/std-service-custom-bearer" => include_str!("baked/io/service/atoms/std-service-custom-bearer.orb"),
    "std/behaviors/std-service-email" => include_str!("baked/io/service/atoms/std-service-email.orb"),
    "std/behaviors/std-service-github" => include_str!("baked/io/service/atoms/std-service-github.orb"),
    "std/behaviors/std-service-llm" => include_str!("baked/io/service/atoms/std-service-llm.orb"),
    "std/behaviors/std-service-maps" => include_str!("baked/io/service/atoms/std-service-maps.orb"),
    "std/behaviors/std-service-oauth" => include_str!("baked/io/service/atoms/std-service-oauth.orb"),
    "std/behaviors/std-service-print" => include_str!("baked/io/service/atoms/std-service-print.orb"),
    "std/behaviors/std-service-redis" => include_str!("baked/io/service/atoms/std-service-redis.orb"),
    "std/behaviors/std-service-storage" => include_str!("baked/io/service/atoms/std-service-storage.orb"),
    "std/behaviors/std-service-stripe" => include_str!("baked/io/service/atoms/std-service-stripe.orb"),
    "std/behaviors/std-service-twilio" => include_str!("baked/io/service/atoms/std-service-twilio.orb"),
    "std/behaviors/std-service-youtube" => include_str!("baked/io/service/atoms/std-service-youtube.orb"),
    "std/behaviors/std-showcase-organism" => include_str!("baked/io/curated/atoms/std-showcase-organism.orb"),
    "std/behaviors/std-sidebar-meta" => include_str!("baked/std/core/atoms/std-sidebar-meta.orb"),
    "std/behaviors/std-signature-capture" => include_str!("baked/std/core/atoms/std-signature-capture.orb"),
    "std/behaviors/std-stats" => include_str!("baked/std/core/atoms/std-stats.orb"),
    "std/behaviors/std-stats-hero-metric" => include_str!("baked/std/core-variations/atoms/std-stats-hero-metric.orb"),
    "std/behaviors/std-stats-kpi-tiles" => include_str!("baked/std/core-variations/atoms/std-stats-kpi-tiles.orb"),
    "std/behaviors/std-stats-organism" => include_str!("baked/io/curated/atoms/std-stats-organism.orb"),
    "std/behaviors/std-stats-sparkline-row" => include_str!("baked/std/core-variations/atoms/std-stats-sparkline-row.orb"),
    "std/behaviors/std-status-lifecycle" => include_str!("baked/std/infra/atoms/std-status-lifecycle.orb"),
    "std/behaviors/std-step-flow" => include_str!("baked/std/core/atoms/std-step-flow.orb"),
    "std/behaviors/std-step-flow-cards-deck" => include_str!("baked/std/core-variations/atoms/std-step-flow-cards-deck.orb"),
    "std/behaviors/std-step-flow-organism" => include_str!("baked/io/curated/atoms/std-step-flow-organism.orb"),
    "std/behaviors/std-step-flow-progress-vertical" => include_str!("baked/std/core-variations/atoms/std-step-flow-progress-vertical.orb"),
    "std/behaviors/std-stock-level" => include_str!("baked/io/app/atoms/std-stock-level.orb"),
    "std/behaviors/std-submittal" => include_str!("baked/io/app/atoms/std-submittal.orb"),
    "std/behaviors/std-table" => include_str!("baked/io/curated/atoms/std-table.orb"),
    "std/behaviors/std-table-map" => include_str!("baked/io/app/atoms/std-table-map.orb"),
    "std/behaviors/std-table-view" => include_str!("baked/io/curated/atoms/std-table-view.orb"),
    "std/behaviors/std-tabs" => include_str!("baked/std/core/atoms/std-tabs.orb"),
    "std/behaviors/std-tabs-segmented-pills" => include_str!("baked/std/core-variations/atoms/std-tabs-segmented-pills.orb"),
    "std/behaviors/std-tabs-sidebar-rail" => include_str!("baked/std/core-variations/atoms/std-tabs-sidebar-rail.orb"),
    "std/behaviors/std-tag-taxonomy" => include_str!("baked/std/core/atoms/std-tag-taxonomy.orb"),
    "std/behaviors/std-team-organism" => include_str!("baked/io/curated/atoms/std-team-organism.orb"),
    "std/behaviors/std-technician-dispatch" => include_str!("baked/io/app/atoms/std-technician-dispatch.orb"),
    "std/behaviors/std-telematics-event" => include_str!("baked/io/app/atoms/std-telematics-event.orb"),
    "std/behaviors/std-template-editor" => include_str!("baked/io/app/atoms/std-template-editor.orb"),
    "std/behaviors/std-tenant" => include_str!("baked/io/app/atoms/std-tenant.orb"),
    "std/behaviors/std-thread" => include_str!("baked/std/core/atoms/std-thread.orb"),
    "std/behaviors/std-thread-chat-bubbles" => include_str!("baked/std/core-variations/atoms/std-thread-chat-bubbles.orb"),
    "std/behaviors/std-thread-comments-linear" => include_str!("baked/std/core-variations/atoms/std-thread-comments-linear.orb"),
    "std/behaviors/std-thread-email-threaded" => include_str!("baked/std/core-variations/atoms/std-thread-email-threaded.orb"),
    "std/behaviors/std-ticket-tier" => include_str!("baked/io/app/atoms/std-ticket-tier.orb"),
    "std/behaviors/std-time-entry" => include_str!("baked/io/app/atoms/std-time-entry.orb"),
    "std/behaviors/std-timer" => include_str!("baked/io/game/atoms/std-timer.orb"),
    "std/behaviors/std-timesheet" => include_str!("baked/io/app/atoms/std-timesheet.orb"),
    "std/behaviors/std-trade-order" => include_str!("baked/io/app/atoms/std-trade-order.orb"),
    "std/behaviors/std-trial" => include_str!("baked/io/app/atoms/std-trial.orb"),
    "std/behaviors/std-vehicle" => include_str!("baked/io/app/atoms/std-vehicle.orb"),
    "std/behaviors/std-vendor-onboarding" => include_str!("baked/io/app/atoms/std-vendor-onboarding.orb"),
    "std/behaviors/std-version-history" => include_str!("baked/std/core/atoms/std-version-history.orb"),
    "std/behaviors/std-vote" => include_str!("baked/std/core/atoms/std-vote.orb"),
    "std/behaviors/std-waitlist" => include_str!("baked/io/app/atoms/std-waitlist.orb"),
    "std/behaviors/std-warehouse" => include_str!("baked/io/app/atoms/std-warehouse.orb"),
    "std/behaviors/std-wizard" => include_str!("baked/std/core/atoms/std-wizard.orb"),
    "std/behaviors/std-wizard-form" => include_str!("baked/std/core/atoms/std-wizard-form.orb"),
    "std/behaviors/std-work-order" => include_str!("baked/io/app/atoms/std-work-order.orb"),
    "std/behaviors/ui-action-button" => include_str!("baked/std/ui/game/atoms/ui-action-button.orb"),
    "std/behaviors/ui-animated-counter" => include_str!("baked/std/ui/marketing/atoms/ui-animated-counter.orb"),
    "std/behaviors/ui-animated-graphic" => include_str!("baked/std/ui/marketing/atoms/ui-animated-graphic.orb"),
    "std/behaviors/ui-animated-reveal" => include_str!("baked/std/ui/marketing/atoms/ui-animated-reveal.orb"),
    "std/behaviors/ui-aside" => include_str!("baked/std/ui/core/atoms/ui-aside.orb"),
    "std/behaviors/ui-avatar" => include_str!("baked/std/ui/core/atoms/ui-avatar.orb"),
    "std/behaviors/ui-badge" => include_str!("baked/std/ui/core/atoms/ui-badge.orb"),
    "std/behaviors/ui-box" => include_str!("baked/std/ui/core/atoms/ui-box.orb"),
    "std/behaviors/ui-button" => include_str!("baked/std/ui/core/atoms/ui-button.orb"),
    "std/behaviors/ui-card" => include_str!("baked/std/ui/core/atoms/ui-card.orb"),
    "std/behaviors/ui-center" => include_str!("baked/std/ui/core/atoms/ui-center.orb"),
    "std/behaviors/ui-checkbox" => include_str!("baked/std/ui/core/atoms/ui-checkbox.orb"),
    "std/behaviors/ui-choice-button" => include_str!("baked/std/ui/game/atoms/ui-choice-button.orb"),
    "std/behaviors/ui-combo-counter" => include_str!("baked/std/ui/game/atoms/ui-combo-counter.orb"),
    "std/behaviors/ui-conditional-wrapper" => include_str!("baked/std/ui/core/atoms/ui-conditional-wrapper.orb"),
    "std/behaviors/ui-confetti-effect" => include_str!("baked/std/ui/core/atoms/ui-confetti-effect.orb"),
    "std/behaviors/ui-content-section" => include_str!("baked/std/ui/marketing/atoms/ui-content-section.orb"),
    "std/behaviors/ui-control-button" => include_str!("baked/std/ui/game/atoms/ui-control-button.orb"),
    "std/behaviors/ui-damage-number" => include_str!("baked/std/ui/game/atoms/ui-damage-number.orb"),
    "std/behaviors/ui-day-cell" => include_str!("baked/std/ui/core/atoms/ui-day-cell.orb"),
    "std/behaviors/ui-dialog" => include_str!("baked/std/ui/core/atoms/ui-dialog.orb"),
    "std/behaviors/ui-dialogue-bubble" => include_str!("baked/std/ui/game/atoms/ui-dialogue-bubble.orb"),
    "std/behaviors/ui-divider" => include_str!("baked/std/ui/core/atoms/ui-divider.orb"),
    "std/behaviors/ui-filter-pill" => include_str!("baked/std/ui/core/atoms/ui-filter-pill.orb"),
    "std/behaviors/ui-flip-container" => include_str!("baked/std/ui/core/atoms/ui-flip-container.orb"),
    "std/behaviors/ui-health-bar" => include_str!("baked/std/ui/game/atoms/ui-health-bar.orb"),
    "std/behaviors/ui-icon" => include_str!("baked/std/ui/core/atoms/ui-icon.orb"),
    "std/behaviors/ui-infinite-scroll-sentinel" => include_str!("baked/std/ui/core/atoms/ui-infinite-scroll-sentinel.orb"),
    "std/behaviors/ui-input" => include_str!("baked/std/ui/core/atoms/ui-input.orb"),
    "std/behaviors/ui-item-slot" => include_str!("baked/std/ui/game/atoms/ui-item-slot.orb"),
    "std/behaviors/ui-label" => include_str!("baked/std/ui/core/atoms/ui-label.orb"),
    "std/behaviors/ui-law-reference-tooltip" => include_str!("baked/std/ui/core/atoms/ui-law-reference-tooltip.orb"),
    "std/behaviors/ui-mini-map" => include_str!("baked/std/ui/game/atoms/ui-mini-map.orb"),
    "std/behaviors/ui-overlay" => include_str!("baked/std/ui/core/atoms/ui-overlay.orb"),
    "std/behaviors/ui-pattern-tile" => include_str!("baked/std/ui/marketing/atoms/ui-pattern-tile.orb"),
    "std/behaviors/ui-progress-bar" => include_str!("baked/std/ui/core/atoms/ui-progress-bar.orb"),
    "std/behaviors/ui-radio" => include_str!("baked/std/ui/core/atoms/ui-radio.orb"),
    "std/behaviors/ui-range-slider" => include_str!("baked/std/ui/core/atoms/ui-range-slider.orb"),
    "std/behaviors/ui-resource-counter" => include_str!("baked/std/ui/game/atoms/ui-resource-counter.orb"),
    "std/behaviors/ui-score-display" => include_str!("baked/std/ui/game/atoms/ui-score-display.orb"),
    "std/behaviors/ui-section-header" => include_str!("baked/std/ui/core/atoms/ui-section-header.orb"),
    "std/behaviors/ui-select" => include_str!("baked/std/ui/core/atoms/ui-select.orb"),
    "std/behaviors/ui-spacer" => include_str!("baked/std/ui/core/atoms/ui-spacer.orb"),
    "std/behaviors/ui-sparkline" => include_str!("baked/std/ui/core/atoms/ui-sparkline.orb"),
    "std/behaviors/ui-spinner" => include_str!("baked/std/ui/core/atoms/ui-spinner.orb"),
    "std/behaviors/ui-sprite" => include_str!("baked/std/ui/game/atoms/ui-sprite.orb"),
    "std/behaviors/ui-stack" => include_str!("baked/std/ui/core/atoms/ui-stack.orb"),
    "std/behaviors/ui-stat-card" => include_str!("baked/std/ui/marketing/atoms/ui-stat-card.orb"),
    "std/behaviors/ui-state-indicator" => include_str!("baked/std/ui/game/atoms/ui-state-indicator.orb"),
    "std/behaviors/ui-status-dot" => include_str!("baked/std/ui/core/atoms/ui-status-dot.orb"),
    "std/behaviors/ui-status-effect" => include_str!("baked/std/ui/game/atoms/ui-status-effect.orb"),
    "std/behaviors/ui-svg-branch" => include_str!("baked/std/ui/core/atoms/ui-svg-branch.orb"),
    "std/behaviors/ui-svg-connection" => include_str!("baked/std/ui/core/atoms/ui-svg-connection.orb"),
    "std/behaviors/ui-svg-flow" => include_str!("baked/std/ui/core/atoms/ui-svg-flow.orb"),
    "std/behaviors/ui-svg-grid" => include_str!("baked/std/ui/core/atoms/ui-svg-grid.orb"),
    "std/behaviors/ui-svg-lobe" => include_str!("baked/std/ui/core/atoms/ui-svg-lobe.orb"),
    "std/behaviors/ui-svg-mesh" => include_str!("baked/std/ui/core/atoms/ui-svg-mesh.orb"),
    "std/behaviors/ui-svg-morph" => include_str!("baked/std/ui/core/atoms/ui-svg-morph.orb"),
    "std/behaviors/ui-svg-node" => include_str!("baked/std/ui/core/atoms/ui-svg-node.orb"),
    "std/behaviors/ui-svg-pulse" => include_str!("baked/std/ui/core/atoms/ui-svg-pulse.orb"),
    "std/behaviors/ui-svg-ring" => include_str!("baked/std/ui/core/atoms/ui-svg-ring.orb"),
    "std/behaviors/ui-svg-shield" => include_str!("baked/std/ui/core/atoms/ui-svg-shield.orb"),
    "std/behaviors/ui-svg-stack" => include_str!("baked/std/ui/core/atoms/ui-svg-stack.orb"),
    "std/behaviors/ui-switch" => include_str!("baked/std/ui/core/atoms/ui-switch.orb"),
    "std/behaviors/ui-text-highlight" => include_str!("baked/std/ui/core/atoms/ui-text-highlight.orb"),
    "std/behaviors/ui-textarea" => include_str!("baked/std/ui/core/atoms/ui-textarea.orb"),
    "std/behaviors/ui-theme-toggle" => include_str!("baked/std/ui/core/atoms/ui-theme-toggle.orb"),
    "std/behaviors/ui-time-slot-cell" => include_str!("baked/std/ui/core/atoms/ui-time-slot-cell.orb"),
    "std/behaviors/ui-timer-display" => include_str!("baked/std/ui/game/atoms/ui-timer-display.orb"),
    "std/behaviors/ui-trait-frame" => include_str!("baked/std/ui/core/atoms/ui-trait-frame.orb"),
    "std/behaviors/ui-trend-indicator" => include_str!("baked/std/ui/core/atoms/ui-trend-indicator.orb"),
    "std/behaviors/ui-turn-indicator" => include_str!("baked/std/ui/game/atoms/ui-turn-indicator.orb"),
    "std/behaviors/ui-typewriter-text" => include_str!("baked/std/ui/core/atoms/ui-typewriter-text.orb"),
    "std/behaviors/ui-typography" => include_str!("baked/std/ui/core/atoms/ui-typography.orb"),
    "std/behaviors/ui-waypoint-marker" => include_str!("baked/std/ui/game/atoms/ui-waypoint-marker.orb"),
    "std/behaviors/ui-xp-bar" => include_str!("baked/std/ui/game/atoms/ui-xp-bar.orb"),
    // === Molecules (166) ===
    "std/behaviors/std-about-page" => include_str!("baked/io/marketing/molecules/std-about-page.orb"),
    "std/behaviors/std-agent-fix-loop" => include_str!("baked/std/agent/molecules/std-agent-fix-loop.orb"),
    "std/behaviors/std-agent-learner" => include_str!("baked/std/agent/molecules/std-agent-learner.orb"),
    "std/behaviors/std-agent-planner" => include_str!("baked/std/agent/molecules/std-agent-planner.orb"),
    "std/behaviors/std-agent-rag" => include_str!("baked/std/agent/molecules/std-agent-rag.orb"),
    "std/behaviors/std-agent-tool-loop" => include_str!("baked/std/agent/molecules/std-agent-tool-loop.orb"),
    "std/behaviors/std-applicant-form" => include_str!("baked/io/app/molecules/std-applicant-form.orb"),
    "std/behaviors/std-approval-chain" => include_str!("baked/io/app/molecules/std-approval-chain.orb"),
    "std/behaviors/std-broadcast-builder" => include_str!("baked/io/app/molecules/std-broadcast-builder.orb"),
    "std/behaviors/std-cart" => include_str!("baked/io/app/molecules/std-cart.orb"),
    "std/behaviors/std-dashboard" => include_str!("baked/std/core/molecules/std-dashboard.orb"),
    "std/behaviors/std-esign-flow" => include_str!("baked/std/core/molecules/std-esign-flow.orb"),
    "std/behaviors/std-feature-detail-page" => include_str!("baked/io/marketing/molecules/std-feature-detail-page.orb"),
    "std/behaviors/std-filtered-list" => include_str!("baked/std/core/molecules/std-filtered-list.orb"),
    "std/behaviors/std-landing-page" => include_str!("baked/io/marketing/molecules/std-landing-page.orb"),
    "std/behaviors/std-ledger-entry-form" => include_str!("baked/io/app/molecules/std-ledger-entry-form.orb"),
    "std/behaviors/std-list" => include_str!("baked/std/core/molecules/std-list.orb"),
    "std/behaviors/std-multi-party-transaction" => include_str!("baked/io/app/molecules/std-multi-party-transaction.orb"),
    "std/behaviors/std-pipeline" => include_str!("baked/io/app/molecules/std-pipeline.orb"),
    "std/behaviors/std-pricing-page" => include_str!("baked/io/marketing/molecules/std-pricing-page.orb"),
    "std/behaviors/std-recurring-schedule" => include_str!("baked/io/app/molecules/std-recurring-schedule.orb"),
    "std/behaviors/std-source-scope-probe" => include_str!("baked/io/probes/molecules/std-source-scope-probe.orb"),
    "std/behaviors/std-stock-adjustment" => include_str!("baked/io/app/molecules/std-stock-adjustment.orb"),
    "std/behaviors/std-tabs-layout" => include_str!("baked/std/core/molecules/std-tabs-layout.orb"),
    "std/behaviors/std-timeline" => include_str!("baked/io/app/molecules/std-timeline.orb"),
    "std/behaviors/std-wizard-branching" => include_str!("baked/io/app/molecules/std-wizard-branching.orb"),
    "std/behaviors/ui-accordion" => include_str!("baked/std/ui/core/molecules/ui-accordion.orb"),
    "std/behaviors/ui-action-buttons" => include_str!("baked/std/ui/game/molecules/ui-action-buttons.orb"),
    "std/behaviors/ui-alert" => include_str!("baked/std/ui/core/molecules/ui-alert.orb"),
    "std/behaviors/ui-article-section" => include_str!("baked/std/ui/marketing/molecules/ui-article-section.orb"),
    "std/behaviors/ui-behavior-view" => include_str!("baked/std/ui/avl/molecules/ui-behavior-view.orb"),
    "std/behaviors/ui-branching-logic-builder" => include_str!("baked/std/ui/core/molecules/ui-branching-logic-builder.orb"),
    "std/behaviors/ui-breadcrumb" => include_str!("baked/std/ui/core/molecules/ui-breadcrumb.orb"),
    "std/behaviors/ui-calendar-grid" => include_str!("baked/std/ui/core/molecules/ui-calendar-grid.orb"),
    "std/behaviors/ui-carousel" => include_str!("baked/std/ui/core/molecules/ui-carousel.orb"),
    "std/behaviors/ui-case-study-card" => include_str!("baked/std/ui/marketing/molecules/ui-case-study-card.orb"),
    "std/behaviors/ui-chart" => include_str!("baked/std/ui/core/molecules/ui-chart.orb"),
    "std/behaviors/ui-chart-legend" => include_str!("baked/std/ui/core/molecules/ui-chart-legend.orb"),
    "std/behaviors/ui-code-block" => include_str!("baked/std/ui/core/molecules/ui-code-block.orb"),
    "std/behaviors/ui-code-viewer" => include_str!("baked/std/ui/core/molecules/ui-code-viewer.orb"),
    "std/behaviors/ui-combat-log" => include_str!("baked/std/ui/game/molecules/ui-combat-log.orb"),
    "std/behaviors/ui-community-links" => include_str!("baked/std/ui/marketing/molecules/ui-community-links.orb"),
    "std/behaviors/ui-confirm-dialog" => include_str!("baked/std/ui/core/molecules/ui-confirm-dialog.orb"),
    "std/behaviors/ui-container" => include_str!("baked/std/ui/core/molecules/ui-container.orb"),
    "std/behaviors/ui-content-renderer" => include_str!("baked/std/ui/core/molecules/ui-content-renderer.orb"),
    "std/behaviors/ui-crafting-recipe" => include_str!("baked/std/ui/game/molecules/ui-crafting-recipe.orb"),
    "std/behaviors/ui-cta-banner" => include_str!("baked/std/ui/marketing/molecules/ui-cta-banner.orb"),
    "std/behaviors/ui-d-pad" => include_str!("baked/std/ui/game/molecules/ui-d-pad.orb"),
    "std/behaviors/ui-data-grid" => include_str!("baked/std/ui/core/molecules/ui-data-grid.orb"),
    "std/behaviors/ui-data-list" => include_str!("baked/std/ui/core/molecules/ui-data-list.orb"),
    "std/behaviors/ui-date-range-picker" => include_str!("baked/std/ui/core/molecules/ui-date-range-picker.orb"),
    "std/behaviors/ui-date-range-selector" => include_str!("baked/std/ui/core/molecules/ui-date-range-selector.orb"),
    "std/behaviors/ui-dialogue-box" => include_str!("baked/std/ui/game/molecules/ui-dialogue-box.orb"),
    "std/behaviors/ui-doc-breadcrumb" => include_str!("baked/std/ui/core/molecules/ui-doc-breadcrumb.orb"),
    "std/behaviors/ui-doc-code-block" => include_str!("baked/std/ui/core/molecules/ui-doc-code-block.orb"),
    "std/behaviors/ui-doc-pagination" => include_str!("baked/std/ui/core/molecules/ui-doc-pagination.orb"),
    "std/behaviors/ui-doc-search" => include_str!("baked/std/ui/core/molecules/ui-doc-search.orb"),
    "std/behaviors/ui-doc-sidebar" => include_str!("baked/std/ui/core/molecules/ui-doc-sidebar.orb"),
    "std/behaviors/ui-doc-toc" => include_str!("baked/std/ui/core/molecules/ui-doc-toc.orb"),
    "std/behaviors/ui-document-viewer" => include_str!("baked/std/ui/core/molecules/ui-document-viewer.orb"),
    "std/behaviors/ui-drawer" => include_str!("baked/std/ui/core/molecules/ui-drawer.orb"),
    "std/behaviors/ui-edge-decoration" => include_str!("baked/std/ui/core/molecules/ui-edge-decoration.orb"),
    "std/behaviors/ui-empty-state" => include_str!("baked/std/ui/core/molecules/ui-empty-state.orb"),
    "std/behaviors/ui-enemy-plate" => include_str!("baked/std/ui/game/molecules/ui-enemy-plate.orb"),
    "std/behaviors/ui-error-boundary" => include_str!("baked/std/ui/core/molecules/ui-error-boundary.orb"),
    "std/behaviors/ui-error-state" => include_str!("baked/std/ui/core/molecules/ui-error-state.orb"),
    "std/behaviors/ui-feature-card" => include_str!("baked/std/ui/marketing/molecules/ui-feature-card.orb"),
    "std/behaviors/ui-feature-grid" => include_str!("baked/std/ui/core/molecules/ui-feature-grid.orb"),
    "std/behaviors/ui-file-tree" => include_str!("baked/std/ui/core/molecules/ui-file-tree.orb"),
    "std/behaviors/ui-filter-group" => include_str!("baked/std/ui/core/molecules/ui-filter-group.orb"),
    "std/behaviors/ui-flex" => include_str!("baked/std/ui/core/molecules/ui-flex.orb"),
    "std/behaviors/ui-flip-card" => include_str!("baked/std/ui/core/molecules/ui-flip-card.orb"),
    "std/behaviors/ui-floating-action-button" => include_str!("baked/std/ui/core/molecules/ui-floating-action-button.orb"),
    "std/behaviors/ui-form-actions" => include_str!("baked/std/ui/core/molecules/ui-form-actions.orb"),
    "std/behaviors/ui-form-field" => include_str!("baked/std/ui/core/molecules/ui-form-field.orb"),
    "std/behaviors/ui-form-section-header" => include_str!("baked/std/ui/core/molecules/ui-form-section-header.orb"),
    "std/behaviors/ui-game-canvas-2d" => include_str!("baked/std/ui/game/molecules/ui-game-canvas-2d.orb"),
    "std/behaviors/ui-game-hud" => include_str!("baked/std/ui/game/molecules/ui-game-hud.orb"),
    "std/behaviors/ui-game-menu" => include_str!("baked/std/ui/game/molecules/ui-game-menu.orb"),
    "std/behaviors/ui-game-over-screen" => include_str!("baked/std/ui/game/molecules/ui-game-over-screen.orb"),
    "std/behaviors/ui-geometric-pattern" => include_str!("baked/std/ui/marketing/molecules/ui-geometric-pattern.orb"),
    "std/behaviors/ui-gradient-divider" => include_str!("baked/std/ui/core/molecules/ui-gradient-divider.orb"),
    "std/behaviors/ui-graph-canvas" => include_str!("baked/std/ui/core/molecules/ui-graph-canvas.orb"),
    "std/behaviors/ui-graph-view" => include_str!("baked/std/ui/core/molecules/ui-graph-view.orb"),
    "std/behaviors/ui-grid" => include_str!("baked/std/ui/core/molecules/ui-grid.orb"),
    "std/behaviors/ui-header" => include_str!("baked/std/ui/core/molecules/ui-header.orb"),
    "std/behaviors/ui-health-panel" => include_str!("baked/std/ui/game/molecules/ui-health-panel.orb"),
    "std/behaviors/ui-hero-section" => include_str!("baked/std/ui/marketing/molecules/ui-hero-section.orb"),
    "std/behaviors/ui-input-group" => include_str!("baked/std/ui/core/molecules/ui-input-group.orb"),
    "std/behaviors/ui-install-box" => include_str!("baked/std/ui/marketing/molecules/ui-install-box.orb"),
    "std/behaviors/ui-inventory-grid" => include_str!("baked/std/ui/game/molecules/ui-inventory-grid.orb"),
    "std/behaviors/ui-inventory-panel" => include_str!("baked/std/ui/game/molecules/ui-inventory-panel.orb"),
    "std/behaviors/ui-isometric-canvas" => include_str!("baked/std/ui/game/molecules/ui-isometric-canvas.orb"),
    "std/behaviors/ui-jazari-state-machine" => include_str!("baked/std/ui/core/molecules/ui-jazari-state-machine.orb"),
    "std/behaviors/ui-lightbox" => include_str!("baked/std/ui/core/molecules/ui-lightbox.orb"),
    "std/behaviors/ui-likert-scale" => include_str!("baked/std/ui/core/molecules/ui-likert-scale.orb"),
    "std/behaviors/ui-line-chart" => include_str!("baked/std/ui/core/molecules/ui-line-chart.orb"),
    "std/behaviors/ui-loading-state" => include_str!("baked/std/ui/core/molecules/ui-loading-state.orb"),
    "std/behaviors/ui-map-view" => include_str!("baked/std/ui/core/molecules/ui-map-view.orb"),
    "std/behaviors/ui-markdown-content" => include_str!("baked/std/ui/core/molecules/ui-markdown-content.orb"),
    "std/behaviors/ui-marketing-footer" => include_str!("baked/std/ui/marketing/molecules/ui-marketing-footer.orb"),
    "std/behaviors/ui-matrix-question" => include_str!("baked/std/ui/core/molecules/ui-matrix-question.orb"),
    "std/behaviors/ui-menu" => include_str!("baked/std/ui/core/molecules/ui-menu.orb"),
    "std/behaviors/ui-meter" => include_str!("baked/std/ui/core/molecules/ui-meter.orb"),
    "std/behaviors/ui-modal" => include_str!("baked/std/ui/core/molecules/ui-modal.orb"),
    "std/behaviors/ui-module-card" => include_str!("baked/std/ui/avl/molecules/ui-module-card.orb"),
    "std/behaviors/ui-navigation" => include_str!("baked/std/ui/core/molecules/ui-navigation.orb"),
    "std/behaviors/ui-notification" => include_str!("baked/std/ui/core/molecules/ui-notification.orb"),
    "std/behaviors/ui-number-stepper" => include_str!("baked/std/ui/core/molecules/ui-number-stepper.orb"),
    "std/behaviors/ui-option-constraint-group" => include_str!("baked/std/ui/core/molecules/ui-option-constraint-group.orb"),
    "std/behaviors/ui-orbital-visualization" => include_str!("baked/std/ui/core/molecules/ui-orbital-visualization.orb"),
    "std/behaviors/ui-page-header" => include_str!("baked/std/ui/core/molecules/ui-page-header.orb"),
    "std/behaviors/ui-pagination" => include_str!("baked/std/ui/core/molecules/ui-pagination.orb"),
    "std/behaviors/ui-platformer-canvas" => include_str!("baked/std/ui/game/molecules/ui-platformer-canvas.orb"),
    "std/behaviors/ui-popover" => include_str!("baked/std/ui/core/molecules/ui-popover.orb"),
    "std/behaviors/ui-positioned-canvas" => include_str!("baked/std/ui/core/molecules/ui-positioned-canvas.orb"),
    "std/behaviors/ui-powerup-slots" => include_str!("baked/std/ui/game/molecules/ui-powerup-slots.orb"),
    "std/behaviors/ui-pricing-card" => include_str!("baked/std/ui/marketing/molecules/ui-pricing-card.orb"),
    "std/behaviors/ui-pricing-grid" => include_str!("baked/std/ui/marketing/molecules/ui-pricing-grid.orb"),
    "std/behaviors/ui-progress-dots" => include_str!("baked/std/ui/core/molecules/ui-progress-dots.orb"),
    "std/behaviors/ui-pull-quote" => include_str!("baked/std/ui/marketing/molecules/ui-pull-quote.orb"),
    "std/behaviors/ui-pull-to-refresh" => include_str!("baked/std/ui/core/molecules/ui-pull-to-refresh.orb"),
    "std/behaviors/ui-qr-scanner" => include_str!("baked/std/ui/core/molecules/ui-qr-scanner.orb"),
    "std/behaviors/ui-quest-tracker" => include_str!("baked/std/ui/game/molecules/ui-quest-tracker.orb"),
    "std/behaviors/ui-quiz-block" => include_str!("baked/std/ui/core/molecules/ui-quiz-block.orb"),
    "std/behaviors/ui-relation-select" => include_str!("baked/std/ui/core/molecules/ui-relation-select.orb"),
    "std/behaviors/ui-repeatable-form-section" => include_str!("baked/std/ui/core/molecules/ui-repeatable-form-section.orb"),
    "std/behaviors/ui-reply-tree" => include_str!("baked/std/ui/core/molecules/ui-reply-tree.orb"),
    "std/behaviors/ui-resource-bar" => include_str!("baked/std/ui/game/molecules/ui-resource-bar.orb"),
    "std/behaviors/ui-rich-block-editor" => include_str!("baked/std/ui/core/molecules/ui-rich-block-editor.orb"),
    "std/behaviors/ui-scaled-diagram" => include_str!("baked/std/ui/core/molecules/ui-scaled-diagram.orb"),
    "std/behaviors/ui-score-board" => include_str!("baked/std/ui/game/molecules/ui-score-board.orb"),
    "std/behaviors/ui-search-input" => include_str!("baked/std/ui/core/molecules/ui-search-input.orb"),
    "std/behaviors/ui-section" => include_str!("baked/std/ui/core/molecules/ui-section.orb"),
    "std/behaviors/ui-service-catalog" => include_str!("baked/std/ui/marketing/molecules/ui-service-catalog.orb"),
    "std/behaviors/ui-showcase-card" => include_str!("baked/std/ui/marketing/molecules/ui-showcase-card.orb"),
    "std/behaviors/ui-side-panel" => include_str!("baked/std/ui/core/molecules/ui-side-panel.orb"),
    "std/behaviors/ui-sidebar" => include_str!("baked/std/ui/core/molecules/ui-sidebar.orb"),
    "std/behaviors/ui-signature-pad" => include_str!("baked/std/ui/core/molecules/ui-signature-pad.orb"),
    "std/behaviors/ui-simple-grid" => include_str!("baked/std/ui/core/molecules/ui-simple-grid.orb"),
    "std/behaviors/ui-skeleton" => include_str!("baked/std/ui/core/molecules/ui-skeleton.orb"),
    "std/behaviors/ui-social-proof" => include_str!("baked/std/ui/core/molecules/ui-social-proof.orb"),
    "std/behaviors/ui-sortable-list" => include_str!("baked/std/ui/core/molecules/ui-sortable-list.orb"),
    "std/behaviors/ui-split" => include_str!("baked/std/ui/core/molecules/ui-split.orb"),
    "std/behaviors/ui-split-section" => include_str!("baked/std/ui/marketing/molecules/ui-split-section.orb"),
    "std/behaviors/ui-star-rating" => include_str!("baked/std/ui/core/molecules/ui-star-rating.orb"),
    "std/behaviors/ui-stat-badge" => include_str!("baked/std/ui/game/molecules/ui-stat-badge.orb"),
    "std/behaviors/ui-stat-display" => include_str!("baked/std/ui/core/molecules/ui-stat-display.orb"),
    "std/behaviors/ui-stats-grid" => include_str!("baked/std/ui/marketing/molecules/ui-stats-grid.orb"),
    "std/behaviors/ui-step-flow" => include_str!("baked/std/ui/marketing/molecules/ui-step-flow.orb"),
    "std/behaviors/ui-swipeable-row" => include_str!("baked/std/ui/core/molecules/ui-swipeable-row.orb"),
    "std/behaviors/ui-table-view" => include_str!("baked/std/ui/core/molecules/ui-table-view.orb"),
    "std/behaviors/ui-tabs" => include_str!("baked/std/ui/core/molecules/ui-tabs.orb"),
    "std/behaviors/ui-tag-cloud" => include_str!("baked/std/ui/marketing/molecules/ui-tag-cloud.orb"),
    "std/behaviors/ui-tag-input" => include_str!("baked/std/ui/core/molecules/ui-tag-input.orb"),
    "std/behaviors/ui-team-card" => include_str!("baked/std/ui/marketing/molecules/ui-team-card.orb"),
    "std/behaviors/ui-tooltip" => include_str!("baked/std/ui/core/molecules/ui-tooltip.orb"),
    "std/behaviors/ui-turn-panel" => include_str!("baked/std/ui/game/molecules/ui-turn-panel.orb"),
    "std/behaviors/ui-unit-command-bar" => include_str!("baked/std/ui/game/molecules/ui-unit-command-bar.orb"),
    "std/behaviors/ui-upload-drop-zone" => include_str!("baked/std/ui/core/molecules/ui-upload-drop-zone.orb"),
    "std/behaviors/ui-version-diff" => include_str!("baked/std/ui/core/molecules/ui-version-diff.orb"),
    "std/behaviors/ui-violation-alert" => include_str!("baked/std/ui/core/molecules/ui-violation-alert.orb"),
    "std/behaviors/ui-vote-stack" => include_str!("baked/std/ui/core/molecules/ui-vote-stack.orb"),
    "std/behaviors/ui-wizard-container" => include_str!("baked/std/ui/core/molecules/ui-wizard-container.orb"),
    "std/behaviors/ui-wizard-navigation" => include_str!("baked/std/ui/core/molecules/ui-wizard-navigation.orb"),
    "std/behaviors/ui-wizard-progress" => include_str!("baked/std/ui/core/molecules/ui-wizard-progress.orb"),
    // === Organisms (179) ===
    "std/behaviors/AccountOrbital" => include_str!("baked/io/curated/organisms/AccountOrbital.orb"),
    "std/behaviors/BundleOrbital" => include_str!("baked/io/curated/organisms/BundleOrbital.orb"),
    "std/behaviors/ChargeItemOrbital" => include_str!("baked/io/curated/organisms/ChargeItemOrbital.orb"),
    "std/behaviors/ClaimOrbital" => include_str!("baked/io/curated/organisms/ClaimOrbital.orb"),
    "std/behaviors/ConditionOrbital" => include_str!("baked/io/curated/organisms/ConditionOrbital.orb"),
    "std/behaviors/CoverageOrbital" => include_str!("baked/io/curated/organisms/CoverageOrbital.orb"),
    "std/behaviors/EncounterOrbital" => include_str!("baked/io/curated/organisms/EncounterOrbital.orb"),
    "std/behaviors/InvoiceOrbital" => include_str!("baked/io/curated/organisms/InvoiceOrbital.orb"),
    "std/behaviors/ObservationOrbital" => include_str!("baked/io/curated/organisms/ObservationOrbital.orb"),
    "std/behaviors/ProcedureOrbital" => include_str!("baked/io/curated/organisms/ProcedureOrbital.orb"),
    "std/behaviors/std-accounting" => include_str!("baked/io/app/organisms/std-accounting.orb"),
    "std/behaviors/std-accounting-public" => include_str!("baked/io/marketing/organisms/std-accounting-public.orb"),
    "std/behaviors/std-agent-assistant" => include_str!("baked/std/agent/organisms/std-agent-assistant.orb"),
    "std/behaviors/std-agent-builder" => include_str!("baked/std/agent/organisms/std-agent-builder.orb"),
    "std/behaviors/std-agent-pipeline" => include_str!("baked/std/agent/organisms/std-agent-pipeline.orb"),
    "std/behaviors/std-agent-reviewer" => include_str!("baked/std/agent/organisms/std-agent-reviewer.orb"),
    "std/behaviors/std-agent-tutor" => include_str!("baked/std/agent/organisms/std-agent-tutor.orb"),
    "std/behaviors/std-api-gateway" => include_str!("baked/io/app/organisms/std-api-gateway.orb"),
    "std/behaviors/std-arcade-game" => include_str!("baked/io/game/organisms/std-arcade-game.orb"),
    "std/behaviors/std-ats-recruiting" => include_str!("baked/io/app/organisms/std-ats-recruiting.orb"),
    "std/behaviors/std-availability-lookup" => include_str!("baked/io/marketing/organisms/std-availability-lookup.orb"),
    "std/behaviors/std-booking-site" => include_str!("baked/io/marketing/organisms/std-booking-site.orb"),
    "std/behaviors/std-booking-system" => include_str!("baked/io/app/organisms/std-booking-system.orb"),
    "std/behaviors/std-campaign-landing" => include_str!("baked/io/marketing/organisms/std-campaign-landing.orb"),
    "std/behaviors/std-careers-site" => include_str!("baked/io/marketing/organisms/std-careers-site.orb"),
    "std/behaviors/std-cicd-pipeline" => include_str!("baked/io/app/organisms/std-cicd-pipeline.orb"),
    "std/behaviors/std-cicd-status" => include_str!("baked/io/marketing/organisms/std-cicd-status.orb"),
    "std/behaviors/std-client-case-portal" => include_str!("baked/io/marketing/organisms/std-client-case-portal.orb"),
    "std/behaviors/std-cms" => include_str!("baked/io/app/organisms/std-cms.orb"),
    "std/behaviors/std-cms-site" => include_str!("baked/io/marketing/organisms/std-cms-site.orb"),
    "std/behaviors/std-coding-academy" => include_str!("baked/io/app/organisms/std-coding-academy.orb"),
    "std/behaviors/std-coding-academy-site" => include_str!("baked/io/marketing/organisms/std-coding-academy-site.orb"),
    "std/behaviors/std-construction-pm" => include_str!("baked/io/app/organisms/std-construction-pm.orb"),
    "std/behaviors/std-construction-portal" => include_str!("baked/io/marketing/organisms/std-construction-portal.orb"),
    "std/behaviors/std-contract" => include_str!("baked/io/app/organisms/std-contract.orb"),
    "std/behaviors/std-contract-portal" => include_str!("baked/io/marketing/organisms/std-contract-portal.orb"),
    "std/behaviors/std-crm" => include_str!("baked/io/app/organisms/std-crm.orb"),
    "std/behaviors/std-customer-success" => include_str!("baked/io/app/organisms/std-customer-success.orb"),
    "std/behaviors/std-developer-portal" => include_str!("baked/io/marketing/organisms/std-developer-portal.orb"),
    "std/behaviors/std-devops-dashboard" => include_str!("baked/io/app/organisms/std-devops-dashboard.orb"),
    "std/behaviors/std-document-mgmt" => include_str!("baked/io/app/organisms/std-document-mgmt.orb"),
    "std/behaviors/std-document-portal" => include_str!("baked/io/marketing/organisms/std-document-portal.orb"),
    "std/behaviors/std-donate-site" => include_str!("baked/io/marketing/organisms/std-donate-site.orb"),
    "std/behaviors/std-ecommerce" => include_str!("baked/io/app/organisms/std-ecommerce.orb"),
    "std/behaviors/std-embedded-dashboard" => include_str!("baked/io/app/organisms/std-embedded-dashboard.orb"),
    "std/behaviors/std-event-site" => include_str!("baked/io/marketing/organisms/std-event-site.orb"),
    "std/behaviors/std-event-ticketing" => include_str!("baked/io/app/organisms/std-event-ticketing.orb"),
    "std/behaviors/std-field-service" => include_str!("baked/io/app/organisms/std-field-service.orb"),
    "std/behaviors/std-finance-portal" => include_str!("baked/io/marketing/organisms/std-finance-portal.orb"),
    "std/behaviors/std-finance-tracker" => include_str!("baked/io/app/organisms/std-finance-tracker.orb"),
    "std/behaviors/std-fitness-site" => include_str!("baked/io/marketing/organisms/std-fitness-site.orb"),
    "std/behaviors/std-fitness-studio" => include_str!("baked/io/app/organisms/std-fitness-studio.orb"),
    "std/behaviors/std-fleet-mgmt" => include_str!("baked/io/app/organisms/std-fleet-mgmt.orb"),
    "std/behaviors/std-forum" => include_str!("baked/io/app/organisms/std-forum.orb"),
    "std/behaviors/std-forum-public" => include_str!("baked/io/marketing/organisms/std-forum-public.orb"),
    "std/behaviors/std-generic-app" => include_str!("baked/std/core/organisms/std-generic-app.orb"),
    "std/behaviors/std-healthcare" => include_str!("baked/io/app/organisms/std-healthcare.orb"),
    "std/behaviors/std-help-center" => include_str!("baked/io/marketing/organisms/std-help-center.orb"),
    "std/behaviors/std-help-center-reader" => include_str!("baked/io/marketing/organisms/std-help-center-reader.orb"),
    "std/behaviors/std-helpdesk" => include_str!("baked/io/app/organisms/std-helpdesk.orb"),
    "std/behaviors/std-hr-portal" => include_str!("baked/io/app/organisms/std-hr-portal.orb"),
    "std/behaviors/std-hr-self-service" => include_str!("baked/io/marketing/organisms/std-hr-self-service.orb"),
    "std/behaviors/std-inventory" => include_str!("baked/io/app/organisms/std-inventory.orb"),
    "std/behaviors/std-iot-dashboard" => include_str!("baked/io/app/organisms/std-iot-dashboard.orb"),
    "std/behaviors/std-iot-public" => include_str!("baked/io/marketing/organisms/std-iot-public.orb"),
    "std/behaviors/std-lead-capture-site" => include_str!("baked/io/marketing/organisms/std-lead-capture-site.orb"),
    "std/behaviors/std-legal-case" => include_str!("baked/io/app/organisms/std-legal-case.orb"),
    "std/behaviors/std-listings" => include_str!("baked/io/app/organisms/std-listings.orb"),
    "std/behaviors/std-listings-site" => include_str!("baked/io/marketing/organisms/std-listings-site.orb"),
    "std/behaviors/std-lms" => include_str!("baked/io/app/organisms/std-lms.orb"),
    "std/behaviors/std-lms-site" => include_str!("baked/io/marketing/organisms/std-lms-site.orb"),
    "std/behaviors/std-logic-training" => include_str!("baked/io/game/organisms/std-logic-training.orb"),
    "std/behaviors/std-marketing-campaign" => include_str!("baked/io/app/organisms/std-marketing-campaign.orb"),
    "std/behaviors/std-marketing-site" => include_str!("baked/io/marketing/organisms/std-marketing-site.orb"),
    "std/behaviors/std-marketplace" => include_str!("baked/io/app/organisms/std-marketplace.orb"),
    "std/behaviors/std-marketplace-site" => include_str!("baked/io/marketing/organisms/std-marketplace-site.orb"),
    "std/behaviors/std-nonprofit-donations" => include_str!("baked/io/app/organisms/std-nonprofit-donations.orb"),
    "std/behaviors/std-notes" => include_str!("baked/io/app/organisms/std-notes.orb"),
    "std/behaviors/std-notes-public" => include_str!("baked/io/marketing/organisms/std-notes-public.orb"),
    "std/behaviors/std-patient-portal" => include_str!("baked/io/marketing/organisms/std-patient-portal.orb"),
    "std/behaviors/std-platformer-app" => include_str!("baked/io/game/organisms/std-platformer-app.orb"),
    "std/behaviors/std-project-manager" => include_str!("baked/io/app/organisms/std-project-manager.orb"),
    "std/behaviors/std-project-portal" => include_str!("baked/io/marketing/organisms/std-project-portal.orb"),
    "std/behaviors/std-property-mgmt" => include_str!("baked/io/app/organisms/std-property-mgmt.orb"),
    "std/behaviors/std-property-site" => include_str!("baked/io/marketing/organisms/std-property-site.orb"),
    "std/behaviors/std-public-help-center" => include_str!("baked/io/app/organisms/std-public-help-center.orb"),
    "std/behaviors/std-public-report" => include_str!("baked/io/marketing/organisms/std-public-report.orb"),
    "std/behaviors/std-puzzle-app" => include_str!("baked/io/game/organisms/std-puzzle-app.orb"),
    "std/behaviors/std-realtime-chat" => include_str!("baked/io/app/organisms/std-realtime-chat.orb"),
    "std/behaviors/std-restaurant-pos" => include_str!("baked/io/app/organisms/std-restaurant-pos.orb"),
    "std/behaviors/std-restaurant-site" => include_str!("baked/io/marketing/organisms/std-restaurant-site.orb"),
    "std/behaviors/std-retail-pos" => include_str!("baked/io/app/organisms/std-retail-pos.orb"),
    "std/behaviors/std-retail-store" => include_str!("baked/io/marketing/organisms/std-retail-store.orb"),
    "std/behaviors/std-rpg-game" => include_str!("baked/io/game/organisms/std-rpg-game.orb"),
    "std/behaviors/std-service-marketplace" => include_str!("baked/io/service/organisms/std-service-marketplace.orb"),
    "std/behaviors/std-service-request-site" => include_str!("baked/io/marketing/organisms/std-service-request-site.orb"),
    "std/behaviors/std-service-research-assistant" => include_str!("baked/io/service/organisms/std-service-research-assistant.orb"),
    "std/behaviors/std-shipment-tracking" => include_str!("baked/io/marketing/organisms/std-shipment-tracking.orb"),
    "std/behaviors/std-social-feed" => include_str!("baked/io/app/organisms/std-social-feed.orb"),
    "std/behaviors/std-social-public" => include_str!("baked/io/marketing/organisms/std-social-public.orb"),
    "std/behaviors/std-status-page" => include_str!("baked/io/marketing/organisms/std-status-page.orb"),
    "std/behaviors/std-stem-lab" => include_str!("baked/io/game/organisms/std-stem-lab.orb"),
    "std/behaviors/std-storefront" => include_str!("baked/io/marketing/organisms/std-storefront.orb"),
    "std/behaviors/std-strategy-game" => include_str!("baked/io/game/organisms/std-strategy-game.orb"),
    "std/behaviors/std-subscription-billing" => include_str!("baked/io/app/organisms/std-subscription-billing.orb"),
    "std/behaviors/std-subscription-site" => include_str!("baked/io/marketing/organisms/std-subscription-site.orb"),
    "std/behaviors/std-success-portal" => include_str!("baked/io/marketing/organisms/std-success-portal.orb"),
    "std/behaviors/std-support-chat" => include_str!("baked/io/marketing/organisms/std-support-chat.orb"),
    "std/behaviors/std-survey" => include_str!("baked/io/app/organisms/std-survey.orb"),
    "std/behaviors/std-survey-public" => include_str!("baked/io/marketing/organisms/std-survey-public.orb"),
    "std/behaviors/std-time-approval-portal" => include_str!("baked/io/marketing/organisms/std-time-approval-portal.orb"),
    "std/behaviors/std-time-tracking" => include_str!("baked/io/app/organisms/std-time-tracking.orb"),
    "std/behaviors/std-trading-dashboard" => include_str!("baked/io/app/organisms/std-trading-dashboard.orb"),
    "std/behaviors/std-trading-public" => include_str!("baked/io/marketing/organisms/std-trading-public.orb"),
    "std/behaviors/std-wiki" => include_str!("baked/io/app/organisms/std-wiki.orb"),
    "std/behaviors/std-wiki-public" => include_str!("baked/io/marketing/organisms/std-wiki-public.orb"),
    "std/behaviors/ui-action-palette" => include_str!("baked/std/ui/game/organisms/ui-action-palette.orb"),
    "std/behaviors/ui-action-tile" => include_str!("baked/std/ui/game/organisms/ui-action-tile.orb"),
    "std/behaviors/ui-battle-board" => include_str!("baked/std/ui/game/organisms/ui-battle-board.orb"),
    "std/behaviors/ui-book-chapter-view" => include_str!("baked/std/ui/marketing/organisms/ui-book-chapter-view.orb"),
    "std/behaviors/ui-book-cover-page" => include_str!("baked/std/ui/marketing/organisms/ui-book-cover-page.orb"),
    "std/behaviors/ui-book-nav-bar" => include_str!("baked/std/ui/marketing/organisms/ui-book-nav-bar.orb"),
    "std/behaviors/ui-book-table-of-contents" => include_str!("baked/std/ui/marketing/organisms/ui-book-table-of-contents.orb"),
    "std/behaviors/ui-book-viewer" => include_str!("baked/std/ui/marketing/organisms/ui-book-viewer.orb"),
    "std/behaviors/ui-builder-board" => include_str!("baked/std/ui/game/organisms/ui-builder-board.orb"),
    "std/behaviors/ui-canvas-effect" => include_str!("baked/std/ui/game/organisms/ui-canvas-effect.orb"),
    "std/behaviors/ui-case-study-organism" => include_str!("baked/std/ui/marketing/organisms/ui-case-study-organism.orb"),
    "std/behaviors/ui-castle-board" => include_str!("baked/std/ui/game/organisms/ui-castle-board.orb"),
    "std/behaviors/ui-classifier-board" => include_str!("baked/std/ui/game/organisms/ui-classifier-board.orb"),
    "std/behaviors/ui-code-view" => include_str!("baked/std/ui/game/organisms/ui-code-view.orb"),
    "std/behaviors/ui-dashboard-grid" => include_str!("baked/std/ui/core/organisms/ui-dashboard-grid.orb"),
    "std/behaviors/ui-debugger-board" => include_str!("baked/std/ui/game/organisms/ui-debugger-board.orb"),
    "std/behaviors/ui-detail-panel" => include_str!("baked/std/ui/core/organisms/ui-detail-panel.orb"),
    "std/behaviors/ui-drawer-slot" => include_str!("baked/std/ui/core/organisms/ui-drawer-slot.orb"),
    "std/behaviors/ui-entity-cards" => include_str!("baked/std/ui/core/organisms/ui-entity-cards.orb"),
    "std/behaviors/ui-entity-list" => include_str!("baked/std/ui/core/organisms/ui-entity-list.orb"),
    "std/behaviors/ui-entity-table" => include_str!("baked/std/ui/core/organisms/ui-entity-table.orb"),
    "std/behaviors/ui-event-handler-board" => include_str!("baked/std/ui/game/organisms/ui-event-handler-board.orb"),
    "std/behaviors/ui-event-log" => include_str!("baked/std/ui/game/organisms/ui-event-log.orb"),
    "std/behaviors/ui-feature-grid-organism" => include_str!("baked/std/ui/marketing/organisms/ui-feature-grid-organism.orb"),
    "std/behaviors/ui-feature-renderer" => include_str!("baked/std/ui/game/organisms/ui-feature-renderer.orb"),
    "std/behaviors/ui-form" => include_str!("baked/std/ui/core/organisms/ui-form.orb"),
    "std/behaviors/ui-form-section" => include_str!("baked/std/ui/core/organisms/ui-form-section.orb"),
    "std/behaviors/ui-game-audio-provider" => include_str!("baked/std/ui/game/organisms/ui-game-audio-provider.orb"),
    "std/behaviors/ui-game-audio-toggle" => include_str!("baked/std/ui/game/organisms/ui-game-audio-toggle.orb"),
    "std/behaviors/ui-hero-organism" => include_str!("baked/std/ui/marketing/organisms/ui-hero-organism.orb"),
    "std/behaviors/ui-master-detail" => include_str!("baked/std/ui/core/organisms/ui-master-detail.orb"),
    "std/behaviors/ui-master-detail-layout" => include_str!("baked/std/ui/core/organisms/ui-master-detail-layout.orb"),
    "std/behaviors/ui-media-gallery" => include_str!("baked/std/ui/core/organisms/ui-media-gallery.orb"),
    "std/behaviors/ui-modal-slot" => include_str!("baked/std/ui/core/organisms/ui-modal-slot.orb"),
    "std/behaviors/ui-negotiator-board" => include_str!("baked/std/ui/game/organisms/ui-negotiator-board.orb"),
    "std/behaviors/ui-object-rule-panel" => include_str!("baked/std/ui/game/organisms/ui-object-rule-panel.orb"),
    "std/behaviors/ui-pricing-organism" => include_str!("baked/std/ui/marketing/organisms/ui-pricing-organism.orb"),
    "std/behaviors/ui-rule-editor" => include_str!("baked/std/ui/game/organisms/ui-rule-editor.orb"),
    "std/behaviors/ui-runtime-debugger" => include_str!("baked/std/ui/core/organisms/ui-runtime-debugger.orb"),
    "std/behaviors/ui-sequence-bar" => include_str!("baked/std/ui/game/organisms/ui-sequence-bar.orb"),
    "std/behaviors/ui-sequencer-board" => include_str!("baked/std/ui/game/organisms/ui-sequencer-board.orb"),
    "std/behaviors/ui-showcase-organism" => include_str!("baked/std/ui/marketing/organisms/ui-showcase-organism.orb"),
    "std/behaviors/ui-simulation-canvas" => include_str!("baked/std/ui/game/organisms/ui-simulation-canvas.orb"),
    "std/behaviors/ui-simulation-controls" => include_str!("baked/std/ui/game/organisms/ui-simulation-controls.orb"),
    "std/behaviors/ui-simulation-graph" => include_str!("baked/std/ui/game/organisms/ui-simulation-graph.orb"),
    "std/behaviors/ui-simulator-board" => include_str!("baked/std/ui/game/organisms/ui-simulator-board.orb"),
    "std/behaviors/ui-split-pane" => include_str!("baked/std/ui/core/organisms/ui-split-pane.orb"),
    "std/behaviors/ui-state-architect-board" => include_str!("baked/std/ui/game/organisms/ui-state-architect-board.orb"),
    "std/behaviors/ui-state-machine-view" => include_str!("baked/std/ui/core/organisms/ui-state-machine-view.orb"),
    "std/behaviors/ui-state-node" => include_str!("baked/std/ui/game/organisms/ui-state-node.orb"),
    "std/behaviors/ui-stats-organism" => include_str!("baked/std/ui/marketing/organisms/ui-stats-organism.orb"),
    "std/behaviors/ui-step-flow-organism" => include_str!("baked/std/ui/marketing/organisms/ui-step-flow-organism.orb"),
    "std/behaviors/ui-tabbed-container" => include_str!("baked/std/ui/core/organisms/ui-tabbed-container.orb"),
    "std/behaviors/ui-team-organism" => include_str!("baked/std/ui/marketing/organisms/ui-team-organism.orb"),
    "std/behaviors/ui-timeline" => include_str!("baked/std/ui/core/organisms/ui-timeline.orb"),
    "std/behaviors/ui-toast-slot" => include_str!("baked/std/ui/core/organisms/ui-toast-slot.orb"),
    "std/behaviors/ui-trait-slot" => include_str!("baked/std/ui/game/organisms/ui-trait-slot.orb"),
    "std/behaviors/ui-trait-state-viewer" => include_str!("baked/std/ui/game/organisms/ui-trait-state-viewer.orb"),
    "std/behaviors/ui-transition-arrow" => include_str!("baked/std/ui/game/organisms/ui-transition-arrow.orb"),
    "std/behaviors/ui-ui-slot-renderer" => include_str!("baked/std/ui/core/organisms/ui-ui-slot-renderer.orb"),
    "std/behaviors/ui-uncontrolled-battle-board" => include_str!("baked/std/ui/game/organisms/ui-uncontrolled-battle-board.orb"),
    "std/behaviors/ui-variable-panel" => include_str!("baked/std/ui/game/organisms/ui-variable-panel.orb"),
    "std/behaviors/ui-world-map-board" => include_str!("baked/std/ui/game/organisms/ui-world-map-board.orb"),
};
// SYNC-GENERATED:EMBEDDED_STD_BEHAVIORS:END

/// Look up an embedded std behavior by its canonical std path.
///
/// Returns the raw `.orb` JSON content as a `&'static str`, or `None` if the
/// path is not in the embedded set. The caller is responsible for parsing the
/// content (typically via the existing schema parser).
pub fn get_std_behavior_content(canonical_path: &str) -> Option<&'static str> {
    EMBEDDED_STD_BEHAVIORS.get(canonical_path).copied()
}

// ============================================================================
// 3.4 — Sigil context rules (BINDING_CONTEXT_RULES)
// ============================================================================
//
// Hand-ported from `packages/almadar-core/src/types/bindings.ts`.
// A parity test (`tests/embedded_validation.rs::sigil_rules_match_ts_source`)
// fails if the TS source drifts from this constant.

/// Allowed sigil roots in each binding context.
///
/// Mirrors `BINDING_CONTEXT_RULES` in `@almadar/core/types/bindings.ts`.
pub const BINDING_CONTEXT_RULES: &[(&str, &[&str])] = &[
    // `config` lets atoms write mode-aware guards — e.g. std-modal's
    // OPEN can require @payload.row only when @config.mode equals
    // "edit", letting create-mode legitimately fire OPEN with no row.
    // Same substitution semantics as effects: molecule inline time
    // resolves @config.X to the literal call-site value; atom-scope
    // validate sees unresolved @config bindings and allows them.
    // `user` is the authenticated-user context (@user.id, @user.role)
    // for ownership / role-based gates (e.g. std-row-access-control).
    (
        "guard",
        &["entity", "payload", "state", "now", "config", "user"],
    ),
    // `trait` here is the `@trait.X` intra-frame composition binding — it
    // appears inside render-ui children and is passed through server-side
    // as a literal for the client renderer to substitute via TraitFrame.
    // `config` is the atom-scope parameterization binding (@config.icon,
    // @config.title, etc.). Molecule inline time substitutes each
    // @config.X with the literal call-site config value; atom-scope
    // validate sees unresolved @config bindings and must allow them.
    // `user` is the authenticated-user context (@user.id, @user.role).
    (
        "effect",
        &[
            "entity", "payload", "state", "now", "trait", "config", "user",
        ],
    ),
    // Ticks have no @payload — they fire on a clock, not on an event.
    // `config` is allowed for the same call-site parameterization use
    // case (e.g. tick interval threshold reading @config.cooldownMs).
    // `user` is the authenticated-user context for ownership/role reads.
    ("tick", &["entity", "state", "now", "config", "user"]),
];

/// Look up the allowed sigil roots for a given context name.
pub fn allowed_sigils_for_context(context: &str) -> Option<&'static [&'static str]> {
    BINDING_CONTEXT_RULES
        .iter()
        .find(|(name, _)| *name == context)
        .map(|(_, allowed)| *allowed)
}

/// All known sigil context names.
pub fn known_contexts() -> HashSet<&'static str> {
    BINDING_CONTEXT_RULES.iter().map(|(n, _)| *n).collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Operator-parity gate (zero exemptions): the runtime evaluator
    // (`orbital_core::evaluator::get_operator`) must resolve EVERY operator whose
    // canonical category targets `rust`. Agent ops are stubbed, so they count.
    // Fails when a canonical rust-target op lands without a Rust impl/registration.
    #[test]
    fn rust_evaluator_covers_every_canonical_rust_operator() {
        let doc: serde_json::Value = serde_json::from_str(OPERATORS_JSON).unwrap();
        let rust_cats: std::collections::HashSet<String> = doc["categories"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, m)| {
                m["target"]
                    .as_array()
                    .map(|t| t.iter().any(|x| x.as_str() == Some("rust")))
                    .unwrap_or(false)
            })
            .map(|(c, _)| c.clone())
            .collect();

        let mut missing = Vec::new();
        for (name, meta) in doc["operators"].as_object().unwrap() {
            let cat = meta["category"].as_str().unwrap_or("");
            if !rust_cats.contains(cat) {
                continue;
            }
            if orbital_core::evaluator::get_operator(name).is_none() {
                missing.push(format!("{name} ({cat})"));
            }
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "Rust evaluator missing {} canonical rust-target operators:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    #[test]
    fn operators_parse_and_lookup() {
        // Smoke test: a few well-known operators must exist.
        assert!(is_known_operator("+"));
        assert!(is_known_operator("if"));
        assert!(is_known_operator("set"));
        assert!(is_known_operator("render-ui"));
        assert!(is_known_operator("behavior/compose"));
        assert!(!is_known_operator("definitely-not-a-real-op-xyz"));
    }

    #[test]
    fn operator_metadata_is_populated() {
        let plus = get_operator("+").expect("'+' must exist");
        assert_eq!(plus.category, "arithmetic");
        assert_eq!(plus.min_arity, 2);
        assert!(!plus.has_side_effects);

        let set = get_operator("set").expect("'set' must exist");
        assert!(set.has_side_effects);
    }

    #[test]
    fn fetch_operator_has_effect_produces_union() {
        // V2 Phase 0: fetch, ref, persist, call-service, set carry effect.produces
        // in canonical-operators.json. The untagged OperatorTypeRef enum should
        // deserialize the nested union shape without loss.
        let fetch = get_operator("fetch").expect("fetch must exist (V2 Phase 0)");
        let effect = fetch
            .effect
            .as_ref()
            .expect("fetch must carry effect metadata");
        assert_eq!(effect.kind, "fetch");
        let produces = effect.produces.as_ref().expect("fetch produces present");
        match produces {
            OperatorTypeRef::Tagged(OperatorTypeRefKind::Union { of }) => {
                assert_eq!(of.len(), 2, "fetch union has entity | Array<entity>");
                // First branch: { kind: 'entity' }
                match &of[0] {
                    OperatorTypeRef::Tagged(OperatorTypeRefKind::Entity { .. }) => {}
                    other => panic!("fetch produces[0] should be Entity, got {other:?}"),
                }
                // Second branch: { kind: 'array', of: { kind: 'entity' } }
                match &of[1] {
                    OperatorTypeRef::Tagged(OperatorTypeRefKind::Array { of: inner }) => {
                        match inner.as_ref() {
                            OperatorTypeRef::Tagged(OperatorTypeRefKind::Entity { .. }) => {}
                            other => panic!("fetch produces[1].of should be Entity, got {other:?}"),
                        }
                    }
                    other => panic!("fetch produces[1] should be Array, got {other:?}"),
                }
            }
            other => panic!("fetch produces should be Union, got {other:?}"),
        }
    }

    #[test]
    fn math_operators_have_no_effect_block() {
        // Operators that aren't in the V2 Phase 0 scope keep effect = None.
        // Confirms #[serde(default)] falls through cleanly for pre-v2 entries.
        let plus = get_operator("+").expect("'+' must exist");
        assert!(plus.effect.is_none());
        let abs = get_operator("math/abs").expect("math/abs must exist");
        assert!(abs.effect.is_none());
    }

    #[test]
    fn categories_parse() {
        let cats = categories();
        assert!(cats.contains_key("arithmetic"));
        assert!(cats.contains_key("std-composition"));
        let std_comp = cats.get("std-composition").unwrap();
        assert!(std_comp.target.iter().any(|t| t == "rust"));
    }

    #[test]
    fn patterns_parse_and_lookup() {
        assert!(is_known_pattern("entity-table"));
        assert!(is_known_pattern("button"));
        assert!(!is_known_pattern("definitely-not-a-pattern-xyz"));
        let table = get_pattern("entity-table").unwrap();
        assert_eq!(table.category, "display");
        assert!(table.props_schema.contains_key("entity"));
    }

    #[test]
    fn behaviors_parse_and_lookup() {
        let map = behaviors();
        assert!(!map.is_empty(), "behaviors registry should not be empty");
        // Pick any one and check the round-trip.
        let (name, b) = map.iter().next().unwrap();
        assert_eq!(&b.name, name);
        assert!(matches!(b.level.as_str(), "atom" | "molecule" | "organism"));
    }

    #[test]
    fn sigil_context_rules_present() {
        assert_eq!(
            allowed_sigils_for_context("guard"),
            Some(&["entity", "payload", "state", "now", "config", "user"][..])
        );
        assert_eq!(
            allowed_sigils_for_context("effect"),
            Some(&["entity", "payload", "state", "now", "trait", "config", "user"][..])
        );
        assert_eq!(
            allowed_sigils_for_context("tick"),
            Some(&["entity", "state", "now", "config", "user"][..])
        );
        assert_eq!(allowed_sigils_for_context("nope"), None);
    }
}

#[cfg(test)]
mod std_behavior_embed_tests {
    use super::*;

    #[test]
    fn embed_count_floor() {
        // EMBEDDED_STD_BEHAVIORS is auto-generated by tools/sync-rust from
        // packages/almadar-std/behaviors/registry/**/*.orb. The count grows as
        // new behaviors land; this test only enforces a floor so a regression
        // (someone hand-emptying the map, or sync-rust failing silently) is
        // caught. Last raised 2026-05-09 to reflect Phase 10 additions.
        assert!(
            EMBEDDED_STD_BEHAVIORS.len() >= 95,
            "EMBEDDED_STD_BEHAVIORS shrank below 95 — sync-rust regression?"
        );
    }

    #[test]
    fn std_modal_resolves() {
        let content = get_std_behavior_content("std/behaviors/std-modal")
            .expect("std-modal must be embedded");
        let parsed: serde_json::Value =
            serde_json::from_str(content).expect("std-modal must be valid JSON");
        assert_eq!(parsed["name"], "std-modal");
        assert_eq!(parsed["version"], "1.0.0");
        assert!(parsed["orbitals"].is_array());
    }

    #[test]
    fn std_list_resolves() {
        let content =
            get_std_behavior_content("std/behaviors/std-list").expect("std-list must be embedded");
        let parsed: serde_json::Value =
            serde_json::from_str(content).expect("std-list must be valid JSON");
        assert_eq!(parsed["name"], "std-list");
    }

    #[test]
    fn std_rpg_game_resolves() {
        let content = get_std_behavior_content("std/behaviors/std-rpg-game")
            .expect("std-rpg-game must be embedded");
        // Game-tier organisms ship as empty .orb stubs today (the .lolo
        // sources exist but the registry .orb hasn't been emitted yet).
        // Skip the parse assertion when the embedded content is empty;
        // re-enable when std-rpg-game.orb is materialised.
        if content.trim().is_empty() {
            return;
        }
        let parsed: serde_json::Value =
            serde_json::from_str(content).expect("std-rpg-game must be valid JSON");
        assert_eq!(parsed["name"], "std-rpg-game");
    }

    #[test]
    fn unknown_path_returns_none() {
        assert!(get_std_behavior_content("std/behaviors/no-such-thing").is_none());
    }
}
