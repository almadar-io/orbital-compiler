//! Embedded JSON Registries
//!
//! Source-of-truth registries embedded directly into the orbital-compiler binary
//! at build time via `include_str!`. After Phase 3 of the Lolo v3 plan, the validator
//! reads operators, patterns, and behaviors from these embedded JSON sources rather
//! than hand-maintained Rust constants. Adding a new operator to `operators.json`
//! plus running `cargo build` is enough to update the validator.
//!
//! See `docs/Lolo_Language_Spec_v3_PLAN.md` Section 8 for context.

use orbital_core::schema::PayloadTypeWhen;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    /// a trailing options object with `emit.success` populated.
    ///
    /// ⚠️ **Currently set on NO operator**, so
    /// `effect::emit_declaration::validate_required_emit_success` returns
    /// immediately for every call and the pass is inert. This comment used to
    /// claim it was "set on fetch / persist / call-service / ref in
    /// canonical-operators.json" — it never was (verified 2026-08-30: zero
    /// occurrences of `requiresEmitSuccess` in that file).
    ///
    /// **Measured and REJECTED 2026-08-30 — do not enable without new
    /// evidence.** Switched on for fetch/persist/ref/call-service and scanned
    /// the whole corpus: **1,413 findings across 67 of 1,200 files.** Sampling
    /// them showed the flag's premise is simply false for this language. A
    /// bare `(fetch AppSearchTarget {})` — no emit at all, as in
    /// `std-app-search` — is the CORRECT idiom: the fetch loads the store and
    /// the reactive `@entity`-bound render picks it up, so there is no event to
    /// route and nothing is broken. Making that a hard error would force ~1,400
    /// ceremonial events into the corpus to satisfy a rule the language does
    /// not have.
    ///
    /// Kept (rather than deleted) because the field, the pass, and the registry
    /// slot are all still wired and someone may want the constraint for a
    /// narrower operator set later — but it is dormant on purpose, not by
    /// oversight. Tracked in `docs/Almadar_Std_Gaps.md` (B7).
    ///
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
    /// Execution site of an effect operator (`client`/`server`/`any`); absent on pure ones. Pinned to `orbital_core::effects::runs_on` by `tests/runs_on_drift.rs`.
    #[serde(rename = "runsOn", default)]
    pub runs_on: Option<String>,
    /// Argument naming a data effect's target entity; the effect runs where that entity's rows live. Pinned to `orbital_core::effects::ENTITY_ARGS` by `tests/runs_on_drift.rs`.
    #[serde(rename = "entityArgPosition", default)]
    pub entity_arg_position: Option<usize>,
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
    /// An array param's element shape: its members' declared types.
    #[serde(default)]
    pub items: Option<EmbeddedParamItems>,
}

/// The element shape of an array service param.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbeddedParamItems {
    #[serde(default)]
    pub properties: HashMap<String, EmbeddedParamMember>,
}

/// One member of an array param's element.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbeddedParamMember {
    #[serde(default, rename = "type")]
    pub member_type: String,
}

/// The `@almadar/core` `EventAddress` contract type: a `listens`-addressed
/// event value, checked against the program's declared external inputs.
pub const EVENT_ADDRESS_TYPE: &str = "EventAddress";

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
    /// Service name → the runtimes its implementation declares (`node`, `browser`).
    /// A service without the field declares none.
    pub runtimes: HashMap<String, Vec<String>>,
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
        #[serde(default)]
        runtimes: Vec<String>,
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
        data.params.insert(name.clone(), params);
        data.runtimes.insert(name, entry.runtimes);
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

#[cfg(test)]
mod service_runtimes_tests {
    use super::parse_service_registry_data;

    #[test]
    fn reads_each_services_declared_runtimes_in_the_same_parse_as_its_actions() {
        let raw = r#"{"services":{"page":{"runtimes":["browser"],"actions":[{"name":"watch"}]},"llm":{"runtimes":["node","browser"]}}}"#;
        let data = parse_service_registry_data(raw).unwrap();
        assert_eq!(data.runtimes["page"], vec!["browser".to_string()]);
        assert_eq!(data.runtimes["llm"], vec!["node".to_string(), "browser".to_string()]);
        assert!(data.actions["page"].contains("watch"));
    }

