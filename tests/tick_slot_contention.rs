//! `ORB_SLOT_CONTENTION_TICK` — two traits repainting one slot every frame.
//!
//! This is the check the riya open world needed and did not have: composing the
//! 28 level atoms' play traits would have put 28 per-frame writers on slot
//! `main`, which the runtime stacks into one column rather than showing any of
//! them. The authors worked around it by hand-splitting every atom into an
//! emit-only data trait and a rendering play trait; nothing enforced the split.
//!
//! Every test here is a pair — the case that must fire beside the neighbouring
//! case that must NOT — because the corpus says the bare "two traits name one
//! slot" predicate is legitimate 106 times over, and only the tick scoping plus
//! a live-guard clause separates the real defect from the ordinary pattern.

use orbital_compiler::phases::validation::validate_comprehensive_without_services;
use orbital_core_base::schema::OrbitalSchema;
use serde_json::{json, Value};

fn parse(schema: Value) -> OrbitalSchema {
    serde_json::from_value(schema).expect("schema fixture must deserialize")
}

fn codes(schema: Value) -> Vec<String> {
    validate_comprehensive_without_services(&parse(schema))
        .errors
        .iter()
        .map(|e| e.code.as_str().to_string())
        .collect()
}

fn findings(schema: Value) -> Vec<(String, String, String)> {
    validate_comprehensive_without_services(&parse(schema))
        .errors
        .iter()
        .map(|e| {
            (
                e.code.as_str().to_string(),
                e.message.clone(),
                e.suggestion.clone().unwrap_or_default(),
            )
        })
        .collect()
}

const TICK: &str = "ORB_SLOT_CONTENTION_TICK";

/// A trait that repaints `slot` on every tick. `guard` is the tick's guard
/// exactly as it stands post-inline — a call site's `config { running: false }`
/// has already been substituted by then, which is what makes it foldable.
fn ticking_trait(name: &str, slot: &str, guard: Value) -> Value {
    json!({
        "name": name,
        "linkedEntity": "Board",
        "category": "interaction",
        "scope": "instance",
        "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
                "event": "INIT",
                "from": "idle",
                "to": "idle",
                "effects": [["render-ui", slot, { "type": "box" }]]
            }]
        },
        "ticks": [{
            "name": "frame",
            "interval": 50.0,
            "guard": guard,
            "effects": [["render-ui", slot, { "type": "box" }]]
        }],
        "effectRow": [
            { "kind": "render-ui", "resource": slot },
            { "kind": "render-ui", "resource": slot, "site": "tick" },
            { "kind": "ticks", "site": "tick" }
        ]
    })
}

/// One orbital, `pages` laid out by the caller. Page mounts are NAME REFERENCES,
/// the way every emitted `.orb` writes them — a fixture with inline page traits
/// would not exercise the resolver at all.
fn schema_with(traits: Vec<Value>, pages: Value) -> Value {
    json!({
        "name": "BoardApp",
        "orbitals": [{
            "name": "BoardOrbital",
            "entity": {
                "name": "Board",
                "persistence": "runtime",
                "fields": [{ "name": "id", "type": "string", "required": true }]
            },
            "traits": traits,
            "pages": pages
        }]
    })
}

fn one_page(mounts: Vec<&str>) -> Value {
    json!([{
        "name": "BoardPage",
        "path": "/board",
        "traits": mounts.iter().map(|n| json!({ "ref": n })).collect::<Vec<_>>()
    }])
}

const LIVE: fn() -> Value = || json!(["==", "@entity.active", true]);

// ---------------------------------------------------------------------------
// The pair: same slot, same page, both live
// ---------------------------------------------------------------------------

#[test]
fn two_live_tick_writers_on_one_page_slot_are_a_conflict() {
    let codes = codes(schema_with(
        vec![
            ticking_trait("BoardPlay", "main", LIVE()),
            ticking_trait("BoardOverlay", "main", LIVE()),
        ],
        one_page(vec!["BoardPlay", "BoardOverlay"]),
    ));
    assert!(
        codes.contains(&TICK.to_string()),
        "two per-frame writers on slot 'main' paint together, got {codes:?}"
    );
}

