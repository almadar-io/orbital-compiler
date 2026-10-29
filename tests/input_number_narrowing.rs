//! §61 OPEN 1 — declared `typeWhen` payload-field conditionals (the generic
//! replacement for the old pattern-specific `inputType: number` narrowing
//! predicate — OWNER RULING: the compiler carries ZERO knob-name or
//! pattern-name vocabulary; a knob-conditional payload type is DECLARED
//! DATA on the field itself, evaluated by one generic evaluator).
//!
//! `.lolo` authors this via type-level `match` now (§25 — `match` REPLACED
//! the `@typeWhen` annotation spelling outright, orbital-lolo's
//! `tests/match_type.rs`). This crate has no `.lolo` front end — it drives
//! `inline_schema` over hand-written `.orb` JSON fixtures directly — so the
//! wire form these fixtures exercise (`PayloadField.typeWhen`,
//! `orbital-core/src/schema/types.rs`) is UNCHANGED and these fixtures
//! stay verbatim: `match` lowers to the identical `typeWhen` list, so
//! everything below (the evaluator, `apply_payload_type_conditionals`, the
//! render-ui binding check) is exactly as reachable from `match`-authored
//! `.lolo` as it always was from `@typeWhen`.
//!
//! Mirrors `config_override_round_trip.rs`'s approach: drive `inline_schema`
//! end-to-end over an in-memory atom + composer fixture shaped like the real
//! `ui-input` atom (a definer-knob `action` emit paired with an `inputType`
//! config knob, its `value` payload field carrying `typeWhen: [{knob:
//! "inputType", equals: "number", type: "number"}]`), then inspect the
//! composed trait's `emits[].payloadSchema` and
//! `stateMachine.events[].payloadSchema`.
//!
//! Coverage:
//!   1. `narrowing_fires_on_literal_number_input_type` — a literal
//!      `inputType: "number"` override narrows `value` on BOTH surfaces.
//!   2. `narrowing_stays_string_with_no_override` / `_with_explicit_text_knob`
//!      — the safety inverse: no knob, or an explicit non-number knob, leaves
//!      `value` typed `string`.
//!   3. `narrowing_stays_string_with_forwarded_config_knob` — a call site
//!      that forwards its OWN unresolved `@config.<knob>` into `inputType`
//!      (the real `ui-form-section` / `ui-input-group` shape) does not
//!      narrow — only a resolved LITERAL `"number"` triggers it.
//!   4. `narrowing_requires_declared_type_when` — a field with NO `typeWhen`
//!      declared is left untouched even when a same-named `inputType` knob
//!      resolves to `"number"` — the mechanism is declaration-gated, never a
//!      structural "definer-knob emit near an inputType knob" shape guess.
//!   5. `narrowed_display_buffer_set_gets_str_concat` /
//!      `unnarrowed_number_field_set_is_untouched` — the atom's OWN internal
//!      `(set @entity.value ?value)` (its `string`-typed DOM display buffer,
//!      triggered by its fixed internal `ACTION` key — the real
//!      service-atom UiInput3 shape, where the OUTBOUND emit renames
//!      to `SET_TTL` but this internal transition still fires on `ACTION`)
//!      gets wrapped in `(str/concat ?value)`; a sibling `set` into an
//!      already-`number`-typed field from the SAME `?value` is left alone.
//!   6. `rui_event_binding_check_passes_under_narrowing` /
//!      `rui_event_binding_check_still_fails_without_narrowing` — the
//!      render-ui `action` prop's event-binding-payload check (registry
//!      `input.action` declares `value: string`, `typeWhen: [{knob:
//!      "inputType", equals: "number", type: "number"}]` — sourced from
//!      `Input.tsx`'s `@payloadTypeWhen` JSDoc via pattern-sync) is bypassed
//!      by the SAME `resolve_payload_type_when` evaluator the inline pass
//!      uses — and still fires when a trait's declared type is `number` for
//!      an unrelated reason (not this narrowing).
//!   7. `type_when_carries_zero_pattern_vocabulary` — the owner's rule,
//!      proven: a fixture using a MADE-UP knob name (`precision`) never seen
//!      in any real pattern narrows identically to the `inputType` fixture,
//!      demonstrating the evaluator (`resolve_payload_type_when` +
//!      `apply_payload_type_conditionals`) carries zero pattern vocabulary —
//!      only the DECLARED `typeWhen` condition drives the outcome.

