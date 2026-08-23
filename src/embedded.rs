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
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

// ============================================================================
// Env-var overrides for baked registries (opt-in; unset = today's behavior)
// ============================================================================
//
// Every baked registry below is embedded at compile time via `include_str!`
// from `packages/almadar-*`'s pinned-npm bake (`make sync`), so a local,
// unpublished edit to a std behavior/pattern/operator is invisible until a
// full re-bake + `cargo build`. This lets a specific env var (or the shared
// `ORBITAL_BAKED_DIR` fallback) point at a live file on disk instead, for
// fast local iteration. Opt-in only: with no env var set this resolves to the
// exact same embedded `&'static str`, byte for byte.

/// Read `specific_env`, or `ORBITAL_BAKED_DIR/<baked_filename>`, or fall back
/// to `embedded`. A set-but-unreadable override path panics with a clear
/// message rather than silently falling through to the embedded source.
fn resolve_baked_source(
    specific_env: &str,
    baked_filename: &str,
    embedded: &'static str,
) -> Cow<'static, str> {
    if let Ok(path) = std::env::var(specific_env) {
        tracing::warn!(env = specific_env, path = %path, "baked registry override active");
        return Cow::Owned(std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("{specific_env} is set to '{path}' but it could not be read: {e}")
        }));
    }
    if let Ok(dir) = std::env::var("ORBITAL_BAKED_DIR") {
        let path = format!("{dir}/{baked_filename}");
        tracing::warn!(env = "ORBITAL_BAKED_DIR", path = %path, "baked registry override active");
        return Cow::Owned(std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("ORBITAL_BAKED_DIR is set to '{dir}' but '{path}' could not be read: {e}")
        }));
    }
    Cow::Borrowed(embedded)
}

// ============================================================================
// Gzipped baked corpus (build.rs → $OUT_DIR/baked-gz/…)
// ============================================================================
//
// Every baked JSON/`.orb` embeds as gzip bytes and inflates on first use —
// the accessors below keep their historical `&'static str` signatures, so
// callers (and the env-var overrides) are unchanged. Inflation is pure-Rust
// (miniz_oxide), wasm32-safe.

/// Inflate a gzipped baked file. Panics on corruption — the bytes are
/// compile-time constants from our own build script, so a failure is a build
/// bug, not a runtime condition.
fn gunzip(gz: &'static [u8]) -> String {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut out = String::new();
    GzDecoder::new(gz)
        .read_to_string(&mut out)
        .expect("baked gzip payload is corrupt — rebuild orbital-compiler");
    out
}

/// Lazily inflate `gz` once into `slot` and return it as `&'static str`.
fn baked_str(slot: &'static OnceLock<String>, gz: &'static [u8]) -> &'static str {
    slot.get_or_init(|| gunzip(gz)).as_str()
}

// ============================================================================
// 3.1 — Embedded operators.json
// ============================================================================

const OPERATORS_JSON_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/canonical-operators.json.gz"
));

/// The raw operators.json text, inflated once on first use.
fn operators_json() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, OPERATORS_JSON_GZ)
}

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
    /// 0-indexed argument position of the operator's lambda/callback, when it
    /// takes one. The sexpr validator rejects a `(fn …)` form at any OTHER
    /// argument position — the decidable wrong-arg-order class
    /// (e.g. `(array/reduce arr (fn …) init)`).
    #[serde(rename = "lambdaArgPosition", default)]
    pub lambda_arg_position: Option<u32>,
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
        let source = resolve_baked_source(
            "ORBITAL_OPERATORS_JSON",
            "canonical-operators.json",
            operators_json(),
        );
        serde_json::from_str(&source).expect("Failed to parse embedded operators.json")
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
    is_shell_target(name, "rust")
}

/// Returns true if the operator's canonical category targets Python.
pub fn is_python_target(name: &str) -> bool {
    is_shell_target(name, "python")
}

/// Returns true if the operator's canonical category lists `shell` in its
/// `target` set. The single lookup behind the per-shell helpers so the
/// operator→category→target resolution can never drift between shells.
pub fn is_shell_target(name: &str, shell: &str) -> bool {
    if let Some(op) = get_operator(name) {
        if let Some(cat) = categories().get(&op.category) {
            return cat.target.iter().any(|t| t == shell);
        }
    }
    false
}

// ============================================================================
// 3.2 — Embedded patterns-registry.json
// ============================================================================

const PATTERNS_JSON_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/patterns-registry.json.gz"
));

/// The raw patterns-registry.json text, inflated once on first use.
fn patterns_json() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, PATTERNS_JSON_GZ)
}

// ============================================================================
// 3.1b — Embedded themes-registry.json (built-in @almadar-ui/themes/ names)
// ============================================================================

const THEMES_JSON_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/themes-registry.json.gz"
));

/// The raw themes-registry.json text, inflated once on first use.
fn themes_json() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, THEMES_JSON_GZ)
}

/// The built-in theme names harvested from `@almadar-ui/themes/*.css`.
/// These (plus any orbital-declared inline theme names) form the valid-theme
/// set the theme-reference validator accepts.
pub fn builtin_theme_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        #[derive(Deserialize)]
        struct ThemesRegistry {
            #[serde(default)]
            themes: Vec<String>,
        }
        let registry: ThemesRegistry = serde_json::from_str(themes_json())
            .expect("baked themes-registry.json is corrupt — rebuild orbital-compiler");
        registry.themes
    })
}

// ============================================================================
// 3.1c — Embedded services-registry.json (call-service integrator contracts)
// ============================================================================

const SERVICES_JSON_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/services-registry.json.gz"
));

/// The raw services-registry.json text, inflated once on first use.
fn services_json() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, SERVICES_JSON_GZ)
}

/// Service name → declared action names, from the baked services registry.
/// A declared-only service (host-injected handler, no static contract) is
/// present with an EMPTY set — its name is known, its actions are unchecked.
///
/// Unlike the behavior registries above, the CLI prefers a live CWD registry
/// over this baked copy (`orbital-cli/src/utils/patterns.rs`), so in-repo
/// registry edits are picked up with no rebuild; this is the out-of-repo
/// fallback (still overridable via `ORBITAL_SERVICES_JSON`/`ORBITAL_BAKED_DIR`).
pub fn baked_service_actions() -> &'static HashMap<String, HashSet<String>> {
    &baked_service_registry().actions
}

/// One declared parameter of a service action, from the services registry.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedServiceParam {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
}

/// The validation projections of the services registry: declared action names
/// plus per-action param contracts. One parse, one source — the CLI's live
/// CWD-registry load and this baked fallback both produce it via
/// [`parse_service_registry_data`], so the action gate and the param checks
/// (I-13) can never draw from two different registry generations.
#[derive(Debug, Clone, Default)]
pub struct ServiceRegistryData {
    /// Service name → declared action names. A declared-only service
    /// (host-injected handler, no static contract) carries an EMPTY set —
    /// its name is known, its actions are unchecked.
    pub actions: HashMap<String, HashSet<String>>,
    /// Service name → action name → declared params.
    pub params: HashMap<String, HashMap<String, Vec<EmbeddedServiceParam>>>,
}

/// Parse raw services-registry.json text into both validation projections.
pub fn parse_service_registry_data(raw: &str) -> Result<ServiceRegistryData, serde_json::Error> {
    #[derive(Deserialize)]
    struct ActionEntry {
        name: String,
        #[serde(default)]
        params: Vec<EmbeddedServiceParam>,
    }
    #[derive(Deserialize)]
    struct ServiceEntry {
        #[serde(default)]
        actions: Vec<ActionEntry>,
    }
    #[derive(Deserialize)]
    struct ServicesRegistry {
        #[serde(default)]
        services: HashMap<String, ServiceEntry>,
    }
    let registry: ServicesRegistry = serde_json::from_str(raw)?;
    let mut data = ServiceRegistryData::default();
    for (name, entry) in registry.services {
        let mut actions = HashSet::new();
        let mut params = HashMap::new();
        for a in entry.actions {
            actions.insert(a.name.clone());
            params.insert(a.name, a.params);
        }
        data.actions.insert(name.clone(), actions);
        data.params.insert(name, params);
    }
    Ok(data)
}

/// The baked services registry, parsed once (overridable via
/// `ORBITAL_SERVICES_JSON`/`ORBITAL_BAKED_DIR` like every baked source).
pub fn baked_service_registry() -> &'static ServiceRegistryData {
    static DATA: OnceLock<ServiceRegistryData> = OnceLock::new();
    DATA.get_or_init(|| {
        let raw = resolve_baked_source(
            "ORBITAL_SERVICES_JSON",
            "services-registry.json",
            services_json(),
        );
        parse_service_registry_data(&raw)
            .expect("baked services-registry.json is corrupt — rebuild orbital-compiler")
    })
}

/// One credential env var a service declares in the services registry.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedServiceCredential {
    #[serde(rename = "envVar")]
    pub env_var: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

/// Service name → declared credential env vars, from the baked services
/// registry. Drives the generated `.env.example` / `SECRETS.md` manifest for
/// the services a schema actually invokes.
pub fn baked_service_credentials() -> &'static HashMap<String, Vec<EmbeddedServiceCredential>> {
    static MAP: OnceLock<HashMap<String, Vec<EmbeddedServiceCredential>>> = OnceLock::new();
    MAP.get_or_init(|| {
        #[derive(Deserialize)]
        struct ServiceEntry {
            #[serde(default)]
            credentials: Vec<EmbeddedServiceCredential>,
        }
        #[derive(Deserialize)]
        struct ServicesRegistry {
            #[serde(default)]
            services: HashMap<String, ServiceEntry>,
        }
        let raw = resolve_baked_source(
            "ORBITAL_SERVICES_JSON",
            "services-registry.json",
            services_json(),
        );
        let registry: ServicesRegistry = serde_json::from_str(&raw)
            .expect("baked services-registry.json is corrupt — rebuild orbital-compiler");
        registry
            .services
            .into_iter()
            .map(|(name, entry)| (name, entry.credentials))
            .collect()
    })
}

/// One inbound-hook declaration a service carries in the services registry:
/// a provider mount (`POST /api/hooks/:provider`) that verifies with
/// `credential_env` and dispatches `event` onto the app bus. Codegen derives
/// per-app provider mounts from invoked services × these entries (I-26).
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedServiceHook {
    pub provider: String,
    pub event: String,
    #[serde(rename = "credentialEnv")]
    pub credential_env: String,
    #[serde(rename = "providerExport")]
    pub provider_export: String,
}

/// Service name → declared inbound hooks, from the baked services registry.
pub fn baked_service_hooks() -> &'static HashMap<String, Vec<EmbeddedServiceHook>> {
    static MAP: OnceLock<HashMap<String, Vec<EmbeddedServiceHook>>> = OnceLock::new();
    MAP.get_or_init(|| {
        #[derive(Deserialize)]
        struct ServiceEntry {
            #[serde(default)]
            hooks: Vec<EmbeddedServiceHook>,
        }
        #[derive(Deserialize)]
        struct ServicesRegistry {
            #[serde(default)]
            services: HashMap<String, ServiceEntry>,
        }
        let raw = resolve_baked_source(
            "ORBITAL_SERVICES_JSON",
            "services-registry.json",
            services_json(),
        );
        let registry: ServicesRegistry = serde_json::from_str(&raw)
            .expect("baked services-registry.json is corrupt — rebuild orbital-compiler");
        registry
            .services
            .into_iter()
            .map(|(name, entry)| (name, entry.hooks))
            .collect()
    })
}

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
    /// Nested structural shape for an object-typed arg (e.g. a canvas
    /// `onShapeClick` payload `{ id?, type?, index }`). The pattern-sync
    /// parser has always written this into `patterns-registry.json`; this
    /// struct used to drop it, forcing listen contracts down to a shapeless
    /// `payload : object`. Reuses `EmbeddedPatternProp` — the same
    /// `types`/`properties`/`required` nesting the JSON carries.
    #[serde(default)]
    pub schema: Option<Box<EmbeddedPatternProp>>,
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
    /// Bounded string-literal union members (e.g. `VisualStyle`, button
    /// `variant`), present at any nesting level — top prop OR a nested
    /// `properties`/`items` field. Codegen consults this to drop an empty
    /// enum sub-field instead of emitting an invalid `""` (CG-SWEEP-1 Cat 11).
    #[serde(rename = "enumValues", default)]
    pub enum_values: Option<Vec<String>>,
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
        let source = resolve_baked_source(
            "ORBITAL_PATTERNS_JSON",
            "patterns-registry.json",
            patterns_json(),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&source).expect("Failed to parse embedded patterns-registry.json");
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

const BEHAVIORS_JSON_STD_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/behaviors-registry-std.json.gz"
));

const BEHAVIORS_JSON_IO_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/behaviors-registry-io.json.gz"
));

/// The raw behaviors-registry-std.json text, inflated once on first use.
fn behaviors_json_std() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, BEHAVIORS_JSON_STD_GZ)
}

/// The raw behaviors-registry-io.json text, inflated once on first use.
fn behaviors_json_io() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, BEHAVIORS_JSON_IO_GZ)
}

