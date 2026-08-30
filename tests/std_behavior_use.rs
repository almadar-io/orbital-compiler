//! Phase 3.5.E acid test: a user `.orb` consumes a std behavior via `use` +
//! reference overrides, with no `behavior/*` operator calls.
//!
//! This is the end-to-end proof that the principled Phase 3.5 model works:
//! the embedded loader resolves `from: "std/behaviors/std-modal"` against the
//! `EMBEDDED_STD_BEHAVIORS` map, and the inline phase substitutes the imported
//! trait/page references with the embedded definitions while preserving the
//! caller's `linkedEntity` and `path` overrides.

use orbital_compiler::phases::inline::{
    inline_schema, inline_schema_with_config_diagnostics, InlineConfig, InlineError,
};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::error::ErrorCode;
use orbital_compiler::phases::validation::trait_ref_config::validate_trait_ref_config_diagnostics;
use orbital_compiler::{compile_file, CompilerError, CompilerOptions};
use orbital_core::{EntityRef, PageRef, TraitReference};
use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/shopping-cart.orb")
}

/// Phase 3.5.G.A.2 fixture — a cart orbital that imports
/// `Modal.traits.ModalRecordModal` AND renames it to `CartItemAddItem` via
/// the `name` field AND re-targets it at `CartItem` via `linkedEntity`.
/// Kept as a SEPARATE fixture from `shopping-cart.orb` so the existing A.1
/// tests (which still assert on the upstream `ModalRecordModal` name)
/// remain untouched.
fn cart_renamed_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/cart-renamed.orb")
}

/// Phase 3.5.G.A.3 fixture — a cart orbital that imports
/// `Modal.traits.ModalRecordModal` TWICE without `name` overrides on either
/// reference. The collision must be detected and rejected by `inline_traits`
/// with `InlineError::DuplicateTraitName`.
fn cart_collision_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/cart-collision.orb")
}

/// Phase 3.5.G.A.3 fixture — the same shape as `cart-collision.orb`, but
/// with distinct `name` overrides on each reference (`CartItemAddItem` and
/// `CartItemEditItem`). The collision detector must allow this through
/// because the post-rename trait names differ.
fn cart_collision_resolved_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/cart-collision-resolved.orb")
}

/// Phase 3.5.G.A.5 fixture — a cart orbital that uses the new
/// `EntityRef::Extends` form to inherit `Modal.entity` (5 fields:
/// id/name/description/status/createdAt) and append two molecule fields
/// (`pendingId`, `lineItemCount`). The lowered OIR must contain a single
/// entity named `CartItem` with all 7 fields, proving that
/// `inline_entity` rewrites Extends → Inline before downstream phases run.
fn cart_extends_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/cart-extends.orb")
}

/// Phase 3.5.G.F.3 fixture — a cart orbital that imports
/// `Modal.traits.ModalRecordModal` and supplies an `events` map renaming
/// `OPEN` → `ADD_ITEM` and `CLOSE` → `CANCEL_ADD`. The fixture also passes
/// `linkedEntity: "CartItem"` on the page reference to exercise the new
/// `PageRefObject.linked_entity` override path.
fn cart_events_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/cart-events.orb")
}

/// Phase 3.5.G.F.7 fixture — exercises the new `listens` and `emitsScope`
/// post-substitution overrides. References `std-agent-tool-call` (which has
/// upstream listens AND multiple emits with default `external` scope), then:
///   - `listens: []` clears all upstream listen entries
///   - `emitsScope: "internal"` sets every emit's scope to internal
fn fixloop_tool_call_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/fixloop-tool-call.orb")
}

/// Phase 3.5.G.F.8 fixture — exercises the per-transition `effects` override.
/// References `Modal.traits.ModalRecordModal` and supplies an `effects` map
/// that REPLACES the OPEN (renamed to ADD_ITEM) transition's effect list with
/// a `[fetch, render-ui]` pair containing cart-specific content. The lowered
/// OIR's ADD_ITEM transition must carry the override effects, not the
/// upstream's defaults.
fn cart_effects_override_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/cart-effects-override.orb")
}

