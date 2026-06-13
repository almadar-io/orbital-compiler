//! Pattern registry — the single embed of the pattern→component / event-contract
//! / props-schema data the codegen shells consume.
//!
//! The three JSON files used to be baked byte-identically into EACH shell's
//! `src/` (orbital-shell-typescript, orbital-shell-python) plus a third copy of
//! `component-mapping.json` under `phases/validation/sync/`. pattern-sync wrote
//! them to 6 destinations, so a partial sync drifted codegen and the validator
//! against each other silently. They now live ONCE here and every consumer
//! imports this module (the shells re-export it; the validator reads the
//! component names from `component_names()`). `registry.json` is the same data
//! as `baked/patterns-registry.json` (same pattern-sync source) — reused, not
//! re-embedded. Enforced by the workspace `gate_grepwall` include_str count.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const COMPONENT_MAPPING_JSON: &str = include_str!("baked/component-mapping.json");
const EVENT_CONTRACTS_JSON: &str = include_str!("baked/event-contracts.json");
// Same data the validator parses as `embedded::patterns()`; reused here for the
// codegen-facing typed view instead of a separate `registry.json` copy.
const REGISTRY_JSON: &str = include_str!("baked/patterns-registry.json");

/// Component mapping for a pattern.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentMapping {
    pub component: String,
    pub import_path: String,
    pub category: String,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default)]
    pub replaced_by: Option<String>,
    /// Client identifier (e.g. "trait-wars") for game-specific patterns.
    #[serde(default)]
    pub client: Option<String>,
}

/// One field of an emit/listen payload schema mirrored into the registry by
/// pattern-sync from the component's `EventEmit<P>` / `EventListen<P>` brand.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryPayloadField {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
}

/// One positional parameter of a React callback prop.
#[derive(Debug, Clone, Deserialize)]
pub struct CallbackArg {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub arg_type: String,
}

/// Recursive type schema for array elements and nested objects. Mirrors
/// `PropTypeSchema` in `tools/almadar-pattern-sync/types.ts`. Lets codegen
/// branch on element shape (string vs object) without name-based heuristics.
#[derive(Debug, Clone, Deserialize)]
pub struct PropTypeSchema {
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(rename = "enumValues", default)]
    pub enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub items: Option<Box<PropTypeSchema>>,
    #[serde(default)]
    pub properties: Option<HashMap<String, PropTypeSchema>>,
    #[serde(default)]
    pub required: Option<Vec<String>>,
}

/// Prop type definition.
#[derive(Debug, Clone, Deserialize)]
pub struct PropSchema {
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub description: String,
    /// F1 marker — see `embedded::EmbeddedPatternProp::kind`.
    #[serde(default)]
    pub kind: Option<String>,
    /// For `kind: "callback"`: positional parameter list. C2 uses the names to
    /// build a named-arg → object-payload wrapper at the dispatch site.
    #[serde(rename = "callbackArgs", default)]
    pub callback_args: Option<Vec<CallbackArg>>,
    /// For `kind: "event-ref"` from an `EventEmit<P>` brand.
    #[serde(rename = "emitPayloadSchema", default)]
    pub emit_payload_schema: Option<Vec<RegistryPayloadField>>,
    /// Element shape for array-typed props (mirrors JSON Schema `items`).
    #[serde(default)]
    pub items: Option<PropTypeSchema>,
    /// Per-key schemas for object-typed props sourced from a declared interface.
    #[serde(default)]
    pub properties: Option<HashMap<String, PropTypeSchema>>,
    /// Required keys for object-typed props.
    #[serde(rename = "propertyRequired", default)]
    pub property_required: Option<Vec<String>>,
    /// For entity-data props: `"record"` (single entity) or `"collection"`
    /// (array). Drives the bind-site cast cardinality.
    #[serde(default)]
    pub cardinality: Option<String>,
    /// For entity-data props declared as `EntityWith<'a' | 'b'>`: the field
    /// names the component requires.
    #[serde(rename = "requiredFields", default)]
    pub required_fields: Option<Vec<String>>,
}

/// Pattern definition from registry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryPattern {
    #[serde(rename = "type")]
    pub pattern_type: String,
    pub category: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub props_schema: HashMap<String, PropSchema>,
}

/// Event payload schema.
#[derive(Debug, Clone, Deserialize)]
pub struct PayloadSchema {
    #[serde(rename = "type")]
    pub payload_type: String,
    #[serde(default)]
    pub shape: Option<HashMap<String, String>>,
}