/// A single behavior entry from `behaviors-registry.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedBehavior {
    pub name: String,
    /// Composition level: "atom" | "molecule" | "organism".
    #[serde(default)]
    pub level: String,
    /// Topic within the package (e.g. "agent", "ui", "rpg") — the browser
    /// catalog groups by `level / topic`; the native catalog mirrors it.
    #[serde(default)]
    pub topic: Option<String>,
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
        let std_source = resolve_baked_source(
            "ORBITAL_BEHAVIORS_STD_JSON",
            "behaviors-registry-std.json",
            behaviors_json_std(),
        );
        let io_source = resolve_baked_source(
            "ORBITAL_BEHAVIORS_IO_JSON",
            "behaviors-registry-io.json",
            behaviors_json_io(),
        );
        parse_behaviors_into(&mut map, &std_source, "std");
        parse_behaviors_into(&mut map, &io_source, "io");
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
// Embedded gzipped (build.rs → `$OUT_DIR/baked-gz/…`) and inflated lazily, so
// the orbital binary doesn't need filesystem access to load signatures — and
// doesn't carry ~100MB of raw JSON in its data section.

const FACTORY_SIGNATURES_JSON_STD_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/factory-signatures-std.json.gz"
));

const FACTORY_SIGNATURES_JSON_IO_GZ: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/baked-gz/factory-signatures-io.json.gz"
));

/// Raw JSON content of the std factory-signature catalog (core + agent
/// topics). Consumers needing the full surface must also read
/// `factory_signatures_json_io()`.
pub fn factory_signatures_json() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, FACTORY_SIGNATURES_JSON_STD_GZ)
}