/// Ordering-gap regression fixture — imports `UIButton.traits.ButtonRender`
/// (the std atom whose `emits { @config.action -> external {...} } }` and
/// `config { action : string = "ACTION" }` mirror `std-trait-wars.orb`'s
/// `UpgradeBuildingButtonRender`) and renames its resolved default emit name
/// `ACTION` to `UPGRADE_WIDGET` via `events`. Before the fix, the
/// events-rename source-key check ran against the trait's RAW declared emits
/// (still carrying the literal token `@config.action`), so this legitimate
/// rename of the RESOLVED name was rejected with `EventRenameUnknownKey`.
fn button_config_ref_events_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/button-config-ref-events.orb")
}

/// Companion fixture — same shape, but the rename's `from` key
/// (`NOPE_NOT_A_REAL_EVENT`) names nothing the trait declares (not even
/// after config-ref resolution). Must still be rejected with
/// `EventRenameUnknownKey`, proving the fix didn't neuter the check.
fn button_config_ref_events_unknown_key_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/button-config-ref-events-unknown-key.orb")
}

#[test]
fn shopping_cart_parses_and_inlines_std_modal() {
    let path = fixture_path();
    let mut schema = parse_file(&path).expect("fixture must parse");

    assert_eq!(schema.name, "MyShoppingApp");
    assert_eq!(schema.orbitals.len(), 1);
    assert_eq!(schema.orbitals[0].name, "ShoppingCartOrbital");

    // Before inlining: entity is the user's literal ShoppingCart, traits/pages
    // contain the std-modal references.
    let pre_orbital = &schema.orbitals[0];
    assert!(matches!(pre_orbital.entity, EntityRef::Inline(_)));

    // Run inline phase. Default LoaderConfig is fine because the embedded
    // branch in ExternalOrbitalLoader will handle "std/behaviors/std-modal"
    // before any filesystem fallback runs.
    let config = InlineConfig::default();
    let result = inline_schema(&mut schema, &config, Some(&path));
    assert!(result.is_ok(), "inline failed: {:?}", result.err());
}

#[test]
fn shopping_cart_keeps_user_entity_after_inline() {
    let path = fixture_path();
    let mut schema = parse_file(&path).expect("fixture must parse");

    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&path)).expect("inline must succeed");

    // The user declared `ShoppingCart` literally in the orbital, so the
    // embedded `ModalRecord` entity from std-modal must NOT replace it.
    let entity = schema.orbitals[0]
        .entity
        .as_inline()
        .expect("entity must be inline (the user's literal ShoppingCart)");
    assert_eq!(entity.name, "ShoppingCart");

    // The fields the user declared survive untouched.
    let field_names: Vec<&str> = entity
        .fields
        .iter()
        .filter_map(|f| f.name.as_deref())
        .collect();
    assert!(field_names.contains(&"id"));
    assert!(field_names.contains(&"lineItemCount"));
}

#[test]
fn shopping_cart_inlines_modal_trait_with_linked_entity_override() {
    let path = fixture_path();
    let mut schema = parse_file(&path).expect("fixture must parse");

    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&path)).expect("inline must succeed");

    // Phase 3.5.G.A.1 fixed the inliner to push a single merged
    // `TraitReference::Inline` with the override already applied, instead
    // of pushing a Reference + Inline pair (which compose/resolve dropped
    // the Reference half of). After inlining there must be exactly one
    // ModalRecordModal Inline entry whose `linked_entity` has been
    // rewritten from the embedded `ModalRecord` default to the user's
    // `ShoppingCart` override.
    let traits = &schema.orbitals[0].traits;
    assert!(
        !traits.is_empty(),
        "traits must contain the inlined ModalRecordModal"
    );

    // There must be NO stray `TraitReference::Reference` entries left
    // over. The whole point of A.1 is that overrides are applied inline.
    let stray_ref = traits
        .iter()
        .find(|t| matches!(t, TraitReference::Reference { .. }));
    assert!(
        stray_ref.is_none(),
        "expected zero TraitReference::Reference entries after inline (they must be merged \
         into Inline), got: {:#?}",
        traits
    );

    // Exactly one inlined ModalRecordModal, and its linked_entity is
    // ShoppingCart (the override, not the embedded default).
    let inlined = traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "ModalRecordModal" => Some(def),
            _ => None,
        })
        .expect("expected an inlined TraitDefinition named ModalRecordModal");
    assert_eq!(
        inlined.linked_entity.as_deref(),
        Some("ShoppingCart"),
        "linked_entity override not applied in inline phase: got {:?}",
        inlined.linked_entity
    );
}

