//! Phase F.2: cross-emitter declared-payload consensus check.
//!
//! When two or more traits declare the same external event name in their
//! `emits` arrays, every emitter must declare the same payload field list.
//! Otherwise consumers that listen on the event will see one shape from one
//! emitter and a different shape from another, the runtime cannot
//! reconcile them, and downstream listeners drop fields silently.
//!
//! This pass emits a hard-error `CrossEmitterDeclaredPayloadMismatch` for
//! every event whose payload-field set is not identical across all
//! declaring traits. The signature compared is the tuple
//! `(name, field_type, required-as-bool)`; description is ignored. Two
//! emitters with no declared payload are considered consistent (both are
//! "empty").

use std::collections::{BTreeSet, HashMap};

use orbital_core_base::schema::{
    EntityDefinition, FieldDefinition, FieldType, ListenSource, OrbitalSchema, PayloadField,
    SExpression, TraitDefinition, TraitReference,
};

use super::error::{ErrorCode, ValidationError};
use super::render_ui::is_render_ui_effect;
use crate::embedded::get_pattern;

/// One emitter of an event: `(trait_path, signature_set, owner_orbital, owner_trait)`.
/// The signature set is `(field_name, field_type, required)` triples.
type EmitterEntry = (String, BTreeSet<(String, String, bool)>, String, String);
use super::ValidationContext;

/// One listener entry in the reverse index. Records the listening trait's
/// location (orbital + trait names + per-trait IR indices for diagnostic
/// paths) and how the listener scopes its source. Used by L2.6 to decide
/// whether a cross-emitter payload mismatch is actually load-bearing
/// (gap #31's narrowing).
#[derive(Debug, Clone)]
pub struct ListenerEntry {
    pub orbital_index: usize,
    pub orbital_name: String,
    pub trait_index: usize,
    pub trait_name: String,
    /// Source-scope from the listener's `listens { ... }` block. `None`
    /// when the listener entry is a local payload-only declaration with
    /// no bus subscription (the schema permits this for documentation).
    pub source: Option<ListenSource>,
}

/// Reverse index: event name → list of trait entries that listen for it.
/// Built in one pass over every inline trait's `listens[]` block, mirrors
/// the forward `emitters` map below. Independent of payload-shape
/// validation — emit/listen contract checks reuse this structure.
pub type ListenIndex = HashMap<String, Vec<ListenerEntry>>;

/// Build the reverse listen-graph index across every inline trait in
/// every orbital. The result keys on the listened event name (the bare
/// event, NOT the qualified `Source.EVENT` form — the source descriptor
/// lives on the `ListenSource` field).
///
/// Rules / future rules consuming this:
///
/// - **L2.6** (`ORB_X_CROSS_EMITTER_DECLARED_PAYLOAD_MISMATCH` narrowing,
///   gap #31): when multiple emitters declare an event with diverging
///   payloads, only fire when at least one listener subscribes from an
///   orbital different from one of the emitters' AND the listener
///   doesn't pin the source orbital (so it could receive both payload
///   shapes through the same subscription).
/// - **L2.7** (`ORB_EMIT_DECLARED_BUT_UNFIRED`, gap #2 defense): orphan
///   declared emits — every entry in `emits` should map to at least one
///   transition's effect, and every external emit ideally has at least
///   one listener somewhere in the schema. The reverse index gives the
///   per-event listener count cheaply.
pub fn build_listen_index(schema: &OrbitalSchema) -> ListenIndex {
    let mut index: ListenIndex = HashMap::new();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let Some(listens) = trait_def.listens.as_deref() else {
                continue;
            };
            for listen in listens {
                let entry = ListenerEntry {
                    orbital_index: oi,
                    orbital_name: orbital.name.clone(),
                    trait_index: ti,
                    trait_name: trait_def.name.clone(),
                    source: listen.source.clone(),
                };
                index.entry(listen.event.clone()).or_default().push(entry);
            }
        }
    }
    index
}