    #[test]
    fn edge_a_service_without_the_field_declares_no_runtime() {
        let data = parse_service_registry_data(r#"{"services":{"old":{}}}"#).unwrap();
        assert!(data.runtimes["old"].is_empty());
    }
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
    /// Declared knob-conditional type list (component JSDoc's
    /// `@payloadTypeWhen`, pattern-synced into the registry as `typeWhen`).
    /// The render-ui binding check (`phases/validation/render_ui/binding.rs`)
    /// evaluates this against the consuming trait's own resolved config —
    /// the SAME `resolve_payload_type_when` the inline narrowing pass uses.
    #[serde(rename = "typeWhen", default)]
    pub type_when: Option<Vec<PayloadTypeWhen>>,
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
    /// carry an event-name field), `"event-map"` (dynamic-key object prop
    /// whose VALUES — not the prop itself — are declared event keys, e.g. a
    /// d-pad's direction→event map or a canvas's keycode→event map; the
    /// Record/index-signature value type resolved to `EventKey`),
    /// `"callback"` (React callback prop bound to an event name at the
    /// schema-author site — NEVER set for a render-prop that returns a
    /// value, see `renderCallback` below).
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
    /// `@notification`: the component reports a change it already applied (hover, close
    /// finished, resize), so a final program may leave the event unhandled (G-ORB-187).
    #[serde(default)]
    pub notification: bool,
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
    /// `@payloadFor <sibling>` pairing read off THIS struct member (not an
    /// enclosing prop — a struct member declared inside an array-item
    /// interface, e.g. `FloatingToolbarItem.actionPayload`, has no parent
    /// `ExtractedProp` to surface it on). Names a sibling key in this SAME
    /// `properties` map, itself carrying `kind: "event"`. The render-ui
    /// binding check (`check_callback_arg_schema`) uses this pair to derive
    /// the correct expected type (`event` / `@payload(.sibling)`) instead of
    /// falling back to `types.first()` ("string"/"object"), which produced a
    /// false `ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH` against the trait's
    /// honestly-declared dependent type.
    #[serde(rename = "payloadFor", default)]
    pub payload_for: Option<String>,
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
    /// `form` for a pattern that collects field values and submits them (`form`, `form-section`).
    #[serde(rename = "fieldsContract", default)]
    pub fields_contract: Option<String>,
}

static PATTERNS: OnceLock<HashMap<String, EmbeddedPattern>> = OnceLock::new();

/// All patterns keyed by pattern type (e.g. `"table-view"`, `"button"`).
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
// 3.3 — Known-behavior index (every behavior package is installed, not baked)
// ============================================================================
//
// `@almadar/std` and `@almadar-io/behaviors` (and any other behavior package)
// are installed, not baked — names are looked up from each discovered
// package's own `orb.index` JSON, never embedded here (Phase 2 of
// Almadar_Behavior_Packages.md removed the last embedded behavior data; the
// resolution order the loader documents — Phase 1 ruling 3 — is unchanged).

/// Returns true if `name` is a known behavior: a key in any installed
/// behavior package's `orb.index`. This call site has no `LoaderConfig`/
/// `packages` to reuse, so it re-discovers from `current_dir()` / the user
/// store, same as `CompilerOptions::from_env`.
pub fn is_known_behavior(name: &str) -> bool {
    installed_behavior_index_contains(name)
}

/// True when `name` is a key in any installed behavior package's `orb.index`
/// JSON file (the on-disk twin of the embedded std map). Package discovery +
/// each index file's names are memoized per index path.
fn installed_behavior_index_contains(name: &str) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    let user_store =
        orbital_core::behavior_packages::user_store_dir().unwrap_or_else(|| cwd.clone());
    let Ok(packages) = orbital_core::behavior_packages::discover(&cwd, &user_store) else {
        return false;
    };
    packages
        .iter()
        .any(|pkg| package_index_names(&pkg.index).contains(name))
}