#[test]
fn shopping_cart_inlines_modal_page_with_path_override() {
    let path = fixture_path();
    let mut schema = parse_file(&path).expect("fixture must parse");

    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(&path)).expect("inline must succeed");

    // After inlining, the page reference `Modal.pages.ModalRecordModalPage`
    // with `path: "/cart"` should produce an inline page with the path
    // override applied.
    let pages = schema.orbitals[0]
        .pages
        .as_ref()
        .expect("pages must be present");
    assert_eq!(pages.len(), 1);

    match &pages[0] {
        PageRef::Inline(page_def) => {
            assert_eq!(page_def.name, "ModalRecordModalPage");
            assert_eq!(
                page_def.path, "/cart",
                "page path must be the user's override, not the embedded default"
            );
        }
        other => panic!("expected PageRef::Inline after override, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Phase 3.5.G.Z — Lowered-OIR assertions (red tests for Phase 3.5.G.A.1)
// ---------------------------------------------------------------------------
//
// The tests above only inspect the post-inline schema shape, which is
// misleading: the inline phase emits BOTH a `TraitReference::Reference`
// (carrying the override) AND a `TraitReference::Inline` (carrying the
// embedded default). The schema-shape assertions pass, but downstream
// `compose.rs` and `resolve.rs` only match the `Inline` arm and silently
// drop the `Reference` arm. As a result the lowered OIR has the embedded
// default `linked_entity = "ModalRecord"` instead of the user's override
// `linked_entity = "ShoppingCart"`.
//
// The two tests below assert against the lowered OIR (the actual final
// output of the compile pipeline). They are expected to FAIL today and
// must PASS after Phase 3.5.G.A.1 fixes the inliner to apply overrides
// inside the inline phase (so a single merged Inline reaches downstream).

fn lowered_oir_options() -> CompilerOptions {
    CompilerOptions {
        skip_validation: true,
        comprehensive_validation: false,
        ..Default::default()
    }
}

#[test]
fn shopping_cart_lowered_output_applies_linked_entity_override() {
    let path = fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the shopping-cart fixture");

    let modal_trait = oir
        .find_trait("ModalRecordModal")
        .expect("lowered OIR must contain a trait named ModalRecordModal");

    assert_eq!(
        modal_trait.linked_entity,
        Some("ShoppingCart".to_string()),
        "Override NOT applied: linked_entity = {:?} (expected Some(\"ShoppingCart\")). \
         The fixture sets `linkedEntity: \"ShoppingCart\"` on the Modal.traits.ModalRecordModal \
         reference, but the inliner pushes BOTH a TraitReference::Reference and a \
         TraitReference::Inline. compose.rs:63-67 and resolve.rs:121-125 only match Inline, \
         so the Reference (with the override) is silently dropped and the embedded default \
         `ModalRecord` survives. Phase 3.5.G.A.1 must fix this by applying overrides inside \
         the inline phase so a single merged Inline reaches downstream.",
        modal_trait.linked_entity
    );
}

#[test]
fn shopping_cart_lowered_output_applies_path_override() {
    let path = fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the shopping-cart fixture");

    let modal_page = oir
        .find_page("ModalRecordModalPage")
        .expect("lowered OIR must contain a page named ModalRecordModalPage");

    assert_eq!(
        modal_page.path, "/cart",
        "Page path override NOT applied: path = {:?} (expected \"/cart\"). \
         The fixture sets `path: \"/cart\"` on the Modal.pages.ModalRecordModalPage reference. \
         Path overrides DO already apply at the inline phase per inline.rs:377-380, so this \
         test may pass today; if it does, Phase 3.5.G.A.1 only needs to fix the trait \
         linked_entity path. If it fails, the page override path needs the same fix.",
        modal_page.path
    );
}

// ---------------------------------------------------------------------------
// Phase 3.5.G.A.2 — Trait renaming via the `name` field
// ---------------------------------------------------------------------------
//
// Uses a NEW fixture (`cart-renamed.orb`) so the 6 existing A.1 assertions
// on the upstream `ModalRecordModal` name remain valid. The fixture sets
// both `name: "CartItemAddItem"` and `linkedEntity: "CartItem"` on the
// same Modal reference, so the lowered OIR must contain a trait named
// `CartItemAddItem` (NOT `ModalRecordModal`) with `linked_entity ==
// Some("CartItem")`.

#[test]
fn cart_renamed_lowered_output_applies_name_and_linked_entity_override() {
    let path = cart_renamed_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the cart-renamed fixture");

    // The rename must have taken effect: no trait named `ModalRecordModal`
    // should survive into the lowered OIR.
    assert!(
        oir.find_trait("ModalRecordModal").is_none(),
        "rename override NOT applied: upstream `ModalRecordModal` survived into lowered OIR. \
         The fixture sets `name: \"CartItemAddItem\"` on the Modal.traits.ModalRecordModal \
         reference; Phase 3.5.G.A.2 must rewrite `trait_def.name` to the override before \
         pushing the merged Inline."
    );

    // The renamed trait must be present.
    let renamed = oir
        .find_trait("CartItemAddItem")
        .expect("lowered OIR must contain a trait named CartItemAddItem (the renamed override)");

    // And its linked_entity override must also have been applied (A.1
    // behavior that must still fire when A.2's rename is in play).
    assert_eq!(
        renamed.linked_entity,
        Some("CartItem".to_string()),
        "linked_entity override must still apply alongside the rename: got {:?}",
        renamed.linked_entity
    );
}

// ---------------------------------------------------------------------------
// Phase 3.5.G.A.3 — Multiple references with collision handling
// ---------------------------------------------------------------------------
//
// A molecule that imports the same upstream trait twice (e.g. `std-cart`
// needing `Modal.traits.ModalRecordModal` once for "add" and once for
// "edit") must EITHER provide a `name` override on at least one reference
// to disambiguate, OR see a `DuplicateTraitName` error from the inliner.
// The two tests below cover both halves of that contract.

#[test]
fn cart_collision_without_name_overrides_errors() {
    let path = cart_collision_fixture_path();
    let result = compile_file(&path, &lowered_oir_options());

    match result {
        Err(CompilerError::Inline(InlineError::DuplicateTraitName { name, .. })) => {
            assert_eq!(
                name, "ModalRecordModal",
                "DuplicateTraitName must carry the original (collided) trait name"
            );
        }
        Ok(_) => panic!(
            "expected compile_file to fail with DuplicateTraitName when two \
             references collide without `name` overrides, but compilation \
             succeeded. Phase 3.5.G.A.3 collision detection is missing or wrong."
        ),
        Err(other) => panic!(
            "expected Err(CompilerError::Inline(InlineError::DuplicateTraitName(...))), \
             got {:?}",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// Ordering-gap regression — `events:` rename source keys must be checked
// against RESOLVED `@config.<knob>` emit names, not the raw unresolved
// token. See `button-config-ref-events.orb` / std-trait-wars.orb.
// ---------------------------------------------------------------------------

#[test]
fn button_config_ref_events_rename_of_resolved_default_name_succeeds() {
    let path = button_config_ref_events_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options()).expect(
        "compile_file must succeed: renaming the RESOLVED default emit name \
         \"ACTION\" (from @config.action's declared default) must not be \
         rejected as an unknown rename source key",
    );

    let trait_def = oir
        .find_trait("UpgradeButtonRender")
        .expect("lowered OIR must contain the renamed trait UpgradeButtonRender");

    assert!(
        trait_def.emits.iter().any(|e| e.event == "UPGRADE_WIDGET"),
        "the @config.action emit must resolve to \"ACTION\" and then be \
         renamed to \"UPGRADE_WIDGET\" by the events override; got emits {:?}",
        trait_def.emits.iter().map(|e| &e.event).collect::<Vec<_>>()
    );
    assert!(
        !trait_def.emits.iter().any(|e| e.event == "ACTION"),
        "the resolved default name \"ACTION\" must not survive alongside the \
         rename target — the rename must replace it, not add a second entry; \
         got emits {:?}",
        trait_def.emits.iter().map(|e| &e.event).collect::<Vec<_>>()
    );
}

#[test]
fn button_config_ref_events_rename_of_truly_unknown_key_diagnoses_without_abort() {
    // New contract: an unknown rename source key must NOT abort inlining
    // (the abort left every later orbital un-inlined and cascaded spurious
    // ORB_T_UNDEFINED_TRAIT / ORB_X_LISTEN_SOURCE_UNRESOLVED findings).
    // Composition proceeds — the rename no-ops — and the offense surfaces as
    // an ORB_T_EVENT_RENAME_UNKNOWN_KEY validation error via the
    // TraitRefConfigDiagnostic channel.
    let path = button_config_ref_events_unknown_key_fixture_path();

    let mut schema = parse_file(&path).expect("fixture must parse");
    let mut diagnostics = Vec::new();
    inline_schema_with_config_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&path),
        &mut diagnostics,
    )
    .expect("inlining must survive an unknown rename source key");

    let diag = diagnostics
        .iter()
        .find(|d| !d.unknown_event_rename_keys.is_empty())
        .expect("an unknown-event-rename diagnostic must be collected");
    assert_eq!(
        diag.unknown_event_rename_keys,
        vec!["NOPE_NOT_A_REAL_EVENT".to_string()],
        "the diagnostic must carry the offending source key"
    );
    assert!(
        !diag.known_events.is_empty(),
        "the diagnostic must carry the trait's known event set for the message"
    );

    let result = validate_trait_ref_config_diagnostics(&diagnostics);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EventRenameUnknownKey
                && e.message.contains("NOPE_NOT_A_REAL_EVENT")),
        "validation must convert the diagnostic to ORB_T_EVENT_RENAME_UNKNOWN_KEY; got {:?}",
        result.errors
    );

    // And the compile path proceeds end-to-end on the same fixture.
    compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed — the bad rename no-ops instead of aborting");
}