/// Run cross-emitter consensus across every inline trait in every orbital.
pub fn validate_cross_emitter(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // event_name -> Vec<(trait_path, signature_set, owner_orbital, owner_trait)>
    // The owner pair is for the future L2.6 narrowing — comparing emitter
    // orbitals to listener orbitals to decide whether a divergence is
    // actually reachable.
    let mut emitters: HashMap<String, Vec<EmitterEntry>> = HashMap::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let Some(emits) = trait_def.emits.as_deref() else {
                continue;
            };
            let trait_path = super::trait_path(oi, &orbital.name, ti, trait_ref);
            for emit in emits {
                let declared_sig = signature(emit.payload_schema.as_deref().unwrap_or(&[]));
                // A render-emitter's declared `emits[]` payload is often left
                // empty (the atom doesn't know the event name until a call
                // site's config resolves it — `std-button`'s `emits {
                // @config.action -> external {} }`). What it actually SENDS
                // lives in the sibling render-ui payload knob (`actionPayload`
                // et al, registry `payloadFor`) — derive from there when
                // present, since that is ground truth and the declared
                // `emits[]` entry is not (gap #82 G6).
                let sig = render_emitter_payload(trait_def, &emit.event, &ctx.entity_definitions)
                    .unwrap_or(declared_sig);
                emitters.entry(emit.event.clone()).or_default().push((
                    trait_path.clone(),
                    sig,
                    orbital.name.clone(),
                    trait_def.name.clone(),
                ));
            }
        }
    }

    // Build the reverse index once and consult it per-event for narrowing.
    // Even though L2.6's full narrowing rule lands separately, having the
    // index built here keeps the cost amortised — every diagnostic site
    // that wants "is this event actually consumed cross-orbitally" has
    // O(1) access.
    let listen_index = build_listen_index(schema);

    for (event, group) in &emitters {
        if group.len() < 2 {
            continue;
        }

        // L2.6 narrowing (gap #31): drop the warning when no listener
        // subscribes to the event from an orbital different from the
        // emitters'. The conflicting emit declarations are uncoupled in
        // that case — every emitter feeds its own orbital's listener
        // (the natural CRUD pattern: ArticleBrowse.VIEW → ArticleView,
        // MediaAssetBrowse.VIEW → MediaAssetView, etc.) and the runtime
        // never reconciles the two payload shapes through the same
        // subscription.
        if !is_cross_orbital_reachable(event, group, &listen_index) {
            continue;
        }

        // Compare every emitter's signature to the first; report when any
        // diverge. We report once per offending pair (first vs. each other).
        let (anchor_path, anchor_sig, _, _) = &group[0];
        for (other_path, other_sig, _, _) in group.iter().skip(1) {
            if other_sig != anchor_sig {
                let diff = describe_diff(anchor_sig, other_sig);
                ctx.add_error(
                    ValidationError::error(
                        ErrorCode::CrossEmitterDeclaredPayloadMismatch,
                        format!("{} vs {}", anchor_path, other_path),
                        format!(
                            "Event '{}' is declared by multiple emitters with conflicting payload fields. {}",
                            event, diff
                        ),
                    )
                    .with_suggestion(
                        "Align the payload field lists across every trait that declares this event, or rename the event so each emitter has its own typed channel.",
                    ),
                );
            }
        }
    }
}