use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_compiler::phases::parse::parse_file;
use orbital_compiler::phases::validation::{render_ui, ValidationContext};
use orbital_core_base::schema::{
    EntityRef, OrbitalSchema, PayloadField, TraitDefinition, TraitReference,
};

/// Atom fixture shaped like `packages/almadar-std/behaviors/lolo/ui/core/atoms/ui-input.lolo`
/// AND verified against the real composed service-atom UiInput3
/// shape (`orb resolve` on the registry `.orb`): a `config.action : event`
/// definer knob driving `emits { @config.action -> external { value :
/// string! } }`; a sibling `<config_knob>` config knob; and — independent of
/// whatever the `action` knob resolves the OUTBOUND emit's name to — a FIXED
/// internal `ACTION` transition that writes the raw payload into the
/// entity's OWN `value` (string) and `count` (number) fields, plus a
/// render-ui `action` prop bound to the real `input` pattern.
///
/// `type_when` is the literal `typeWhen` JSON array embedded on the `value`
/// payload field of BOTH the emit and the `stateMachine.events[]` entry —
/// `""` (empty string) declares no conditional at all, proving test 4's
/// declaration-gated safety inverse. `config_knob` names the sibling config
/// key the condition (when present) tests against — parameterized so test 7
/// can swap in a knob name that appears in no real pattern anywhere, proving
/// the evaluator carries no vocabulary of its own. `value_field_type` is the
/// emit's/event's AUTHORED `value` payload type — `"string"` for every
/// narrowing-pass test (so narrowing has something to narrow FROM),
/// `"number"` only for the render-ui safety-inverse test (a trait declaring
/// `number` for a reason other than this narrowing).
fn atom_fixture(config_knob: &str, type_when: &str, value_field_type: &str) -> String {
    let payload_field = format!(
        r#"{{ "name": "value", "type": "{value_field_type}", "required": true{type_when} }}"#,
    );
    format!(
        r#"{{
  "name": "test-number-input-atom",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "TestInputAtomOrbital",
      "entity": {{
        "name": "TestInputItem",
        "persistence": "runtime",
        "fields": [
          {{ "name": "id", "type": "string", "required": true }},
          {{ "name": "value", "type": "string" }},
          {{ "name": "count", "type": "number" }}
        ]
      }},
      "traits": [
        {{
          "name": "TestInputRender",
          "linkedEntity": "TestInputItem",
          "category": "interaction",
          "config": {{
            "action": {{ "type": "event", "default": "ACTION" }},
            "{config_knob}": {{ "type": "string", "default": "text" }}
          }},
          "stateMachine": {{
            "states": [ {{ "name": "idle", "isInitial": true }} ],
            "events": [
              {{
                "key": "@config.action",
                "name": "Action",
                "payloadSchema": [ {payload_field} ]
              }}
            ],
            "transitions": [
              {{
                "from": "idle", "to": "idle", "event": "ACTION",
                "effects": [
                  ["set", "@entity.value", "?value"],
                  ["set", "@entity.count", "?value"],
                  ["render-ui", "main", {{ "type": "input", "action": "@config.action", "inputType": "@config.{config_knob}" }}]
                ]
              }}
            ]
          }},
          "emits": [
            {{
              "event": "@config.action",
              "definerKnob": "action",
              "scope": "external",
              "payloadSchema": [ {payload_field} ]
            }}
          ]
        }}
      ]
    }}
  ]
}}"#,
        config_knob = config_knob,
        payload_field = payload_field,
    )
}

/// `typeWhen` JSON fragment testing `<config_knob>=number->number` — the
/// real ui-input shape (parameterized knob name so test 7 can prove the
/// mechanism is vocabulary-free).
fn type_when_number(config_knob: &str) -> String {
    format!(
        r#", "typeWhen": [ {{ "knob": "{config_knob}", "equals": "number", "type": "number" }} ]"#
    )
}