#[test]
fn bad_event_rename_does_not_uninline_sibling_orbitals() {
    // The cascade this guards against: one orbital's bad rename aborted
    // `inline_schema` mid-loop, every LATER orbital kept its
    // TraitReference::Reference form, and validators (which index Inline
    // traits only) reported those healthy siblings as ORB_T_UNDEFINED_TRAIT.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-schemas/use-std-behavior/button-config-ref-events-unknown-key-sibling.orb");

    let mut schema = parse_file(&path).expect("fixture must parse");
    let mut diagnostics = Vec::new();
    inline_schema_with_config_diagnostics(
        &mut schema,
        &InlineConfig::default(),
        Some(&path),
        &mut diagnostics,
    )
    .expect("inlining must survive the bad rename in the first orbital");

    let gadget = schema
        .orbitals
        .iter()
        .find(|o| o.name == "GadgetOrbital")
        .expect("sibling orbital present");
    assert!(
        gadget
            .traits
            .iter()
            .any(|t| matches!(t, TraitReference::Inline(td) if td.name == "GadgetButtonRender")),
        "the healthy sibling orbital's trait must be fully inlined despite the \
         bad rename in the preceding orbital; got {:?}",
        gadget.traits
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.unknown_event_rename_keys.contains(&"NOPE_NOT_A_REAL_EVENT".to_string())),
        "the bad rename must still be diagnosed"
    );
}