/// Raw JSON content of the io factory-signature catalog (app/game/marketing/
/// probes/service/variations topics).
pub fn factory_signatures_json_io() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    baked_str(&S, FACTORY_SIGNATURES_JSON_IO_GZ)
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
// Values are gzip bytes (build.rs → $OUT_DIR/baked-gz/…), inflated lazily by
// `get_std_behavior_content` — the baked corpus is ~31MB raw, ~2MB gzipped.
pub static EMBEDDED_STD_BEHAVIORS: phf::Map<&'static str, &'static [u8]> = phf::phf_map! {
    // === Atoms (596) ===
    "std/behaviors/dnd-combat" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/atoms/dnd-combat.orb.gz")),
    "std/behaviors/dnd-dice" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/atoms/dnd-dice.orb.gz")),
    "std/behaviors/dnd-dungeon-gen" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/atoms/dnd-dungeon-gen.orb.gz")),
    "std/behaviors/dnd-hero" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/atoms/dnd-hero.orb.gz")),
    "std/behaviors/dnd-narrator" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/atoms/dnd-narrator.orb.gz")),
    "std/behaviors/dnd-token-art" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/atoms/dnd-token-art.orb.gz")),
    "std/behaviors/mesh-asset-catalog" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-asset-catalog.orb.gz")),
    "std/behaviors/mesh-material-crystal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-material-crystal.orb.gz")),
    "std/behaviors/mesh-material-magic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-material-magic.orb.gz")),
    "std/behaviors/mesh-material-metal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-material-metal.orb.gz")),
    "std/behaviors/mesh-palette" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-palette.orb.gz")),
    "std/behaviors/mesh-part-gear" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-part-gear.orb.gz")),
    "std/behaviors/mesh-part-head-banded" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-part-head-banded.orb.gz")),
    "std/behaviors/mesh-part-limb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-part-limb.orb.gz")),
    "std/behaviors/mesh-part-torso-suit" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-part-torso-suit.orb.gz")),
    "std/behaviors/mesh-part-unit-base" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-part-unit-base.orb.gz")),
    "std/behaviors/mesh-primitives" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/atoms/mesh-primitives.orb.gz")),
    "std/behaviors/riya-level-accum" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-accum.orb.gz")),
    "std/behaviors/riya-level-area" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-area.orb.gz")),
    "std/behaviors/riya-level-contour" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-contour.orb.gz")),
    "std/behaviors/riya-level-corner" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-corner.orb.gz")),
    "std/behaviors/riya-level-cubic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-cubic.orb.gz")),
    "std/behaviors/riya-level-decay" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-decay.orb.gz")),
    "std/behaviors/riya-level-deriv" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-deriv.orb.gz")),
    "std/behaviors/riya-level-doubling" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-doubling.orb.gz")),
    "std/behaviors/riya-level-gallery" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-gallery.orb.gz")),
    "std/behaviors/riya-level-gradient" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-gradient.orb.gz")),
    "std/behaviors/riya-level-log" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-log.orb.gz")),
    "std/behaviors/riya-level-mountain" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-mountain.orb.gz")),
    "std/behaviors/riya-level-partial" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-partial.orb.gz")),
    "std/behaviors/riya-level-plain" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-plain.orb.gz")),
    "std/behaviors/riya-level-pole" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-pole.orb.gz")),
    "std/behaviors/riya-level-ramp" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-ramp.orb.gz")),
    "std/behaviors/riya-level-saddle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-saddle.orb.gz")),
    "std/behaviors/riya-level-series" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-series.orb.gz")),
    "std/behaviors/riya-level-shifts" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-shifts.orb.gz")),
    "std/behaviors/riya-level-tail" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-tail.orb.gz")),
    "std/behaviors/riya-level-taylor" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-taylor.orb.gz")),
    "std/behaviors/riya-level-valley" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-valley.orb.gz")),
    "std/behaviors/riya-level-vector" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-vector.orb.gz")),
    "std/behaviors/riya-level-volume" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/atoms/riya-level-volume.orb.gz")),
    "std/behaviors/rpg-dialogue" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/atoms/rpg-dialogue.orb.gz")),
    "std/behaviors/rpg-enemies" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/atoms/rpg-enemies.orb.gz")),
    "std/behaviors/rpg-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/atoms/rpg-flow.orb.gz")),
    "std/behaviors/rpg-fx" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/atoms/rpg-fx.orb.gz")),
    "std/behaviors/rpg-hero" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/atoms/rpg-hero.orb.gz")),
    "std/behaviors/rpg-inventory" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/atoms/rpg-inventory.orb.gz")),
    "std/behaviors/rpg-quests" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/atoms/rpg-quests.orb.gz")),
    "std/behaviors/std-agent-completion" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-completion.orb.gz")),
    "std/behaviors/std-agent-context" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-context.orb.gz")),
    "std/behaviors/std-agent-fix-loop" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-fix-loop.orb.gz")),
    "std/behaviors/std-agent-memory" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-memory.orb.gz")),
    "std/behaviors/std-agent-planner" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-planner.orb.gz")),
    "std/behaviors/std-agent-session" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-session.orb.gz")),
    "std/behaviors/std-agent-tool-call" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-tool-call.orb.gz")),
    "std/behaviors/std-agent-tool-loop" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-tool-loop.orb.gz")),
    "std/behaviors/std-agent-trace" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/atoms/std-agent-trace.orb.gz")),
    "std/behaviors/std-algo-astargraph" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-astargraph.orb.gz")),
    "std/behaviors/std-algo-astargrid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-astargrid.orb.gz")),
    "std/behaviors/std-algo-bfs" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-bfs.orb.gz")),
    "std/behaviors/std-algo-binarysearch" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-binarysearch.orb.gz")),
    "std/behaviors/std-algo-bst" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-bst.orb.gz")),
    "std/behaviors/std-algo-bubblesort" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-bubblesort.orb.gz")),
    "std/behaviors/std-algo-dfs" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-dfs.orb.gz")),
    "std/behaviors/std-algo-dijkstra" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-dijkstra.orb.gz")),
    "std/behaviors/std-algo-dp" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-dp.orb.gz")),
    "std/behaviors/std-algo-floodfill" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-floodfill.orb.gz")),
    "std/behaviors/std-algo-gameoflife" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-gameoflife.orb.gz")),
    "std/behaviors/std-algo-gridbfs" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-gridbfs.orb.gz")),
    "std/behaviors/std-algo-hashing" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-hashing.orb.gz")),
    "std/behaviors/std-algo-heapsort" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-heapsort.orb.gz")),
    "std/behaviors/std-algo-insertionsort" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-insertionsort.orb.gz")),
    "std/behaviors/std-algo-mazegen" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-mazegen.orb.gz")),
    "std/behaviors/std-algo-mergesort" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-mergesort.orb.gz")),
    "std/behaviors/std-algo-mst" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-mst.orb.gz")),
    "std/behaviors/std-algo-queue" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-queue.orb.gz")),
    "std/behaviors/std-algo-quicksort" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-quicksort.orb.gz")),
    "std/behaviors/std-algo-recursion" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-recursion.orb.gz")),
    "std/behaviors/std-algo-selectionsort" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-selectionsort.orb.gz")),
    "std/behaviors/std-algo-slidingwindow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-slidingwindow.orb.gz")),
    "std/behaviors/std-algo-stack" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-stack.orb.gz")),
    "std/behaviors/std-algo-toposort" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-toposort.orb.gz")),
    "std/behaviors/std-algo-twopointer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-algo-twopointer.orb.gz")),
    "std/behaviors/std-anim-tick" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-anim-tick.orb.gz")),
    "std/behaviors/std-ap-bill" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-ap-bill.orb.gz")),
    "std/behaviors/std-app-layout" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-app-layout.orb.gz")),
    "std/behaviors/std-app-search" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-app-search.orb.gz")),
    "std/behaviors/std-applicant" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-applicant.orb.gz")),
    "std/behaviors/std-applicant-form" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-applicant-form.orb.gz")),
    "std/behaviors/std-application" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-application.orb.gz")),
    "std/behaviors/std-appointment-policy" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-appointment-policy.orb.gz")),
    "std/behaviors/std-approval-chain" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-approval-chain.orb.gz")),
    "std/behaviors/std-approval-gate" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-approval-gate.orb.gz")),
    "std/behaviors/std-approval-request" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-approval-request.orb.gz")),
    "std/behaviors/std-arcade-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-arcade-flow.orb.gz")),
    "std/behaviors/std-arcade-play" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-arcade-play.orb.gz")),
    "std/behaviors/std-astro-orbit" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-astro-orbit.orb.gz")),
    "std/behaviors/std-astro-orbit-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-astro-orbit-3d.orb.gz")),
    "std/behaviors/std-astro-stellar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-astro-stellar.orb.gz")),
    "std/behaviors/std-audit-capture" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-audit-capture.orb.gz")),
    "std/behaviors/std-billable-hour" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-billable-hour.orb.gz")),
    "std/behaviors/std-bio-cell" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-cell.orb.gz")),
    "std/behaviors/std-bio-cell-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-cell-3d.orb.gz")),
    "std/behaviors/std-bio-dna-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-dna-3d.orb.gz")),
    "std/behaviors/std-bio-dna-replication" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-dna-replication.orb.gz")),
    "std/behaviors/std-bio-foodweb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-foodweb.orb.gz")),
    "std/behaviors/std-bio-mitosis" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-mitosis.orb.gz")),
    "std/behaviors/std-bio-network" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-network.orb.gz")),
    "std/behaviors/std-bio-phasecycle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-phasecycle.orb.gz")),
    "std/behaviors/std-bio-population" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-population.orb.gz")),
    "std/behaviors/std-bio-processrate" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-bio-processrate.orb.gz")),
    "std/behaviors/std-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-board.orb.gz")),
    "std/behaviors/std-branching-logic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-branching-logic.orb.gz")),
    "std/behaviors/std-brick-breaker" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/atoms/std-brick-breaker.orb.gz")),
    "std/behaviors/std-broadcast-builder" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-broadcast-builder.orb.gz")),
    "std/behaviors/std-browse" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-browse.orb.gz")),
    "std/behaviors/std-cache-aside" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-cache-aside.orb.gz")),
    "std/behaviors/std-calendar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-calendar.orb.gz")),
    "std/behaviors/std-calendar-sync" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-calendar-sync.orb.gz")),
    "std/behaviors/std-campaign-goal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-campaign-goal.orb.gz")),
    "std/behaviors/std-cascade-on-delete" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-cascade-on-delete.orb.gz")),
    "std/behaviors/std-change-order" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-change-order.orb.gz")),
    "std/behaviors/std-chart-of-accounts" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-chart-of-accounts.orb.gz")),
    "std/behaviors/std-chase-ai" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-chase-ai.orb.gz")),
    "std/behaviors/std-checkin" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-checkin.orb.gz")),
    "std/behaviors/std-checkout-terminal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-checkout-terminal.orb.gz")),
    "std/behaviors/std-chem-crystal-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-crystal-3d.orb.gz")),
    "std/behaviors/std-chem-diffusion" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-diffusion.orb.gz")),
    "std/behaviors/std-chem-molecule" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-molecule.orb.gz")),
    "std/behaviors/std-chem-molecule-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-molecule-3d.orb.gz")),
    "std/behaviors/std-chem-osmosis" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-osmosis.orb.gz")),
    "std/behaviors/std-chem-reaction" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-reaction.orb.gz")),
    "std/behaviors/std-chem-reaction-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-reaction-3d.orb.gz")),
    "std/behaviors/std-chem-titration" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-chem-titration.orb.gz")),
    "std/behaviors/std-circuit-breaker" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-circuit-breaker.orb.gz")),
    "std/behaviors/std-civics-billprocess" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-civics-billprocess.orb.gz")),
    "std/behaviors/std-civics-checks" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-civics-checks.orb.gz")),
    "std/behaviors/std-civics-electoral" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-civics-electoral.orb.gz")),
    "std/behaviors/std-civics-gerrymander" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-civics-gerrymander.orb.gz")),
    "std/behaviors/std-class-roster" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-class-roster.orb.gz")),
    "std/behaviors/std-class-session" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-class-session.orb.gz")),
    "std/behaviors/std-confirmation" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-confirmation.orb.gz")),
    "std/behaviors/std-conversation-policy" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-conversation-policy.orb.gz")),
    "std/behaviors/std-cooldown" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-cooldown.orb.gz")),
    "std/behaviors/std-court-deadline" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-court-deadline.orb.gz")),
    "std/behaviors/std-cross-reference" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-cross-reference.orb.gz")),
    "std/behaviors/std-customer-account" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-customer-account.orb.gz")),
    "std/behaviors/std-dashboard-grid-embedded" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-dashboard-grid-embedded.orb.gz")),
    "std/behaviors/std-data-collector" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-data-collector.orb.gz")),
    "std/behaviors/std-data-erasure" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-data-erasure.orb.gz")),
    "std/behaviors/std-date-range" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-date-range.orb.gz")),
    "std/behaviors/std-detail-layout" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-detail-layout.orb.gz")),
    "std/behaviors/std-device-fleet" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-device-fleet.orb.gz")),
    "std/behaviors/std-document" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-document.orb.gz")),
    "std/behaviors/std-donation-receipt" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-donation-receipt.orb.gz")),
    "std/behaviors/std-donor" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-donor.orb.gz")),
    "std/behaviors/std-drip-sequence" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-drip-sequence.orb.gz")),
    "std/behaviors/std-driver" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-driver.orb.gz")),
    "std/behaviors/std-dungeon-combat" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-dungeon-combat.orb.gz")),
    "std/behaviors/std-dunning" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-dunning.orb.gz")),
    "std/behaviors/std-earth-plates" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-earth-plates.orb.gz")),
    "std/behaviors/std-earth-plates-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-earth-plates-3d.orb.gz")),
    "std/behaviors/std-earth-watercycle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-earth-watercycle.orb.gz")),
    "std/behaviors/std-econ-elasticity" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-econ-elasticity.orb.gz")),
    "std/behaviors/std-econ-gdp" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-econ-gdp.orb.gz")),
    "std/behaviors/std-econ-market" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-econ-market.orb.gz")),
    "std/behaviors/std-econ-monopoly" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-econ-monopoly.orb.gz")),
    "std/behaviors/std-econ-phillips" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-econ-phillips.orb.gz")),
    "std/behaviors/std-econ-ppf" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-econ-ppf.orb.gz")),
    "std/behaviors/std-email-broadcast" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-email-broadcast.orb.gz")),
    "std/behaviors/std-embedded-dashboard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-embedded-dashboard.orb.gz")),
    "std/behaviors/std-esign-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-esign-flow.orb.gz")),
    "std/behaviors/std-esign-request" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-esign-request.orb.gz")),
    "std/behaviors/std-evade-chase" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/atoms/std-evade-chase.orb.gz")),
    "std/behaviors/std-event" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-event.orb.gz")),
    "std/behaviors/std-event-log" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-event-log.orb.gz")),
    "std/behaviors/std-export" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-export.orb.gz")),
    "std/behaviors/std-file-store" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-file-store.orb.gz")),
    "std/behaviors/std-filter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-filter.orb.gz")),
    "std/behaviors/std-form-advanced" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-form-advanced.orb.gz")),
    "std/behaviors/std-form-builder" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-form-builder.orb.gz")),
    "std/behaviors/std-fx-particles" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-fx-particles.orb.gz")),
    "std/behaviors/std-fx-trail" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-fx-trail.orb.gz")),
    "std/behaviors/std-gallery" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-gallery.orb.gz")),
    "std/behaviors/std-game-clock" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-game-clock.orb.gz")),
    "std/behaviors/std-gate-lock" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-gate-lock.orb.gz")),
    "std/behaviors/std-gather" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-gather.orb.gz")),
    "std/behaviors/std-geo-dtm" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-geo-dtm.orb.gz")),
    "std/behaviors/std-geo-dtm-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-geo-dtm-3d.orb.gz")),
    "std/behaviors/std-geo-koppen" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-geo-koppen.orb.gz")),
    "std/behaviors/std-geo-population" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-geo-population.orb.gz")),
    "std/behaviors/std-geo-urban" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-geo-urban.orb.gz")),
    "std/behaviors/std-geo-vonthunen" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-geo-vonthunen.orb.gz")),
    "std/behaviors/std-geosearch" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-geosearch.orb.gz")),
    "std/behaviors/std-gold" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-gold.orb.gz")),
    "std/behaviors/std-graph-builder" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-graph-builder.orb.gz")),
    "std/behaviors/std-graphs" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-graphs.orb.gz")),
    "std/behaviors/std-gravity-flap" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/atoms/std-gravity-flap.orb.gz")),
    "std/behaviors/std-grid-tactics" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-grid-tactics.orb.gz")),
    "std/behaviors/std-health-score" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-health-score.orb.gz")),
    "std/behaviors/std-help-article" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-help-article.orb.gz")),
    "std/behaviors/std-help-category" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-help-category.orb.gz")),
    "std/behaviors/std-helpdesk-ticket" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-helpdesk-ticket.orb.gz")),
    "std/behaviors/std-hero-nav" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-hero-nav.orb.gz")),
    "std/behaviors/std-hist-causalchain" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-hist-causalchain.orb.gz")),
    "std/behaviors/std-hist-ccot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-hist-ccot.orb.gz")),
    "std/behaviors/std-hist-comparison" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-hist-comparison.orb.gz")),
    "std/behaviors/std-hist-periodization" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-hist-periodization.orb.gz")),
    "std/behaviors/std-hist-sourcing" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-hist-sourcing.orb.gz")),
    "std/behaviors/std-image-upload-multi" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-image-upload-multi.orb.gz")),
    "std/behaviors/std-import" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-import.orb.gz")),
    "std/behaviors/std-incident" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-incident.orb.gz")),
    "std/behaviors/std-insurance-claim" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-insurance-claim.orb.gz")),
    "std/behaviors/std-integration-settings" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-integration-settings.orb.gz")),
    "std/behaviors/std-interview-slot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-interview-slot.orb.gz")),
    "std/behaviors/std-invoice" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-invoice.orb.gz")),
    "std/behaviors/std-kitchen-display" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-kitchen-display.orb.gz")),
    "std/behaviors/std-lane-hazard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/atoms/std-lane-hazard.orb.gz")),
    "std/behaviors/std-learn-arrangement" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-arrangement.orb.gz")),
    "std/behaviors/std-learn-classify" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-classify.orb.gz")),
    "std/behaviors/std-learn-compare" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-compare.orb.gz")),
    "std/behaviors/std-learn-coupled-sim" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-coupled-sim.orb.gz")),
    "std/behaviors/std-learn-derivation" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-derivation.orb.gz")),
    "std/behaviors/std-learn-diagram" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-diagram.orb.gz")),
    "std/behaviors/std-learn-doser" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-doser.orb.gz")),
    "std/behaviors/std-learn-estimate" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-estimate.orb.gz")),
    "std/behaviors/std-learn-fx-cues" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-fx-cues.orb.gz")),
    "std/behaviors/std-learn-goal-seek" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-goal-seek.orb.gz")),
    "std/behaviors/std-learn-grid-sim" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-grid-sim.orb.gz")),
    "std/behaviors/std-learn-hotspot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-hotspot.orb.gz")),
    "std/behaviors/std-learn-live-sim" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-live-sim.orb.gz")),
    "std/behaviors/std-learn-measure" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-measure.orb.gz")),
    "std/behaviors/std-learn-param-explorer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-param-explorer.orb.gz")),
    "std/behaviors/std-learn-perturb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-perturb.orb.gz")),
    "std/behaviors/std-learn-predict" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-predict.orb.gz")),
    "std/behaviors/std-learn-sample-accumulator" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-sample-accumulator.orb.gz")),
    "std/behaviors/std-learn-sandbox" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-sandbox.orb.gz")),
    "std/behaviors/std-learn-scatter-fit" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-scatter-fit.orb.gz")),
    "std/behaviors/std-learn-scenario" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-scenario.orb.gz")),
    "std/behaviors/std-learn-sim-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-sim-3d.orb.gz")),
    "std/behaviors/std-learn-slope-probe" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-slope-probe.orb.gz")),
    "std/behaviors/std-learn-speed-drill" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-speed-drill.orb.gz")),
    "std/behaviors/std-learn-stepper" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-stepper.orb.gz")),
    "std/behaviors/std-learn-tour-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-tour-3d.orb.gz")),
    "std/behaviors/std-learn-trace-player" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-trace-player.orb.gz")),
    "std/behaviors/std-learn-transport" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-transport.orb.gz")),
    "std/behaviors/std-learn-tree" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/std-learn-tree.orb.gz")),
    "std/behaviors/std-lease" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-lease.orb.gz")),
    "std/behaviors/std-ledger-entry" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-ledger-entry.orb.gz")),
    "std/behaviors/std-ledger-entry-form" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-ledger-entry-form.orb.gz")),
    "std/behaviors/std-legal-hold" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-legal-hold.orb.gz")),
    "std/behaviors/std-lifecycle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-lifecycle.orb.gz")),
    "std/behaviors/std-ling-ipa" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-ling-ipa.orb.gz")),
    "std/behaviors/std-ling-morphology" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-ling-morphology.orb.gz")),
    "std/behaviors/std-ling-phonology" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-ling-phonology.orb.gz")),
    "std/behaviors/std-ling-soundchange" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-ling-soundchange.orb.gz")),
    "std/behaviors/std-ling-syntaxtree" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-ling-syntaxtree.orb.gz")),
    "std/behaviors/std-ling-vowels" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-ling-vowels.orb.gz")),
    "std/behaviors/std-list" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-list.orb.gz")),
    "std/behaviors/std-lives" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-lives.orb.gz")),
    "std/behaviors/std-logic-fallacy" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-logic-fallacy.orb.gz")),
    "std/behaviors/std-logic-gates" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-logic-gates.orb.gz")),
    "std/behaviors/std-logic-inference" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-logic-inference.orb.gz")),
    "std/behaviors/std-logic-quantifier" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-logic-quantifier.orb.gz")),
    "std/behaviors/std-logic-truthtable" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-logic-truthtable.orb.gz")),
    "std/behaviors/std-logic-venn" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-logic-venn.orb.gz")),
    "std/behaviors/std-maintenance-request" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-maintenance-request.orb.gz")),
    "std/behaviors/std-margin-account" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-margin-account.orb.gz")),
    "std/behaviors/std-market-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-market-flow.orb.gz")),
    "std/behaviors/std-marketing-article" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-article.orb.gz")),
    "std/behaviors/std-marketing-cta" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-cta.orb.gz")),
    "std/behaviors/std-marketing-features" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-features.orb.gz")),
    "std/behaviors/std-marketing-footer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-footer.orb.gz")),
    "std/behaviors/std-marketing-hero" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-hero.orb.gz")),
    "std/behaviors/std-marketing-nav" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-nav.orb.gz")),
    "std/behaviors/std-marketing-pricing" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-pricing.orb.gz")),
    "std/behaviors/std-marketing-showcase" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-showcase.orb.gz")),
    "std/behaviors/std-marketing-split" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-split.orb.gz")),
    "std/behaviors/std-marketing-stats" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-stats.orb.gz")),
    "std/behaviors/std-marketing-steps" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-steps.orb.gz")),
    "std/behaviors/std-marketing-team" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/std-marketing-team.orb.gz")),
    "std/behaviors/std-math-derivative" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-derivative.orb.gz")),
    "std/behaviors/std-math-integral" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-integral.orb.gz")),
    "std/behaviors/std-math-linear" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-linear.orb.gz")),
    "std/behaviors/std-math-matrix" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-matrix.orb.gz")),
    "std/behaviors/std-math-numberline" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-numberline.orb.gz")),
    "std/behaviors/std-math-quadratic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-quadratic.orb.gz")),
    "std/behaviors/std-math-sequence" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-sequence.orb.gz")),
    "std/behaviors/std-math-sinewave" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-sinewave.orb.gz")),
    "std/behaviors/std-math-unitcircle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-unitcircle.orb.gz")),
    "std/behaviors/std-math-vectors" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-vectors.orb.gz")),
    "std/behaviors/std-math-vectors-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-math-vectors-3d.orb.gz")),
    "std/behaviors/std-matter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-matter.orb.gz")),
    "std/behaviors/std-membership" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-membership.orb.gz")),
    "std/behaviors/std-menu" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-menu.orb.gz")),
    "std/behaviors/std-mesh-asset-art" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-mesh-asset-art.orb.gz")),
    "std/behaviors/std-migration-job" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-migration-job.orb.gz")),
    "std/behaviors/std-ml-classify" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-ml-classify.orb.gz")),
    "std/behaviors/std-ml-exact-check" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-ml-exact-check.orb.gz")),
    "std/behaviors/std-ml-infer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-ml-infer.orb.gz")),
    "std/behaviors/std-ml-label-capture" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-ml-label-capture.orb.gz")),
    "std/behaviors/std-ml-lookup" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-ml-lookup.orb.gz")),
    "std/behaviors/std-ml-posterior" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-ml-posterior.orb.gz")),
    "std/behaviors/std-ml-similarity" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-ml-similarity.orb.gz")),
    "std/behaviors/std-mod-queue" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-mod-queue.orb.gz")),
    "std/behaviors/std-modal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-modal.orb.gz")),
    "std/behaviors/std-moderation-rule" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-moderation-rule.orb.gz")),
    "std/behaviors/std-modifier-group" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-modifier-group.orb.gz")),
    "std/behaviors/std-motion-body" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-motion-body.orb.gz")),
    "std/behaviors/std-multi-party-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-multi-party-flow.orb.gz")),
    "std/behaviors/std-notification-center" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-notification-center.orb.gz")),
    "std/behaviors/std-notification-panel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-notification-panel.orb.gz")),
    "std/behaviors/std-notify-on-event" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-notify-on-event.orb.gz")),
    "std/behaviors/std-nps-survey" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-nps-survey.orb.gz")),
    "std/behaviors/std-objective-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-objective-flow.orb.gz")),
    "std/behaviors/std-objective-reach" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-objective-reach.orb.gz")),
    "std/behaviors/std-offer-letter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-offer-letter.orb.gz")),
    "std/behaviors/std-pagination" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-pagination.orb.gz")),
    "std/behaviors/std-patience" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-patience.orb.gz")),
    "std/behaviors/std-patient" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-patient.orb.gz")),
    "std/behaviors/std-patrol-hazard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-patrol-hazard.orb.gz")),
    "std/behaviors/std-payout-ledger" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-payout-ledger.orb.gz")),
    "std/behaviors/std-physics-circular" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-circular.orb.gz")),
    "std/behaviors/std-physics-collision" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-collision.orb.gz")),
    "std/behaviors/std-physics-diagram" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-diagram.orb.gz")),
    "std/behaviors/std-physics-field-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-field-3d.orb.gz")),
    "std/behaviors/std-physics-freefall" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-freefall.orb.gz")),
    "std/behaviors/std-physics-friction" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-friction.orb.gz")),
    "std/behaviors/std-physics-gas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-gas.orb.gz")),
    "std/behaviors/std-physics-incline" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-incline.orb.gz")),
    "std/behaviors/std-physics-magnetic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-magnetic.orb.gz")),
    "std/behaviors/std-physics-orbit" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-orbit.orb.gz")),
    "std/behaviors/std-physics-orbit-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-orbit-3d.orb.gz")),
    "std/behaviors/std-physics-pendulum" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-pendulum.orb.gz")),
    "std/behaviors/std-physics-projectile" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-projectile.orb.gz")),
    "std/behaviors/std-physics-projectile-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-projectile-3d.orb.gz")),
    "std/behaviors/std-physics-reflection" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-reflection.orb.gz")),
    "std/behaviors/std-physics-spring" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-spring.orb.gz")),
    "std/behaviors/std-physics-wave" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-wave.orb.gz")),
    "std/behaviors/std-physics-wave-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-physics-wave-3d.orb.gz")),
    "std/behaviors/std-platformer-body" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-platformer-body.orb.gz")),
    "std/behaviors/std-pong-rally" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/atoms/std-pong-rally.orb.gz")),
    "std/behaviors/std-portfolio" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-portfolio.orb.gz")),
    "std/behaviors/std-prescription" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-prescription.orb.gz")),
    "std/behaviors/std-prob-bayes" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-bayes.orb.gz")),
    "std/behaviors/std-prob-binomial" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-binomial.orb.gz")),
    "std/behaviors/std-prob-clt" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-clt.orb.gz")),
    "std/behaviors/std-prob-confidence" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-confidence.orb.gz")),
    "std/behaviors/std-prob-galton" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-galton.orb.gz")),
    "std/behaviors/std-prob-montecarlo" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-montecarlo.orb.gz")),
    "std/behaviors/std-prob-normal-distribution" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-normal-distribution.orb.gz")),
    "std/behaviors/std-prob-regression" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-regression.orb.gz")),
    "std/behaviors/std-prob-trials" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-trials.orb.gz")),
    "std/behaviors/std-prob-walk" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-prob-walk.orb.gz")),
    "std/behaviors/std-production" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-production.orb.gz")),
    "std/behaviors/std-psych-conditioning" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-psych-conditioning.orb.gz")),
    "std/behaviors/std-psych-memory" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-psych-memory.orb.gz")),
    "std/behaviors/std-psych-operant" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-psych-operant.orb.gz")),
    "std/behaviors/std-psych-signal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-psych-signal.orb.gz")),
    "std/behaviors/std-psych-stroop" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-psych-stroop.orb.gz")),
    "std/behaviors/std-purchase-order" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-purchase-order.orb.gz")),
    "std/behaviors/std-question-bank" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-question-bank.orb.gz")),
    "std/behaviors/std-rate-limiter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-rate-limiter.orb.gz")),
    "std/behaviors/std-rating-review" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-rating-review.orb.gz")),
    "std/behaviors/std-receipt" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-receipt.orb.gz")),
    "std/behaviors/std-recurrence" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-recurrence.orb.gz")),
    "std/behaviors/std-recurring-charge" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-recurring-charge.orb.gz")),
    "std/behaviors/std-related" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-related.orb.gz")),
    "std/behaviors/std-reminder-scheduler" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-reminder-scheduler.orb.gz")),
    "std/behaviors/std-renewal-risk" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-renewal-risk.orb.gz")),
    "std/behaviors/std-rent-charge" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-rent-charge.orb.gz")),
    "std/behaviors/std-reorder-rule" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-reorder-rule.orb.gz")),
    "std/behaviors/std-response-collector" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-response-collector.orb.gz")),
    "std/behaviors/std-rfi" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-rfi.orb.gz")),
    "std/behaviors/std-rich-editor" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-rich-editor.orb.gz")),
    "std/behaviors/std-round-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-round-flow.orb.gz")),
    "std/behaviors/std-route-optimize" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-route-optimize.orb.gz")),
    "std/behaviors/std-row-access-control" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-row-access-control.orb.gz")),
    "std/behaviors/std-runbook" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-runbook.orb.gz")),
    "std/behaviors/std-saved-search" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-saved-search.orb.gz")),
    "std/behaviors/std-scatter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-scatter.orb.gz")),
    "std/behaviors/std-score" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-score.orb.gz")),
    "std/behaviors/std-search" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-search.orb.gz")),
    "std/behaviors/std-segment" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-segment.orb.gz")),
    "std/behaviors/std-selection" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-selection.orb.gz")),
    "std/behaviors/std-sensor-threshold" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-sensor-threshold.orb.gz")),
    "std/behaviors/std-service-calendar-ical" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-calendar-ical.orb.gz")),
    "std/behaviors/std-service-custom-bearer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-custom-bearer.orb.gz")),
    "std/behaviors/std-service-drive" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-drive.orb.gz")),
    "std/behaviors/std-service-email" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-email.orb.gz")),
    "std/behaviors/std-service-github" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-github.orb.gz")),
    "std/behaviors/std-service-llm" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-llm.orb.gz")),
    "std/behaviors/std-service-maps" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-maps.orb.gz")),
    "std/behaviors/std-service-oauth" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-oauth.orb.gz")),
    "std/behaviors/std-service-print" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-print.orb.gz")),
    "std/behaviors/std-service-redis" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-redis.orb.gz")),
    "std/behaviors/std-service-storage" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-storage.orb.gz")),
    "std/behaviors/std-service-stripe" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-stripe.orb.gz")),
    "std/behaviors/std-service-twilio" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-twilio.orb.gz")),
    "std/behaviors/std-service-youtube" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/atoms/std-service-youtube.orb.gz")),
    "std/behaviors/std-signature-capture" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-signature-capture.orb.gz")),
    "std/behaviors/std-snake-loop" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/atoms/std-snake-loop.orb.gz")),
    "std/behaviors/std-soc-conformity" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-soc-conformity.orb.gz")),
    "std/behaviors/std-soc-deviance" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-soc-deviance.orb.gz")),
    "std/behaviors/std-soc-imagination" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-soc-imagination.orb.gz")),
    "std/behaviors/std-soc-mobility" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-soc-mobility.orb.gz")),
    "std/behaviors/std-soc-network" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-soc-network.orb.gz")),
    "std/behaviors/std-soc-stratification" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/atoms/std-soc-stratification.orb.gz")),
    "std/behaviors/std-stats" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-stats.orb.gz")),
    "std/behaviors/std-status-lifecycle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/atoms/std-status-lifecycle.orb.gz")),
    "std/behaviors/std-step-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-step-flow.orb.gz")),
    "std/behaviors/std-stock-adjustment" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-stock-adjustment.orb.gz")),
    "std/behaviors/std-stock-level" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-stock-level.orb.gz")),
    "std/behaviors/std-submittal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-submittal.orb.gz")),
    "std/behaviors/std-survival-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-survival-flow.orb.gz")),
    "std/behaviors/std-svg-asset-art" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-svg-asset-art.orb.gz")),
    "std/behaviors/std-table-map" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-table-map.orb.gz")),
    "std/behaviors/std-tabs" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-tabs.orb.gz")),
    "std/behaviors/std-tag-taxonomy" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-tag-taxonomy.orb.gz")),
    "std/behaviors/std-technician-dispatch" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-technician-dispatch.orb.gz")),
    "std/behaviors/std-telematics-event" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-telematics-event.orb.gz")),
    "std/behaviors/std-template-editor" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-template-editor.orb.gz")),
    "std/behaviors/std-tenant" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-tenant.orb.gz")),
    "std/behaviors/std-thread" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-thread.orb.gz")),
    "std/behaviors/std-ticket-tier" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-ticket-tier.orb.gz")),
    "std/behaviors/std-time-entry" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-time-entry.orb.gz")),
    "std/behaviors/std-timeline" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-timeline.orb.gz")),
    "std/behaviors/std-timesheet" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-timesheet.orb.gz")),
    "std/behaviors/std-tokenizer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-tokenizer.orb.gz")),
    "std/behaviors/std-trade-order" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-trade-order.orb.gz")),
    "std/behaviors/std-trial" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-trial.orb.gz")),
    "std/behaviors/std-tutorial-overlay" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-tutorial-overlay.orb.gz")),
    "std/behaviors/std-vehicle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-vehicle.orb.gz")),
    "std/behaviors/std-vendor-onboarding" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-vendor-onboarding.orb.gz")),
    "std/behaviors/std-version-history" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-version-history.orb.gz")),
    "std/behaviors/std-vote" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-vote.orb.gz")),
    "std/behaviors/std-waitlist" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-waitlist.orb.gz")),
    "std/behaviors/std-warehouse" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-warehouse.orb.gz")),
    "std/behaviors/std-weight-validator" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/atoms/std-weight-validator.orb.gz")),
    "std/behaviors/std-wizard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/std-wizard.orb.gz")),
    "std/behaviors/std-work-order" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/atoms/std-work-order.orb.gz")),
    "std/behaviors/std-xp" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/std-xp.orb.gz")),
    "std/behaviors/svg-asset-catalog" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-asset-catalog.orb.gz")),
    "std/behaviors/svg-material-crystal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-material-crystal.orb.gz")),
    "std/behaviors/svg-material-magic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-material-magic.orb.gz")),
    "std/behaviors/svg-material-metal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-material-metal.orb.gz")),
    "std/behaviors/svg-palette" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-palette.orb.gz")),
    "std/behaviors/svg-part-gear" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear.orb.gz")),
    "std/behaviors/svg-part-gear-blaster" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear-blaster.orb.gz")),
    "std/behaviors/svg-part-gear-bow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear-bow.orb.gz")),
    "std/behaviors/svg-part-gear-cannon" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear-cannon.orb.gz")),
    "std/behaviors/svg-part-gear-hammer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear-hammer.orb.gz")),
    "std/behaviors/svg-part-gear-katana" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear-katana.orb.gz")),
    "std/behaviors/svg-part-gear-staff" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear-staff.orb.gz")),
    "std/behaviors/svg-part-gear-sword" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-gear-sword.orb.gz")),
    "std/behaviors/svg-part-hat-pointed" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-hat-pointed.orb.gz")),
    "std/behaviors/svg-part-head-banded" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-head-banded.orb.gz")),
    "std/behaviors/svg-part-head-optic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-head-optic.orb.gz")),
    "std/behaviors/svg-part-helm-space" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-helm-space.orb.gz")),
    "std/behaviors/svg-part-helm-visor" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-helm-visor.orb.gz")),
    "std/behaviors/svg-part-helm-winged" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-helm-winged.orb.gz")),
    "std/behaviors/svg-part-hood-ranger" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-hood-ranger.orb.gz")),
    "std/behaviors/svg-part-limb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-limb.orb.gz")),
    "std/behaviors/svg-part-mask-shinobi" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-mask-shinobi.orb.gz")),
    "std/behaviors/svg-part-torso-chassis" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-chassis.orb.gz")),
    "std/behaviors/svg-part-torso-hunter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-hunter.orb.gz")),
    "std/behaviors/svg-part-torso-plate" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-plate.orb.gz")),
    "std/behaviors/svg-part-torso-power" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-power.orb.gz")),
    "std/behaviors/svg-part-torso-robe" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-robe.orb.gz")),
    "std/behaviors/svg-part-torso-shinobi" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-shinobi.orb.gz")),
    "std/behaviors/svg-part-torso-suit" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-suit.orb.gz")),
    "std/behaviors/svg-part-torso-templar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-torso-templar.orb.gz")),
    "std/behaviors/svg-part-unit-base" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-part-unit-base.orb.gz")),
    "std/behaviors/svg-primitives" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-primitives.orb.gz")),
    "std/behaviors/svg-shading" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-shading.orb.gz")),
    "std/behaviors/svg-silhouette" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/atoms/svg-silhouette.orb.gz")),
    "std/behaviors/ui-action-palette" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-action-palette.orb.gz")),
    "std/behaviors/ui-action-tile" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-action-tile.orb.gz")),
    "std/behaviors/ui-algo-graph-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/ui-algo-graph-canvas.orb.gz")),
    "std/behaviors/ui-algorithm-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/ui-algorithm-canvas.orb.gz")),
    "std/behaviors/ui-animated-counter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-animated-counter.orb.gz")),
    "std/behaviors/ui-animated-graphic" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-animated-graphic.orb.gz")),
    "std/behaviors/ui-animated-reveal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-animated-reveal.orb.gz")),
    "std/behaviors/ui-article-section" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-article-section.orb.gz")),
    "std/behaviors/ui-aside" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-aside.orb.gz")),
    "std/behaviors/ui-atlas-image" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-atlas-image.orb.gz")),
    "std/behaviors/ui-atlas-panel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-atlas-panel.orb.gz")),
    "std/behaviors/ui-avatar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-avatar.orb.gz")),
    "std/behaviors/ui-badge" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-badge.orb.gz")),
    "std/behaviors/ui-behavior-view" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/avl/atoms/ui-behavior-view.orb.gz")),
    "std/behaviors/ui-biology-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/ui-biology-canvas.orb.gz")),
    "std/behaviors/ui-box" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-box.orb.gz")),
    "std/behaviors/ui-button" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-button.orb.gz")),
    "std/behaviors/ui-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-canvas.orb.gz")),
    "std/behaviors/ui-canvas-2d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-canvas-2d.orb.gz")),
    "std/behaviors/ui-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-card.orb.gz")),
    "std/behaviors/ui-case-study-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-case-study-card.orb.gz")),
    "std/behaviors/ui-center" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-center.orb.gz")),
    "std/behaviors/ui-checkbox" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-checkbox.orb.gz")),
    "std/behaviors/ui-chemistry-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/ui-chemistry-canvas.orb.gz")),
    "std/behaviors/ui-choice-button" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-choice-button.orb.gz")),
    "std/behaviors/ui-community-links" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-community-links.orb.gz")),
    "std/behaviors/ui-conditional-wrapper" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-conditional-wrapper.orb.gz")),
    "std/behaviors/ui-confetti-effect" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-confetti-effect.orb.gz")),
    "std/behaviors/ui-content-section" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-content-section.orb.gz")),
    "std/behaviors/ui-control-button" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-control-button.orb.gz")),
    "std/behaviors/ui-control-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-control-grid.orb.gz")),
    "std/behaviors/ui-cta-banner" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-cta-banner.orb.gz")),
    "std/behaviors/ui-day-cell" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-day-cell.orb.gz")),
    "std/behaviors/ui-dialog" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-dialog.orb.gz")),
    "std/behaviors/ui-dialogue-bubble" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-dialogue-bubble.orb.gz")),
    "std/behaviors/ui-divider" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-divider.orb.gz")),
    "std/behaviors/ui-draw-fx-layer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-fx-layer.orb.gz")),
    "std/behaviors/ui-draw-group" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-group.orb.gz")),
    "std/behaviors/ui-draw-mesh" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-mesh.orb.gz")),
    "std/behaviors/ui-draw-shape" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-shape.orb.gz")),
    "std/behaviors/ui-draw-shape-layer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-shape-layer.orb.gz")),
    "std/behaviors/ui-draw-sprite" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-sprite.orb.gz")),
    "std/behaviors/ui-draw-sprite-layer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-sprite-layer.orb.gz")),
    "std/behaviors/ui-draw-text" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-text.orb.gz")),
    "std/behaviors/ui-draw-text-layer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-draw-text-layer.orb.gz")),
    "std/behaviors/ui-feature-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-feature-card.orb.gz")),
    "std/behaviors/ui-filter-pill" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-filter-pill.orb.gz")),
    "std/behaviors/ui-flip-container" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-flip-container.orb.gz")),
    "std/behaviors/ui-game-audio-toggle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-game-audio-toggle.orb.gz")),
    "std/behaviors/ui-game-hud" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-game-hud.orb.gz")),
    "std/behaviors/ui-game-icon" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-game-icon.orb.gz")),
    "std/behaviors/ui-game-menu" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-game-menu.orb.gz")),
    "std/behaviors/ui-geometric-pattern" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-geometric-pattern.orb.gz")),
    "std/behaviors/ui-health-bar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-health-bar.orb.gz")),
    "std/behaviors/ui-hero-section" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-hero-section.orb.gz")),
    "std/behaviors/ui-hstack" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-hstack.orb.gz")),
    "std/behaviors/ui-icon" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-icon.orb.gz")),
    "std/behaviors/ui-infinite-scroll-sentinel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-infinite-scroll-sentinel.orb.gz")),
    "std/behaviors/ui-input" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-input.orb.gz")),
    "std/behaviors/ui-install-box" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-install-box.orb.gz")),
    "std/behaviors/ui-label" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-label.orb.gz")),
    "std/behaviors/ui-law-reference-tooltip" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-law-reference-tooltip.orb.gz")),
    "std/behaviors/ui-learning-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/ui-learning-canvas.orb.gz")),
    "std/behaviors/ui-marketing-footer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-marketing-footer.orb.gz")),
    "std/behaviors/ui-marketing-stat-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-marketing-stat-card.orb.gz")),
    "std/behaviors/ui-math-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/ui-math-canvas.orb.gz")),
    "std/behaviors/ui-module-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/avl/atoms/ui-module-card.orb.gz")),
    "std/behaviors/ui-overlay" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-overlay.orb.gz")),
    "std/behaviors/ui-pattern-tile" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-pattern-tile.orb.gz")),
    "std/behaviors/ui-physics-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/learning/atoms/ui-physics-canvas.orb.gz")),
    "std/behaviors/ui-presence" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-presence.orb.gz")),
    "std/behaviors/ui-pricing-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-pricing-card.orb.gz")),
    "std/behaviors/ui-pricing-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-pricing-grid.orb.gz")),
    "std/behaviors/ui-progress-bar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-progress-bar.orb.gz")),
    "std/behaviors/ui-pull-quote" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-pull-quote.orb.gz")),
    "std/behaviors/ui-radio" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-radio.orb.gz")),
    "std/behaviors/ui-range-slider" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-range-slider.orb.gz")),
    "std/behaviors/ui-score-display" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-score-display.orb.gz")),
    "std/behaviors/ui-section-header" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-section-header.orb.gz")),
    "std/behaviors/ui-select" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-select.orb.gz")),
    "std/behaviors/ui-sequence-bar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-sequence-bar.orb.gz")),
    "std/behaviors/ui-service-catalog" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-service-catalog.orb.gz")),
    "std/behaviors/ui-showcase-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-showcase-card.orb.gz")),
    "std/behaviors/ui-spacer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-spacer.orb.gz")),
    "std/behaviors/ui-sparkline" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-sparkline.orb.gz")),
    "std/behaviors/ui-spinner" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-spinner.orb.gz")),
    "std/behaviors/ui-split-section" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-split-section.orb.gz")),
    "std/behaviors/ui-stack" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-stack.orb.gz")),
    "std/behaviors/ui-stat-badge" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-stat-badge.orb.gz")),
    "std/behaviors/ui-state-graph" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-state-graph.orb.gz")),
    "std/behaviors/ui-state-json-view" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-state-json-view.orb.gz")),
    "std/behaviors/ui-stats-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-stats-grid.orb.gz")),
    "std/behaviors/ui-status-dot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-status-dot.orb.gz")),
    "std/behaviors/ui-step-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-step-flow.orb.gz")),
    "std/behaviors/ui-svg-branch" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-branch.orb.gz")),
    "std/behaviors/ui-svg-connection" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-connection.orb.gz")),
    "std/behaviors/ui-svg-flow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-flow.orb.gz")),
    "std/behaviors/ui-svg-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-grid.orb.gz")),
    "std/behaviors/ui-svg-lobe" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-lobe.orb.gz")),
    "std/behaviors/ui-svg-mesh" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-mesh.orb.gz")),
    "std/behaviors/ui-svg-morph" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-morph.orb.gz")),
    "std/behaviors/ui-svg-node" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-node.orb.gz")),
    "std/behaviors/ui-svg-pulse" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-pulse.orb.gz")),
    "std/behaviors/ui-svg-ring" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-ring.orb.gz")),
    "std/behaviors/ui-svg-shield" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-shield.orb.gz")),
    "std/behaviors/ui-svg-stack" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-svg-stack.orb.gz")),
    "std/behaviors/ui-switch" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-switch.orb.gz")),
    "std/behaviors/ui-tag-cloud" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-tag-cloud.orb.gz")),
    "std/behaviors/ui-team-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/atoms/ui-team-card.orb.gz")),
    "std/behaviors/ui-text-highlight" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-text-highlight.orb.gz")),
    "std/behaviors/ui-textarea" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-textarea.orb.gz")),
    "std/behaviors/ui-theme-toggle" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-theme-toggle.orb.gz")),
    "std/behaviors/ui-time-slot-cell" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-time-slot-cell.orb.gz")),
    "std/behaviors/ui-timer-display" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-timer-display.orb.gz")),
    "std/behaviors/ui-trait-frame" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-trait-frame.orb.gz")),
    "std/behaviors/ui-trait-slot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/atoms/ui-trait-slot.orb.gz")),
    "std/behaviors/ui-trend-indicator" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-trend-indicator.orb.gz")),
    "std/behaviors/ui-typewriter-text" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-typewriter-text.orb.gz")),
    "std/behaviors/ui-typography" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-typography.orb.gz")),
    "std/behaviors/ui-vstack" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/atoms/ui-vstack.orb.gz")),
    // === Molecules (107) ===
    "std/behaviors/std-stat-rows" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/std-stat-rows.orb.gz")),
    "std/behaviors/ui-accordion" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-accordion.orb.gz")),
    "std/behaviors/ui-activation-block" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-activation-block.orb.gz")),
    "std/behaviors/ui-alert" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-alert.orb.gz")),
    "std/behaviors/ui-bloom-quiz-block" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-bloom-quiz-block.orb.gz")),
    "std/behaviors/ui-branching-logic-builder" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-branching-logic-builder.orb.gz")),
    "std/behaviors/ui-breadcrumb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-breadcrumb.orb.gz")),
    "std/behaviors/ui-calendar-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-calendar-grid.orb.gz")),
    "std/behaviors/ui-carousel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-carousel.orb.gz")),
    "std/behaviors/ui-chart" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-chart.orb.gz")),
    "std/behaviors/ui-chart-legend" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-chart-legend.orb.gz")),
    "std/behaviors/ui-code-block" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-code-block.orb.gz")),
    "std/behaviors/ui-confirm-dialog" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-confirm-dialog.orb.gz")),
    "std/behaviors/ui-connection-block" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-connection-block.orb.gz")),
    "std/behaviors/ui-container" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-container.orb.gz")),
    "std/behaviors/ui-content-renderer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-content-renderer.orb.gz")),
    "std/behaviors/ui-data-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-data-grid.orb.gz")),
    "std/behaviors/ui-data-list" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-data-list.orb.gz")),
    "std/behaviors/ui-date-range-picker" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-date-range-picker.orb.gz")),
    "std/behaviors/ui-date-range-selector" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-date-range-selector.orb.gz")),
    "std/behaviors/ui-doc-breadcrumb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-doc-breadcrumb.orb.gz")),
    "std/behaviors/ui-doc-pagination" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-doc-pagination.orb.gz")),
    "std/behaviors/ui-doc-search" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-doc-search.orb.gz")),
    "std/behaviors/ui-doc-sidebar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-doc-sidebar.orb.gz")),
    "std/behaviors/ui-doc-toc" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-doc-toc.orb.gz")),
    "std/behaviors/ui-document-viewer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-document-viewer.orb.gz")),
    "std/behaviors/ui-drawer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-drawer.orb.gz")),
    "std/behaviors/ui-edge-decoration" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-edge-decoration.orb.gz")),
    "std/behaviors/ui-emoji-picker" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-emoji-picker.orb.gz")),
    "std/behaviors/ui-empty-state" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-empty-state.orb.gz")),
    "std/behaviors/ui-error-boundary" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-error-boundary.orb.gz")),
    "std/behaviors/ui-error-state" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-error-state.orb.gz")),
    "std/behaviors/ui-feature-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-feature-grid.orb.gz")),
    "std/behaviors/ui-file-tree" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-file-tree.orb.gz")),
    "std/behaviors/ui-filter-group" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-filter-group.orb.gz")),
    "std/behaviors/ui-flex" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-flex.orb.gz")),
    "std/behaviors/ui-flip-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-flip-card.orb.gz")),
    "std/behaviors/ui-floating-action-button" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-floating-action-button.orb.gz")),
    "std/behaviors/ui-form-actions" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-form-actions.orb.gz")),
    "std/behaviors/ui-form-field" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-form-field.orb.gz")),
    "std/behaviors/ui-form-layout" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-form-layout.orb.gz")),
    "std/behaviors/ui-form-section-header" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-form-section-header.orb.gz")),
    "std/behaviors/ui-fx-overlay" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-fx-overlay.orb.gz")),
    "std/behaviors/ui-gradient-divider" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-gradient-divider.orb.gz")),
    "std/behaviors/ui-graph-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-graph-canvas.orb.gz")),
    "std/behaviors/ui-graph-view" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-graph-view.orb.gz")),
    "std/behaviors/ui-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-grid.orb.gz")),
    "std/behaviors/ui-header" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-header.orb.gz")),
    "std/behaviors/ui-import-preview-tree" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-import-preview-tree.orb.gz")),
    "std/behaviors/ui-import-progress" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-import-progress.orb.gz")),
    "std/behaviors/ui-import-source-picker" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-import-source-picker.orb.gz")),
    "std/behaviors/ui-input-group" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-input-group.orb.gz")),
    "std/behaviors/ui-jazari-state-machine" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-jazari-state-machine.orb.gz")),
    "std/behaviors/ui-lightbox" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-lightbox.orb.gz")),
    "std/behaviors/ui-likert-scale" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-likert-scale.orb.gz")),
    "std/behaviors/ui-line-chart" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-line-chart.orb.gz")),
    "std/behaviors/ui-loading-state" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-loading-state.orb.gz")),
    "std/behaviors/ui-map-view" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-map-view.orb.gz")),
    "std/behaviors/ui-markdown-content" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-markdown-content.orb.gz")),
    "std/behaviors/ui-matrix-question" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-matrix-question.orb.gz")),
    "std/behaviors/ui-menu" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-menu.orb.gz")),
    "std/behaviors/ui-meter" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-meter.orb.gz")),
    "std/behaviors/ui-modal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-modal.orb.gz")),
    "std/behaviors/ui-navigation" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-navigation.orb.gz")),
    "std/behaviors/ui-notification" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-notification.orb.gz")),
    "std/behaviors/ui-number-stepper" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-number-stepper.orb.gz")),
    "std/behaviors/ui-option-constraint-group" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-option-constraint-group.orb.gz")),
    "std/behaviors/ui-orbital-visualization" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-orbital-visualization.orb.gz")),
    "std/behaviors/ui-page-header" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-page-header.orb.gz")),
    "std/behaviors/ui-page-transition" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-page-transition.orb.gz")),
    "std/behaviors/ui-pagination" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-pagination.orb.gz")),
    "std/behaviors/ui-popover" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-popover.orb.gz")),
    "std/behaviors/ui-positioned-canvas" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-positioned-canvas.orb.gz")),
    "std/behaviors/ui-progress-dots" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-progress-dots.orb.gz")),
    "std/behaviors/ui-pull-to-refresh" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-pull-to-refresh.orb.gz")),
    "std/behaviors/ui-qr-scanner" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-qr-scanner.orb.gz")),
    "std/behaviors/ui-quiz-block" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-quiz-block.orb.gz")),
    "std/behaviors/ui-reflection-block" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-reflection-block.orb.gz")),
    "std/behaviors/ui-relation-select" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-relation-select.orb.gz")),
    "std/behaviors/ui-repeatable-form-section" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-repeatable-form-section.orb.gz")),
    "std/behaviors/ui-reply-tree" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-reply-tree.orb.gz")),
    "std/behaviors/ui-rich-block-editor" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-rich-block-editor.orb.gz")),
    "std/behaviors/ui-scaled-diagram" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-scaled-diagram.orb.gz")),
    "std/behaviors/ui-search-input" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-search-input.orb.gz")),
    "std/behaviors/ui-section" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-section.orb.gz")),
    "std/behaviors/ui-side-panel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-side-panel.orb.gz")),
    "std/behaviors/ui-sidebar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-sidebar.orb.gz")),
    "std/behaviors/ui-signature-pad" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-signature-pad.orb.gz")),
    "std/behaviors/ui-simple-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-simple-grid.orb.gz")),
    "std/behaviors/ui-skeleton" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-skeleton.orb.gz")),
    "std/behaviors/ui-social-proof" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-social-proof.orb.gz")),
    "std/behaviors/ui-sortable-list" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-sortable-list.orb.gz")),
    "std/behaviors/ui-split" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-split.orb.gz")),
    "std/behaviors/ui-star-rating" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-star-rating.orb.gz")),
    "std/behaviors/ui-stat-display" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-stat-display.orb.gz")),
    "std/behaviors/ui-swipeable-row" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-swipeable-row.orb.gz")),
    "std/behaviors/ui-table-view" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-table-view.orb.gz")),
    "std/behaviors/ui-tabs" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-tabs.orb.gz")),
    "std/behaviors/ui-tag-input" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-tag-input.orb.gz")),
    "std/behaviors/ui-tooltip" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-tooltip.orb.gz")),
    "std/behaviors/ui-upload-drop-zone" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-upload-drop-zone.orb.gz")),
    "std/behaviors/ui-version-diff" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-version-diff.orb.gz")),
    "std/behaviors/ui-violation-alert" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-violation-alert.orb.gz")),
    "std/behaviors/ui-vote-stack" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-vote-stack.orb.gz")),
    "std/behaviors/ui-wizard-container" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-wizard-container.orb.gz")),
    "std/behaviors/ui-wizard-navigation" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-wizard-navigation.orb.gz")),
    "std/behaviors/ui-wizard-progress" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/molecules/ui-wizard-progress.orb.gz")),
    // === Organisms (318) ===
    "std/behaviors/dnd-character-hud" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/organisms/dnd-character-hud.orb.gz")),
    "std/behaviors/dnd-dungeon-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/organisms/dnd-dungeon-board.orb.gz")),
    "std/behaviors/dnd-dungeon-delver" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/organisms/dnd-dungeon-delver.orb.gz")),
    "std/behaviors/dnd-svg-gallery" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/dnd/organisms/dnd-svg-gallery.orb.gz")),
    "std/behaviors/kflow-learning-companion" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/kflow-learning-companion.orb.gz")),
    "std/behaviors/learning-algorithms" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-algorithms.orb.gz")),
    "std/behaviors/learning-astronomy-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-astronomy-lab.orb.gz")),
    "std/behaviors/learning-biology" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-biology.orb.gz")),
    "std/behaviors/learning-biology-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-biology-lab.orb.gz")),
    "std/behaviors/learning-charts" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-charts.orb.gz")),
    "std/behaviors/learning-chemistry" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-chemistry.orb.gz")),
    "std/behaviors/learning-chemistry-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-chemistry-lab.orb.gz")),
    "std/behaviors/learning-civics-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-civics-lab.orb.gz")),
    "std/behaviors/learning-diagram" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-diagram.orb.gz")),
    "std/behaviors/learning-earth-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-earth-lab.orb.gz")),
    "std/behaviors/learning-economics-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-economics-lab.orb.gz")),
    "std/behaviors/learning-geography-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-geography-lab.orb.gz")),
    "std/behaviors/learning-history-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-history-lab.orb.gz")),
    "std/behaviors/learning-linguistics-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-linguistics-lab.orb.gz")),
    "std/behaviors/learning-logic-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-logic-lab.orb.gz")),
    "std/behaviors/learning-math" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-math.orb.gz")),
    "std/behaviors/learning-math-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-math-lab.orb.gz")),
    "std/behaviors/learning-physics" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-physics.orb.gz")),
    "std/behaviors/learning-physics-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-physics-lab.orb.gz")),
    "std/behaviors/learning-probability" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-probability.orb.gz")),
    "std/behaviors/learning-probability-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-probability-lab.orb.gz")),
    "std/behaviors/learning-psychology-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-psychology-lab.orb.gz")),
    "std/behaviors/learning-sociology-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/learning/organisms/learning-sociology-lab.orb.gz")),
    "std/behaviors/mesh-material-gallery" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/organisms/mesh-material-gallery.orb.gz")),
    "std/behaviors/mesh-style-flip" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/organisms/mesh-style-flip.orb.gz")),
    "std/behaviors/mesh-units-crystal-golem" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/organisms/mesh-units-crystal-golem.orb.gz")),
    "std/behaviors/mesh-units-flame-wisp" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/organisms/mesh-units-flame-wisp.orb.gz")),
    "std/behaviors/mesh-units-sentinel-robot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/organisms/mesh-units-sentinel-robot.orb.gz")),
    "std/behaviors/mesh-units-stealth-operative" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/mesh/organisms/mesh-units-stealth-operative.orb.gz")),
    "std/behaviors/riya-game" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/organisms/riya-game.orb.gz")),
    "std/behaviors/riya-game-platformer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/organisms/riya-game-platformer.orb.gz")),
    "std/behaviors/riya-game-platformer-open-world" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/riya/organisms/riya-game-platformer-open-world.orb.gz")),
    "std/behaviors/rpg-dungeon-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/organisms/rpg-dungeon-board.orb.gz")),
    "std/behaviors/shadowdepths" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/rpg/organisms/shadowdepths.orb.gz")),
    "std/behaviors/std-accounting" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-accounting.orb.gz")),
    "std/behaviors/std-accounting-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-accounting-public.orb.gz")),
    "std/behaviors/std-agent-builder" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/organisms/std-agent-builder.orb.gz")),
    "std/behaviors/std-agent-rabit" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/agent/organisms/std-agent-rabit.orb.gz")),
    "std/behaviors/std-almadar-orb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/websites/organisms/std-almadar-orb.orb.gz")),
    "std/behaviors/std-almadar-services" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/websites/organisms/std-almadar-services.orb.gz")),
    "std/behaviors/std-almadar-studio" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/websites/organisms/std-almadar-studio.orb.gz")),
    "std/behaviors/std-almadar-website" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/websites/organisms/std-almadar-website.orb.gz")),
    "std/behaviors/std-almadar-website-blog" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/websites/organisms/std-almadar-website-blog.orb.gz")),
    "std/behaviors/std-alummaalraqamia" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-alummaalraqamia.orb.gz")),
    "std/behaviors/std-api-gateway" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-api-gateway.orb.gz")),
    "std/behaviors/std-arcade-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-arcade-board-3d.orb.gz")),
    "std/behaviors/std-ats-recruiting" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-ats-recruiting.orb.gz")),
    "std/behaviors/std-availability-lookup" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-availability-lookup.orb.gz")),
    "std/behaviors/std-blaz-klemenc" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-blaz-klemenc.orb.gz")),
    "std/behaviors/std-booking-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-booking-site.orb.gz")),
    "std/behaviors/std-booking-system" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-booking-system.orb.gz")),
    "std/behaviors/std-breakout" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-breakout.orb.gz")),
    "std/behaviors/std-builder" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-builder.orb.gz")),
    "std/behaviors/std-campaign-landing" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-campaign-landing.orb.gz")),
    "std/behaviors/std-careers-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-careers-site.orb.gz")),
    "std/behaviors/std-cicd-pipeline" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-cicd-pipeline.orb.gz")),
    "std/behaviors/std-cicd-status" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-cicd-status.orb.gz")),
    "std/behaviors/std-client-case-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-client-case-portal.orb.gz")),
    "std/behaviors/std-cms" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-cms.orb.gz")),
    "std/behaviors/std-cms-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-cms-site.orb.gz")),
    "std/behaviors/std-coding-academy" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-coding-academy.orb.gz")),
    "std/behaviors/std-coding-academy-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-coding-academy-site.orb.gz")),
    "std/behaviors/std-construction-pm" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-construction-pm.orb.gz")),
    "std/behaviors/std-construction-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-construction-portal.orb.gz")),
    "std/behaviors/std-contract" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-contract.orb.gz")),
    "std/behaviors/std-contract-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-contract-portal.orb.gz")),
    "std/behaviors/std-crew-board-2d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-crew-board-2d.orb.gz")),
    "std/behaviors/std-crm" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-crm.orb.gz")),
    "std/behaviors/std-customer-success" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-customer-success.orb.gz")),
    "std/behaviors/std-developer-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-developer-portal.orb.gz")),
    "std/behaviors/std-devops-dashboard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-devops-dashboard.orb.gz")),
    "std/behaviors/std-document-mgmt" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-document-mgmt.orb.gz")),
    "std/behaviors/std-document-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-document-portal.orb.gz")),
    "std/behaviors/std-donate-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-donate-site.orb.gz")),
    "std/behaviors/std-dungeon-board-2d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-dungeon-board-2d.orb.gz")),
    "std/behaviors/std-ecommerce" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-ecommerce.orb.gz")),
    "std/behaviors/std-escalating-decision" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/organisms/std-escalating-decision.orb.gz")),
    "std/behaviors/std-evade" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-evade.orb.gz")),
    "std/behaviors/std-event-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-event-site.orb.gz")),
    "std/behaviors/std-event-ticketing" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-event-ticketing.orb.gz")),
    "std/behaviors/std-field-service" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-field-service.orb.gz")),
    "std/behaviors/std-finance-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-finance-portal.orb.gz")),
    "std/behaviors/std-finance-tracker" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-finance-tracker.orb.gz")),
    "std/behaviors/std-fitness-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-fitness-site.orb.gz")),
    "std/behaviors/std-fitness-studio" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-fitness-studio.orb.gz")),
    "std/behaviors/std-flappy" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-flappy.orb.gz")),
    "std/behaviors/std-flashcards" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/curated/organisms/std-flashcards.orb.gz")),
    "std/behaviors/std-fleet-mgmt" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-fleet-mgmt.orb.gz")),
    "std/behaviors/std-forum" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-forum.orb.gz")),
    "std/behaviors/std-forum-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-forum-public.orb.gz")),
    "std/behaviors/std-frogger" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-frogger.orb.gz")),
    "std/behaviors/std-gem-rush" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-gem-rush.orb.gz")),
    "std/behaviors/std-healthcare" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-healthcare.orb.gz")),
    "std/behaviors/std-help-center" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-help-center.orb.gz")),
    "std/behaviors/std-help-center-reader" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-help-center-reader.orb.gz")),
    "std/behaviors/std-helpdesk" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-helpdesk.orb.gz")),
    "std/behaviors/std-hr-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-hr-portal.orb.gz")),
    "std/behaviors/std-hr-self-service" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-hr-self-service.orb.gz")),
    "std/behaviors/std-inspection-system" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-inspection-system.orb.gz")),
    "std/behaviors/std-inventory" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-inventory.orb.gz")),
    "std/behaviors/std-iot-dashboard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-iot-dashboard.orb.gz")),
    "std/behaviors/std-iot-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-iot-public.orb.gz")),
    "std/behaviors/std-iram" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-iram.orb.gz")),
    "std/behaviors/std-kekec" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/kekec/organisms/std-kekec.orb.gz")),
    "std/behaviors/std-kflow" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-kflow.orb.gz")),
    "std/behaviors/std-knowledge-tracing" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ml/organisms/std-knowledge-tracing.orb.gz")),
    "std/behaviors/std-lead-capture-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-lead-capture-site.orb.gz")),
    "std/behaviors/std-legal-case" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-legal-case.orb.gz")),
    "std/behaviors/std-listings" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-listings.orb.gz")),
    "std/behaviors/std-listings-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-listings-site.orb.gz")),
    "std/behaviors/std-lms" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-lms.orb.gz")),
    "std/behaviors/std-lms-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-lms-site.orb.gz")),
    "std/behaviors/std-market-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-market-board-3d.orb.gz")),
    "std/behaviors/std-marketing-campaign" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-marketing-campaign.orb.gz")),
    "std/behaviors/std-marketing-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-marketing-site.orb.gz")),
    "std/behaviors/std-marketplace" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-marketplace.orb.gz")),
    "std/behaviors/std-marketplace-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-marketplace-site.orb.gz")),
    "std/behaviors/std-masar-website" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/websites/organisms/std-masar-website.orb.gz")),
    "std/behaviors/std-mesh-tactics" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-mesh-tactics.orb.gz")),
    "std/behaviors/std-nonprofit-donations" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-nonprofit-donations.orb.gz")),
    "std/behaviors/std-notes" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-notes.orb.gz")),
    "std/behaviors/std-notes-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-notes-public.orb.gz")),
    "std/behaviors/std-objective-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-objective-board-3d.orb.gz")),
    "std/behaviors/std-patient-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-patient-portal.orb.gz")),
    "std/behaviors/std-platformer-board-2d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-platformer-board-2d.orb.gz")),
    "std/behaviors/std-pong" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-pong.orb.gz")),
    "std/behaviors/std-project-manager" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-project-manager.orb.gz")),
    "std/behaviors/std-project-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-project-portal.orb.gz")),
    "std/behaviors/std-property-mgmt" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-property-mgmt.orb.gz")),
    "std/behaviors/std-property-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-property-site.orb.gz")),
    "std/behaviors/std-public-help-center" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-public-help-center.orb.gz")),
    "std/behaviors/std-public-report" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-public-report.orb.gz")),
    "std/behaviors/std-realtime-chat" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-realtime-chat.orb.gz")),
    "std/behaviors/std-restaurant-pos" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-restaurant-pos.orb.gz")),
    "std/behaviors/std-restaurant-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-restaurant-site.orb.gz")),
    "std/behaviors/std-retail-pos" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-retail-pos.orb.gz")),
    "std/behaviors/std-retail-store" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-retail-store.orb.gz")),
    "std/behaviors/std-service-marketplace" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/organisms/std-service-marketplace.orb.gz")),
    "std/behaviors/std-service-request-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-service-request-site.orb.gz")),
    "std/behaviors/std-service-research-assistant" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/service/organisms/std-service-research-assistant.orb.gz")),
    "std/behaviors/std-shipment-tracking" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-shipment-tracking.orb.gz")),
    "std/behaviors/std-simple-rpg" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-simple-rpg.orb.gz")),
    "std/behaviors/std-snake" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-snake.orb.gz")),
    "std/behaviors/std-social-feed" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-social-feed.orb.gz")),
    "std/behaviors/std-social-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-social-public.orb.gz")),
    "std/behaviors/std-status-lifecycle-control" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/infra/organisms/std-status-lifecycle-control.orb.gz")),
    "std/behaviors/std-status-page" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-status-page.orb.gz")),
    "std/behaviors/std-storefront" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-storefront.orb.gz")),
    "std/behaviors/std-subscription-billing" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-subscription-billing.orb.gz")),
    "std/behaviors/std-subscription-site" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-subscription-site.orb.gz")),
    "std/behaviors/std-success-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-success-portal.orb.gz")),
    "std/behaviors/std-support-chat" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-support-chat.orb.gz")),
    "std/behaviors/std-survey" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-survey.orb.gz")),
    "std/behaviors/std-survey-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-survey-public.orb.gz")),
    "std/behaviors/std-survival-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-survival-board-3d.orb.gz")),
    "std/behaviors/std-tactics-board-2d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-tactics-board-2d.orb.gz")),
    "std/behaviors/std-tactics-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-tactics-board-3d.orb.gz")),
    "std/behaviors/std-task-manager" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-task-manager.orb.gz")),
    "std/behaviors/std-time-approval-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-time-approval-portal.orb.gz")),
    "std/behaviors/std-time-tracking" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-time-tracking.orb.gz")),
    "std/behaviors/std-trading-dashboard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-trading-dashboard.orb.gz")),
    "std/behaviors/std-trading-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-trading-public.orb.gz")),
    "std/behaviors/std-trait-wars" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-trait-wars.orb.gz")),
    "std/behaviors/std-vector-platformer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-vector-platformer.orb.gz")),
    "std/behaviors/std-vector-tactics" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/std-vector-tactics.orb.gz")),
    "std/behaviors/std-wiki" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-wiki.orb.gz")),
    "std/behaviors/std-wiki-public" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/marketing/organisms/std-wiki-public.orb.gz")),
    "std/behaviors/std-winning-11" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/app/organisms/std-winning-11.orb.gz")),
    "std/behaviors/std-worldmap-board-2d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/organisms/std-worldmap-board-2d.orb.gz")),
    "std/behaviors/svg-background-cavern-depths" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-background-cavern-depths.orb.gz")),
    "std/behaviors/svg-background-dusk-ridge" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-background-dusk-ridge.orb.gz")),
    "std/behaviors/svg-background-forest-night" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-background-forest-night.orb.gz")),
    "std/behaviors/svg-background-storm-peaks" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-background-storm-peaks.orb.gz")),
    "std/behaviors/svg-board-overlay" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-board-overlay.orb.gz")),
    "std/behaviors/svg-card-token" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-card-token.orb.gz")),
    "std/behaviors/svg-cards-hero-back" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-cards-hero-back.orb.gz")),
    "std/behaviors/svg-cards-relic-frame" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-cards-relic-frame.orb.gz")),
    "std/behaviors/svg-cards-spell-frame" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-cards-spell-frame.orb.gz")),
    "std/behaviors/svg-effect-arcane-portal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-effect-arcane-portal.orb.gz")),
    "std/behaviors/svg-effect-explosion-burst" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-effect-explosion-burst.orb.gz")),
    "std/behaviors/svg-effect-heal-burst" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-effect-heal-burst.orb.gz")),
    "std/behaviors/svg-effect-poison-cloud" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-effect-poison-cloud.orb.gz")),
    "std/behaviors/svg-effect-shield-bubble" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-effect-shield-bubble.orb.gz")),
    "std/behaviors/svg-effect-vfx" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-effect-vfx.orb.gz")),
    "std/behaviors/svg-facet-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-facet-lab.orb.gz")),
    "std/behaviors/svg-feature-ancient-obelisk" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-feature-ancient-obelisk.orb.gz")),
    "std/behaviors/svg-feature-campfire" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-feature-campfire.orb.gz")),
    "std/behaviors/svg-feature-crystal-cluster" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-feature-crystal-cluster.orb.gz")),
    "std/behaviors/svg-feature-object" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-feature-object.orb.gz")),
    "std/behaviors/svg-feature-treasure-chest" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-feature-treasure-chest.orb.gz")),
    "std/behaviors/svg-gem-token" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-gem-token.orb.gz")),
    "std/behaviors/svg-material-gallery" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-material-gallery.orb.gz")),
    "std/behaviors/svg-paint-lab" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-paint-lab.orb.gz")),
    "std/behaviors/svg-shield-emblem" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-shield-emblem.orb.gz")),
    "std/behaviors/svg-style-flip" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-style-flip.orb.gz")),
    "std/behaviors/svg-style-showcase" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-style-showcase.orb.gz")),
    "std/behaviors/svg-terrain-grass-block" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-terrain-grass-block.orb.gz")),
    "std/behaviors/svg-terrain-lava-rock" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-terrain-lava-rock.orb.gz")),
    "std/behaviors/svg-terrain-sand-dune" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-terrain-sand-dune.orb.gz")),
    "std/behaviors/svg-terrain-stone-floor" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-terrain-stone-floor.orb.gz")),
    "std/behaviors/svg-terrain-tile" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-terrain-tile.orb.gz")),
    "std/behaviors/svg-terrain-water-tile" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-terrain-water-tile.orb.gz")),
    "std/behaviors/svg-ui-health-crystal" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-ui-health-crystal.orb.gz")),
    "std/behaviors/svg-ui-mana-orb" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-ui-mana-orb.orb.gz")),
    "std/behaviors/svg-ui-victory-banner" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-ui-victory-banner.orb.gz")),
    "std/behaviors/svg-ui-widget" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-ui-widget.orb.gz")),
    "std/behaviors/svg-unit-token" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-unit-token.orb.gz")),
    "std/behaviors/svg-units-arcane-wizard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-arcane-wizard.orb.gz")),
    "std/behaviors/svg-units-combat-android" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-combat-android.orb.gz")),
    "std/behaviors/svg-units-crimson-vanguard" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-crimson-vanguard.orb.gz")),
    "std/behaviors/svg-units-emerald-ranger" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-emerald-ranger.orb.gz")),
    "std/behaviors/svg-units-flame-imp" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-flame-imp.orb.gz")),
    "std/behaviors/svg-units-forest-archer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-forest-archer.orb.gz")),
    "std/behaviors/svg-units-knight-errant" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-knight-errant.orb.gz")),
    "std/behaviors/svg-units-plasma-trooper" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-plasma-trooper.orb.gz")),
    "std/behaviors/svg-units-platformer-hero" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-platformer-hero.orb.gz")),
    "std/behaviors/svg-units-royal-knight" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-royal-knight.orb.gz")),
    "std/behaviors/svg-units-shadow-shinobi" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-shadow-shinobi.orb.gz")),
    "std/behaviors/svg-units-shadow-wraith" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-shadow-wraith.orb.gz")),
    "std/behaviors/svg-units-solar-templar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-solar-templar.orb.gz")),
    "std/behaviors/svg-units-stealth-operative" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-stealth-operative.orb.gz")),
    "std/behaviors/svg-units-stone-golem" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/svg/organisms/svg-units-stone-golem.orb.gz")),
    "std/behaviors/ui-arcade-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-arcade-board-3d.orb.gz")),
    "std/behaviors/ui-arena-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-arena-board.orb.gz")),
    "std/behaviors/ui-arena-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-arena-board-3d.orb.gz")),
    "std/behaviors/ui-battle-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-battle-board.orb.gz")),
    "std/behaviors/ui-boardgame-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-boardgame-board.orb.gz")),
    "std/behaviors/ui-book-chapter-view" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-book-chapter-view.orb.gz")),
    "std/behaviors/ui-book-cover-page" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-book-cover-page.orb.gz")),
    "std/behaviors/ui-book-nav-bar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-book-nav-bar.orb.gz")),
    "std/behaviors/ui-book-table-of-contents" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-book-table-of-contents.orb.gz")),
    "std/behaviors/ui-book-viewer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-book-viewer.orb.gz")),
    "std/behaviors/ui-brick-sandbox-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-brick-sandbox-board-3d.orb.gz")),
    "std/behaviors/ui-case-study-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-case-study-organism.orb.gz")),
    "std/behaviors/ui-castle-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-castle-board.orb.gz")),
    "std/behaviors/ui-chat-bar" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-chat-bar.orb.gz")),
    "std/behaviors/ui-city-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-city-board-3d.orb.gz")),
    "std/behaviors/ui-city-builder-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-city-builder-board.orb.gz")),
    "std/behaviors/ui-code-runner-panel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-code-runner-panel.orb.gz")),
    "std/behaviors/ui-dashboard-grid" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-dashboard-grid.orb.gz")),
    "std/behaviors/ui-debugger-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/kekec/organisms/ui-debugger-board.orb.gz")),
    "std/behaviors/ui-detail-panel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-detail-panel.orb.gz")),
    "std/behaviors/ui-drawer-slot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-drawer-slot.orb.gz")),
    "std/behaviors/ui-entity-cards" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-entity-cards.orb.gz")),
    "std/behaviors/ui-entity-list" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-entity-list.orb.gz")),
    "std/behaviors/ui-entity-table" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-entity-table.orb.gz")),
    "std/behaviors/ui-event-handler-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/kekec/organisms/ui-event-handler-board.orb.gz")),
    "std/behaviors/ui-fantasy-town-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-fantasy-town-board-3d.orb.gz")),
    "std/behaviors/ui-feature-grid-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-feature-grid-organism.orb.gz")),
    "std/behaviors/ui-fishing-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-fishing-board.orb.gz")),
    "std/behaviors/ui-form" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-form.orb.gz")),
    "std/behaviors/ui-form-section" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-form-section.orb.gz")),
    "std/behaviors/ui-graveyard-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-graveyard-board-3d.orb.gz")),
    "std/behaviors/ui-grid-tactics-demo-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-grid-tactics-demo-board.orb.gz")),
    "std/behaviors/ui-hero-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-hero-organism.orb.gz")),
    "std/behaviors/ui-hex-strategy-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-hex-strategy-board.orb.gz")),
    "std/behaviors/ui-hex-strategy-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-hex-strategy-board-3d.orb.gz")),
    "std/behaviors/ui-holiday-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-holiday-board-3d.orb.gz")),
    "std/behaviors/ui-holiday-runner-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-holiday-runner-board.orb.gz")),
    "std/behaviors/ui-market-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-market-board-3d.orb.gz")),
    "std/behaviors/ui-master-detail" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-master-detail.orb.gz")),
    "std/behaviors/ui-master-detail-layout" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-master-detail-layout.orb.gz")),
    "std/behaviors/ui-match-puzzle-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-match-puzzle-board.orb.gz")),
    "std/behaviors/ui-media-gallery" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-media-gallery.orb.gz")),
    "std/behaviors/ui-minigolf-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-minigolf-board.orb.gz")),
    "std/behaviors/ui-minigolf-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-minigolf-board-3d.orb.gz")),
    "std/behaviors/ui-modal-slot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-modal-slot.orb.gz")),
    "std/behaviors/ui-nature-survival-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-nature-survival-board-3d.orb.gz")),
    "std/behaviors/ui-physics-sandbox-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-physics-sandbox-board.orb.gz")),
    "std/behaviors/ui-pinball-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-pinball-board.orb.gz")),
    "std/behaviors/ui-pirate-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-pirate-board.orb.gz")),
    "std/behaviors/ui-pirate-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-pirate-board-3d.orb.gz")),
    "std/behaviors/ui-platformer-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-platformer-board.orb.gz")),
    "std/behaviors/ui-platformer-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-platformer-board-3d.orb.gz")),
    "std/behaviors/ui-pricing-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-pricing-organism.orb.gz")),
    "std/behaviors/ui-racing-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-racing-board.orb.gz")),
    "std/behaviors/ui-racing-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-racing-board-3d.orb.gz")),
    "std/behaviors/ui-roguelike-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-roguelike-board.orb.gz")),
    "std/behaviors/ui-runtime-debugger" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-runtime-debugger.orb.gz")),
    "std/behaviors/ui-segment-renderer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-segment-renderer.orb.gz")),
    "std/behaviors/ui-sequencer-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/kekec/organisms/ui-sequencer-board.orb.gz")),
    "std/behaviors/ui-showcase-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-showcase-organism.orb.gz")),
    "std/behaviors/ui-skatepark-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-skatepark-board-3d.orb.gz")),
    "std/behaviors/ui-sokoban-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-sokoban-board.orb.gz")),
    "std/behaviors/ui-space-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-space-board-3d.orb.gz")),
    "std/behaviors/ui-space-shmup-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-space-shmup-board.orb.gz")),
    "std/behaviors/ui-space-station-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-space-station-board.orb.gz")),
    "std/behaviors/ui-split-pane" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-split-pane.orb.gz")),
    "std/behaviors/ui-sports-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-sports-board.orb.gz")),
    "std/behaviors/ui-stat-card" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-stat-card.orb.gz")),
    "std/behaviors/ui-state-architect-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/kekec/organisms/ui-state-architect-board.orb.gz")),
    "std/behaviors/ui-state-machine-view" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-state-machine-view.orb.gz")),
    "std/behaviors/ui-stats-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-stats-organism.orb.gz")),
    "std/behaviors/ui-step-flow-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-step-flow-organism.orb.gz")),
    "std/behaviors/ui-subagent-trace-panel" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-subagent-trace-panel.orb.gz")),
    "std/behaviors/ui-tabbed-container" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-tabbed-container.orb.gz")),
    "std/behaviors/ui-tactics-demo-2d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-tactics-demo-2d.orb.gz")),
    "std/behaviors/ui-tactics-demo-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-tactics-demo-3d.orb.gz")),
    "std/behaviors/ui-tanks-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-tanks-board.orb.gz")),
    "std/behaviors/ui-team-organism" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/organisms/ui-team-organism.orb.gz")),
    "std/behaviors/ui-timeline" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-timeline.orb.gz")),
    "std/behaviors/ui-toast-slot" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-toast-slot.orb.gz")),
    "std/behaviors/ui-top-down-shooter-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-top-down-shooter-board.orb.gz")),
    "std/behaviors/ui-topdown-dungeon-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-topdown-dungeon-board.orb.gz")),
    "std/behaviors/ui-topdown-rts-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-topdown-rts-board.orb.gz")),
    "std/behaviors/ui-tower-defense-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-tower-defense-board.orb.gz")),
    "std/behaviors/ui-tower-defense-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-tower-defense-board-3d.orb.gz")),
    "std/behaviors/ui-toy-track-board-3d" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-toy-track-board-3d.orb.gz")),
    "std/behaviors/ui-ui-slot-renderer" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/organisms/ui-ui-slot-renderer.orb.gz")),
    "std/behaviors/ui-uncontrolled-battle-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-uncontrolled-battle-board.orb.gz")),
    "std/behaviors/ui-visual-novel-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-visual-novel-board.orb.gz")),
    "std/behaviors/ui-winter-ski-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-winter-ski-board.orb.gz")),
    "std/behaviors/ui-world-map-board" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/io/game/organisms/ui-world-map-board.orb.gz")),
    // === Templates (9) ===
    "std/behaviors/ui-about-page-template" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/templates/ui-about-page-template.orb.gz")),
    "std/behaviors/ui-auth-layout" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/templates/ui-auth-layout.orb.gz")),
    "std/behaviors/ui-counter-template" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/templates/ui-counter-template.orb.gz")),
    "std/behaviors/ui-dashboard-layout" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/templates/ui-dashboard-layout.orb.gz")),
    "std/behaviors/ui-feature-detail-page-template" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/templates/ui-feature-detail-page-template.orb.gz")),
    "std/behaviors/ui-game-shell" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/game/templates/ui-game-shell.orb.gz")),
    "std/behaviors/ui-generic-app-template" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/core/templates/ui-generic-app-template.orb.gz")),
    "std/behaviors/ui-landing-page-template" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/templates/ui-landing-page-template.orb.gz")),
    "std/behaviors/ui-pricing-page-template" => include_bytes!(concat!(env!("OUT_DIR"), "/baked-gz/std/ui/marketing/templates/ui-pricing-page-template.orb.gz")),
};
// SYNC-GENERATED:EMBEDDED_STD_BEHAVIORS:END