/// L2.6 narrowing predicate: returns true when at least one listener for
/// `event` could plausibly consume divergent payload shapes from the
/// emitter group — i.e. listens cross-orbitally without pinning the
/// source orbital, OR listens to the bare event from any source.
///
/// Returns false when every listener pins to a specific source orbital
/// AND that orbital is one of the emitters' (single-shape subscription),
/// in which case the warning is signal-zero.
fn is_cross_orbital_reachable(
    event: &str,
    emitters: &[EmitterEntry],
    listen_index: &ListenIndex,
) -> bool {
    let Some(listeners) = listen_index.get(event) else {
        // No listener at all → divergence is harmless (no subscription
        // ever reconciles the two shapes). Drop.
        return false;
    };
    // Exact (orbital, trait) pairs that actually emit this event, at the
    // SAME precision the runtime's own delivery rule uses
    // (`buildSourceMatcher`, `packages/almadar-runtime/src/identity/
    // routing.ts`): both a `Trait`-kind and an `Orbital`-kind pin match on
    // orbital AND trait name together, never on either alone. An orbital
    // name match with no trait match is not a real subscription — crediting
    // it with the OTHER trait's payload is exactly gap #82 G6's defect.
    let emitter_pairs: BTreeSet<(&str, &str)> = emitters
        .iter()
        .map(|(_, _, orb, t)| (orb.as_str(), t.as_str()))
        .collect();

    for listener in listeners {
        match &listener.source {
            // `Orbital.Trait.EVENT -> TRIGGER` — cross-orbital pin. Reaches
            // the divergent shape only when it names one of THIS group's
            // actual (orbital, trait) emitters and that orbital differs
            // from the listener's own (a same-orbital orbital-kind pin is
            // still a single fixed producer).
            Some(ListenSource::Orbital {
                orbital,
                trait_name,
                ..
            }) => {
                if orbital != &listener.orbital_name
                    && emitter_pairs.contains(&(orbital.as_str(), trait_name.as_str()))
                {
                    return true;
                }
            }
            // `TraitName.EVENT -> TRIGGER` — the schema's `Trait` source
            // carries no orbital field at all: the runtime resolves it
            // against `listenerOrbital` unconditionally
            // (`buildSourceMatcher`'s `trait` arm requires `source.orbital
            // === listenerOrbital && source.trait === wantedTrait`). A
            // pinned listener therefore receives from exactly ONE producer
            // and can never witness a second, divergent shape through the
            // same subscription — regardless of whether that one producer
            // even appears in `emitters` (a config-driven producer, e.g. a
            // Browse `itemActions` entry, legitimately declares no
            // `emits[]` for the event at all and is still single-shape).
            // Treating an absent match as "reachable from anywhere in the
            // schema" was gap #82 G6's false positive.
            Some(ListenSource::Trait { .. }) => continue,
            // `* EVENT -> TRIGGER` — open subscription. Sees every
            // emitter's payload, including the divergent shape.
            Some(ListenSource::Any) => return true,
            // No source descriptor — local payload-declaration only,
            // not a real subscription. Skip.
            None => continue,
        }
    }
    false
}