#[test]
fn cart_collision_resolved_with_name_overrides_succeeds() {
    let path = cart_collision_resolved_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options()).expect(
        "compile_file must succeed when both references carry distinct \
         `name` overrides — there is no name collision after rename",
    );

    // Both renamed traits must be present in the lowered OIR.
    let add = oir
        .find_trait("CartItemAddItem")
        .expect("lowered OIR must contain a trait named CartItemAddItem (the first rename)");
    let edit = oir
        .find_trait("CartItemEditItem")
        .expect("lowered OIR must contain a trait named CartItemEditItem (the second rename)");

    // The upstream `ModalRecordModal` name must NOT survive — both
    // references were renamed, so neither should retain the original.
    assert!(
        oir.find_trait("ModalRecordModal").is_none(),
        "rename should have removed the upstream name from the lowered OIR"
    );

    // Both renamed traits carry the linked_entity override.
    assert_eq!(
        add.linked_entity,
        Some("CartItem".to_string()),
        "first inlined trait must carry linkedEntity override: got {:?}",
        add.linked_entity
    );
    assert_eq!(
        edit.linked_entity,
        Some("CartItem".to_string()),
        "second inlined trait must carry linkedEntity override: got {:?}",
        edit.linked_entity
    );
}

// ---------------------------------------------------------------------------
// Phase 3.5.G.A.5 — Entity field merging via EntityRef::Extends
// ---------------------------------------------------------------------------
//
// `cart-extends.orb` uses the new `EntityRef::Extends` form to inherit the
// upstream `Modal.entity` (5 fields: id/name/description/status/createdAt)
// AND append two molecule fields (`pendingId`, `lineItemCount`). The
// lowered OIR must contain a single `CartItem` entity carrying all 7
// fields, with the rename, persistence override, and field merge applied
// inside the inline phase before resolve runs.