/// Event emitted by a component.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryEvent {
    pub event: String,
    pub trigger: String,
    pub payload: PayloadSchema,
    #[serde(default)]
    pub optional: bool,
}

/// Event contract for a pattern.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventContract {
    pub emits: Vec<RegistryEvent>,
    pub requires: Vec<String>,
    #[serde(default)]
    pub entity_aware: bool,
    #[serde(default)]
    pub config_driven: bool,
}

#[derive(Debug, Deserialize)]
struct ComponentMappingFile {
    mappings: HashMap<String, ComponentMapping>,
}

#[derive(Debug, Deserialize)]
struct EventContractsFile {
    contracts: HashMap<String, EventContract>,
}

#[derive(Debug, Deserialize)]
struct PatternRegistryFile {
    patterns: HashMap<String, RegistryPattern>,
}

fn component_mappings() -> &'static HashMap<String, ComponentMapping> {
    static PARSED: OnceLock<HashMap<String, ComponentMapping>> = OnceLock::new();
    PARSED.get_or_init(|| {
        let file: ComponentMappingFile =
            serde_json::from_str(COMPONENT_MAPPING_JSON).expect("parse baked component-mapping");
        file.mappings
    })
}

fn event_contracts() -> &'static HashMap<String, EventContract> {
    static PARSED: OnceLock<HashMap<String, EventContract>> = OnceLock::new();
    PARSED.get_or_init(|| {
        let file: EventContractsFile =
            serde_json::from_str(EVENT_CONTRACTS_JSON).expect("parse baked event-contracts");
        file.contracts
    })
}

fn pattern_definitions() -> &'static HashMap<String, RegistryPattern> {
    static PARSED: OnceLock<HashMap<String, RegistryPattern>> = OnceLock::new();
    PARSED.get_or_init(|| {
        let file: PatternRegistryFile =
            serde_json::from_str(REGISTRY_JSON).expect("parse baked patterns registry");
        file.patterns
    })
}

/// Get component mapping for a pattern type.
pub fn get_component_mapping(pattern_type: &str) -> Option<&'static ComponentMapping> {
    component_mappings().get(pattern_type)
}

/// Get event contract for a pattern type.
pub fn get_event_contract(pattern_type: &str) -> Option<&'static EventContract> {
    event_contracts().get(pattern_type)
}

/// Get pattern definition (includes props schema).
pub fn get_pattern_definition(pattern_type: &str) -> Option<&'static RegistryPattern> {
    pattern_definitions().get(pattern_type)
}

/// Check if a pattern type is registered.
pub fn is_registered_pattern(pattern_type: &str) -> bool {
    component_mappings().contains_key(pattern_type)
}

/// Get all registered pattern types.
pub fn get_all_patterns() -> Vec<&'static str> {
    component_mappings().keys().map(|s| s.as_str()).collect()
}

/// Get the props schema for a pattern type.
pub fn get_props_schema(pattern_type: &str) -> Option<&'static HashMap<String, PropSchema>> {
    pattern_definitions()
        .get(pattern_type)
        .map(|p| &p.props_schema)
}

/// All component identifiers in the mapping (e.g. `StatCard`, `EntityTable`).
/// The validator uses this to reserve names that would collide with generated
/// entity types — replacing its former private read of `component-mapping.json`.
pub fn component_names() -> impl Iterator<Item = &'static str> {
    component_mappings().values().map(|m| m.component.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_component_mappings() {
        assert!(get_component_mapping("stat-card").is_some());
        assert!(get_component_mapping("entity-table").is_some());
        assert!(get_component_mapping("form-section").is_some());
    }

    #[test]
    fn loads_event_contracts() {
        let form = get_event_contract("form").expect("form contract");
        assert!(form.requires.contains(&"SAVE".to_string()));
    }

    #[test]
    fn stat_card_mapping() {
        let mapping = get_component_mapping("stat-card").unwrap();
        assert_eq!(mapping.component, "StatCard");
        assert_eq!(mapping.category, "display");
    }

    #[test]
    fn loads_pattern_definitions() {
        let stats = get_pattern_definition("stat-card").unwrap();
        assert_eq!(stats.category, "display");
        assert!(stats.props_schema.contains_key("label"));
        assert!(stats.props_schema.contains_key("value"));
    }

    #[test]
    fn props_schema_and_component_names() {
        let schema = get_props_schema("entity-table").unwrap();
        assert!(schema.contains_key("columns"));
        assert!(schema.contains_key("entity"));
        assert!(component_names().any(|n| n == "StatCard"));
    }
}