/// Composer fixture parameterized by the caller-supplied `config` JSON
/// fragment (leading-comma, matching `config_override_round_trip.rs`'s
/// convention) applied to the sibling config knob at the `InputRender` call
/// site. Does NOT rebind `linkedEntity` — matches the real corpus
/// (a service atom's `UiInput3 = UiInput.traits.InputRender { config {
/// action: SET_TTL, inputType: number } }` keeps the atom's own `InputItem`
/// entity, confirmed via `orb resolve`), and keeps the atom's `value`/`count`
/// fields visible under their own entity for the display-buffer wrap lookup.
fn composer_source(config_fragment: &str) -> String {
    format!(
        r#"{{
  "name": "test-number-input-composer",
  "version": "1.0.0",
  "orbitals": [
    {{
      "name": "TestComposerOrbital",
      "uses": [
        {{ "from": "./atom.orb", "as": "AtomAlias" }}
      ],
      "entity": {{
        "name": "ComposerRecord",
        "persistence": "runtime",
        "fields": [ {{ "name": "id", "type": "string", "required": true }} ]
      }},
      "traits": [
        {{
          "ref": "AtomAlias.traits.TestInputRender"{config}
        }}
      ]
    }}
  ]
}}"#,
        config = config_fragment,
    )
}

fn materialize(atom_json: &str, config_fragment: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("atom.orb"), atom_json).expect("write atom");
    let composer_path = dir.path().join("composer.orb");
    std::fs::write(&composer_path, composer_source(config_fragment)).expect("write composer");
    (dir, composer_path)
}

/// Inline the composer and return the composed `TestInputRender` trait.
fn inlined_input_trait(path: &std::path::Path) -> TraitDefinition {
    trait_def_named(&inlined_schema(path), "TestInputRender")
}

/// Inline the composer and return the full composed schema — needed by the
/// display-buffer-wrap and render-ui-check tests below, which look past the
/// single extracted trait.
fn inlined_schema(path: &std::path::Path) -> OrbitalSchema {
    let mut schema = parse_file(path).expect("composer must parse");
    let config = InlineConfig::default();
    inline_schema(&mut schema, &config, Some(path)).expect("inline must succeed");
    schema
}

fn trait_def_named(schema: &OrbitalSchema, name: &str) -> TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == name => Some(TraitDefinition::clone(def)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected inlined trait {name}"))
}

/// Run the render-ui binding checks (the pass `ORB_RUI_EVENT_BINDING_PAYLOAD_
/// MISMATCH` comes from) over an already-inlined schema, mirroring
/// `node_field_validation.rs`'s harness. Returns `(error codes, warning
/// codes)`.
fn render_ui_diagnostics(schema: &OrbitalSchema) -> (Vec<String>, Vec<String>) {
    let mut ctx = ValidationContext::new();
    for orbital in &schema.orbitals {
        if let EntityRef::Inline(e) = &orbital.entity {
            ctx.entity_definitions.insert(e.name.clone(), e.clone());
        }
    }
    render_ui::validate_all_render_ui(schema, &mut ctx);
    (
        ctx.result
            .errors
            .iter()
            .map(|e| e.code.as_str().to_string())
            .collect(),
        ctx.result
            .warnings
            .iter()
            .map(|e| e.code.as_str().to_string())
            .collect(),
    )
}

/// The `value` field's declared type off the trait's FIRST emit's
/// `payloadSchema`.
fn emit_value_type(trait_def: &TraitDefinition) -> String {
    let emit = trait_def
        .emits
        .as_ref()
        .and_then(|e| e.first())
        .expect("expected one emit");
    value_field_type(emit.payload_schema.as_ref())
}

/// The `value` field's declared type off the trait's FIRST
/// `stateMachine.events[]` entry that carries a `payloadSchema`.
fn event_value_type(trait_def: &TraitDefinition) -> String {
    let ev = trait_def
        .state_machine
        .events
        .as_ref()
        .into_iter()
        .flatten()
        .find(|e| e.payload_schema.is_some())
        .expect("expected one event with a payloadSchema");
    value_field_type(ev.payload_schema.as_ref())
}

fn value_field_type(schema: Option<&Vec<PayloadField>>) -> String {
    schema
        .expect("payloadSchema present")
        .iter()
        .find(|f| f.name == "value")
        .expect("value field present")
        .field_type
        .clone()
}

// ---------------------------------------------------------------------------
// 1. Narrowing fires
// ---------------------------------------------------------------------------

#[test]
fn narrowing_fires_on_literal_number_input_type() {
    let config_fragment = r#",
          "config": {
            "inputType": { "type": "unknown", "default": "number" }
          }"#;
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "string"),
        config_fragment,
    );
    let trait_def = inlined_input_trait(&composer_path);

    assert_eq!(
        emit_value_type(&trait_def),
        "number",
        "a literal inputType: \"number\" call-site override must narrow the emit's `value` payload field"
    );
    assert_eq!(
        event_value_type(&trait_def),
        "number",
        "the narrowing must mirror onto the matching stateMachine.events[] entry"
    );
}