#[test]
fn cart_extends_parses_as_extends_variant() {
    // Sanity: serde untagged enum dispatch must pick `EntityRef::Extends`
    // for the cart-extends fixture (and NOT `EntityRef::Inline`, which
    // would silently swallow the `extends` field as an unknown key on
    // EntityDefinition).
    let path = cart_extends_fixture_path();
    let schema = parse_file(&path).expect("fixture must parse");
    match &schema.orbitals[0].entity {
        EntityRef::Extends(e) => {
            assert_eq!(e.reference, "Modal.entity");
            assert_eq!(e.name.as_deref(), Some("CartItem"));
            assert_eq!(e.fields.as_ref().map(|f| f.len()), Some(2));
        }
        EntityRef::Inline(e) => panic!(
            "fixture parsed as Inline (name={}, fields={}); the `extends` \
             field was silently dropped. Add `#[serde(deny_unknown_fields)]` \
             to EntityDefinition or rework variant ordering.",
            e.name,
            e.fields.len()
        ),
        EntityRef::Reference(s) => panic!("unexpected Reference variant: {}", s),
    }
}

#[test]
fn cart_extends_lowered_output_has_inherited_and_extra_fields() {
    let path = cart_extends_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the cart-extends fixture");

    // The molecule renamed the inherited entity to `CartItem`. The
    // upstream `ModalRecord` name must NOT survive into the lowered OIR.
    assert!(
        oir.find_entity("ModalRecord").is_none(),
        "rename via Extends must remove the upstream `ModalRecord` name from \
         the lowered OIR; found a stray `ModalRecord` entity"
    );

    let cart = oir
        .find_entity("CartItem")
        .expect("lowered OIR must contain a `CartItem` entity (rename + extends merge)");

    // Collect the merged field names. Order matters for human readers
    // (inherited first, then extras), but for the assertion we care about
    // SET membership: every inherited field plus every molecule field
    // must be present, and there must be no duplicates.
    let field_names: Vec<&str> = cart.fields.iter().map(|f| f.name.as_str()).collect();

    // The 5 inherited fields from std-modal's `ModalRecord`.
    // (`seedRow` joined ModalRecord upstream 2026-08 — the @intrinsic
    // edit-mode seed stash; the inherited set tracks the live atom.)
    for inherited in ["id", "name", "description", "status", "seedRow", "createdAt"] {
        assert!(
            field_names.contains(&inherited),
            "inherited field `{}` missing from merged CartItem; got {:?}",
            inherited,
            field_names
        );
    }
    // The 2 extra fields the molecule appended.
    for extra in ["pendingId", "lineItemCount"] {
        assert!(
            field_names.contains(&extra),
            "extra field `{}` missing from merged CartItem; got {:?}",
            extra,
            field_names
        );
    }

    // Total field count == 6 inherited + 2 extras (no collisions in this
    // fixture, so no dedup happens).
    assert_eq!(
        cart.fields.len(),
        8,
        "CartItem must have exactly 8 fields (6 inherited + 2 extras), got {}: {:?}",
        cart.fields.len(),
        field_names
    );

    // The page path override `/cart/extends` must also have applied,
    // proving the Extends path doesn't break the existing trait/page
    // override pipeline.
    let modal_page = oir
        .find_page("ModalRecordModalPage")
        .expect("lowered OIR must contain the inlined ModalRecordModalPage");
    assert_eq!(
        modal_page.path, "/cart/extends",
        "page path override on the Extends-using molecule must apply"
    );
}