/// Behavior names declared in a package's `orb.index` JSON file (the same
/// `{"behaviors": {<name>: {...}}}` shape as the baked std registry).
fn package_index_names(index_path: &Path) -> HashSet<String> {
    use std::sync::Mutex;
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, HashSet<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(names) = cache
        .lock()
        .expect("package index name cache poisoned")
        .get(index_path)
    {
        return names.clone();
    }
    let names: HashSet<String> = std::fs::read_to_string(index_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("behaviors").and_then(|b| b.as_object().cloned()))
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();
    cache
        .lock()
        .expect("package index name cache poisoned")
        .insert(index_path.to_path_buf(), names.clone());
    names
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
        &[
            "entity",
            "payload",
            "state",
            "now",
            "config",
            "user",
            // The dispatch roots (Runtime Spec Clause 5.5); never in ticks, which have no delivery.
            "event",
            "prevEvents",
            "prevStates",
            "fromState",
            "toState",
        ],
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
            "locale",
            "event",
            "prevEvents",
            "prevStates",
            "fromState",
            "toState",
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
    fn fetch_operator_produces_the_data_wrapper_it_actually_emits() {
        // `effect.produces` is defined as "the payload the runtime puts on the
        // bus when emit.success fires". For fetch that payload is the WRAPPER
        // `{ data, totalCount }` — verified in both executors that emit onto
        // the bus: EffectExecutor.ts:851-855 and
        // orbital-shell-typescript codegen/effect/server.rs:177-185.
        //
        // It previously declared a bare `entity | [entity]`, i.e. the wrapper
        // was real but undeclared — the mirror image of the persist bug where a
        // wrapper was declared but unreal. `data` keeps the entity/array union
        // inside the wrapper so a by-id vs collection call site still narrows.
        let fetch = get_operator("fetch").expect("fetch must exist (V2 Phase 0)");
        let effect = fetch
            .effect
            .as_ref()
            .expect("fetch must carry effect metadata");
        assert_eq!(effect.kind, "fetch");
        let produces = effect.produces.as_ref().expect("fetch produces present");
        let OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. }) = produces else {
            panic!("fetch produces should be the {{data,totalCount}} Object, got {produces:?}");
        };
        assert!(fields.contains_key("data"), "fetch emits a `data` key");
        assert!(
            fields.contains_key("totalCount"),
            "fetch emits `totalCount` alongside `data`"
        );
        match fields.get("data").expect("data field") {
            OperatorTypeRef::Tagged(OperatorTypeRefKind::Union { of }) => {
                assert_eq!(of.len(), 2, "data is entity | Array<entity>");
            }
            other => panic!("fetch produces.data should stay a Union, got {other:?}"),
        }
    }

    #[test]
    fn ref_operator_produces_the_same_wrapper_as_fetch() {
        // `ref` emits the identical wrapper — and fires it on `on_change`, not
        // `success` (EffectExecutor.ts:919-923, codegen server.rs:248-253).
        let r = get_operator("ref").expect("ref must exist");
        let produces = r
            .effect
            .as_ref()
            .expect("ref effect")
            .produces
            .as_ref()
            .expect("ref produces");
        let OperatorTypeRef::Tagged(OperatorTypeRefKind::Object { fields, .. }) = produces else {
            panic!("ref produces should be an Object wrapper, got {produces:?}");
        };
        assert!(fields.contains_key("data"));
        assert!(fields.contains_key("totalCount"));
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
        assert!(is_known_pattern("table-view"));
        assert!(is_known_pattern("button"));
        assert!(!is_known_pattern("definitely-not-a-pattern-xyz"));
        let table = get_pattern("table-view").unwrap();
        assert_eq!(table.category, "display");
        assert!(table.props_schema.contains_key("entity"));
    }

    #[test]
    fn sigil_context_rules_present() {
        assert_eq!(
            allowed_sigils_for_context("guard"),
            Some(
                &[
                    "entity",
                    "payload",
                    "state",
                    "now",
                    "config",
                    "user",
                    "event",
                    "prevEvents",
                    "prevStates",
                    "fromState",
                    "toState"
                ][..]
            )
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
                    "currentTheme",
                    "locale",
                    "event",
                    "prevEvents",
                    "prevStates",
                    "fromState",
                    "toState"
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