/// Look up an embedded std behavior by its canonical std path.
///
/// Returns the raw `.orb` JSON content as a `&'static str`, or `None` if the
/// path is not in the embedded set. The caller is responsible for parsing the
/// content (typically via the existing schema parser).
///
/// If `ORBITAL_STD_BEHAVIORS_DIR` is set (a registry root, e.g.
/// `packages/almadar-std/behaviors/registry`), a local `.orb` file there wins
/// over the compiled-in one — this is what lets a local/unpublished std
/// behavior edit be `uses`-resolved without a full `make sync` re-bake +
/// Rust rebuild. Unset (the default) is byte-for-byte today's behavior.
pub fn get_std_behavior_content(canonical_path: &str) -> Option<&'static str> {
    if let Some(overridden) = std_behaviors_override_index().get(canonical_path) {
        return Some(*overridden);
    }
    EMBEDDED_STD_BEHAVIORS
        .get(canonical_path)
        .map(|gz| inflated_behavior(canonical_path, gz))
}

/// Lazily inflate a behavior's gzipped `.orb` once per path, then leak it to
/// `'static` so the accessor keeps its reference shape. Resolve touches a
/// handful of behaviors per run, so the cache stays tiny; the leak is bounded
/// by the number of distinct behaviors ever resolved.
fn inflated_behavior(canonical_path: &str, gz: &'static [u8]) -> &'static str {
    use std::sync::Mutex;
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("behavior inflate cache poisoned");
    if let Some(s) = guard.get(canonical_path) {
        return s;
    }
    let inflated: &'static str = Box::leak(gunzip(gz).into_boxed_str());
    guard.insert(canonical_path.to_string(), inflated);
    inflated
}

/// Lazily-built index of `ORBITAL_STD_BEHAVIORS_DIR` overrides, keyed by the
/// canonical `std/behaviors/<name>` path (matching `EMBEDDED_STD_BEHAVIORS`'s
/// own keys). Empty (and free) when the env var is unset. File contents are
/// read once and leaked to `'static` so this returns the same reference shape
/// as the compiled-in map.
fn std_behaviors_override_index() -> &'static HashMap<String, &'static str> {
    static INDEX: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let Ok(dir) = std::env::var("ORBITAL_STD_BEHAVIORS_DIR") else {
            return HashMap::new();
        };
        let root = std::path::Path::new(&dir);
        if !root.is_dir() {
            panic!(
                "ORBITAL_STD_BEHAVIORS_DIR is set to '{dir}' but that is not a directory"
            );
        }
        let mut index = HashMap::new();
        collect_orb_overrides(root, &mut index);
        tracing::warn!(
            dir = %dir,
            behaviors_found = index.len(),
            "ORBITAL_STD_BEHAVIORS_DIR override active — local .orb files win over the compiled-in registry"
        );
        index
    })
}