// ---------------------------------------------------------------------------
// Phase 3.5.G.F.3 — Generic identifier substitution: events + page rebind
// ---------------------------------------------------------------------------
//
// `cart-events.orb` exercises three Phase F additions in one fixture:
//
// 1. **`events` argument on a TraitReference** — renames `OPEN` → `ADD_ITEM`
//    and `CLOSE` → `CANCEL_ADD`. The lowered OIR must contain `ADD_ITEM`
//    and `CANCEL_ADD` in the trait's events list and in every transition
//    that triggered on the original key. Any render-ui button event whose
//    string value matched an old key must also have been substituted.
//
// 2. **`linkedEntity` argument on a PageRefObject** — rebinds the page's
//    primary_entity from the embedded default to `CartItem`. Phase A only
//    handled `path` overrides on pages; F.3 adds linked_entity (and traits)
//    overrides via the `apply_overrides_to_page` helper.
//
// 3. **The new `events` field round-trips through serde** — without serde
//    `default`, the cart-events fixture would fail to parse because the
//    Reference variant's other fields are also Optional and serde untagged
//    would not pick the Reference arm.

#[test]
fn cart_events_lowered_output_renames_event_keys_in_state_machine() {
    let path = cart_events_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the cart-events fixture");

    let trait_def = oir
        .find_trait("CartItemAddItem")
        .expect("lowered OIR must contain the renamed trait CartItemAddItem");

    let event_keys: Vec<&str> = trait_def.events.iter().map(|e| e.key.as_str()).collect();

    assert!(
        event_keys.contains(&"ADD_ITEM"),
        "events arg must rename OPEN → ADD_ITEM in StateMachine.events; got {:?}",
        event_keys
    );
    assert!(
        event_keys.contains(&"CANCEL_ADD"),
        "events arg must rename CLOSE → CANCEL_ADD in StateMachine.events; got {:?}",
        event_keys
    );
    assert!(
        !event_keys.contains(&"OPEN"),
        "events arg must REMOVE the upstream OPEN key from StateMachine.events; got {:?}",
        event_keys
    );
    assert!(
        !event_keys.contains(&"CLOSE"),
        "events arg must REMOVE the upstream CLOSE key from StateMachine.events; got {:?}",
        event_keys
    );
}

#[test]
fn cart_events_lowered_output_renames_event_keys_on_transitions() {
    let path = cart_events_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the cart-events fixture");

    let trait_def = oir
        .find_trait("CartItemAddItem")
        .expect("lowered OIR must contain the renamed trait CartItemAddItem");

    let transition_events: Vec<&str> = trait_def
        .transitions
        .iter()
        .map(|t| t.event.as_str())
        .collect();

    assert!(
        transition_events.contains(&"ADD_ITEM"),
        "events arg must rename transition.event OPEN → ADD_ITEM; got {:?}",
        transition_events
    );
    assert!(
        transition_events.contains(&"CANCEL_ADD"),
        "events arg must rename transition.event CLOSE → CANCEL_ADD; got {:?}",
        transition_events
    );
    assert!(
        !transition_events.contains(&"OPEN"),
        "events arg must REMOVE the upstream OPEN trigger from any transition; got {:?}",
        transition_events
    );
    assert!(
        !transition_events.contains(&"CLOSE"),
        "events arg must REMOVE the upstream CLOSE trigger from any transition; got {:?}",
        transition_events
    );
}

// ---------------------------------------------------------------------------
// Phase 3.5.G.F.7 — listens / emitsScope post-substitution overrides
// ---------------------------------------------------------------------------
//
// `fixloop-tool-call.orb` references `std-agent-tool-call` (an atom whose
// trait declares external-scoped emits AND a non-empty listens array) and
// passes `listens: []` + `emitsScope: "internal"`. The lowered OIR must
// show the trait's listeners cleared AND every emit's scope set to internal.

// ---------------------------------------------------------------------------
// Phase 3.5.G.F.8 — Per-transition effects override
// ---------------------------------------------------------------------------
//
// `cart-effects-override.orb` references std-modal and supplies an `effects`
// map keyed by the (renamed) ADD_ITEM event. The lowered OIR's CartItemAddItem
// trait must have its ADD_ITEM transition's effects REPLACED by the override
// list — confirming the unified content + effect customization mechanism.

