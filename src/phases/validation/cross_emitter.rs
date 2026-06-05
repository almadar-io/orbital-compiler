//! Phase F.2: cross-emitter declared-payload consensus check.
//!
//! When two or more traits declare the same external event name in their
//! `emits` arrays, every emitter must declare the same payload field list.
//! Otherwise consumers that listen on the event will see one shape from one
//! emitter and a different shape from another, the runtime cannot
//! reconcile them, and downstream listeners drop fields silently.
//!
//! This pass emits a warn-only `CrossEmitterDeclaredPayloadMismatch` for
//! every event whose payload-field set is not identical across all
//! declaring traits. The signature compared is the tuple
//! `(name, field_type, required-as-bool)`; description is ignored. Two
//! emitters with no declared payload are considered consistent (both are
//! "empty").

use std::collections::{BTreeSet, HashMap};

use orbital_core::schema::{ListenSource, OrbitalSchema, PayloadField, TraitReference};

use super::error::{ErrorCode, ValidationError};
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
                index
                    .entry(listen.event.clone())
                    .or_default()
                    .push(entry);
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
    let mut emitters: HashMap<
        String,
        Vec<(String, BTreeSet<(String, String, bool)>, String, String)>,
    > = HashMap::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let Some(emits) = trait_def.emits.as_deref() else {
                continue;
            };
            let trait_path = format!("orbitals[{}].traits[{}]({})", oi, ti, trait_def.name);
            for emit in emits {
                let sig = signature(emit.payload_schema.as_deref().unwrap_or(&[]));
                emitters
                    .entry(emit.event.clone())
                    .or_default()
                    .push((
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
                    ValidationError::phase_f_warning(
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
    emitters: &[(String, BTreeSet<(String, String, bool)>, String, String)],
    listen_index: &ListenIndex,
) -> bool {
    let Some(listeners) = listen_index.get(event) else {
        // No listener at all → divergence is harmless (no subscription
        // ever reconciles the two shapes). Drop.
        return false;
    };
    // Set of orbitals that emit this event. Listeners pinned to one of
    // these get a single-shape subscription.
    let emitter_orbitals: BTreeSet<&str> =
        emitters.iter().map(|(_, _, orb, _)| orb.as_str()).collect();

    for listener in listeners {
        match &listener.source {
            // `Source.EVENT -> TRIGGER` — explicit cross-trait pin. If
            // it pins an orbital other than itself, the listener could
            // receive the divergent shape from that other orbital. If it
            // pins its own orbital, single-shape (skip).
            Some(ListenSource::Orbital { orbital, .. }) => {
                if orbital != &listener.orbital_name && emitter_orbitals.contains(orbital.as_str())
                {
                    return true;
                }
            }
            // `TraitName EVENT -> TRIGGER` — trait-pinned. If the named
            // trait is in the listener's own orbital AND that orbital
            // emits the event, single-shape; otherwise we can't tell
            // statically and conservatively assume reachable.
            Some(ListenSource::Trait { trait_name }) => {
                let listener_orbital = &listener.orbital_name;
                let same_orbital_has_emitter = emitters
                    .iter()
                    .any(|(_, _, orb, t)| orb == listener_orbital && t == trait_name);
                if !same_orbital_has_emitter {
                    return true;
                }
            }
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