/// Recursively walk `dir` collecting every `<name>.orb` file into `index`,
/// keyed as `std/behaviors/<name>` (the category subdirectory, e.g.
/// `agent/atoms/`, isn't derivable from the canonical path, so this matches
/// by basename alone — same convention `EMBEDDED_STD_BEHAVIORS`'s keys use).
fn collect_orb_overrides(dir: &std::path::Path, index: &mut HashMap<String, &'static str>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
        panic!(
            "ORBITAL_STD_BEHAVIORS_DIR: failed to read directory '{}': {e}",
            dir.display()
        )
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| {
            panic!(
                "ORBITAL_STD_BEHAVIORS_DIR: failed to read entry in '{}': {e}",
                dir.display()
            )
        });
        let path = entry.path();
        if path.is_dir() {
            collect_orb_overrides(&path, index);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("orb") {
            continue;
        }
        let Some(basename) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let content = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "ORBITAL_STD_BEHAVIORS_DIR: failed to read '{}': {e}",
                path.display()
            )
        });
        let leaked: &'static str = Box::leak(content.into_boxed_str());
        index.insert(format!("std/behaviors/{basename}"), leaked);
    }
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
    // `callsitePayload` is the call-site-captured event payload emitted by
    // the inline-trait hoisting phase (phases/inline/trait.rs) when an
    // extracted render block captured @payload; the runtimes resolve it at
    // the composing effect (BindingResolver.CALLSITE_PAYLOAD_PREFIX).
    (
        "effect",
        &[
            "entity",
            "payload",
            "state",
            "now",
            "trait",
            "config",
            "user",
            "callsitePayload",
            // Render-resolved schema sigils — config-default only, sourced
            // from the OrbitalSchema at compile time (the compiler resolves
            // them to concrete values during inline). Allowed in effect
            // context because render-ui is validated here.
            "pages",
            "currentTheme",
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
        let doc: serde_json::Value = serde_json::from_str(operators_json()).unwrap();
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

    // Mobile shells (android/ios) are NO-OP-DISPATCH: they interpret
    // server-returned clientEffects at runtime and never compile S-expr
    // operators, so no canonical category may target them. If this ever
    // changes, the exemption must be revisited (that shell would then need its
    // own operator-completeness gate, like Python's).
    #[test]
    fn no_canonical_category_targets_a_mobile_shell() {
        let doc: serde_json::Value = serde_json::from_str(operators_json()).unwrap();
        let forbidden = ["android", "ios", "kotlin", "swift", "mobile"];
        let mut offenders = Vec::new();
        for (cat, meta) in doc["categories"].as_object().unwrap() {
            if let Some(targets) = meta["target"].as_array() {
                for t in targets.iter().filter_map(|t| t.as_str()) {
                    if forbidden.contains(&t) {
                        offenders.push(format!("{cat} -> {t}"));
                    }
                }
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "Mobile shells are NO-OP-DISPATCH and must not be an operator target, \
             but these categories target one:\n  {}",
            offenders.join("\n  ")
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
            Some(
                &[
                    "entity",
                    "payload",
                    "state",
                    "now",
                    "trait",
                    "config",
                    "user",
                    "callsitePayload",
                    "pages",
                    "currentTheme"
                ][..]
            )
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
    fn embedded_behavior_inflates_and_parses() {
        // std-rpg-game (the previous subject) left the baked map in the P8
        // game-folder retirement; std-browse is a permanent core atom.
        let content = get_std_behavior_content("std/behaviors/std-browse")
            .expect("std-browse must be embedded");
        let parsed: serde_json::Value =
            serde_json::from_str(content).expect("std-browse must be valid JSON");
        assert_eq!(parsed["name"], "std-browse");
    }

    #[test]
    fn unknown_path_returns_none() {
        assert!(get_std_behavior_content("std/behaviors/no-such-thing").is_none());
    }
}