// ---------------------------------------------------------------------------
// 2. Safety inverse — no knob / a text knob both stay string
// ---------------------------------------------------------------------------

#[test]
fn narrowing_stays_string_with_no_override() {
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "string"),
        "",
    );
    let trait_def = inlined_input_trait(&composer_path);

    assert_eq!(
        emit_value_type(&trait_def),
        "string",
        "with no inputType override, the atom's declared default (\"text\") must leave `value` as string"
    );
    assert_eq!(event_value_type(&trait_def), "string");
}

#[test]
fn narrowing_stays_string_with_explicit_text_knob() {
    let config_fragment = r#",
          "config": {
            "inputType": { "type": "unknown", "default": "text" }
          }"#;
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "string"),
        config_fragment,
    );
    let trait_def = inlined_input_trait(&composer_path);

    assert_eq!(
        emit_value_type(&trait_def),
        "string",
        "an explicit non-number inputType override must leave `value` as string"
    );
    assert_eq!(event_value_type(&trait_def), "string");
}

// ---------------------------------------------------------------------------
// 3. Forwarded / unresolved knob stays string
// ---------------------------------------------------------------------------

#[test]
fn narrowing_stays_string_with_forwarded_config_knob() {
    // The real ui-form-section / ui-input-group shape: the call site forwards
    // its OWN (not-yet-resolved) `mode` knob into inputType rather than
    // supplying a literal. Only a resolved LITERAL "number" may narrow —
    // this call site never produces one.
    let config_fragment = r#",
          "config": {
            "mode": { "type": "string" },
            "inputType": { "type": "unknown", "default": "@config.mode" }
          }"#;
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "string"),
        config_fragment,
    );
    let trait_def = inlined_input_trait(&composer_path);

    assert_eq!(
        emit_value_type(&trait_def),
        "string",
        "a forwarded/unresolved @config.<knob> inputType value must not narrow `value`"
    );
    assert_eq!(event_value_type(&trait_def), "string");
}

// ---------------------------------------------------------------------------
// 4. Declaration-gated — not just "an inputType knob resolves to number nearby"
// ---------------------------------------------------------------------------

#[test]
fn narrowing_requires_declared_type_when() {
    // Same resolved inputType == "number", but the `value` field declares NO
    // `typeWhen` at all. The evaluator must not invent a condition off shape
    // or proximity — only a DECLARED `typeWhen` can ever narrow a field.
    let config_fragment = r#",
          "config": {
            "inputType": { "type": "unknown", "default": "number" }
          }"#;
    let (_dir, composer_path) =
        materialize(&atom_fixture("inputType", "", "string"), config_fragment);
    let trait_def = inlined_input_trait(&composer_path);

    assert_eq!(
        emit_value_type(&trait_def),
        "string",
        "a field with no declared `typeWhen` must not narrow even when a same-named knob resolves to \"number\""
    );
    assert_eq!(event_value_type(&trait_def), "string");
}

// ---------------------------------------------------------------------------
// 5. The internal display-buffer `set` converts (or doesn't)
// ---------------------------------------------------------------------------

/// Pull the fixed internal `ACTION` transition's effects off the composed
/// `TestInputRender` trait — the atom's own display-buffer transition,
/// independent of whatever the `action` knob resolved the outbound emit to.
fn action_transition_effects(
    trait_def: &TraitDefinition,
) -> Vec<orbital_core_base::schema::SExpression> {
    trait_def
        .state_machine
        .transitions
        .iter()
        .find(|t| t.event == "ACTION")
        .and_then(|t| t.effects.clone())
        .expect("expected the ACTION transition with its effects")
}

