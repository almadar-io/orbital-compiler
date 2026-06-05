//! Compose Behaviors
//!
//! Main entry point for composing multiple orbital definitions into a single
//! `OrbitalSchema` application. Handles event wiring, layout strategy
//! detection, and page generation.
//!
//! Faithful Rust port of `packages/almadar-core/src/builders/compose-behaviors.ts`.
//! This Rust port must produce byte-identical output to the TypeScript
//! implementation for the same input. To guarantee that, the public entry
//! point operates on `serde_json::Value` directly so that field ordering and
//! optional-field presence exactly mirror what JavaScript produces. The
//! strongly-typed `Orbital` schema is intentionally bypassed here.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map as JsonMap, Value};
use thiserror::Error;

use super::event_wiring::EventWiringEntry;
use super::layout_strategy::LayoutStrategy;

// ============================================================================
// Errors
// ============================================================================

/// Errors emitted by the compose pass.
#[derive(Debug, Error)]
pub enum ComposeError {
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Compose call missing required argument: {0}")]
    MissingArgument(String),

    #[error("Compose call argument was the wrong shape: {0}")]
    InvalidArgument(String),

    #[error("Unknown composition operator: {0}")]
    UnknownOperator(String),
}

// ============================================================================
// Types
// ============================================================================

/// Strongly-typed mirror of the TypeScript `ComposeBehaviorsInput` shape.
/// Used by inline tests and the parity script for ergonomic construction.
/// The actual compose entry point accepts a `serde_json::Value` so that
/// arbitrary input fields pass through untouched for byte-identical parity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeBehaviorsInput {
    /// Application name
    #[serde(rename = "appName")]
    pub app_name: String,

    /// Orbital definitions to compose. Stored as JSON values to preserve any
    /// extra fields the caller may have set.
    pub orbitals: Vec<Value>,

    /// Layout strategy override, or `auto` to detect.
    #[serde(rename = "layoutStrategy", skip_serializing_if = "Option::is_none")]
    pub layout_strategy: Option<String>,

    /// Cross-orbital event wiring
    #[serde(rename = "eventWiring", skip_serializing_if = "Option::is_none")]
    pub event_wiring: Option<Vec<EventWiringEntry>>,

    /// Optional entity name mappings (original -> renamed). Currently unused
    /// but preserved for parity with the TS shape.
    #[serde(
        rename = "entityMappings",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub entity_mappings: Option<std::collections::HashMap<String, String>>,
}

/// Layout metadata returned by `compose_behaviors`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutMetadata {
    pub strategy: LayoutStrategy,
    #[serde(rename = "pageCount")]
    pub page_count: usize,
}

/// Wiring metadata returned by `compose_behaviors`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WiringMetadata {
    pub connections: usize,
}

/// Result of composing behaviors. The `schema` field is a raw JSON value to
/// preserve byte-identical output ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeBehaviorsResult {
    /// The composed `OrbitalSchema`
    pub schema: Value,
    /// Layout metadata
    pub layout: LayoutMetadata,
    /// Wiring metadata
    pub wiring: WiringMetadata,
}

// ============================================================================
// Page Generation
// ============================================================================

/// Convert a name to kebab-case.
///
/// Mirrors the TS implementation:
/// ```text
/// name.replace(/([a-z])([A-Z])/g, '$1-$2').replace(/[\s_]+/g, '-').toLowerCase();
/// ```
fn to_kebab_case(name: &str) -> String {
    // Step 1: Insert '-' between a lowercase letter and an uppercase letter.
    let mut step1 = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (i, ch) in chars.iter().enumerate() {
        if i > 0 && chars[i - 1].is_ascii_lowercase() && ch.is_ascii_uppercase() {
            step1.push('-');
        }
        step1.push(*ch);
    }

    // Step 2: Replace runs of whitespace or underscores with a single '-'.
    let mut step2 = String::with_capacity(step1.len());
    let mut in_run = false;
    for ch in step1.chars() {
        if ch == '_' || ch.is_whitespace() {
            if !in_run {
                step2.push('-');
                in_run = true;
            }
        } else {
            step2.push(ch);
            in_run = false;
        }
    }

    // Step 3: Lowercase.
    step2.to_lowercase()
}

/// Extract entity name from an orbital JSON value. Handles both inline
/// entities (object with `name`) and string references (e.g. `"Alias.entity"`).
/// Returns `None` if the entity is missing or has no usable name.
fn get_entity_name(orbital: Option<&Value>) -> Option<String> {
    let orbital = orbital?;
    let entity = orbital.get("entity")?;
    if let Some(s) = entity.as_str() {
        return Some(s.replace(".entity", ""));
    }
    if let Some(obj) = entity.as_object() {
        if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
            return Some(name.to_string());
        }
    }
    None
}