/// A render-emitter's ACTUAL payload for `event` — what the pattern really
/// sends, not the (frequently empty) declared `emits[].payloadSchema` an
/// atom like `std-button` carries when the event name is only known at the
/// call site (`emits { @config.action -> external {} }`). The registry
/// pairs an event-valued prop (`kind: "event"`, e.g. button's `action`)
/// with its payload prop via `payloadFor` (e.g. `actionPayload`) — declared
/// data, so this dispatches on that pairing for every pattern that
/// declares one, never on a hardcoded prop name. `None` when no render node
/// in this trait produces `event` this way (caller falls back to the
/// declared schema).
fn render_emitter_payload(
    trait_def: &TraitDefinition,
    event: &str,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> Option<BTreeSet<(String, String, bool)>> {
    let entity_fields = trait_def
        .linked_entity
        .as_deref()
        .and_then(|name| entity_definitions.get(name))
        .map(|e| e.fields.as_slice());

    for transition in &trait_def.state_machine.transitions {
        for effect in transition.effects.as_deref().unwrap_or(&[]) {
            if !is_render_ui_effect(effect) {
                continue;
            }
            let SExpression::Array(arr) = effect else {
                continue;
            };
            let Some(pattern_root) = arr.get(2) else {
                continue;
            };
            if let Some(sig) = find_payload_in_pattern_tree(pattern_root, event, entity_fields) {
                return Some(sig);
            }
        }
    }
    None
}

/// Depth-first search for a pattern node whose registry-declared
/// event-valued prop fires `event`, returning the signature derived from
/// its paired `payloadFor` prop's literal object. Mirrors the JS twin's
/// `scanRenderActionEvents` (`packages/almadar-verify/src/observer/
/// wiring-lint.ts`) traversal shape — every object's values and every
/// array's elements — but resolves fields instead of only names.
fn find_payload_in_pattern_tree(
    expr: &SExpression,
    event: &str,
    entity_fields: Option<&[FieldDefinition]>,
) -> Option<BTreeSet<(String, String, bool)>> {
    match expr {
        SExpression::Object(node) => {
            if let Some(SExpression::String(pattern_type)) = node.get("type") {
                for (event_prop, payload_prop) in event_payload_prop_pairs(pattern_type) {
                    let fires = matches!(
                        node.get(&event_prop),
                        Some(SExpression::String(v)) if v == event
                    );
                    if !fires {
                        continue;
                    }
                    if let Some(SExpression::Object(payload)) = node.get(&payload_prop) {
                        return Some(signature_from_payload_literal(payload, entity_fields));
                    }
                }
            }
            node.values()
                .find_map(|v| find_payload_in_pattern_tree(v, event, entity_fields))
        }
        SExpression::Array(items) => items
            .iter()
            .find_map(|v| find_payload_in_pattern_tree(v, event, entity_fields)),
        _ => None,
    }
}

/// Every `(event_prop, payload_prop)` pairing a pattern's registry declares
/// — a payload prop names its event prop via `payloadFor`, and the named
/// prop must itself be `kind: "event"` (the same pairing
/// `render_ui::binding` reads for `ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH`).
/// Declared data, not a name convention: this covers every pattern the
/// registry pairs this way (button/box/stack's `action`/`actionPayload`,
/// `calendar-grid`'s `longPressEvent`/`longPressPayload`, …) with no
/// per-pattern special case.
fn event_payload_prop_pairs(pattern_type: &str) -> Vec<(String, String)> {
    let Some(pattern) = get_pattern(pattern_type) else {
        return Vec::new();
    };
    pattern
        .props_schema
        .iter()
        .filter_map(|(payload_prop, prop)| {
            let event_prop = prop.payload_for.as_deref()?;
            let is_event_prop = pattern
                .props_schema
                .get(event_prop)
                .and_then(|p| p.kind.as_deref())
                == Some("event");
            is_event_prop.then(|| (event_prop.to_string(), payload_prop.clone()))
        })
        .collect()
}

/// One payload object literal's fields, typed from the bound entity where
/// the value is `@entity.<f>` (gap #82 G6's stated rule) — mirrors
/// `wiring-lint.ts`'s native `{id, row}` treatment of item-action payloads
/// in spirit: the shape is derived from what the call site actually binds,
/// not guessed. A literal (non-`@entity.`) value is always supplied, so it
/// is typed by its own JSON shape and marked required.
fn signature_from_payload_literal(
    payload: &orbital_core_base::schema::SObject,
    entity_fields: Option<&[FieldDefinition]>,
) -> BTreeSet<(String, String, bool)> {
    payload
        .iter()
        .map(|(key, value)| {
            let (field_type, required) = match value {
                SExpression::String(s) => s
                    .strip_prefix("@entity.")
                    .and_then(|field_name| {
                        entity_fields.and_then(|fields| {
                            fields
                                .iter()
                                .find(|f| f.name.as_deref() == Some(field_name))
                        })
                    })
                    .map(|f| (field_type_name(&f.field_type), f.required.unwrap_or(false)))
                    .unwrap_or(("string".to_string(), true)),
                SExpression::Number(_) => ("number".to_string(), true),
                SExpression::Bool(_) => ("boolean".to_string(), true),
                _ => ("string".to_string(), true),
            };
            (key.to_string(), field_type, required)
        })
        .collect()
}

/// `FieldType`'s wire spelling (`#[serde(rename_all = "lowercase")]`) — the
/// same string vocabulary `PayloadField.field_type` uses, so the derived
/// signature compares equal to a hand-declared one for the same shape.
fn field_type_name(field_type: &FieldType) -> String {
    serde_json::to_value(field_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "string".to_string())
}

fn signature(fields: &[PayloadField]) -> BTreeSet<(String, String, bool)> {
    fields
        .iter()
        .map(|f| {
            (
                f.name.clone(),
                f.field_type.clone(),
                f.required.unwrap_or(false),
            )
        })
        .collect()
}

fn describe_diff(
    anchor: &BTreeSet<(String, String, bool)>,
    other: &BTreeSet<(String, String, bool)>,
) -> String {
    let only_in_anchor: Vec<String> = anchor
        .difference(other)
        .map(|(n, t, r)| format!("{}:{}{}", n, t, if *r { "!" } else { "" }))
        .collect();
    let only_in_other: Vec<String> = other
        .difference(anchor)
        .map(|(n, t, r)| format!("{}:{}{}", n, t, if *r { "!" } else { "" }))
        .collect();
    format!(
        "anchor only: [{}]; other only: [{}]",
        only_in_anchor.join(", "),
        only_in_other.join(", ")
    )
}