#[test]
#[ignore = "feature removed in Phase 9.5.H — trait-ref `effects:` overrides no longer exist (effects now live only in the inlined trait's transitions). Test predates the removal; keep as a historical marker of the old feature's behaviour."]
fn cart_effects_override_replaces_transition_effects() {
    let path = cart_effects_override_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the cart-effects-override fixture");

    let trait_def = oir
        .find_trait("CartItemAddItem")
        .expect("lowered OIR must contain the renamed trait CartItemAddItem");

    // Find the (closed → open) transition triggered by ADD_ITEM. The
    // upstream OPEN was renamed to ADD_ITEM via `events`, then the override
    // replaced its effects array.
    let add_item_transition = trait_def
        .transitions
        .iter()
        .find(|t| t.event == "ADD_ITEM")
        .expect("must find a transition with event ADD_ITEM after rename");

    // The override produced exactly 2 effects: a fetch + a render-ui block.
    assert_eq!(
        add_item_transition.effects.len(),
        2,
        "override must replace the transition's effects with the 2-element list, got {} effects",
        add_item_transition.effects.len()
    );

    // Spot-check that the first effect is the fetch operator. The OirExpr
    // form is the lowered SExpression — operator name lives in the head
    // position of an Array variant.
    let serialized = serde_json::to_string(&add_item_transition.effects[0])
        .expect("first effect must serialize");
    assert!(
        serialized.contains("\"fetch\""),
        "first effect must be the [\"fetch\", ...] override; got {}",
        serialized
    );

    // Spot-check that the second effect is the render-ui block carrying the
    // cart-specific typography content.
    let render_serialized = serde_json::to_string(&add_item_transition.effects[1])
        .expect("second effect must serialize");
    assert!(
        render_serialized.contains("\"render-ui\""),
        "second effect must be the [\"render-ui\", ...] override; got {}",
        render_serialized
    );
    assert!(
        render_serialized.contains("Add to Cart"),
        "render-ui override must carry the cart-specific typography content; got {}",
        render_serialized
    );
}

#[test]
fn fixloop_tool_call_clears_listens_via_listens_override() {
    let path = fixloop_tool_call_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the fixloop-tool-call fixture");

    let trait_def = oir
        .find_trait("FixLoopValidateCall")
        .expect("lowered OIR must contain the renamed trait FixLoopValidateCall");

    assert!(
        trait_def.listeners.is_empty(),
        "listens: [] override must clear the upstream's listeners; got {} entries",
        trait_def.listeners.len()
    );
}

#[test]
fn fixloop_tool_call_sets_emit_scope_via_emits_scope_override() {
    use orbital_core::ir::OirEventScope;
    let path = fixloop_tool_call_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the fixloop-tool-call fixture");

    let trait_def = oir
        .find_trait("FixLoopValidateCall")
        .expect("lowered OIR must contain the renamed trait FixLoopValidateCall");

    assert!(
        !trait_def.emits.is_empty(),
        "the upstream agent-tool-call atom is expected to declare at least one emit"
    );
    for emit in &trait_def.emits {
        assert_eq!(
            emit.scope,
            OirEventScope::Internal,
            "emitsScope: \"internal\" override must set every emit's scope to Internal; \
             emit \"{}\" still has scope {:?}",
            emit.event,
            emit.scope
        );
    }
}

#[test]
fn cart_events_lowered_output_rebinds_page_linked_entity() {
    let path = cart_events_fixture_path();
    let oir = compile_file(&path, &lowered_oir_options())
        .expect("compile_file must succeed on the cart-events fixture");

    let modal_page = oir
        .find_page("ModalRecordModalPage")
        .expect("lowered OIR must contain the inlined ModalRecordModalPage");

    // Path override (existing Phase A behavior) still applies.
    assert_eq!(
        modal_page.path, "/cart/add",
        "path override must still apply alongside the new linkedEntity override"
    );

    // The PageRefObject.linkedEntity override (Phase F.3) must have
    // rebound primary_entity to CartItem. The TraitReference.linkedEntity
    // arg on the same fixture also rebinds the trait, so primary_entity
    // and the trait's linked_entity should agree.
    let trait_def = oir
        .find_trait("CartItemAddItem")
        .expect("lowered OIR must contain the renamed trait CartItemAddItem");
    assert_eq!(
        trait_def.linked_entity,
        Some("CartItem".to_string()),
        "trait linked_entity override must apply alongside the page override"
    );
}