/// Get the orbital name (`orbital.name`), defaulting to "Main".
fn get_orbital_name(orbital: Option<&Value>) -> String {
    orbital
        .and_then(|o| o.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Main".to_string())
}

/// Build a single page object as a JSON map. Mirrors the literal field
/// ordering of the TypeScript object literals exactly:
/// `{ name, path, isInitial, primaryEntity }`. When `primary_entity` is
/// `None`, the key is omitted entirely (matching `JSON.stringify`'s behaviour
/// of dropping `undefined` values).
fn build_page(
    name: String,
    path: String,
    is_initial: bool,
    primary_entity: Option<String>,
) -> Value {
    let mut map = JsonMap::new();
    map.insert("name".to_string(), Value::String(name));
    map.insert("path".to_string(), Value::String(path));
    map.insert("isInitial".to_string(), Value::Bool(is_initial));
    if let Some(s) = primary_entity {
        map.insert("primaryEntity".to_string(), Value::String(s));
    }
    Value::Object(map)
}

/// Build the special dashboard page. TS literal:
/// `{ name: 'DashboardPage', path: '/', viewType: 'dashboard', isInitial: true }`.
fn build_dashboard_page() -> Value {
    let mut map = JsonMap::new();
    map.insert(
        "name".to_string(),
        Value::String("DashboardPage".to_string()),
    );
    map.insert("path".to_string(), Value::String("/".to_string()));
    map.insert(
        "viewType".to_string(),
        Value::String("dashboard".to_string()),
    );
    map.insert("isInitial".to_string(), Value::Bool(true));
    Value::Object(map)
}

/// Generate pages for each orbital based on the layout strategy.
fn generate_pages(orbitals: &[Value], strategy: LayoutStrategy) -> Vec<Value> {
    match strategy {
        LayoutStrategy::Single => {
            let orbital = orbitals.first();
            let name = get_orbital_name(orbital);
            let primary_entity = get_entity_name(orbital);
            vec![build_page(
                format!("{}Page", name),
                "/".to_string(),
                true,
                primary_entity,
            )]
        }
        LayoutStrategy::Dashboard => {
            vec![build_dashboard_page()]
        }
        LayoutStrategy::Sidebar | LayoutStrategy::Tabs | LayoutStrategy::WizardFlow => orbitals
            .iter()
            .enumerate()
            .map(|(index, orbital)| {
                let orbital_name = orbital
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("Main")
                    .to_string();
                let primary_entity = get_entity_name(Some(orbital));
                build_page(
                    format!("{}Page", orbital_name),
                    if index == 0 {
                        "/".to_string()
                    } else {
                        format!("/{}", to_kebab_case(&orbital_name))
                    },
                    index == 0,
                    primary_entity,
                )
            })
            .collect(),
    }
}

// ============================================================================
// Layout Strategy Resolution
// ============================================================================

fn parse_layout_strategy_string(s: &str) -> Result<LayoutStrategy, ComposeError> {
    match s {
        "single" => Ok(LayoutStrategy::Single),
        "tabs" => Ok(LayoutStrategy::Tabs),
        "sidebar" => Ok(LayoutStrategy::Sidebar),
        "dashboard" => Ok(LayoutStrategy::Dashboard),
        "wizard-flow" => Ok(LayoutStrategy::WizardFlow),
        other => Err(ComposeError::InvalidArgument(format!(
            "unknown layout strategy: {}",
            other
        ))),
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Compose multiple orbital definitions into a single application schema.
///
/// Steps (matching the TS implementation):
/// 1. Apply event wiring (adds emits/listens to traits)
/// 2. Detect or use provided layout strategy
/// 3. Generate pages based on the strategy
/// 4. Assign generated pages to orbitals (merge, don't replace existing)
/// 5. Build the final `OrbitalSchema`
pub fn compose_behaviors(
    input: ComposeBehaviorsInput,
) -> Result<ComposeBehaviorsResult, ComposeError> {
    let ComposeBehaviorsInput {
        app_name,
        orbitals: raw_orbitals,
        layout_strategy: strategy_input,
        event_wiring,
        entity_mappings: _,
    } = input;

    // Step 1: Apply event wiring.
    //
    // The TS code deep-clones via JSON when wiring is provided; we mirror that
    // behaviour by always cloning into a fresh mutable Vec when wiring is
    // non-empty.
    let mut wired_orbitals: Vec<Value> = if let Some(ref wiring) = event_wiring {
        if !wiring.is_empty() {
            let json = serde_json::to_string(&raw_orbitals)?;
            let mut cloned: Vec<Value> = serde_json::from_str(&json)?;
            apply_event_wiring_value(&mut cloned, wiring);
            cloned
        } else {
            raw_orbitals
        }
    } else {
        raw_orbitals
    };

    // Step 2: Determine layout strategy.
    let strategy: LayoutStrategy = match strategy_input.as_deref() {
        None | Some("auto") => super::layout_strategy::detect_layout_strategy_from_values(
            &wired_orbitals,
            event_wiring.as_deref(),
        ),
        Some(other) => parse_layout_strategy_string(other)?,
    };

    // Step 3: Generate pages.
    let pages = generate_pages(&wired_orbitals, strategy);

    // Step 4: Assign generated pages to orbitals (merge, don't replace existing).
    for (index, orbital) in wired_orbitals.iter_mut().enumerate() {
        let obj = match orbital.as_object_mut() {
            Some(o) => o,
            None => continue,
        };

        // If the orbital already has non-empty pages, keep them.
        let already_has_pages = obj
            .get("pages")
            .and_then(|p| p.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);

        if already_has_pages {
            continue;
        }

        let page = match strategy {
            LayoutStrategy::Dashboard | LayoutStrategy::Single => pages.first(),
            _ => pages.get(index),
        };

        let pages_value = match page {
            Some(p) => Value::Array(vec![p.clone()]),
            None => Value::Array(Vec::new()),
        };

        // Match TS spread semantics: if the original object had a `pages`
        // key (even an empty array), assigning to it preserves its position.
        // If it did not, the new key is appended at the end. `serde_json::Map`
        // preserves insertion order, so an `insert` on a missing key appends
        // it which exactly matches the TS spread behaviour.
        obj.insert("pages".to_string(), pages_value);
    }

    // Step 5: Build the schema as a JSON object literal in the same field
    // order the TS code uses: `{ name, version, orbitals }`.
    let mut schema_map = JsonMap::new();
    schema_map.insert("name".to_string(), Value::String(app_name));
    schema_map.insert("version".to_string(), Value::String("1.0.0".to_string()));
    schema_map.insert("orbitals".to_string(), Value::Array(wired_orbitals));
    let schema = Value::Object(schema_map);

    let page_count = pages.len();
    let connection_count = event_wiring.as_ref().map(|w| w.len()).unwrap_or(0);

    Ok(ComposeBehaviorsResult {
        schema,
        layout: LayoutMetadata {
            strategy,
            page_count,
        },
        wiring: WiringMetadata {
            connections: connection_count,
        },
    })
}

// ============================================================================
// Value-based event wiring
// ============================================================================

/// Apply event wiring to orbital JSON values. Mirrors `apply_event_wiring`
/// from the typed module but operates on `serde_json::Value` so we never lose
/// untyped input fields.
fn apply_event_wiring_value(orbitals: &mut [Value], wiring: &[EventWiringEntry]) {
    for entry in wiring {
        // Wire the source: add emit.
        if let Some(source_trait) = find_inline_trait_mut(orbitals, &entry.from) {
            ensure_array_field(source_trait, "emits");
            let emits = source_trait
                .get_mut("emits")
                .and_then(|v| v.as_array_mut())
                .expect("ensured");
            let already = emits
                .iter()
                .any(|e| e.get("event").and_then(|v| v.as_str()) == Some(entry.event.as_str()));
            if !already {
                emits.push(json!({
                    "event": entry.event,
                    "scope": "external",
                }));
            }
        }

        // Wire the target: add listen.
        if let Some(target_trait) = find_inline_trait_mut(orbitals, &entry.to) {
            ensure_array_field(target_trait, "listens");
            let listens = target_trait
                .get_mut("listens")
                .and_then(|v| v.as_array_mut())
                .expect("ensured");
            let already = listens.iter().any(|l| {
                l.get("event").and_then(|v| v.as_str()) == Some(entry.event.as_str())
                    && l.get("triggers").and_then(|v| v.as_str()) == Some(entry.triggers.as_str())
            });
            if !already {
                listens.push(json!({
                    "event": entry.event,
                    "triggers": entry.triggers,
                    "scope": "external",
                }));
            }
        }
    }
}

/// Ensure a JSON object has an array field. If absent or non-array, replaces
/// with an empty array. Mirrors `if (!sourceTrait.emits) sourceTrait.emits = []`.
fn ensure_array_field(obj: &mut JsonMap<String, Value>, key: &str) {
    let needs = !matches!(obj.get(key), Some(v) if v.is_array());
    if needs {
        obj.insert(key.to_string(), Value::Array(Vec::new()));
    }
}

/// Find an inline trait by name across all orbitals. An "inline trait" is an
/// element of `orbital.traits` that is an object with a `name` field but no
/// `ref` field, matching the TS `isInlineTrait` predicate.
fn find_inline_trait_mut<'a>(
    orbitals: &'a mut [Value],
    name: &str,
) -> Option<&'a mut JsonMap<String, Value>> {
    for orbital in orbitals.iter_mut() {
        let traits = match orbital
            .as_object_mut()
            .and_then(|o| o.get_mut("traits"))
            .and_then(|t| t.as_array_mut())
        {
            Some(t) => t,
            None => continue,
        };
        for trait_ref in traits.iter_mut() {
            if let Some(obj) = trait_ref.as_object_mut() {
                let is_inline = obj.contains_key("name") && !obj.contains_key("ref");
                if is_inline && obj.get("name").and_then(|n| n.as_str()) == Some(name) {
                    return Some(obj);
                }
            }
        }
    }
    None
}

// `find_inline_trait_mut` returns a single mutable reference. We rely on the
// borrow checker — the function returns immediately on the first match, so
// the loop above is sound. (No multi-borrow issues because the &mut is
// returned to the caller and used before the next call.)
//
// Note: in Rust, returning a mutable reference from a function with such a
// loop traditionally trips the borrow checker without NLL Polonius, but the
// 2021 edition + Rust 1.78+ NLL handles it. If a future toolchain regresses,
// rewrite using indices.