#[test]
fn the_same_two_traits_on_separate_pages_are_not_a_conflict() {
    let codes = codes(schema_with(
        vec![
            ticking_trait("BoardPlay", "main", LIVE()),
            ticking_trait("BoardOverlay", "main", LIVE()),
        ],
        json!([
            { "name": "PlayPage", "path": "/play", "traits": [{ "ref": "BoardPlay" }] },
            { "name": "OverlayPage", "path": "/overlay", "traits": [{ "ref": "BoardOverlay" }] }
        ]),
    ));
    assert!(
        !codes.contains(&TICK.to_string()),
        "traits that never share a page never share a frame, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// The guard clause — the sanctioned remedy must never be flagged
// ---------------------------------------------------------------------------

#[test]
fn a_config_disabled_clock_is_not_a_second_writer() {
    // What `config { running: false }` looks like once inline has substituted it.
    let disabled = json!(["and", false, ["==", "@entity.active", true]]);
    let codes = codes(schema_with(
        vec![
            ticking_trait("BoardPlay", "main", LIVE()),
            ticking_trait("BoardOverlay", "main", disabled),
        ],
        one_page(vec!["BoardPlay", "BoardOverlay"]),
    ));
    assert!(
        !codes.contains(&TICK.to_string()),
        "a tick whose guard folds to false can never paint; flagging it would flag \
         the very knob authors are told to use, got {codes:?}"
    );
}

#[test]
fn the_same_pair_with_both_clocks_enabled_is_a_conflict() {
    let enabled = json!(["and", true, ["==", "@entity.active", true]]);
    let codes = codes(schema_with(
        vec![
            ticking_trait("BoardPlay", "main", LIVE()),
            ticking_trait("BoardOverlay", "main", enabled),
        ],
        one_page(vec!["BoardPlay", "BoardOverlay"]),
    ));
    assert!(
        codes.contains(&TICK.to_string()),
        "`and true` leaves the clock running, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Scoping: transitions alternate, tick bodies do not
// ---------------------------------------------------------------------------

#[test]
fn transition_only_writers_on_one_slot_are_not_a_tick_conflict() {
    let mut catalog = ticking_trait("Catalog", "main", LIVE());
    let mut modal = ticking_trait("EditModal", "main", LIVE());
    for t in [&mut catalog, &mut modal] {
        // Strip the clock: both still render 'main', but only from transitions,
        // which is the ordinary catalog-behind-a-modal shape 106 corpus sites use.
        t.as_object_mut().unwrap().remove("ticks");
        t["effectRow"] = json!([{ "kind": "render-ui", "resource": "main" }]);
    }
    let codes = codes(schema_with(
        vec![catalog, modal],
        one_page(vec!["Catalog", "EditModal"]),
    ));
    assert!(
        !codes.contains(&TICK.to_string()),
        "state-gated renders alternate; only tick bodies paint together, got {codes:?}"
    );
}

#[test]
fn different_slots_are_not_a_conflict() {
    let codes = codes(schema_with(
        vec![
            ticking_trait("BoardPlay", "main", LIVE()),
            ticking_trait("BoardHud", "hud-top", LIVE()),
        ],
        one_page(vec!["BoardPlay", "BoardHud"]),
    ));
    assert!(
        !codes.contains(&TICK.to_string()),
        "two clocks are fine as long as they own different slots, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// The resolver — the reason the older slot checks never fired
// ---------------------------------------------------------------------------

#[test]
fn page_mounts_are_resolved_through_their_name_references() {
    // Identical to the failing pair except the page mounts nobody. If the check
    // read page.traits without resolving refs it would see nothing in BOTH, and
    // this test would pass for the wrong reason — so its partner above is what
    // proves the resolver works, and this proves the scoping is real.
    let codes = codes(schema_with(
        vec![
            ticking_trait("BoardPlay", "main", LIVE()),
            ticking_trait("BoardOverlay", "main", LIVE()),
        ],
        json!([{ "name": "EmptyPage", "path": "/empty", "traits": [] }]),
    ));
    assert!(
        !codes.contains(&TICK.to_string()),
        "traits mounted on no page share no frame, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Message legibility — the validator is the teaching surface for authoring agents
// ---------------------------------------------------------------------------

#[test]
fn the_message_names_every_participant_and_the_fix_tokens() {
    let found = findings(schema_with(
        vec![
            ticking_trait("BoardPlay", "main", LIVE()),
            ticking_trait("BoardOverlay", "main", LIVE()),
        ],
        one_page(vec!["BoardPlay", "BoardOverlay"]),
    ));
    let (_, message, suggestion) = found
        .into_iter()
        .find(|(code, _, _)| code == TICK)
        .expect("the conflict fires");

    for needle in ["/board", "'main'", "'BoardPlay'", "'BoardOverlay'", "ticks"] {
        assert!(
            message.contains(needle),
            "a reader must not have to guess {needle}; got: {message}"
        );
    }
    assert!(
        !message.contains("multiple traits"),
        "name them, do not count them; got: {message}"
    );
    for needle in ["running: false", "@config.surface"] {
        assert!(
            suggestion.contains(needle),
            "the fix must spell out the tokens to type ({needle}); got: {suggestion}"
        );
    }
}