#[test]
fn narrowed_display_buffer_set_gets_str_concat() {
    let config_fragment = r#",
          "config": {
            "inputType": { "type": "unknown", "default": "number" }
          }"#;
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "string"),
        config_fragment,
    );
    let trait_def = inlined_input_trait(&composer_path);
    let effects = action_transition_effects(&trait_def);

    let expected = orbital_core_base::schema::SExpression::Array(vec![
        orbital_core_base::schema::SExpression::String(("set".to_string()).into()),
        orbital_core_base::schema::SExpression::String(("@entity.value".to_string()).into()),
        orbital_core_base::schema::SExpression::Array(vec![
            orbital_core_base::schema::SExpression::String(("str/concat".to_string()).into()),
            orbital_core_base::schema::SExpression::String(("?value".to_string()).into()),
        ].into()),
    ].into());
    assert_eq!(
        effects[0], expected,
        "the string-typed display-buffer `set` must wrap its `?value` read in `(str/concat ...)`, not bare `concat`"
    );
}

#[test]
fn unnarrowed_number_field_set_is_untouched() {
    let config_fragment = r#",
          "config": {
            "inputType": { "type": "unknown", "default": "number" }
          }"#;
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "string"),
        config_fragment,
    );
    let trait_def = inlined_input_trait(&composer_path);
    let effects = action_transition_effects(&trait_def);

    let untouched = orbital_core_base::schema::SExpression::Array(vec![
        orbital_core_base::schema::SExpression::String(("set".to_string()).into()),
        orbital_core_base::schema::SExpression::String(("@entity.count".to_string()).into()),
        orbital_core_base::schema::SExpression::String(("?value".to_string()).into()),
    ].into());
    assert_eq!(
        effects[1], untouched,
        "a `set` into an already-number-typed entity field must NOT be wrapped — only a string-typed target needs stringifying"
    );
}

// ---------------------------------------------------------------------------
// 6. The render-ui `action` prop's event-binding-payload check
// ---------------------------------------------------------------------------

#[test]
fn rui_event_binding_check_passes_under_narrowing() {
    // Registry `input.action` declares `value: string` with a `typeWhen`
    // condition sourced from Input.tsx's `@payloadTypeWhen` JSDoc; the
    // narrowed trait declares `value: number`. Without the shared evaluator
    // this is a primitive mismatch (`ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH`)
    // — the real corpus proof (a service atom's UiInput3, pre-fix). The
    // check must recognise the SAME narrowing the inline pass applied and
    // not report it.
    let config_fragment = r#",
          "config": {
            "inputType": { "type": "unknown", "default": "number" }
          }"#;
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "string"),
        config_fragment,
    );
    let schema = inlined_schema(&composer_path);
    let (errors, _warnings) = render_ui_diagnostics(&schema);

    assert!(
        !errors.contains(&"ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH".to_string()),
        "a narrowed (number) `value` against the registry's declared `typeWhen`-conditional action payload must not mismatch; errors: {errors:?}"
    );
}

#[test]
fn rui_event_binding_check_still_fails_without_narrowing() {
    // Safety inverse: the trait declares `value: number` for a reason OTHER
    // than this narrowing (inputType stays the default "text" — never
    // resolves to the literal "number"). The registry's `typeWhen` must not
    // resolve either, so the registry-vs-trait mismatch is still reported.
    let (_dir, composer_path) = materialize(
        &atom_fixture("inputType", &type_when_number("inputType"), "number"),
        "",
    );
    let schema = inlined_schema(&composer_path);
    let (errors, _warnings) = render_ui_diagnostics(&schema);

    assert!(
        errors.contains(&"ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH".to_string()),
        "an unnarrowed number `value` (inputType never resolved to \"number\") must still mismatch against the registry's string action payload; errors: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Owner's rule, proven — zero pattern vocabulary in the compiler
// ---------------------------------------------------------------------------

#[test]
fn type_when_carries_zero_pattern_vocabulary() {
    // `precision` names no knob on any real pattern in the corpus (`grep -rn
    // '"precision"' orbital-rust/crates packages/almadar-std packages/almadar-behaviors`
    // — none). The evaluator narrows on it identically to `inputType`,
    // because it holds no knob-name knowledge at all: it only evaluates
    // whatever `typeWhen` condition the field DECLARES.
    let config_fragment = r#",
          "config": {
            "precision": { "type": "unknown", "default": "number" }
          }"#;
    let (_dir, composer_path) = materialize(
        &atom_fixture("precision", &type_when_number("precision"), "string"),
        config_fragment,
    );
    let trait_def = inlined_input_trait(&composer_path);

    assert_eq!(
        emit_value_type(&trait_def),
        "number",
        "a made-up knob name never seen in any real pattern must narrow identically to `inputType` — the compiler carries zero pattern vocabulary"
    );
    assert_eq!(event_value_type(&trait_def), "number");
}
