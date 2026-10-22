//! Config field type validation for `slot` and `pattern`, plus
//! structural shape validation against the typed declaration carriers
//! (`items` / `properties` / `values`) populated by the lolo lowerer.
//!
//! Checks that config fields typed `slot` have defaults in `VALID_SLOTS`,
//! that config fields typed `pattern` have defaults in `PATTERN_TYPES`,
//! and that every config field's `default` value matches its declared
//! structural shape (array vs scalar, object vs primitive, enum
//! membership, per-element / per-property typing recursively).
//! Also validates config overrides in trait references (TODO: requires
//! inline resolution for the upstream declaration lookup).

use std::collections::{HashMap, HashSet};

use orbital_core_base::schema::{
    config_ref_event_knob, definer_knobs_for_trait, ConfigField, EntityDefinition, FieldDefinition,
    FieldType, Orbital, OrbitalSchema, PayloadField, SExpression, TagReading, TraitDefinition,
    TraitReference, UnionVariantChoice,
};
use orbital_core_base::type_compat::set_value_type_compatible;

use super::error::{ErrorCode, ValidationError};
use super::render_ui_binding::{resolve_binding_type, BindingContext, BindingType};
use super::sync::patterns::is_valid_pattern;
use super::{ValidationContext, ValidationResult, VALID_SLOTS};
use crate::embedded::is_known_operator;

/// True when `value` is a computed S-expression (a binding reference or an
/// operator call) rather than a literal — its shape only materializes at
/// runtime, so structural checks against the declared type must defer to
/// the runtime/Layer 2 obligation instead of firing statically. Reuses the
/// same call-detection predicate as `sexpr::validate` (`is_known_operator`
/// or a namespaced `/` operator name) so "is this a call" has one
/// definition across the validator.
fn is_deferred_computed(value: &SExpression) -> bool {
    if value.is_binding() {
        return true;
    }
    if let SExpression::Array(arr) = value {
        if let Some(SExpression::String(first)) = arr.first() {
            return is_known_operator(first) || first.contains('/');
        }
    }
    false
}

/// The trait's declared-event universe for `event`-typed knob membership,
/// plus the knobs wired in DEFINER positions. The event union mirrors the
/// stale-literal check's set (`render_ui/binding.rs` `check_event_literal_stale`
/// — keep the two in sync): emits ∪ listens.event ∪ listens.triggers ∪
/// stateMachine.events keys ∪ transition events. Two twists specific to
/// this consumer (SCAN-EVENT-KNOB-1 corpus simulation, 2026-08-30):
/// - a raw entry that is itself a `@config.<knob>` SENTINEL ("transition on
///   whatever the config says") resolves through that knob's settled default
///   (post-inline the merged trait's `ConfigField.default` carries call-site
///   overrides); unresolvable sentinels drop out of the set;
/// - any knob named in a sentinel/emit position is a DEFINER: its value
///   CREATES the event name (A-UNIFY), so it is exempt from the membership
///   check — any name is valid by construction — while a non-string
///   default is still rejected the same as a reference knob's.
struct TraitEventCtx {
    events: HashSet<String>,
    definer_knobs: HashSet<String>,
}

fn trait_event_ctx(trait_def: &TraitDefinition) -> TraitEventCtx {
    let mut raw: Vec<&str> = Vec::new();
    if let Some(emits) = &trait_def.emits {
        for emit in emits {
            raw.push(emit.event.as_str());
        }
    }
    if let Some(listens) = &trait_def.listens {
        for listen in listens {
            raw.push(listen.event.as_str());
            if let Some(trigger) = &listen.triggers {
                raw.push(trigger.as_str());
            }
        }
    }
    if let Some(events) = &trait_def.state_machine.events {
        for event in events {
            raw.push(event.key.as_str());
        }
    }
    for transition in &trait_def.state_machine.transitions {
        raw.push(transition.event.as_str());
    }
    let mut events: HashSet<String> = HashSet::new();
    for entry in raw {
        if let Some(knob) = config_ref_event_knob(entry) {
            let resolved = trait_def
                .config
                .as_ref()
                .and_then(|c| c.get(knob))
                .and_then(|cf| match &cf.default {
                    Some(SExpression::String(s)) => Some(s.clone()),
                    _ => None,
                });
            if let Some(value) = resolved {
                if !value.is_empty() && !value.starts_with('@') {
                    events.insert(value);
                }
            }
        } else if !entry.is_empty() {
            events.insert(entry.to_string());
        }
    }
    TraitEventCtx {
        events,
        definer_knobs: definer_knobs_for_trait(trait_def),
    }
}

/// The events a scalar `event` knob of `trait_def` may name.
pub fn trait_event_names(trait_def: &TraitDefinition) -> HashSet<String> {
    trait_event_ctx(trait_def).events
}

/// The events an `event`-typed struct member of a knob in `orbital` may name:
/// every inline trait's transitions and emits.
pub fn orbital_event_names(orbital: &Orbital) -> HashSet<String> {
    orbital
        .traits
        .iter()
        .filter_map(|tr| match tr {
            TraitReference::Inline(td) => Some(td),
            _ => None,
        })
        .flat_map(|td| {
            td.state_machine
                .transitions
                .iter()
                .map(|t| t.event.clone())
                .chain(td.emits.iter().flatten().map(|e| e.event.clone()))
        })
        .collect()
}

/// The event vocabulary an `event`-typed struct member is checked against:
/// the owning orbital's own events, plus the program-wide index that
/// resolves a `listens`-addressed value. `refs: None` skips address checks.
#[derive(Default)]
pub(crate) struct EventScope<'a> {
    pub(crate) events: HashSet<String>,
    pub(crate) orbital: &'a str,
    pub(crate) refs: Option<&'a EventRefIndex>,
}

/// An `event` value written with `listens` addressing (`Trait.EVENT`,
/// `Orbital.Trait.EVENT`) rather than a bare own-trait name.
fn is_event_address(value: &str) -> bool {
    !value.starts_with('@') && value.contains('.')
}

/// G-CROSS-046 rung 2: an `EventAddress`-typed member of a `call-service` arg
/// (per the services registry) must name a declared external input of the
/// resolved program. A value baked into an effect is checked here, where an
/// import rename that missed the copy would otherwise reach runtime.
fn validate_call_service_event_addresses(
    schema: &OrbitalSchema,
    registry: &crate::embedded::ServiceRegistryData,
    refs: &EventRefIndex,
    result: &mut ValidationResult,
) {
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for tr in &orbital.traits {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            for (ti, transition) in td.state_machine.transitions.iter().enumerate() {
                for (ei, effect) in transition.effects.iter().flatten().enumerate() {
                    let path = format!(
                        "orbitals[{oi}].traits[{}].stateMachine.transitions[{ti}].effects[{ei}]",
                        td.name
                    );
                    check_event_address_args(effect, &orbital.name, &path, registry, refs, result);
                }
            }
            for (ki, tick) in td.ticks.iter().flatten().enumerate() {
                for (ei, effect) in tick.effects.iter().enumerate() {
                    let path = format!(
                        "orbitals[{oi}].traits[{}].ticks[{ki}].effects[{ei}]",
                        td.name
                    );
                    check_event_address_args(effect, &orbital.name, &path, registry, refs, result);
                }
            }
        }
    }
}

fn check_event_address_args(
    expr: &SExpression,
    owner_orbital: &str,
    path: &str,
    registry: &crate::embedded::ServiceRegistryData,
    refs: &EventRefIndex,
    result: &mut ValidationResult,
) {
    use crate::embedded::EVENT_ADDRESS_TYPE;
    let SExpression::Array(items) = expr else {
        return;
    };
    let head = match items.first() {
        Some(SExpression::String(h)) => h.as_str(),
        _ => "",
    };
    if head == "browser/tab-watch" {
        if let Some(SExpression::Object(request)) = items.get(1) {
            if let Some(SExpression::String(value)) = request.get("event") {
                if !(value.starts_with('@') || value.starts_with('?') || value.starts_with('$')) {
                    let at = format!("{path}.event");
                    for err in refs.check(value, owner_orbital, &at) {
                        result.add_error(err);
                    }
                }
            }
        }
    }
    if head == "call-service" || head == "call_service" {
        if let (
            Some(SExpression::String(service)),
            Some(SExpression::String(action)),
            Some(SExpression::Object(args)),
        ) = (items.get(1), items.get(2), items.get(3))
        {
            let declared = registry.params.get(service).and_then(|a| a.get(action));
            for param in declared.into_iter().flatten() {
                if param.param_type == EVENT_ADDRESS_TYPE {
                    if let Some(SExpression::String(value)) = args.get(&param.name) {
                        if !(value.starts_with('@')
                            || value.starts_with('?')
                            || value.starts_with('$'))
                        {
                            let at = format!("{path}.{}", param.name);
                            for err in refs.check(value, owner_orbital, &at) {
                                result.add_error(err);
                            }
                        }
                    }
                    continue;
                }
                let Some(members) = param.items.as_ref() else {
                    continue;
                };
                let Some(SExpression::Array(entries)) = args.get(&param.name) else {
                    continue;
                };
                for (k, entry) in entries.iter().enumerate() {
                    let SExpression::Object(fields) = entry else {
                        continue;
                    };
                    for (member, decl) in &members.properties {
                        if decl.member_type != EVENT_ADDRESS_TYPE {
                            continue;
                        }
                        let Some(SExpression::String(value)) = fields.get(member) else {
                            continue;
                        };
                        if value.starts_with('@')
                            || value.starts_with('?')
                            || value.starts_with('$')
                        {
                            continue;
                        }
                        let at = format!("{path}.{}[{k}].{member}", param.name);
                        for err in refs.check(value, owner_orbital, &at) {
                            result.add_error(err);
                        }
                    }
                }
            }
        }
    }
    for child in items.iter().skip(1) {
        check_event_address_args(child, owner_orbital, path, registry, refs, result);
    }
}

/// Every inline trait of the program, by `(orbital, trait)`, with its
/// declared external inputs — what a `listens`-addressed event value resolves
/// against.
pub(crate) struct EventRefIndex {
    inputs_by_trait: HashMap<(String, String), Vec<String>>,
}

impl EventRefIndex {
    fn of(schema: &OrbitalSchema) -> Self {
        let mut inputs_by_trait: HashMap<(String, String), Vec<String>> = HashMap::new();
        for orbital in &schema.orbitals {
            for tr in &orbital.traits {
                if let TraitReference::Inline(td) = tr {
                    inputs_by_trait
                        .entry((orbital.name.clone(), td.name.clone()))
                        .or_default();
                }
            }
        }
        for input in orbital_core_base::external_inputs::external_inputs_of(schema) {
            inputs_by_trait
                .entry((input.orbital, input.trait_name))
                .or_default()
                .push(input.event);
        }
        Self { inputs_by_trait }
    }

    /// Resolve `value` from a knob owned by `owner_orbital`: the addressed trait
    /// must exist (a `Trait.EVENT` form within the owner's orbital) and the
    /// event must be one of its declared external inputs.
    fn check(&self, value: &str, owner_orbital: &str, path: &str) -> Vec<ValidationError> {
        use orbital_core_base::schema::{join_event_address, split_event_address, ListenSource};
        let (event, source) = split_event_address(value);
        let (orbital, trait_name) = match &source {
            Some(ListenSource::Trait { trait_name, .. }) => {
                (owner_orbital.to_string(), trait_name.clone())
            }
            Some(ListenSource::Orbital {
                orbital,
                trait_name,
                ..
            }) => (orbital.clone(), trait_name.clone()),
            Some(ListenSource::Any) | None => {
                return vec![ValidationError::error(
                    ErrorCode::TraitEventRefUnresolved,
                    path,
                    format!("Event value '{}' does not address a trait", value),
                )
                .with_suggestion("Write `Trait.EVENT` or `Orbital.Trait.EVENT`")];
            }
        };
        let Some(inputs) = self
            .inputs_by_trait
            .get(&(orbital.clone(), trait_name.clone()))
        else {
            let mut err = ValidationError::error(
                ErrorCode::TraitEventRefUnresolved,
                path,
                format!(
                    "Event value '{}' addresses trait '{}.{}', which is not in this program",
                    value, orbital, trait_name
                ),
            );
            let sibling = matches!(source, Some(ListenSource::Trait { .. }))
                .then(|| {
                    let mut hits: Vec<&String> = self
                        .inputs_by_trait
                        .keys()
                        .filter(|(o, t)| t == &trait_name && o != &orbital)
                        .map(|(o, _)| o)
                        .collect();
                    hits.sort();
                    hits.first().map(|o| (*o).clone())
                })
                .flatten();
            err = match sibling {
                Some(o) => err.with_suggestion(format!(
                    "`Trait.EVENT` resolves within this orbital only; trait '{}' is in orbital '{}' — write `{}`",
                    trait_name,
                    o,
                    join_event_address(
                        &event,
                        Some(&ListenSource::Orbital {
                            orbital: o.clone(),
                            orbital_id: None,
                            trait_name: trait_name.clone(),
                            trait_id: None,
                        })
                    )
                )),
                None => err.with_suggestion(
                    "Import the behavior's orbital first (`orbital X = Alias.orbitals.Y`), then address it by its local name",
                ),
            };
            return vec![err];
        };
        if inputs.contains(&event) {
            return Vec::new();
        }
        let mut valid = inputs.clone();
        valid.sort();
        let declared = if valid.is_empty() {
            "it declares no external inputs".to_string()
        } else {
            format!("its declared external inputs: {}", valid.join(", "))
        };
        vec![ValidationError::error(
            ErrorCode::TraitEventRefNotExternalInput,
            path,
            format!(
                "Event value '{}' names '{}' on '{}.{}', which is not a declared external input — {}",
                value, event, orbital, trait_name, declared
            ),
        )
        .with_suggestion(format!(
            "Declare it on '{}': `listens {{ {} -> external {{ … }} }}`",
            trait_name, event
        ))]
    }
}

/// What `check_reference_binding_type` needs to resolve a config knob's
/// `@entity.X` / `@payload.X` default against the OWNING trait's own scope —
/// built once per `TraitReference::Inline` trait (post-inline, see the call
/// site below), never for a pre-inline `TraitReference::Reference` (no base
/// atom vocabulary is in hand there; the merged Inline arm is the only place
/// both a real declared type AND a resolvable binding co-exist).
struct ReferenceBindingCtx<'a> {
    trait_def: &'a TraitDefinition,
    entity_definitions: &'a HashMap<String, EntityDefinition>,
    /// Union of every payload field this trait's own declared events carry.
    /// A bare config knob default has no single triggering transition the
    /// way a render-ui prop under one specific transition does, so this
    /// widens to the trait's whole event vocabulary — the same
    /// broader-than-render-ui posture `orbital_events` (Phase B, below)
    /// already takes for `event`-typed config knobs in this file.
    payload_fields: Vec<PayloadField>,
}

/// Union of `field.name → PayloadField` across every event this trait's own
/// `state_machine.events[]` declares a `payload_schema` for. First
/// occurrence of a name wins (mirrors the file's other "own-orbital,
/// best-effort" unions rather than rejecting on cross-event disagreement —
/// that's a distinct check, not this one's job).
fn trait_own_payload_fields(trait_def: &TraitDefinition) -> Vec<PayloadField> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    if let Some(events) = &trait_def.state_machine.events {
        for evt in events {
            if let Some(schema) = &evt.payload_schema {
                for f in schema {
                    if seen.insert(f.name.clone()) {
                        out.push(f.clone());
                    }
                }
            }
        }
    }
    out
}

/// A `ConfigField.field_type` tag normalized to the vocabulary
/// `set_value_type_compatible` speaks. That predicate's vocabulary targets
/// entity/struct field types (`string`/`number`/`money`/`node`/PascalCase
/// names/…); a handful of config-knob-specific tags are STRINGS at the
/// binding level but aren't in that vocabulary — `event`/`slot`/`pattern`/
/// `icon`/`component`/`secret`/`asset` (mirrors `check_primitive_shape`'s own
/// `type_tag` handling below, and `FieldType::Slot | FieldType::Pattern |
/// FieldType::Event => BindingType::String` in the render-ui binding
/// resolver). `None` defers entirely for a handful of tags this file has no
/// static opinion on (an unresolved `@trait.X`-only tag, or a generic form
/// like `Map<string,string>`) — permissive by design, matching this file's
/// posture of skipping opaque types it doesn't know how to validate.
fn config_declared_kind(field_type: &str) -> Option<&str> {
    match field_type {
        "event" | "slot" | "pattern" | "icon" | "component" | "secret" | "asset" => Some("string"),
        "int" | "float" => Some("number"),
        "trait" => None,
        // `Map K V` is a real dictionary type, not an opaque generic: it lowers
        // to a `Map<K,V>` tag with the VALUE shape in `items`, and codegen
        // emits `Record<K,V>`. Reporting it as `object` lets a bound
        // `@entity.X` be type-compared (an object/json binding fits, a string
        // does not) instead of opting out of the check entirely. Guarded on
        // the `Map<` PREFIX, never `contains("Map")` — `[MapViewMarkersItem]`
        // is an array of a struct that merely starts with those letters.
        s if s.starts_with("Map<") => Some("object"),
        s if s.contains('<') => None,
        other => Some(other),
    }
}

/// Every `PropSchema.types` value for a prop bound to exactly `value_str`
/// inside a pattern object anywhere in `trait_def`'s own render-ui trees, as
/// a deduped union. `lolo-ui` deliberately collapses a pattern prop's
/// base-type union down to ONE scalar when generating a factory knob
/// (`stat-display.value : number | string` in the registry becomes a knob
/// declared `value : string` — the "collapsed base-union" note in
/// `tools/almadar-pattern-sync/src/commands/lolo-ui.ts`; same root cause
/// tracked for the default-literal side as `PATTERN-BASE-UNION-COLLAPSE-vs-
/// NUMERIC-DEFAULT` in `docs/Almadar_Sync_Gaps.md`), so `ConfigField.
/// field_type` alone under-reports what the knob actually accepts. The
/// pattern registry's `PropSchema.types` still carries the full union —
/// consulted here (read-only; `pattern_registry.rs` is out of this file's
/// edit scope) as the more authoritative "expected" side whenever a site is
/// found; `check_reference_binding_type` falls back to the collapsed
/// `field.field_type` only when it isn't.
///
/// Matching works by VALUE, not by re-deriving which knob feeds which prop:
/// `@config.<knob>` substitutes to its resolved value (declared default,
/// call-site-overridden) directly into the atom's own render-ui tree during
/// inline (`phases/inline/trait.rs` step 3c's `effective_config` fold), so
/// post-inline the override value (`value_str`) sits AT the prop position —
/// the `@config.X` marker is already gone by the time this check runs.
/// Exact-string match only; a value that happens to equal a second,
/// unrelated prop's binding only widens the accepted-type union, never
/// narrows it — the safe direction for a permissive static check.
fn knob_prop_type_union(trait_def: &TraitDefinition, value_str: &str) -> Option<Vec<String>> {
    fn walk(
        expr: &SExpression,
        value_str: &str,
        seen: &mut HashSet<String>,
        union: &mut Vec<String>,
    ) {
        match expr {
            SExpression::Object(map) => {
                if let Some(SExpression::String(ty)) = map.get("type") {
                    if let Some(props) = crate::pattern_registry::get_props_schema(ty.as_str()) {
                        for (k, v) in map {
                            let SExpression::String(s) = v else { continue };
                            if s != value_str {
                                continue;
                            }
                            let Some(schema) = props.get(k) else { continue };
                            for t in &schema.types {
                                if seen.insert(t.clone()) {
                                    union.push(t.clone());
                                }
                            }
                        }
                    }
                }
                for v in map.values() {
                    walk(v, value_str, seen, union);
                }
            }
            SExpression::Array(items) => {
                for item in items {
                    walk(item, value_str, seen, union);
                }
            }
            _ => {}
        }
    }
    let mut seen = HashSet::new();
    let mut union = Vec::new();
    for transition in &trait_def.state_machine.transitions {
        for effect in transition.effects.iter().flatten() {
            walk(effect, value_str, &mut seen, &mut union);
        }
    }
    for tick in trait_def.ticks.iter().flatten() {
        for effect in &tick.effects {
            walk(effect, value_str, &mut seen, &mut union);
        }
    }
    if union.is_empty() {
        None
    } else {
        Some(union)
    }
}

/// C-CONFIG-REF-TYPE-HOLE: a trait-reference call-site config override wires
/// a knob to `@entity.X` / `@payload.X` and is stored `{"type":"unknown",
/// "default":"@entity.X"}` — `"unknown"` is the structural discriminator for
/// wiring-vs-declaration (`is_declaration_form`) and must never be
/// overwritten, so the binding's type was never compared against anything.
/// The merge in `phases/inline/trait.rs` (`apply_overrides_to_trait`) only
/// ever replaces `field.default`, never the atom's own declared
/// `field_type` — so on the post-inline `TraitReference::Inline` arm,
/// `field.field_type` is STILL the real declared type even though
/// `field.default` is the symbolic override value. That is the one point a
/// call-site wiring value has both a knowable type and its own target type
/// in hand at once; resolve the binding here via `resolve_binding_type`
/// (already built for the identical render-ui-prop rule,
/// `ORB_RUI_BINDING_TYPE_MISMATCH`) rather than inventing a second
/// comparator, and reuse `set_value_type_compatible` for the compare.
///
/// Scoped to `@entity` and `@payload` deliberately — the two scopes
/// `resolve_binding_type` already resolves. `@config` is a distinct,
/// separately-measured check (`orbital-lolo/src/lower/checks.rs`); `@trait`
/// is a sentinel with no value type; `@user`/`@item`/`@row`/
/// `@callsitePayload` have no resolver wired here yet and are left
/// deferred (each needs its own measured pass — most `@payload.X` call-site
/// values are in fact retagged to `@callsitePayload.X` during inline before
/// this check ever sees them; see `phases/inline/trait.rs`'s
/// `retag_callsite_payload` — so this arm is live defense for the case
/// where the field is declared LOCALLY, not a no-op).
///
/// **HARD ERROR.** Measured at 508 hits across 1200 files, ALL of which fell
/// into two systemic, largely benign classes rather than 508 distinct bugs —
/// (1) 411 cases where the TARGET RENDER PATTERN accepts a wider type than
/// the wrapping atom's own config-knob declares (`stat-display.value` is
/// registry-typed `[number, string]`, but atoms wrap it behind a knob
/// declared `string`-only; ditto `typography.content` against `node`) —
/// `set_value_type_compatible` has no "number renders fine as a
/// string/node" widening (unlike its `date` family arm); (2) 96 cases where
/// `@entity.focusedRow.<field>` resolves to `object` because `focusedRow` is
/// declared a schemaless `object` (no `properties`) on `std-thread.lolo` —
/// `resolve_binding_type` conservatively stops at the container's own type
/// when it can't see inside, which is correct behavior, not a resolver bug,
/// but leaves nested access through it unresolvable to a leaf type.
fn check_reference_binding_type(
    field: &ConfigField,
    value_str: &str,
    path: &str,
    bctx: &ReferenceBindingCtx,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    // The structural discriminator itself — a knob whose own declared type
    // is still `unknown` has nothing to check against. Only reachable for a
    // brand-new knob an alias trait declares via a plain wiring value with
    // no type of its own (`phases/inline/trait.rs`'s "declare a brand-new
    // knob" step); ordinary overrides of an existing knob always carry the
    // base atom's real type here.
    if field.field_type == "unknown" {
        return result;
    }
    let render_ctx = BindingContext {
        trait_def: bctx.trait_def,
        entity_definitions: bctx.entity_definitions,
        event_payload: Some(&bctx.payload_fields),
        sibling_trait_names: &[],
    };
    let resolved = resolve_binding_type(&SExpression::String(value_str.to_string()), &render_ctx);
    // `Unknown` (resolver couldn't determine — unlinked entity, undeclared
    // payload field), `Null`, and `TraitRef` (never produced for `@entity`/
    // `@payload` paths, kept for lattice completeness) all mean "can't
    // compare" rather than "mismatch" — skip, mirroring the render-ui
    // binding rule's own posture (`BindingType::Unknown` doc comment).
    let actual = match resolved {
        BindingType::Unknown | BindingType::Null | BindingType::TraitRef => return result,
        // A path that bottoms out in a SCHEMALESS object carries no leaf type:
        // `@entity.focusedRow.title` where `focusedRow : object` has no
        // `properties` resolves to Object, which is the resolver correctly
        // declining to guess — not evidence of a mismatch. Reporting it anyway
        // produced 81 unactionable findings in one corpus sweep, all on the
        // same schemaless field. Treat "could not resolve a leaf" as unresolved,
        // exactly like Unknown above.
        BindingType::Object if value_str.matches('.').count() > 1 => return result,
        other => other.registry_type(),
    };
    // Prefer the pattern registry's full prop-type union when a direct site
    // is found (see `knob_prop_type_union`'s doc — this is the fix for the
    // `lolo-ui` base-union collapse); an opaque member (a tag
    // `config_declared_kind` has no opinion on) counts as a pass rather than
    // a fail, same permissive posture as the collapsed single-tag path
    // below. Fall back to the atom's own (possibly collapsed)
    // `field.field_type` only when no site is found — e.g. the value flows
    // into a non-render-ui effect, or the pattern-prop position doesn't
    // carry it verbatim.
    // `node` accepts any scalar (React paints a number or boolean as text
    // just as happily as a string) — that widening now lives in the Node arm
    // of orbital-core's `sub` relation (Phase 4.1), so no file-local
    // special case is needed here.
    let compatible = if let Some(types) = knob_prop_type_union(bctx.trait_def, value_str) {
        types.iter().any(|t| match config_declared_kind(t) {
            Some(d) => set_value_type_compatible(d, actual),
            None => true,
        })
    } else {
        match config_declared_kind(&field.field_type) {
            Some(d) => set_value_type_compatible(d, actual),
            None => return result,
        }
    };
    if !compatible {
        let scope = if value_str.starts_with("@entity.") {
            "entity"
        } else {
            "event-payload"
        };
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitConfigShapeMismatch,
                path,
                format!(
                    "`{}` resolves to `{}`, incompatible with the config knob's declared type `{}`",
                    value_str, actual, field.field_type
                ),
            )
            .with_suggestion(format!(
                "bind a `{}`-typed {} field here, or change the knob's declared type",
                field.field_type, scope
            )),
        );
    }
    result
}

/// Validate all `slot` and `pattern` config fields in the schema.
pub fn validate_all_config_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    result.merge(validate_dangling_config_refs(schema));
    validate_groupby_enum_column_gap(schema, ctx, &mut result);
    validate_embed_config_forward_multi_referrer(schema, &mut result);

    let event_refs = EventRefIndex::of(schema);
    validate_call_service_event_addresses(schema, &ctx.service_registry, &event_refs, &mut result);
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        // Phase B (SCAN-EVENT-KNOB-2): orbital-wide event vocabulary for
        // struct-field `event` acceptance (e.g. `ItemAction.event`) — every
        // inline trait's own transitions ∪ every inline trait's own emits,
        // mirroring closed_circuit.rs's orphan-event acceptance predicate
        // (`orbital_handled` ∪ `declared_emits`) at a per-ORBITAL grain
        // rather than closed_circuit's per-trait grain (deliberately a
        // touch broader — CIRCUIT_ORPHAN_EVENT remains the precise, only
        // authoritative orphan detector; an honest double-report on the
        // same defect from both codes is fine, no dedup machinery here).
        let orbital_events = EventScope {
            events: orbital_event_names(orbital),
            orbital: &orbital.name,
            refs: Some(&event_refs),
        };

        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let path_prefix = super::trait_path(oi, &orbital.name, ti, trait_ref);
            match trait_ref {
                TraitReference::Inline(trait_def) => {
                    if let Some(config) = &trait_def.config {
                        let event_ctx = trait_event_ctx(trait_def);
                        let binding_ctx = ReferenceBindingCtx {
                            trait_def,
                            entity_definitions: &ctx.entity_definitions,
                            payload_fields: trait_own_payload_fields(trait_def),
                        };
                        for (field_name, field) in config.iter() {
                            let field_path = format!("{}.config({})", path_prefix, field_name);
                            result.merge(validate_config_field(
                                field,
                                &field_path,
                                Some((&event_ctx, field_name.as_str())),
                                &orbital_events,
                                Some(&binding_ctx),
                            ));
                        }
                    }
                }
                TraitReference::Reference {
                    config: Some(config),
                    ..
                } => {
                    for (field_name, cf) in config.iter() {
                        let field_path = format!("{}.config({})", path_prefix, field_name);
                        // When the call-site entry is a full annotated declaration
                        // (not a plain wiring), validate its `@tier` and structural
                        // shape — the same checks that run for atom declarations.
                        // Plain wiring entries (`field_type == "unknown"`) are
                        // skipped: the base atom's declaration validates the type;
                        // we only have a value here with no declared shape. The
                        // `@entity`/`@payload` binding-type check (C-CONFIG-REF-
                        // TYPE-HOLE) has the identical constraint — no base-atom
                        // type is knowable pre-inline — and is likewise deferred
                        // to the merged Inline arm above, which is the ONLY point
                        // both the atom's real type and the override value are in
                        // hand together.
                        // Event membership is skipped here too (`None` ctx): a
                        // pre-inline Reference has no base-trait vocabulary; the
                        // merged Inline arm above owns that check post-inline.
                        if cf.field_type != "unknown" {
                            result.merge(validate_config_field(
                                cf,
                                &field_path,
                                None,
                                &orbital_events,
                                None,
                            ));
                        }
                        let _ = (field_name, field_path);
                    }
                }
                _ => {}
            }
        }
    }

    ctx.merge(result);
}

/// Validate a single config field's default value against its declared type.
/// `event_ctx` carries the owning trait's event universe + definer-knob set
/// and this field's knob name; `None` = Reference arm / shape-only callers
/// (event membership is skipped, everything else unchanged). `orbital_events`
/// is the Phase B struct-field acceptance set (see `validate_all_config_types`).
/// `binding_ctx` is `Some` only for the post-inline `TraitReference::Inline`
/// arm — it drives the `@entity`/`@payload` binding-type check
/// (`check_reference_binding_type`), which needs the base atom's declared
/// type to already be in hand (see that function's doc comment).
fn validate_config_field(
    field: &orbital_core_base::schema::ConfigField,
    path: &str,
    event_ctx: Option<(&TraitEventCtx, &str)>,
    orbital_events: &EventScope<'_>,
    binding_ctx: Option<&ReferenceBindingCtx>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Reject `[object]` (and the bare `object` variant) as a declared
    // type — both lower to an untyped `Array<Json>` / `Json` that the
    // strict-lint floor catches as `EventPayloadValue` / `Record<string,
    // FieldValue | undefined>[]` in emitted TS. The lolo author should
    // declare a named struct alias (e.g. `type FieldConfig = { name:
    // string, label: string, variant: string }`) and use `[FieldConfig]`
    // so the codegen carries real types end-to-end.
    //
    // The `render-ui` type is the deliberate exception: it carries a
    // recursive render-ui SExpr that's validated via the existing
    // render-ui validator + emitted as a proper RenderTree TS type by
    // the std-ts factory-signature codegen — not an untyped Json bag.
    let ft = field.field_type.as_str();
    if (ft == "[object]" || ft == "object") && ft != "render-ui" {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitConfigObjectArrayForbidden,
                path,
                format!(
                    "Config field uses banned type '{}'. Declare a named struct alias (e.g. `type FieldConfig = {{ name: string, label: string }}`) and use the alias (`[FieldConfig]`) so the catalog + TS codegen carry real types. For inline render-ui trees, use the `render-ui` type instead.",
                    ft
                ),
            )
            .with_suggestion(
                "Author the shape: `type <Name> = { field1: <T1>, field2: <T2> }`, then use `[<Name>]` (for arrays) or `<Name>` (for single objects) on the config field. For render-ui trees use `: render-ui`.",
            ),
        );
    }

    // Structural shape check — runs against the typed carriers
    // (`items` / `properties` / `values`) populated by the lolo lowerer.
    // Skips fields with no `default` and skips opaque types we don't
    // know how to validate (`object` / `unknown` / etc.).
    if let Some(default) = &field.default {
        result.merge(check_config_default_shape(
            field,
            default,
            path,
            orbital_events,
            binding_ctx,
        ));
    }

    // `@tier` annotation — defense-in-depth. The parser already
    // restricts the accepted sigil to one of `label | description |
    // synonyms | tier`, but the tier *value* string is free-form at
    // parse time. The L2 rule pins it to the five allowed buckets so
    // any malformed `.orb` IR that bypasses the parser also fails fast.
    // `policy` (governance: audit/access/approval/erasure/notify) and
    // `infra` (operational: lifecycle/reminder/cascade/cross-reference)
    // are decision kinds shown by default alongside `domain`.
    if let Some(tier) = &field.tier {
        const VALID_TIERS: &[&str] = &["domain", "policy", "infra", "presentation", "internal"];
        if !VALID_TIERS.contains(&tier.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigTierInvalid,
                    path,
                    format!(
                        "Config field `@tier` value '{}' is invalid. Allowed values: {}.",
                        tier,
                        VALID_TIERS.join(", ")
                    ),
                )
                .with_suggestion(format!("Use one of: {}.", VALID_TIERS.join(", "))),
            );
        }
    }

    // `render-ui` typed config fields carry a full render-ui SExpr tree
    // as their default — variants override it with their own tree. The
    // tree is validated by the same render-ui validator that runs on
    // inline `(render-ui main { ... })` effects; it knows to skip
    // `@`-bindings, validate pattern names, recurse children, etc.
    //
    // Note: we do NOT call into render_ui::validate_render_ui_effect
    // here because that function expects the full `(render-ui slot
    // { tree })` envelope. The default is already the tree itself;
    // the existing render-ui binding context isn't available at config
    // declaration time (entity defs, extra patterns). Trust the
    // default's shape here — defense-in-depth lives in the call-site
    // override validation, which runs with full context.
    if field.field_type == "render-ui" {
        if let Some(default) = &field.default {
            if !matches!(default, SExpression::Object(_) | SExpression::String(_)) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        "Config field typed `render-ui` must default to an object literal (a render-ui tree) or a string binding (e.g. `@config.someTree`).",
                    )
                    .with_suggestion(
                        "Author the default as an object literal: `bodyContent : render-ui = { type: \"stack\", children: [...] }`.",
                    ),
                );
            }
        }
        return result;
    }

    // Gate A (type-integrity campaign): a `node`-typed config knob's default
    // is renderable content — a pattern object, a literal array of them, or a
    // string leaf / `@config.*` binding. Static pattern checks (registry
    // membership, prop validity) run with an empty entity context — binding-
    // level checks need the call site and skip harmlessly. Closes the
    // `check_primitive_shape` `_ => true` fallthrough that let e.g.
    // `children : node = [{ type: canvas-2d }]` pass unexamined.
    if field.field_type == "node" || field.field_type == "[node]" {
        if let Some(default) = &field.default {
            match default {
                SExpression::Object(_) | SExpression::Array(_) => {
                    let empty: std::collections::HashMap<
                        String,
                        orbital_core_base::schema::EntityDefinition,
                    > = std::collections::HashMap::new();
                    // A node knob legally holds one pattern object OR an array
                    // of them — validate per element so the array root itself
                    // is never misread as a pattern (string leaves and null
                    // slots are legal node values and skipped).
                    // An operator-call array (`(if …)`, `(array/concat …)` —
                    // first element is a String head) is a COMPUTED node
                    // value, not a literal: skip static pattern checks (the
                    // call-site/binding layers own it).
                    if let SExpression::Array(items) = default {
                        if matches!(items.first(), Some(SExpression::String(_))) {
                            return result;
                        }
                    }
                    let elements: Vec<&SExpression> = match default {
                        SExpression::Array(items) => items.iter().collect(),
                        other => vec![other],
                    };
                    for element in elements {
                        if matches!(element, SExpression::String(_) | SExpression::Null) {
                            continue;
                        }
                        result.merge(
                            crate::phases::validation::render_ui::pattern::validate_pattern(
                                element, path, &empty, None, None,
                            ),
                        );
                    }
                }
                SExpression::String(_) | SExpression::Null => {}
                _ => {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitConfigShapeMismatch,
                            path,
                            "Config field typed `node` must default to a pattern object, an array of pattern objects, a string leaf, or a `@config.*` binding.",
                        )
                        .with_suggestion(
                            "Author the default as renderable content: `children : node = [{ type: \"canvas-2d\" }]`.",
                        ),
                    );
                }
            }
        }
        return result;
    }

    // `event` values are event-name REFERENCES into the owning trait's own
    // event set (SCAN-EVENT-KNOB-1). Post-inline the merged trait's config
    // carries the settled (override-folded, rename-folded) value, so
    // membership here is the declaration-site twin of the render-ui
    // stale-literal check — same set, three phases earlier. Empty string is
    // the default-off outlet; `@`-prefixed values are forwards resolved
    // elsewhere; an empty universe is vacuous (mirrors the stale check).
    if field.field_type == "event" {
        if let Some((ctx, knob_name)) = event_ctx {
            // A-UNIFY: a DEFINER knob (`emits { @config.<k> }` or a
            // stateMachine sentinel key) is where the event name is
            // CREATED, not referenced — any name is valid by construction,
            // so it skips membership in the trait's own event set. Its
            // default still must be a plain string (or absent/deferred):
            // the `Some(other)` arm below rejects non-string values for
            // both roles alike.
            let is_definer = ctx.definer_knobs.contains(knob_name);
            match &field.default {
                None => {}
                Some(SExpression::String(s)) if is_event_address(s) => {
                    if is_definer {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitInvalidEventDefault,
                                path,
                                format!(
                                    "Config field typed `event` holds the address '{}' but this knob names an event the trait emits — an emitted event is a bare name",
                                    s
                                ),
                            )
                            .with_suggestion("Use a bare event name here; `Trait.EVENT` addresses another trait's input"),
                        );
                    } else if let Some(refs) = orbital_events.refs {
                        result.add_errors(refs.check(s, orbital_events.orbital, path));
                    }
                }
                Some(SExpression::String(s)) => {
                    if !is_definer
                        && !s.is_empty()
                        && !s.starts_with('@')
                        && !ctx.events.is_empty()
                        && !ctx.events.contains(s)
                    {
                        let mut valid: Vec<String> = ctx.events.iter().cloned().collect();
                        valid.sort();
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitInvalidEventDefault,
                                path,
                                format!(
                                    "Config field typed `event` holds '{}' but the trait's event set has no such key — must be one of: {}",
                                    s,
                                    valid.join(", ")
                                ),
                            )
                            .with_valid_values(valid),
                        );
                    }
                }
                Some(other) => {
                    if !is_deferred_computed(other) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitInvalidEventDefault,
                                path,
                                "Config field typed `event` must hold a string event name",
                            )
                            .with_suggestion("Use a string event key, e.g. `event : event = SEARCH`, or \"\" for a default-off outlet"),
                        );
                    }
                }
            }
        }
        return result;
    }

    // `secret` values are credential REFERENCES — env-var key names resolved
    // by `resolveCredentialRef`, never secret material. A non-string default
    // is decidably wrong (I-29).
    if field.field_type == "secret" {
        if let Some(default) = &field.default {
            if !matches!(default, SExpression::String(_)) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitSecretDefaultNotString,
                        path,
                        "Config field typed `secret` must default to a string credential reference (an env-var key name)",
                    )
                    .with_suggestion("Use a string env-var name, e.g. `connectionRef : secret = \"CRM_DB_URL\"`"),
                );
            }
        }
        return result;
    }

    let default_str = match &field.default {
        Some(SExpression::String(s)) => s.as_str(),
        _ => return result, // Non-string defaults can't be slot/pattern values
    };

    match field.field_type.as_str() {
        "slot" => {
            if !VALID_SLOTS.contains(&default_str) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitInvalidSlotDefault,
                        path,
                        format!(
                            "Config field typed `slot` has invalid default '{}' — must be one of: {}",
                            default_str,
                            VALID_SLOTS.join(", ")
                        ),
                    )
                    .with_valid_values(VALID_SLOTS.iter().map(|s| s.to_string()).collect()),
                );
            }
        }
        "pattern" => {
            if !is_valid_pattern(default_str) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitInvalidPatternDefault,
                        path,
                        format!(
                            "Config field typed `pattern` has invalid default '{}' — must be a known pattern type",
                            default_str
                        ),
                    )
                    .with_suggestion("Use a known pattern type or ensure custom pattern is registered"),
                );
            }
        }
        _ => {}
    }

    result
}

/// Walk the declared shape (a `ConfigField` carrying `field_type` +
/// optional `items` / `properties` / `values`) and check the actual
/// `default` value against it.
fn check_config_default_shape(
    field: &ConfigField,
    default: &SExpression,
    path: &str,
    orbital_events: &EventScope<'_>,
    binding_ctx: Option<&ReferenceBindingCtx>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    // Binding-ref defaults resolve at inline time against the composing
    // scope — their shape can't be checked statically against the declared
    // type here, with two exceptions: `@entity.X` / `@payload.X` are
    // resolvable THIS side of inline (see `check_reference_binding_type`),
    // so on the post-inline Inline arm (`binding_ctx: Some`) they get a real
    // type-compare instead of a blanket skip. Every other sigil (`@config`,
    // `@trait`, `@user`/`@item`/`@row`/`@callsitePayload`) still defers.
    // (Degenerate same-key self-reference is guarded at L1, where
    // declarations and rebind override blocks are still syntactically
    // distinct.)
    if let SExpression::String(s) = default {
        if s.starts_with('@') {
            if let Some(bctx) = binding_ctx {
                if s.starts_with("@entity.") || s.starts_with("@payload.") {
                    result.merge(check_reference_binding_type(field, s, path, bctx));
                }
            }
            return result;
        }
    }
    // Computed S-expression default (e.g. `(array/map @entity.tiles (fn
    // [t] {...}))`) — an operator call, not a literal. It lowers to the
    // same `SExpression::Array` variant as a literal array, so it must be
    // distinguished before the array/object literal checks below run, or
    // the call's operator/argument slots get misread as literal elements.
    // Its shape only materializes at runtime — defer to Layer 2.
    if is_deferred_computed(default) {
        return result;
    }
    // Map slot — `field_type` is `Map<K,V>` and `items` carries the VALUE
    // shape (the lowerer reuses the array carrier; `resolve_config_field_shape`
    // disambiguates map-from-array by this tag). Verify the value is an object
    // literal, then recurse every entry through the `FieldDefinition`
    // validator. Keys are dynamic by construction, so only values are checked.
    //
    // Without this arm a `Map` knob fell straight through to
    // `check_primitive_shape`'s `_ => true`, so `Map<string,string>` accepted
    // `{ a: 3 }` — the same hole that made a named struct type decoration, and
    // the reason `Map` could not be offered as the honest replacement for
    // `json` (retyping into an unchecked type is laundering, not typing).
    //
    if field.field_type.starts_with("Map<") {
        let SExpression::Object(entries) = default else {
            result.add_error(ValidationError::error(
                ErrorCode::TraitConfigShapeMismatch,
                path,
                format!(
                    "config default has type '{}' but the value isn't an object",
                    field.field_type
                ),
            ));
            return result;
        };
        if let Some(value_schema) = &field.items {
            let ancestors = root_ancestor_map(field, Some(value_schema));
            let mut probe = ValidationResult::new();
            for (key, entry) in entries {
                probe.merge(check_field_def_shape(
                    value_schema,
                    entry,
                    &format!("{}.{}", path, key),
                    orbital_events,
                    &ancestors,
                    false,
                ));
            }
            for e in probe.errors {
                result.add_error(ValidationError::error(e.code, e.path, e.message));
            }
            result.warnings.extend(probe.warnings);
        }
        return result;
    }
    // Tuple slot — `field_type` is `[T1, T2, …]`; the slots ride `properties` by index.
    if orbital_core_base::type_compat::tuple_type_slots(&field.field_type).is_some() {
        let slots = orbital_core_base::schema::tuple_slots_of(field.properties.as_ref());
        return check_tuple_shape(
            &slots,
            default,
            path,
            orbital_events,
            &HashMap::new(),
            false,
        );
    }
    // Array slot — `field_type` is `[T]` (string tag) and `items` carries
    // the per-element shape. Verify the value is an array, then recurse
    // each element through the `FieldDefinition` validator.
    if field.field_type.starts_with('[') && field.field_type.ends_with(']') {
        let SExpression::Array(items) = default else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    format!(
                        "config default has type '{}' but the value isn't an array",
                        field.field_type
                    ),
                )
                .with_suggestion(format!(
                    "supply an array literal matching {}",
                    field.field_type
                )),
            );
            return result;
        };
        if let Some(item_schema) = &field.items {
            let ancestors = root_ancestor_map(field, Some(item_schema));
            for (i, elem) in items.iter().enumerate() {
                let elem_path = format!("{}[{}]", path, i);
                result.merge(check_field_def_shape(
                    item_schema,
                    elem,
                    &elem_path,
                    orbital_events,
                    &ancestors,
                    false,
                ));
            }
        }
        return result;
    }
    // Closed-string enum (lolo `"a" | "b" | "c"` lowers to
    // `field_type: "string"` + `values: ["a","b","c"]`). Default must be
    // one of the listed strings.
    if let Some(values) = &field.values {
        if let SExpression::String(s) = default {
            if !s.starts_with('@') && !values.iter().any(|v| v == s) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "config default '{}' isn't a legal enum value (allowed: {})",
                            s,
                            values.join(", ")
                        ),
                    )
                    .with_valid_values(values.clone()),
                );
            }
            return result;
        }
    }
    // Named struct type (`lighting : Lighting3D`, `position : ScenePos`) — the
    // shape lives in `properties`, and until now nothing walked it. The checks
    // above cover `[T]` and closed enums; everything else fell to
    // `check_primitive_shape`, whose last arm is `_ => true // Unknown type
    // tags pass`. So a declared struct was a DECLARATION, not a check: a knob
    // typed `EmptyPayload` accepted `{ id: "x", extra: 3 }` silently.
    //
    // Only declared keys that are PRESENT are checked. Absent-key (required
    // member missing) is deliberately not added in the same change — one new
    // rule at a time, so a corpus movement has one attributable cause.
    //
    if let Some(props) = &field.properties {
        if let SExpression::Object(map) = default {
            // A property can self-reference the OWNING alias (`type MenuItem
            // = { subMenu : [MenuItem] }` on a knob declared `lighting :
            // MenuItem`) — the ancestor for that case is THIS struct itself,
            // synthesized here since a `ConfigField` isn't a `FieldDefinition`
            // (they're structurally parallel but distinct wire types).
            let root_struct = FieldDefinition {
                mock: None,
                merged_from: None,
                name: None,
                field_type: FieldType::Object,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: None,
                items: None,
                properties: Some(props.clone()),
                intrinsic: None,
                description: None,
                synonyms: None,
                projected_from: None,
            };
            let ancestors = root_ancestor_map(field, Some(&root_struct));
            let mut probe = ValidationResult::new();
            for (key, prop_schema) in props {
                if let Some(prop_value) = map.get(key) {
                    probe.merge(check_field_def_shape(
                        prop_schema,
                        prop_value,
                        &format!("{}.{}", path, key),
                        orbital_events,
                        &ancestors,
                        false,
                    ));
                }
            }
            for e in probe.errors {
                result.add_error(ValidationError::error(e.code, e.path, e.message));
            }
        }
    }
    // Primitive leaf type check.
    result.merge(check_primitive_shape(&field.field_type, default, path));
    result
}

/// The bare alias-name candidate a `ConfigField.field_type` string names,
/// stripping array (`[X]` → `X`) and Map-value (`Map<K,V>` → `V`) wrapper
/// syntax. A harmless no-op key when the config field isn't actually
/// recursive — the returned name is only ever consulted by a back-reference
/// that spells the SAME name, and a non-recursive field's descendants never
/// carry one.
fn config_field_root_alias_name(field_type: &str) -> &str {
    let trimmed = field_type.trim();
    if let Some(inner) = trimmed
        .strip_prefix("Map<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return inner.rsplit(',').next().map(str::trim).unwrap_or(inner);
    }
    trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed)
}

/// Seed `check_field_def_shape`'s ancestor map from the ONE `.orb` position a
/// struct alias's own NAME survives to — the declaring `ConfigField`'s
/// `field_type` string itself. Everything the field's `items`/`properties`
/// carry below that is anonymous (the alias name is erased by lowering), so
/// this can only ever discover the field's OWN root alias, never a nested
/// one — matching `check_field_def_shape`'s doc comment on the limit.
fn root_ancestor_map<'a>(
    field: &ConfigField,
    root_schema: Option<&'a FieldDefinition>,
) -> HashMap<String, &'a FieldDefinition> {
    let mut map = HashMap::new();
    if let Some(schema) = root_schema {
        let name = config_field_root_alias_name(&field.field_type);
        if !name.is_empty() {
            map.insert(name.to_string(), schema);
        }
    }
    map
}

/// Walk a `FieldDefinition` (used by `ConfigField.items` /
/// `ConfigField.properties[k]`) against a concrete `SExpression`. Same
/// rules as `check_config_default_shape` but typed against
/// `FieldDefinition` instead of `ConfigField`.
///
/// `ancestors`: alias name → the `FieldDefinition` it names, seeded by the
/// caller from the outermost `ConfigField.field_type` (the one position that
/// still carries the alias's NAME — `resolve_config_field_shape`'s "[Name]"/
/// "Map<K,Name>" string; everything below is the anonymous, name-erased
/// `.orb` object/array carrier). A recursive struct's back-reference
/// (`FieldType::Union`, `values: [Name]`, `properties: None`) re-enters
/// `ancestors[Name]` instead of skipping — the depth-3 literal check (feature
/// test 5) needs this to actually walk INTO the recursion rather than
/// stopping at "nothing carried to check". Only names discoverable at the
/// outermost declaration are ever in the map (a config field's `field_type`
/// string is the only wire-format position a struct alias's OWN name
/// survives to); a back-reference to any other name still falls back to
/// "nothing to check", never panics.
/// `skip_required_members`: entity defaults are an INITIAL state, not a
/// complete value — `!` on a struct/alias member is enforced at PERSIST
/// time (lifecycle ruling), so the entity adapter (`entity.rs`) passes
/// `true` to skip the "required member missing on object literal" check
/// below while still checking every member's KIND. Trait config keeps the
/// full literal (`false`) — a config default has no later persist step to
/// defer completeness to.
/// A tuple value: an array of exactly the tuple's arity, each element checked against
/// its own slot.
fn check_tuple_shape(
    slots: &[&FieldDefinition],
    value: &SExpression,
    path: &str,
    orbital_events: &EventScope<'_>,
    ancestors: &HashMap<String, &FieldDefinition>,
    skip_required_members: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let SExpression::Array(items) = value else {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitConfigShapeMismatch,
                path,
                format!(
                    "expected a tuple of {} values, got {}",
                    slots.len(),
                    super::sexpr::render_sexpr_brief(value)
                ),
            )
            .with_suggestion(format!(
                "supply `[ … ]` with exactly {} values",
                slots.len()
            )),
        );
        return result;
    };
    if items.len() != slots.len() {
        result.add_error(
            ValidationError::error(
                ErrorCode::TraitConfigShapeMismatch,
                path,
                format!(
                    "expected a tuple of {} values, got {}",
                    slots.len(),
                    items.len()
                ),
            )
            .with_suggestion(format!("supply exactly {} values", slots.len())),
        );
        return result;
    }
    for (i, (elem, slot)) in items.iter().zip(slots).enumerate() {
        result.merge(check_field_def_shape(
            slot,
            elem,
            &format!("{}[{}]", path, i),
            orbital_events,
            ancestors,
            skip_required_members,
        ));
    }
    result
}

pub(crate) fn check_field_def_shape(
    schema: &FieldDefinition,
    value: &SExpression,
    path: &str,
    orbital_events: &EventScope<'_>,
    ancestors: &HashMap<String, &FieldDefinition>,
    skip_required_members: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    // Same computed-value deferral as the top-level default: a per-element
    // or per-property value can itself be a binding or an operator call
    // (e.g. an object property whose value is `(array/map ...)`).
    if is_deferred_computed(value) {
        return result;
    }
    match schema.field_type {
        // Tagged union: pick the variant by the value's own discriminator, then
        // check the value against THAT variant only. Checking against all of
        // them would report every non-matching variant's fields as errors; not
        // checking at all is what `json` did.
        FieldType::Union => {
            let (Some(names), Some(_)) = (&schema.values, &schema.properties) else {
                // By-name recursive back-reference. Re-enter the ancestor
                // whose expansion this reference names, if the outermost
                // declaration made it discoverable; otherwise there is
                // genuinely nothing carried to check — skip, never panic.
                let Some(names) = &schema.values else {
                    return result;
                };
                let Some(ancestor) = names.first().and_then(|n| ancestors.get(n)) else {
                    return result;
                };
                return check_field_def_shape(
                    ancestor,
                    value,
                    path,
                    orbital_events,
                    ancestors,
                    skip_required_members,
                );
            };
            let SExpression::Object(map) = value else {
                let fits = schema
                    .properties
                    .iter()
                    .flat_map(|p| names.iter().filter_map(|n| p.get(n)))
                    .any(|v| {
                        let fit = check_field_def_shape(
                            v,
                            value,
                            path,
                            orbital_events,
                            ancestors,
                            skip_required_members,
                        );
                        fit.is_valid()
                    });
                if !fits {
                    result.add_error(ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "expected one of the union variants ({}) but the value is {} and fits none of them",
                            names.join(" | "),
                            sexpr_kind(value)
                        ),
                    ));
                }
                return result;
            };
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            let choice = schema.select_union_variant(&keys, |field, tag| match map.get(field) {
                Some(SExpression::String(actual)) if actual.starts_with('@') => TagReading::LateBound,
                Some(SExpression::String(actual)) if actual == tag => TagReading::Equal,
                _ => TagReading::Differs,
            });
            match choice {
                UnionVariantChoice::Matched(chosen) => {
                    result.merge(check_field_def_shape(
                        chosen,
                        value,
                        path,
                        orbital_events,
                        ancestors,
                        skip_required_members,
                    ));
                    // A union variant is a CLOSED shape — that is the whole
                    // point of discriminating. Elsewhere an undeclared key is
                    // ignored (only declared-and-present keys are checked),
                    // and that leniency is exactly what let a `shape: "path"`
                    // carry `radiusX`: the tag selected the path variant and
                    // the ellipse-only key was then silently dropped. Closing
                    // it here, and ONLY here, keeps struct checking unchanged.
                    if let Some(props) = &chosen.properties {
                        let mut unknown: Vec<&str> = map
                            .keys()
                            .filter(|k| !props.contains_key(*k))
                            .map(|k| k.as_str())
                            .collect();
                        unknown.sort_unstable();
                        for key in unknown {
                            result.add_error(ValidationError::error(
                                ErrorCode::TraitConfigShapeMismatch,
                                &format!("{path}.{key}"),
                                format!(
                                    "'{}' is not a field of union variant '{}' — the variant's own tag selected it, so this key belongs to a different variant or to none",
                                    key,
                                    chosen.name.as_deref().unwrap_or("?")
                                ),
                            ));
                        }
                    }
                }
                UnionVariantChoice::Deferred => {}
                UnionVariantChoice::Ambiguous(fitting) => {
                    result.add_error(ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "value fits several union variants ({}) equally — give them a discriminator field",
                            fitting.join(", ")
                        ),
                    ));
                }
                UnionVariantChoice::Malformed(reason) => {
                    result.add_error(ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!("the union definition is ill-formed: {reason}"),
                    ));
                }
                UnionVariantChoice::NoMatch => {
                    result.add_error(ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "value matches none of the union variants ({}) — no variant's discriminator agrees with this value's tag, and no untagged variant's closed shape fits its keys",
                            names.join(" | ")
                        ),
                    ));
                }
            }
            return result;
        }
        FieldType::Tuple => {
            return check_tuple_shape(
                &schema.tuple_slots(),
                value,
                path,
                orbital_events,
                ancestors,
                skip_required_members,
            );
        }
        FieldType::Array => {
            let SExpression::Array(items) = value else {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "expected an array, got {}",
                            super::sexpr::render_sexpr_brief(value)
                        ),
                    )
                    .with_suggestion("wrap the value(s) in `[ … ]`".to_string()),
                );
                return result;
            };
            if let Some(inner) = &schema.items {
                for (i, elem) in items.iter().enumerate() {
                    let elem_path = format!("{}[{}]", path, i);
                    result.merge(check_field_def_shape(
                        inner,
                        elem,
                        &elem_path,
                        orbital_events,
                        ancestors,
                        skip_required_members,
                    ));
                }
            }
        }
        FieldType::Object => {
            let SExpression::Object(map) = value else {
                // Name what was FOUND and the expected member keys — the bare
                // "expected an object" left a run-22 agent with nothing to
                // act on (no line resolves for config(…) paths).
                let expected_fields = schema.properties.as_ref().map(|p| {
                    let mut keys: Vec<&str> = p.keys().map(String::as_str).collect();
                    keys.sort_unstable();
                    keys.join(", ")
                });
                let mut err = ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    format!(
                        "expected an object, got {}",
                        super::sexpr::render_sexpr_brief(value)
                    ),
                );
                err = err.with_suggestion(match &expected_fields {
                    Some(keys) => {
                        format!(
                            "write this entry as an object literal `{{ … }}` with fields: {keys}"
                        )
                    }
                    None => "write this entry as an object literal `{ … }`".to_string(),
                });
                result.add_error(err);
                return result;
            };
            if let Some(props) = &schema.properties {
                for (key, prop_schema) in props {
                    let prop_path = format!("{}.{}", path, key);
                    match map.get(key) {
                        Some(prop_value) => {
                            result.merge(check_field_def_shape(
                                prop_schema,
                                prop_value,
                                &prop_path,
                                orbital_events,
                                ancestors,
                                skip_required_members,
                            ));
                        }
                        None if !skip_required_members && prop_schema.required.unwrap_or(false) => {
                            result.add_error(ValidationError::error(
                                ErrorCode::TraitConfigShapeMismatch,
                                &prop_path,
                                format!("required field '{}' missing on object literal", key),
                            ));
                        }
                        None => {}
                    }
                }
            }
        }
        _ => {
            // Primitive leaf — enum first (closed-string union), then
            // structural type.
            if let Some(values) = &schema.values {
                if let SExpression::String(s) = value {
                    if !s.starts_with('@') && !values.iter().any(|v| v == s) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitConfigShapeMismatch,
                                path,
                                format!(
                                    "'{}' isn't a legal enum value (allowed: {})",
                                    s,
                                    values.join(", ")
                                ),
                            )
                            .with_valid_values(values.clone()),
                        );
                        return result;
                    }
                } else if !matches!(value, SExpression::Null) {
                    // Every enum member is a string; a number/boolean/array/
                    // object default can't be one no matter what it holds.
                    // Null stays legal — an unset optional knob, same as the
                    // `icon`/`slot`/`pattern` primitive tags below.
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::TraitConfigShapeMismatch,
                            path,
                            format!(
                                "expected a string member of [{}], got {}",
                                values.join(", "),
                                sexpr_kind(value)
                            ),
                        )
                        .with_valid_values(values.clone()),
                    );
                    return result;
                }
            }
            // Phase B (SCAN-EVENT-KNOB-2): an `event`-typed struct member
            // (e.g. `ItemAction.event`) is an event-name REFERENCE —
            // membership in the orbital's event vocabulary. Mirrors the
            // scalar `event` config-knob check (the `event_ctx` arm above
            // in `validate_config_field`) but ORBITAL-WIDE instead of
            // own-trait-only (owner decision D2: item actions legitimately
            // target sibling traits' transitions — the whole point of the
            // fix). Same skips as the scalar arm: `""` is the default-off
            // outlet, `@`-bindings resolve elsewhere, a vacuous orbital
            // vocabulary is skipped (mirrors the stale-literal check's
            // posture); `is_deferred_computed` already returned early at
            // the top of this function, so a non-string value here is
            // always a genuine type violation.
            if schema.field_type == FieldType::Event {
                match value {
                    SExpression::String(s) if is_event_address(s) => {
                        if let Some(refs) = orbital_events.refs {
                            result.add_errors(refs.check(s, orbital_events.orbital, path));
                        }
                    }
                    SExpression::String(s) => {
                        if !s.is_empty()
                            && !s.starts_with('@')
                            && !orbital_events.events.is_empty()
                            && !orbital_events.events.contains(s)
                        {
                            let mut valid: Vec<String> =
                                orbital_events.events.iter().cloned().collect();
                            valid.sort();
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::TraitInvalidEventDefault,
                                    path,
                                    format!(
                                        "Struct field typed `event` holds '{}' but the orbital's event set has no such key — must be one of: {}",
                                        s,
                                        valid.join(", ")
                                    ),
                                )
                                .with_valid_values(valid),
                            );
                        }
                    }
                    _ => {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitInvalidEventDefault,
                                path,
                                "Struct field typed `event` must hold a string event name",
                            )
                            .with_suggestion("Use a string event key, e.g. `event: SEARCH`, or \"\" for a default-off outlet"),
                        );
                    }
                }
                return result;
            }
            // An `EventAddress` must ADDRESS a declared external input — the
            // same rule the `EventAddress` service param enforces; a bare name
            // addresses no trait. `""` is the default-off outlet and `@`/`?`/`$`
            // values resolve elsewhere.
            if schema.field_type == FieldType::EventAddress {
                match value {
                    SExpression::String(s)
                        if s.is_empty()
                            || s.starts_with('@')
                            || s.starts_with('?')
                            || s.starts_with('$') => {}
                    SExpression::String(s) => {
                        if let Some(refs) = orbital_events.refs {
                            result.add_errors(refs.check(s, orbital_events.orbital, path));
                        }
                    }
                    _ => {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitInvalidEventDefault,
                                path,
                                "Struct field typed `EventAddress` must hold a string address",
                            )
                            .with_suggestion("Write `Trait.EVENT` or `Orbital.Trait.EVENT`"),
                        );
                    }
                }
                return result;
            }
            // A relation field's default is never a stored row — that shape
            // belongs to a typed `Map` field (see `std-record-detail.lolo`'s
            // `loadedRow`). Cardinality-aware per the field's OWN relation
            // metadata (`schema.relation.cardinality` — the `RelationDefinition`
            // the validator sees, `orbital-core/src/schema/types.rs`), never
            // inferred from the type spelling: a to-many relation
            // (`positionedNodes : [TableNode] = []`, cardinality `many` /
            // `one-to-many` / `many-to-many`) stores an ARRAY of id strings,
            // so `check_primitive_shape`'s single-tag interface can't express
            // it — every OTHER cardinality (`one`, `many-to-one`, or
            // unspecified) keeps the single id-string-or-null shape.
            if schema.field_type == FieldType::Relation {
                let is_many = schema.relation.as_ref().is_some_and(|r| r.is_to_many());
                if !is_many {
                    result.merge(check_primitive_shape("relation", value, path));
                    return result;
                }
                match value {
                    // Absent is handled by the caller (no `default` to check
                    // at all); null is legal here too, gated the same way
                    // the entity adapter gates every optional field's null
                    // default — a REQUIRED to-many relation defaulting to
                    // null still errors.
                    SExpression::Null => {
                        if schema.required.unwrap_or(false) {
                            result.add_error(ValidationError::error(
                                ErrorCode::TraitConfigShapeMismatch,
                                path,
                                "expected an array of relation ids, got null on a required field",
                            ));
                        }
                    }
                    SExpression::Array(items) => {
                        for (i, item) in items.iter().enumerate() {
                            if is_deferred_computed(item) {
                                continue;
                            }
                            if let SExpression::String(s) = item {
                                if s.starts_with('@') {
                                    continue;
                                }
                            }
                            if !matches!(item, SExpression::String(_)) {
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::TraitConfigShapeMismatch,
                                        &format!("{path}[{i}]"),
                                        format!(
                                            "expected a relation id string, got {}",
                                            sexpr_kind(item)
                                        ),
                                    )
                                    .with_suggestion(
                                        "a to-many relation's default holds an array of id strings, never rows — store rows in a typed `Map string <Row>` field",
                                    ),
                                );
                            }
                        }
                    }
                    _ => {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::TraitConfigShapeMismatch,
                                path,
                                format!(
                                    "expected an array of relation ids, got {}",
                                    sexpr_kind(value)
                                ),
                            )
                            .with_suggestion(
                                "a to-many relation's default is absent, null, or an array of id strings",
                            ),
                        );
                    }
                }
                return result;
            }
            let type_tag = match schema.field_type {
                FieldType::String => "string",
                FieldType::Number => "number",
                FieldType::Boolean => "boolean",
                FieldType::Date => "date",
                FieldType::Datetime => "datetime",
                FieldType::Timestamp => "timestamp",
                _ => return result, // Other field types are opaque to the leaf check.
            };
            result.merge(check_primitive_shape(type_tag, value, path));
        }
    }
    result
}

/// The value-kind tag `check_primitive_shape` and the enum-membership check
/// both report a mismatch against — one definition so the two callers can't
/// drift on what a "got X" means.
fn sexpr_kind(value: &SExpression) -> &'static str {
    match value {
        SExpression::String(_) => "string",
        SExpression::Number(_) => "number",
        SExpression::Bool(_) => "boolean",
        SExpression::Array(_) => "array",
        SExpression::Object(_) => "object",
        SExpression::Null => "null",
    }
}

pub(crate) fn check_primitive_shape(
    type_tag: &str,
    value: &SExpression,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if let SExpression::String(s) = value {
        if s.starts_with('@') {
            return result;
        }
    }
    if is_deferred_computed(value) {
        return result;
    }
    let actual = sexpr_kind(value);
    let ok = match type_tag {
        "string" => actual == "string",
        "number" => actual == "number",
        "boolean" => actual == "boolean",
        // `int`/`float` are numeric knob tags that used to fall through the
        // catch-all below and accept anything, so `float` took a string or a
        // bool silently.
        "int" | "float" => actual == "number",
        // Name-carrying scalar tags: a Lucide icon name, a slot key, a pattern
        // name, a component name. Each is a STRING in every emitted program, so
        // a number or boolean here is decidably wrong. They also fell through
        // the catch-all, which is the gap that made L1's dev gate unsafe: L1
        // rejected these, L2 did not, so skipping L1 in dev lost the check
        // entirely. `icon`/`slot`/`pattern` are ~7k knobs between them, so this
        // is the bulk of that hole. Null stays permitted — an explicitly unset
        // optional knob is legal, and only L1 ever rejected it.
        "icon" | "slot" | "pattern" | "component" => actual == "string" || actual == "null",
        // An id string, or absent — never a stored row/object. `seedRow :
        // Note = {}` is exactly the shape this rejects.
        "relation" => actual == "string" || actual == "null",
        "date" | "datetime" | "timestamp" => actual == "string" || actual == "number",
        _ => true, // Unknown type tags pass — covered by other validators.
    };
    if !ok {
        let mut err = ValidationError::error(
            ErrorCode::TraitConfigShapeMismatch,
            path,
            format!("expected {}, got {}", type_tag, actual),
        );
        // An object supplied where a scalar element was expected is the
        // `[json]`/`[string]`-carries-objects trap: name the typed-alias form
        // as the precise way to declare an array of object records.
        if actual == "object" {
            err = err.with_suggestion(TYPED_ALIAS_ARRAY_HINT);
        }
        result.add_error(err);
    }
    result
}

/// The precise remedy for an object-array config knob: declare a named type
/// alias and annotate the knob with it, so the object element shape is checked.
const TYPED_ALIAS_ARRAY_HINT: &str = "to declare an array of object records, \
define a named type alias — `type Item = { field : T, … }` — then annotate the \
knob `name : [Item]`. A bare `[json]` array lowers to unconstrained object \
elements; `[string]`/scalar element types cannot carry objects.";

/// Config knobs declared at APP level (`schema.config`) or at this
/// ORBITAL's own level (`orbital.config`) — reachable with no forwarding
/// trait in between (`Almadar_Orbital_Import.md` §4.5). Shared owner for
/// `validate_dangling_config_refs`'s reachability check and
/// `orbital_config::validate_orbital_config_forwarding`'s dead-knob walk —
/// both used to recompute this union independently.
pub(crate) fn declared_config_keys<'a>(
    schema: &'a OrbitalSchema,
    orbital: &'a Orbital,
) -> HashSet<&'a str> {
    schema
        .config
        .iter()
        .chain(orbital.config.iter())
        .flat_map(|c| c.keys())
        .map(|k| k.as_str())
        .collect()
}

/// C-CONFIG-DANGLING-REF-NO-VALIDATE: a literal `@config.<key>` in a resolved
/// trait's body (config defaults, transition effects, tick effects) whose key
/// is declared by NEITHER the owning trait NOR any transitive `@trait.X`
/// referrer chain is a dangling reference — at runtime the forward has no
/// schema to chain through and the component receives the literal string
/// (silent empty subtree / `.map` crash class). Referrer-declared keys are
/// LEGIT: `buildResolvedTraitConfigs` chains a child's `@config.fields`
/// through its embedder, so only keys unreachable from the whole chain flag.
/// `true` for a bare `@config.<key>` string leaf — the WHOLE string, nothing
/// trailing (no `.field`, no `[i]`). Stricter than `config_key_of` above
/// (which extracts a key out of a longer dotted/indexed path for the
/// dangling-ref reachability walk): this is the forward-declaration shape
/// `traitDeclaresConfigForward` tests for, and a nested path off a forward
/// is not itself a re-declaration. Rust twin of `CONFIG_FORWARD_RE` in
/// `embedded-trait-config.ts`.
fn is_bare_config_forward(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("@config.") else {
        return false;
    };
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `true` when part of what the trait renders is decided by whoever embeds
/// it: a knob whose whole default is an `@config.<key>` forward, or an
/// `@config.<key>` nested in a value naming a knob the trait does NOT declare.
/// A nested token naming one of its own knobs is the trait reading itself —
/// the same for every embedder. Rust twin of `traitDeclaresConfigForward`
/// (`packages/almadar-core/src/embedded-trait-config.ts`).
fn trait_config_declares_forward(config: &HashMap<String, ConfigField>) -> bool {
    fn walk(value: &SExpression, own: &HashMap<String, ConfigField>) -> bool {
        match value {
            SExpression::String(s) => {
                is_bare_config_forward(s) && !own.contains_key(&s["@config.".len()..])
            }
            SExpression::Array(items) => items.iter().any(|v| walk(v, own)),
            SExpression::Object(map) => map.values().any(|v| walk(v, own)),
            _ => false,
        }
    }
    config
        .values()
        .filter_map(|f| f.default.as_ref())
        .any(|default| match default {
            SExpression::String(s) => is_bare_config_forward(s),
            nested => walk(nested, config),
        })
}

/// A trait embedded via `@trait.X` by ≥2 referrers that itself declares an
/// `@config.<key>` forward.
///
/// One declaration cannot serve two embedders: the sibling-pull materializes
/// a sub-view PER EMBEDDER, so a child's `@config.X` forwards chain to its
/// FIRST referrer only (`EmbedGraph::direct_host`) and the client, keyed by
/// trait name, renders the first referrer's resolved config for both. A
/// child with no config forward renders identically regardless of embedder
/// (inert shared chrome — corpus calibration: 44 of 45 doubly-embedded
/// traits are exactly this, `wiring-lint.ts` L774-780), so only the
/// forward-declaring class is a real defect. JS twin:
/// `embedded-sibling-single-referrer`.
fn validate_embed_config_forward_multi_referrer(
    schema: &OrbitalSchema,
    result: &mut ValidationResult,
) {
    for orbital in &schema.orbitals {
        let graph = crate::phases::embed_graph::EmbedGraph::build(&orbital.traits);
        let declared: HashMap<&str, &HashMap<String, ConfigField>> =
            graph.declared_iter().collect();
        for (child, referrers) in graph.referrers_iter() {
            if referrers.len() < 2 {
                continue;
            }
            let Some(config) = declared.get(child) else {
                continue;
            };
            if !trait_config_declares_forward(config) {
                continue;
            }
            let mut sorted: Vec<&str> = referrers.iter().map(String::as_str).collect();
            sorted.sort_unstable();
            result.add_error(
                ValidationError::error(
                    ErrorCode::EmbedConfigForwardMultiReferrer,
                    format!("{}.@trait", child),
                    format!(
                        "{} declares @config forwards and is embedded via @trait by {} referrers \
                         ({}) — one declaration cannot serve two embedders: its forwards chain to \
                         the first referrer only, and the client keys rendered content by trait \
                         name, so the second embedder shows the first's resolved config",
                        child,
                        referrers.len(),
                        sorted.join(", ")
                    ),
                )
                .with_suggestion(format!(
                    "declare {} once per embedder (the sibling pull does exactly this for \
                     imported atoms), or move the forwarded knobs to literals so the render no \
                     longer depends on which trait embeds it",
                    child
                )),
            );
        }
    }
}

/// A trait config carrying BOTH a `groupByField` string and a `columns`
/// array of `{ key }` descriptors buckets rows by strict key equality — a
/// linked-entity enum value with no matching column key silently hides its
/// rows (the std-board cancelled-class). Detected by config SHAPE, never by
/// atom name — structured config only, per `fired_events.rs` doctrine. JS
/// twin: `groupby-enum-column-gap`.
fn validate_groupby_enum_column_gap(
    schema: &OrbitalSchema,
    ctx: &ValidationContext,
    result: &mut ValidationResult,
) {
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(config) = &td.config else {
                continue;
            };
            let Some(group_by_field) = config
                .get("groupByField")
                .and_then(|f| f.default.as_ref())
                .and_then(SExpression::as_str)
            else {
                continue;
            };
            if group_by_field.is_empty() {
                continue;
            }
            let Some(SExpression::Array(columns)) =
                config.get("columns").and_then(|f| f.default.as_ref())
            else {
                continue;
            };
            if columns.is_empty() {
                continue;
            }
            let mut column_keys: Vec<&str> = Vec::new();
            for col in columns {
                let SExpression::Object(map) = col else {
                    continue;
                };
                if let Some(SExpression::String(key)) = map.get("key") {
                    if !key.is_empty() && !column_keys.contains(&key.as_str()) {
                        column_keys.push(key.as_str());
                    }
                }
            }
            if column_keys.is_empty() {
                continue;
            }
            let Some(entity_name) = &td.linked_entity else {
                continue;
            };
            let Some(entity) = ctx.entity_definitions.get(entity_name) else {
                continue;
            };
            let Some(field) = entity
                .fields
                .iter()
                .find(|f| f.name.as_deref() == Some(group_by_field.as_ref()))
            else {
                continue;
            };
            let Some(values) = &field.values else {
                continue;
            };
            if values.is_empty() {
                continue;
            }
            let hidden: Vec<&str> = values
                .iter()
                .map(String::as_str)
                .filter(|v| !column_keys.contains(v))
                .collect();
            if hidden.is_empty() {
                continue;
            }
            result.add_error(
                ValidationError::error(
                    ErrorCode::RenderGroupbyEnumColumnGap,
                    format!("{}.config.groupByField", td.name),
                    format!(
                        "{} buckets {} rows by '{}' into columns [{}], but the field's enum also \
                         allows [{}] — rows with those values match no column and silently \
                         disappear",
                        td.name,
                        entity_name,
                        group_by_field,
                        column_keys.join(", "),
                        hidden.join(", ")
                    ),
                )
                .with_suggestion(format!(
                    "add a column entry for each missing key ({}) or narrow the {}.{} enum",
                    hidden.join(", "),
                    entity_name,
                    group_by_field
                )),
            );
        }
    }
}

fn validate_dangling_config_refs(schema: &OrbitalSchema) -> ValidationResult {
    use std::collections::{HashMap, HashSet};

    fn config_key_of(s: &str) -> Option<&str> {
        let rest = s.strip_prefix("@config.")?;
        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let key = &rest[..end];
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        Some(key)
    }

    fn collect_config_keys(expr: &SExpression, out: &mut HashSet<String>) {
        collect_config_keys_inner(expr, None, out);
    }

    /// `prop_context` is Some((prop_key, pattern_has_prop)) when `expr` is the
    /// direct value of a prop on a registry-known pattern node. A SELF-NAMED
    /// token there (`amount: "@config.amount"` on a stat-display) is the
    /// pattern layer's declaration-form default for an unset prop — enrich
    /// materializes it and the component resolves/nulls it — not a forward
    /// that needs a declaring trait. Everything else collects.
    fn collect_config_keys_inner(
        expr: &SExpression,
        prop_context: Option<(&str, bool)>,
        out: &mut HashSet<String>,
    ) {
        match expr {
            SExpression::String(s) => {
                if let Some(key) = config_key_of(s) {
                    let is_declaration_default = matches!(
                        prop_context,
                        Some((prop_key, true)) if prop_key == key
                    );
                    if !is_declaration_default {
                        out.insert(key.to_string());
                    }
                }
            }
            SExpression::Array(items) => {
                for item in items {
                    collect_config_keys_inner(item, None, out);
                }
            }
            SExpression::Object(map) => {
                let pattern_props = match map.get("type") {
                    Some(SExpression::String(ty)) => {
                        crate::pattern_registry::get_props_schema(ty.as_str())
                    }
                    _ => None,
                };
                for (k, v) in map {
                    let ctx =
                        pattern_props.map(|props| (k.as_str(), props.contains_key(k.as_str())));
                    collect_config_keys_inner(v, ctx, out);
                }
            }
            _ => {}
        }
    }

    let mut result = ValidationResult::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        // App-level `config {}` and this orbital's own `config {}` are both
        // declaring sources (§4.5) — reachable with no forwarding trait.
        let declared_keys = declared_config_keys(schema, orbital);
        // Declared knob keys per trait + child -> referrers adjacency — the
        // SAME graph `phases::embed_graph::EmbedGraph` builds for the inline
        // phase's `@config.<knob>` forward resolver, so a forward the
        // resolver folds to a literal is exactly a forward this check judges
        // reachable. By validation time every entry is `TraitReference::Inline`
        // (comprehensive validation runs post-inline/-compose), so only the
        // `Inline` half of the graph's contract is ever exercised here.
        let graph = crate::phases::embed_graph::EmbedGraph::build(&orbital.traits);
        let declared: HashMap<&str, HashSet<&str>> = graph
            .declared_iter()
            .map(|(name, cfg)| (name, cfg.keys().map(|k| k.as_str()).collect()))
            .collect();
        // Undirected embed adjacency: forwards chain UP through referrers
        // (`buildResolvedTraitConfigs`), and a parent's effect tree also
        // inherits its CHILDREN's materialized pattern props whose defaults
        // are self-forward tokens (`amount: "@config.amount"` declared on the
        // embedded stat-display) — so a key declared anywhere in the embed
        // component is reachable; only component-wide undeclared keys dangle.
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for (child, parents) in graph.referrers_iter() {
            for parent in parents {
                adjacency.entry(child).or_default().push(parent.as_str());
                adjacency.entry(parent.as_str()).or_default().push(child);
            }
        }

        // `self_forward`: the key is the owner's own knob whose value is `@config.<key>` — that
        // declaration can't supply itself, so it must be declared outside that knob.
        let key_reachable = |owner: &str, key: &str, self_forward: bool| -> bool {
            if declared_keys.contains(key) {
                return true;
            }
            let mut queue = vec![owner];
            let mut seen: HashSet<&str> = HashSet::new();
            while let Some(name) = queue.pop() {
                if !seen.insert(name) {
                    continue;
                }
                let own_forward = self_forward && name == owner;
                if !own_forward && declared.get(name).is_some_and(|keys| keys.contains(key)) {
                    return true;
                }
                if let Some(neighbors) = adjacency.get(name) {
                    queue.extend(neighbors.iter().copied());
                }
            }
            false
        };

        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let mut used: HashSet<String> = HashSet::new();
            for cf in td.config.iter().flat_map(|c| c.values()) {
                if let Some(default) = &cf.default {
                    collect_config_keys(default, &mut used);
                }
            }
            for transition in &td.state_machine.transitions {
                for effect in transition.effects.iter().flatten() {
                    collect_config_keys(effect, &mut used);
                }
            }
            for tick in td.ticks.iter().flatten() {
                for effect in &tick.effects {
                    collect_config_keys(effect, &mut used);
                }
            }
            let self_forwards: HashSet<&str> = td
                .config
                .iter()
                .flat_map(|c| c.iter())
                .filter(|(k, cf)| {
                    matches!(&cf.default, Some(SExpression::String(v)) if config_key_of(v) == Some(k.as_str()))
                })
                .map(|(k, _)| k.as_str())
                .collect();
            let mut dangling: Vec<&String> = used
                .iter()
                .filter(|key| !key_reachable(&td.name, key, self_forwards.contains(key.as_str())))
                .collect();
            dangling.sort();
            for key in dangling {
                let path = format!(
                    "orbitals[{} ({})].traits[{} ({})]",
                    oi, orbital.name, ti, td.name
                );
                result.add_error(
                    ValidationError::error(
                        ErrorCode::ConfigDanglingRef,
                        path,
                        format!(
                            "`@config.{}` — no trait in {}'s referrer chain declares config knob '{}'; \
                             the forward cannot resolve and renders as the literal string",
                            key, td.name, key
                        ),
                    )
                    .with_suggestion(format!(
                        "Declare `{}` on the owning trait (or its embedder) or remove the reference",
                        key
                    )),
                );
            }
        }
    }

    result
}

#[cfg(test)]
mod dangling_config_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    const TRUE_DANGLE: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Panel",
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "typography", "content": "@config.missingTitle" }]]
            }]
          }
        }]
      }]
    }"#;

    const DECLARATION_DEFAULT: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Panel",
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "stat-display", "label": "@config.label" }]]
            }]
          }
        }]
      }]
    }"#;

    const REFERRER_DECLARED: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          {
            "name": "Child",
            "stateMachine": {
              "states": [{ "name": "idle", "isInitial": true }],
              "transitions": [{
                "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["render-ui", "main", { "type": "typography", "content": "@config.fields" }]]
              }]
            }
          },
          {
            "name": "Parent",
            "config": { "fields": { "type": "string", "default": "x" } },
            "stateMachine": {
              "states": [{ "name": "idle", "isInitial": true }],
              "transitions": [{
                "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["render-ui", "main", { "type": "stack", "children": ["@trait.Child"] }]]
              }]
            }
          }
        ]
      }]
    }"#;

    fn dangles(json: &str) -> usize {
        validate_dangling_config_refs(&schema_from_json(json))
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ConfigDanglingRef)
            .count()
    }

    #[test]
    fn flags_component_wide_undeclared_key() {
        assert_eq!(dangles(TRUE_DANGLE), 1, "undeclared key must flag");
    }

    #[test]
    fn accepts_self_named_pattern_declaration_default() {
        assert_eq!(
            dangles(DECLARATION_DEFAULT),
            0,
            "self-named pattern-prop declaration default is the enrich idiom, not a dangle"
        );
    }

    const ORBITAL_DECLARED: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "config": { "browseColumns": { "type": "json", "default": [] } },
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Browse",
          "config": { "columns": { "type": "json", "default": "@config.browseColumns" } },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "typography", "content": "@config.browseColumns" }]]
            }]
          }
        }]
      }]
    }"#;

    const APP_DECLARED: &str = r#"{
      "name": "t", "version": "1.0.0",
      "config": { "appName": { "type": "string", "default": "Time Tracking" } },
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Shell",
          "config": { "title": { "type": "string", "default": "@config.appName" } },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "typography", "content": "@config.appName" }]]
            }]
          }
        }]
      }]
    }"#;

    #[test]
    fn accepts_key_declared_at_orbital_level() {
        assert_eq!(
            dangles(ORBITAL_DECLARED),
            0,
            "an orbital `config {{}}` knob is a declaring source"
        );
    }

    #[test]
    fn accepts_key_declared_at_app_level() {
        assert_eq!(
            dangles(APP_DECLARED),
            0,
            "an app-level `config {{}}` knob is a declaring source"
        );
    }

    /// The `app-1789833059828` crash: a knob whose value forwards to its own
    /// key (`navItems: @config.navItems`) with nothing declaring `navItems`
    /// outside that knob. The runtime renders the literal string and a
    /// component calling `.map` on it crashes.
    const SELF_FORWARD_UNDECLARED: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Layout",
          "config": { "navItems": { "type": "unknown", "default": "@config.navItems" } },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "dashboard-layout", "navItems": "@config.navItems" }]]
            }]
          }
        }]
      }]
    }"#;

    const SELF_FORWARD_APP_DECLARED: &str = r#"{
      "name": "t", "version": "1.0.0",
      "config": { "navItems": { "type": "json", "default": [] } },
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Layout",
          "config": { "navItems": { "type": "unknown", "default": "@config.navItems" } },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [{
              "from": "idle", "to": "idle", "event": "INIT",
              "effects": [["render-ui", "main", { "type": "dashboard-layout", "navItems": "@config.navItems" }]]
            }]
          }
        }]
      }]
    }"#;

    const SELF_FORWARD_EMBEDDER_DECLARED: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          {
            "name": "Layout",
            "config": { "navItems": { "type": "unknown", "default": "@config.navItems" } },
            "stateMachine": {
              "states": [{ "name": "idle", "isInitial": true }],
              "transitions": [{
                "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["render-ui", "main", { "type": "dashboard-layout", "navItems": "@config.navItems" }]]
              }]
            }
          },
          {
            "name": "Page",
            "config": { "navItems": { "type": "json", "default": [] } },
            "stateMachine": {
              "states": [{ "name": "idle", "isInitial": true }],
              "transitions": [{
                "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["render-ui", "main", { "type": "stack", "children": ["@trait.Layout"] }]]
              }]
            }
          }
        ]
      }]
    }"#;

    #[test]
    fn flags_self_forward_with_no_outer_declaration() {
        assert_eq!(
            dangles(SELF_FORWARD_UNDECLARED),
            1,
            "a knob forwarding to its own key is not its own declaration"
        );
    }

    #[test]
    fn accepts_self_forward_declared_at_app_level() {
        assert_eq!(
            dangles(SELF_FORWARD_APP_DECLARED),
            0,
            "app `config {{}}` resolves the forward"
        );
    }

    #[test]
    fn accepts_self_forward_declared_by_another_trait() {
        assert_eq!(
            dangles(SELF_FORWARD_EMBEDDER_DECLARED),
            0,
            "another trait in the embed component declaring the key resolves the forward"
        );
    }

    #[test]
    fn accepts_key_declared_by_embed_component() {
        assert_eq!(
            dangles(REFERRER_DECLARED),
            0,
            "a key declared anywhere in the undirected embed component is reachable"
        );
    }
}

#[cfg(test)]
mod embed_config_forward_multi_referrer_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    fn findings(json: &str) -> Vec<ValidationError> {
        let schema = schema_from_json(json);
        let mut result = ValidationResult::new();
        validate_embed_config_forward_multi_referrer(&schema, &mut result);
        result.errors
    }

    /// `Child` declares `@config.title` (a forward) and is embedded by two
    /// referrers — the real defect class.
    const TWO_REFERRERS_WITH_FORWARD: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          { "name": "Child", "config": { "title": { "type": "string", "default": "@config.title" } } },
          { "name": "ParentA", "config": { "slotA": { "type": "string", "default": "@trait.Child" } } },
          { "name": "ParentB", "config": { "slotB": { "type": "string", "default": "@trait.Child" } } }
        ]
      }]
    }"#;

    /// Same two-referrer shape, but `Child` declares no `@config.*` forward
    /// — inert shared chrome, the 44/45 corpus case. Must stay silent.
    const TWO_REFERRERS_NO_FORWARD: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          { "name": "Child", "config": { "title": { "type": "string", "default": "static" } } },
          { "name": "ParentA", "config": { "slotA": { "type": "string", "default": "@trait.Child" } } },
          { "name": "ParentB", "config": { "slotB": { "type": "string", "default": "@trait.Child" } } }
        ]
      }]
    }"#;

    /// One referrer only, forward or not — never flagged.
    const ONE_REFERRER_WITH_FORWARD: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          { "name": "Child", "config": { "title": { "type": "string", "default": "@config.title" } } },
          { "name": "ParentA", "config": { "slotA": { "type": "string", "default": "@trait.Child" } } }
        ]
      }]
    }"#;

    #[test]
    fn two_referrers_and_a_config_forward_errors() {
        let errors = findings(TWO_REFERRERS_WITH_FORWARD);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, ErrorCode::EmbedConfigForwardMultiReferrer);
        assert!(errors[0].message.contains("Child"));
    }

    /// Two referrers; `Child` only READS its own knob (`title`) nested in
    /// another knob's value — that renders the same for every embedder.
    const TWO_REFERRERS_OWN_KNOB_READ: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          { "name": "Child", "config": {
            "title": { "type": "string", "default": "Stats" },
            "body": { "type": "unknown", "default": { "type": "typography", "content": "@config.title" } }
          } },
          { "name": "ParentA", "config": { "slotA": { "type": "string", "default": "@trait.Child" } } },
          { "name": "ParentB", "config": { "slotB": { "type": "string", "default": "@trait.Child" } } }
        ]
      }]
    }"#;

    /// Edge: a nested token naming a knob `Child` does NOT declare is an
    /// embedder forward — still flagged.
    const TWO_REFERRERS_NESTED_FOREIGN: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [
          { "name": "Child", "config": {
            "body": { "type": "unknown", "default": { "type": "typography", "content": "@config.heading" } }
          } },
          { "name": "ParentA", "config": { "slotA": { "type": "string", "default": "@trait.Child" } } },
          { "name": "ParentB", "config": { "slotB": { "type": "string", "default": "@trait.Child" } } }
        ]
      }]
    }"#;

    #[test]
    fn two_referrers_reading_only_own_knobs_is_silent() {
        assert!(findings(TWO_REFERRERS_OWN_KNOB_READ).is_empty());
    }

    #[test]
    fn two_referrers_with_a_nested_foreign_forward_errors() {
        let errors = findings(TWO_REFERRERS_NESTED_FOREIGN);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, ErrorCode::EmbedConfigForwardMultiReferrer);
    }

    #[test]
    fn two_referrers_with_no_forward_is_inert_chrome_and_silent() {
        assert!(findings(TWO_REFERRERS_NO_FORWARD).is_empty());
    }

    #[test]
    fn one_referrer_is_never_flagged() {
        assert!(findings(ONE_REFERRER_WITH_FORWARD).is_empty());
    }
}

#[cfg(test)]
mod groupby_enum_column_gap_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    fn findings(json: &str) -> Vec<ValidationError> {
        let schema = schema_from_json(json);
        let mut ctx = ValidationContext::new();
        for orbital in &schema.orbitals {
            if let Some(entity) = orbital.entity.as_inline() {
                ctx.entity_definitions
                    .insert(entity.name.clone(), entity.clone());
            }
        }
        let mut result = ValidationResult::new();
        validate_groupby_enum_column_gap(&schema, &ctx, &mut result);
        result.errors
    }

    const GAP: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": {
          "name": "Deal",
          "fields": [{ "name": "status", "type": "enum", "values": ["open", "won", "lost"] }]
        },
        "traits": [{
          "name": "Board",
          "linkedEntity": "Deal",
          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] },
          "config": {
            "groupByField": { "type": "string", "default": "status" },
            "columns": { "type": "unknown", "default": [
              { "key": "open" },
              { "key": "won" }
            ] }
          }
        }]
      }]
    }"#;

    const NO_GAP: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": {
          "name": "Deal",
          "fields": [{ "name": "status", "type": "enum", "values": ["open", "won", "lost"] }]
        },
        "traits": [{
          "name": "Board",
          "linkedEntity": "Deal",
          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] },
          "config": {
            "groupByField": { "type": "string", "default": "status" },
            "columns": { "type": "unknown", "default": [
              { "key": "open" },
              { "key": "won" },
              { "key": "lost" }
            ] }
          }
        }]
      }]
    }"#;

    const NO_ENUM: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": {
          "name": "Deal",
          "fields": [{ "name": "status", "type": "string" }]
        },
        "traits": [{
          "name": "Board",
          "linkedEntity": "Deal",
          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] },
          "config": {
            "groupByField": { "type": "string", "default": "status" },
            "columns": { "type": "unknown", "default": [ { "key": "open" } ] }
          }
        }]
      }]
    }"#;

    const NO_COLUMNS: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": {
          "name": "Deal",
          "fields": [{ "name": "status", "type": "enum", "values": ["open", "won", "lost"] }]
        },
        "traits": [{
          "name": "Board",
          "linkedEntity": "Deal",
          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] },
          "config": {
            "groupByField": { "type": "string", "default": "status" }
          }
        }]
      }]
    }"#;

    /// A `groupByField` computed from a binding rather than authored as a
    /// literal string is not a structural declaration this check can read —
    /// structured-config-only, never a runtime guess.
    const SILENT_ON_BINDING_GROUP_FIELD: &str = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": {
          "name": "Deal",
          "fields": [{ "name": "status", "type": "enum", "values": ["open", "won"] }]
        },
        "traits": [{
          "name": "Board",
          "linkedEntity": "Deal",
          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] },
          "config": {
            "groupByField": { "type": "string", "default": "@config.groupByField" },
            "columns": { "type": "unknown", "default": [ { "key": "open" } ] }
          }
        }]
      }]
    }"#;

    #[test]
    fn hidden_enum_value_flags_the_gap() {
        let errors = findings(GAP);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, ErrorCode::RenderGroupbyEnumColumnGap);
        assert!(errors[0].message.contains("lost"));
    }

    #[test]
    fn every_enum_value_covered_is_silent() {
        assert!(findings(NO_GAP).is_empty());
    }

    #[test]
    fn a_non_enum_group_field_is_silent() {
        assert!(findings(NO_ENUM).is_empty());
    }

    #[test]
    fn no_columns_declared_is_silent() {
        assert!(findings(NO_COLUMNS).is_empty());
    }

    #[test]
    fn a_bound_group_field_is_silent() {
        assert!(findings(SILENT_ON_BINDING_GROUP_FIELD).is_empty());
    }
}

#[cfg(test)]
mod node_config_tests {
    use super::*;
    use orbital_core_base::schema::ConfigField;

    fn field(field_type: &str, default: SExpression) -> ConfigField {
        serde_json::from_value(serde_json::json!({ "type": field_type }))
            .map(|mut f: ConfigField| {
                f.default = Some(default);
                f
            })
            .expect("config field fixture")
    }

    #[test]
    fn node_array_of_patterns_passes_per_element() {
        let default = serde_json::from_value(serde_json::json!([
            { "type": "canvas-2d" },
            { "type": "typography", "content": "hi" }
        ]))
        .unwrap();
        let result = validate_config_field(
            &field("node", default),
            "t.config.children",
            None,
            &EventScope::default(),
            None,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn node_computed_operator_call_is_skipped() {
        let default = serde_json::from_value(serde_json::json!(
            ["if", ["==", "@entity.result", "none"], null, { "type": "game-menu" }]
        ))
        .unwrap();
        let result = validate_config_field(
            &field("node", default),
            "t.config.overlay",
            None,
            &EventScope::default(),
            None,
        );
        assert!(
            result.errors.is_empty(),
            "computed node config must not be pattern-checked: {:?}",
            result.errors
        );
    }

    #[test]
    fn node_bad_pattern_in_array_errors() {
        let default = serde_json::from_value(serde_json::json!([
            { "type": "not-a-real-pattern", "x": 1 }
        ]))
        .unwrap();
        let result = validate_config_field(
            &field("node", default),
            "t.config.children",
            None,
            &EventScope::default(),
            None,
        );
        assert!(!result.errors.is_empty());
    }
}

#[cfg(test)]
mod event_config_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    fn run(schema_json: &str) -> ValidationResult {
        let schema = schema_from_json(schema_json);
        let mut ctx = ValidationContext::new();
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    fn event_trait_schema(knob_default: &str, knob_type: &str) -> String {
        format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {{ "name": "W", "fields": [] }},
        "traits": [{{
          "name": "Picker",
          "config": {{ "clickEvent": {{ "type": "{knob_type}", "default": "{knob_default}" }} }},
          "emits": [{{ "event": "PICK", "scope": "internal" }}],
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }},
              {{ "from": "idle", "to": "idle", "event": "PICK", "effects": [] }}
            ]
          }}
        }}]
      }}]
    }}"#
        )
    }

    fn event_errors(result: &ValidationResult) -> Vec<&ValidationError> {
        result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitInvalidEventDefault)
            .collect()
    }

    #[test]
    fn member_value_passes() {
        let result = run(&event_trait_schema("PICK", "event"));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn a_dotted_value_is_an_address_not_a_member_name() {
        let result = run(&event_trait_schema("note.search", "event"));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitEventRefUnresolved),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn non_member_value_fails_with_valid_values() {
        let result = run(&event_trait_schema("NOTE_SEARCH", "event"));
        let errs = event_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", result.errors);
        assert_eq!(errs[0].code, ErrorCode::TraitInvalidEventDefault);
        let valid = errs[0].valid_values.as_ref().expect("valid_values");
        assert!(valid.contains(&"PICK".to_string()));
    }

    #[test]
    fn empty_string_is_default_off_outlet() {
        let result = run(&event_trait_schema("", "event"));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn config_forward_value_skips() {
        let result = run(&event_trait_schema("@config.other", "event"));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn string_typed_knob_is_untouched() {
        let result = run(&event_trait_schema("note.search", "string"));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn vacuous_event_set_skips() {
        let result = run(r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "RenderOnly",
          "config": { "clickEvent": { "type": "event", "default": "ANYTHING" } },
          "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] }
        }]
      }]
    }"#);
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn sentinel_resolves_through_knob_default_for_membership() {
        // The trait transitions on `@config.onPageChange` (dynamic sentinel);
        // a SIBLING event-typed knob naming the sentinel's resolved value is
        // a member (the pageChangeEvent edge from the corpus simulation).
        let result = run(r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Pager",
          "config": {
            "onPageChange": { "type": "string", "default": "PAGE_CHANGE" },
            "pageChangeEvent": { "type": "event", "default": "PAGE_CHANGE" }
          },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "@config.onPageChange", "effects": [] }
            ]
          }
        }]
      }]
    }"#);
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn definer_typed_event_skips_membership() {
        // A-UNIFY: sentinel position — the knob's value CREATES the event
        // key, so any name is valid by construction; it must not be checked
        // against the trait's own event set (which wouldn't even contain a
        // literal "CLICKED" here, since the trait's only member is the
        // sentinel form `@config.onEventClick` itself).
        let result = run(r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Cal",
          "config": { "onEventClick": { "type": "event", "default": "CLICKED" } },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "@config.onEventClick", "effects": [] }
            ]
          }
        }]
      }]
    }"#);
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn definer_typed_event_still_rejects_non_string_default() {
        // The definer skip only lifts the MEMBERSHIP check — a non-string
        // default is still decidably wrong regardless of role.
        let result = run(r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Cal",
          "config": { "onEventClick": { "type": "event", "default": 42 } },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "@config.onEventClick", "effects": [] }
            ]
          }
        }]
      }]
    }"#);
        let errs = event_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", result.errors);
        assert_eq!(errs[0].code, ErrorCode::TraitInvalidEventDefault);
    }

    #[test]
    fn reference_arm_skips_membership() {
        // A pre-inline Reference override has no base-trait vocabulary — the
        // event membership check must not fire there.
        let result = run(r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "ref": "Atom.traits.Picker",
          "name": "MyPicker",
          "config": { "clickEvent": { "type": "event", "default": "WHATEVER" } }
        }]
      }]
    }"#);
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }
}

#[cfg(test)]
mod struct_event_field_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    fn run(schema_json: &str) -> ValidationResult {
        let schema = schema_from_json(schema_json);
        let mut ctx = ValidationContext::new();
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    fn event_errors(result: &ValidationResult) -> Vec<&ValidationError> {
        result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitInvalidEventDefault)
            .collect()
    }

    /// A `Header` trait with an `itemActions : [ItemAction]` knob (struct
    /// alias, `event` member) whose default carries one item, plus a
    /// SIBLING `Editor` trait with its own transition — proves the D2
    /// orbital-wide acceptance (own transitions ∪ orbital-wide transitions
    /// ∪ own emits), the whole point of Phase B / SCAN-EVENT-KNOB-2.
    fn item_actions_schema(event_value: &str) -> String {
        format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {{ "name": "W", "fields": [] }},
        "traits": [
          {{
            "name": "Header",
            "config": {{
              "itemActions": {{
                "type": "[ItemAction]",
                "default": [{{ "event": "{event_value}", "label": "Go" }}],
                "items": {{
                  "type": "object",
                  "properties": {{
                    "event": {{ "type": "event" }},
                    "label": {{ "type": "string" }}
                  }}
                }}
              }}
            }},
            "stateMachine": {{
              "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [
                {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }},
                {{ "from": "idle", "to": "idle", "event": "EDIT", "effects": [] }}
              ]
            }}
          }},
          {{
            "name": "Editor",
            "stateMachine": {{
              "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [
                {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }},
                {{ "from": "idle", "to": "idle", "event": "DELETE_ITEM", "effects": [] }}
              ]
            }}
          }}
        ]
      }}]
    }}"#
        )
    }

    #[test]
    fn own_transition_member_passes() {
        let result = run(&item_actions_schema("EDIT"));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn sibling_orbital_wide_transition_member_passes() {
        // The whole point of D2: an item action targeting a SIBLING trait's
        // transition is legal — item actions are orbital-wide by design.
        let result = run(&item_actions_schema("DELETE_ITEM"));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn invented_member_fails_typed() {
        let result = run(&item_actions_schema("GHOST"));
        let errs = event_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", result.errors);
        let valid = errs[0].valid_values.as_ref().expect("valid_values");
        assert!(valid.contains(&"EDIT".to_string()));
        assert!(valid.contains(&"DELETE_ITEM".to_string()));
    }

    #[test]
    fn empty_string_member_is_default_off_outlet() {
        let result = run(&item_actions_schema(""));
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn config_forward_member_skips() {
        let schema = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Header",
          "config": {
            "itemActions": {
              "type": "[ItemAction]",
              "default": [{ "event": "@config.other", "label": "Go" }],
              "items": {
                "type": "object",
                "properties": {
                  "event": { "type": "event" },
                  "label": { "type": "string" }
                }
              }
            }
          },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT", "effects": [] }
            ]
          }
        }]
      }]
    }"#;
        let result = run(schema);
        assert!(event_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn non_string_member_fails_typed() {
        let schema = r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Header",
          "config": {
            "itemActions": {
              "type": "[ItemAction]",
              "default": [{ "event": 42, "label": "Go" }],
              "items": {
                "type": "object",
                "properties": {
                  "event": { "type": "event" },
                  "label": { "type": "string" }
                }
              }
            }
          },
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": []
          }
        }]
      }]
    }"#;
        let result = run(schema);
        let errs = event_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", errs);
    }
}

/// C-CONFIG-REF-TYPE-HOLE: `@entity.X` / `@payload.X` on a post-inline
/// config field default, checked against the field's declared type (or,
/// when a direct pattern-prop site is found, the pattern registry's full
/// `PropSchema.types` union — see `knob_prop_type_union`). Hard error:
/// assertions read `result.errors`.
#[cfg(test)]
mod reference_binding_type_tests {
    use super::*;

    fn schema_from_json(json: &str) -> OrbitalSchema {
        serde_json::from_str(json).expect("fixture parses")
    }

    /// Mirrors the real pipeline's Phase 1 (`collect_names`) just enough to
    /// populate `ctx.entity_definitions` — the piece `validate_all_config_types`
    /// needs for `@entity.X` resolution but doesn't populate itself when
    /// called standalone (as every other test in this file already does).
    fn run(schema_json: &str) -> ValidationResult {
        let schema = schema_from_json(schema_json);
        let mut ctx = ValidationContext::new();
        crate::phases::validation::collect_names(&schema, &mut ctx);
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    fn mismatch_errors(result: &ValidationResult) -> Vec<&ValidationError> {
        result
            .errors
            .iter()
            .filter(|e| {
                e.code == ErrorCode::TraitConfigShapeMismatch && e.message.contains("resolves to")
            })
            .collect()
    }

    const ITEM_ENTITY: &str = r#"{
      "name": "Item",
      "fields": [
        { "name": "title", "type": "string" },
        { "name": "count", "type": "number" }
      ]
    }"#;

    fn entity_trait_schema(field_type: &str, default_ref: &str, extra_effect: &str) -> String {
        format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {ITEM_ENTITY},
        "traits": [{{
          "name": "Panel",
          "linkedEntity": "Item",
          "config": {{ "label": {{ "type": "{field_type}", "default": "{default_ref}" }} }},
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [{extra_effect}] }}
            ]
          }}
        }}]
      }}]
    }}"#
        )
    }

    #[test]
    fn entity_number_field_on_string_knob_warns() {
        // `count : number` bound to a knob declared `string`, no pattern-prop
        // site in the tree — falls back to the collapsed declared type, which
        // genuinely disagrees.
        let result = run(&entity_trait_schema("string", "@entity.count", "[]"));
        let errs = mismatch_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", result.errors);
        assert!(
            result.warnings.is_empty(),
            "must be blocking: {:?}",
            result.warnings
        );
    }

    #[test]
    fn entity_string_field_on_string_knob_is_silent() {
        // The negative pairing: same shape, compatible types.
        let result = run(&entity_trait_schema("string", "@entity.title", "[]"));
        assert!(mismatch_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn entity_binding_through_wider_pattern_union_is_silent() {
        // `stat-display.value` is registry-typed `[number, string]`; the knob
        // wrapping it collapses to `string` (the `lolo-ui` base-union
        // collapse). A number-typed entity field bound to that knob must NOT
        // warn once the pattern site is visible, because the underlying prop
        // genuinely accepts a number — this is the regression test for the
        // union fix (508 → 148 corpus hits, all from this exact class).
        let effect = r#"["render-ui", "main", { "type": "stat-display", "value": "@entity.count", "label": "Total" }]"#;
        let result = run(&entity_trait_schema("string", "@entity.count", effect));
        assert!(
            mismatch_errors(&result).is_empty(),
            "pattern union must widen past the collapsed knob type: {:?}",
            result.errors
        );
    }

    #[test]
    fn entity_binding_through_narrow_pattern_union_still_warns() {
        // Same shape as the widening test, but the pattern site is typed
        // `string`-only — the union must not blanket-silence every hit just
        // because A site was found.
        //
        // Uses `typography.align` rather than `.content`: `content` is declared
        // `node` in the registry, and a scalar bound to a `node` target is
        // deliberately NOT reported (every scalar renders — React paints a
        // number as text). That exemption is what took this class from 67
        // unactionable findings to 0. `align` is genuinely `string`-only, so it
        // still exercises the narrow-union path this test exists to pin.
        let effect = r#"["render-ui", "main", { "type": "typography", "align": "@entity.count" }]"#;
        let result = run(&entity_trait_schema("string", "@entity.count", effect));
        assert_eq!(mismatch_errors(&result).len(), 1, "{:?}", result.errors);
    }

    fn payload_trait_schema(field_type: &str, payload_type: &str, default_ref: &str) -> String {
        format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {ITEM_ENTITY},
        "traits": [{{
          "name": "Panel",
          "linkedEntity": "Item",
          "config": {{ "label": {{ "type": "{field_type}", "default": "{default_ref}" }} }},
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "events": [{{ "key": "SUBMIT", "payloadSchema": [{{ "name": "amount", "type": "{payload_type}" }}] }}],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "SUBMIT", "effects": [] }}
            ]
          }}
        }}]
      }}]
    }}"#
        )
    }

    #[test]
    fn payload_number_field_on_string_knob_warns() {
        // A hand-authored Inline trait's OWN `@payload.X` (never routed
        // through the Reference-merge `retag_callsite_payload` pass, since
        // there is no `ref` here) is a live, checkable case.
        let result = run(&payload_trait_schema("string", "number", "@payload.amount"));
        let errs = mismatch_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", result.errors);
        assert!(
            result.warnings.is_empty(),
            "must be blocking: {:?}",
            result.warnings
        );
    }

    #[test]
    fn payload_string_field_on_string_knob_is_silent() {
        let result = run(&payload_trait_schema("string", "string", "@payload.amount"));
        assert!(mismatch_errors(&result).is_empty(), "{:?}", result.errors);
    }
}

#[cfg(test)]
mod recursive_type_literal_tests {
    use super::*;

    /// A config field declared `subMenu : [MenuItem]` where `MenuItem` is
    /// self-referential — `resolve_config_field_shape`'s wire shape:
    /// `field_type: "[MenuItem]"`, `items` carries MenuItem's own fields, and
    /// the recursion point is `FieldType::Union { values: ["MenuItem"],
    /// properties: None }`.
    fn menu_item_config_field() -> ConfigField {
        let back_ref = FieldDefinition {
            mock: None,
            merged_from: None,
            name: Some("subMenu".to_string()),
            field_type: FieldType::Union,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: Some(vec!["MenuItem".to_string()]),
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
            projected_from: None,
        };
        let sub_menu_array = FieldDefinition {
            mock: None,
            merged_from: None,
            name: Some("subMenu".to_string()),
            field_type: FieldType::Array,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: Some(Box::new(back_ref)),
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
            projected_from: None,
        };
        let label = FieldDefinition {
            mock: None,
            merged_from: None,
            name: Some("label".to_string()),
            field_type: FieldType::String,
            required: Some(true),
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
            projected_from: None,
        };
        let mut props = HashMap::new();
        props.insert("label".to_string(), label);
        props.insert("subMenu".to_string(), sub_menu_array);
        let menu_item = FieldDefinition {
            mock: None,
            merged_from: None,
            name: None,
            field_type: FieldType::Object,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: Some(props),
            intrinsic: None,
            description: None,
            synonyms: None,
            projected_from: None,
        };
        ConfigField {
            field_type: "[MenuItem]".to_string(),
            default: None,
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(menu_item)),
            properties: None,
            required: None,
            ref_id: None,
            forwarded_from: None,
            nested_forwarded_from: Vec::new(),
            payload_for: None,
            projected_from: None,
        }
    }

    fn item(label: &str, sub_menu: Vec<SExpression>) -> SExpression {
        let mut obj = orbital_core_base::schema::SObject::new();
        obj.insert("label".to_string(), SExpression::String(label.to_string()));
        obj.insert("subMenu".to_string(), SExpression::Array(sub_menu));
        SExpression::Object(obj)
    }

    /// Feature test 5, valid half: a depth-3 literal against MenuItem
    /// validates — the ancestor-stack re-entry actually walks INTO the
    /// recursion (proving the Union back-reference no longer silently skips
    /// every literal beneath it) and finds it well-formed.
    #[test]
    fn depth_3_literal_against_recursive_struct_validates() {
        let field = menu_item_config_field();
        let events = HashSet::new();
        let depth3 = item("A", vec![item("B", vec![item("C", vec![])])]);
        let result = check_config_default_shape(
            &field,
            &SExpression::Array(vec![depth3]),
            "t.config.subMenu",
            &EventScope {
                events: events.clone(),
                ..Default::default()
            },
            None,
        );
        assert!(
            result.errors.is_empty(),
            "a well-formed depth-3 literal must validate: {:?}",
            result.errors
        );
    }

    /// Feature test 5, invalid half: the same depth-3 literal with a wrong
    /// inner field name (the deepest node's required `label` key renamed to
    /// a typo) must error — proof the recursion re-entry actually CHECKS the
    /// nested literal instead of only reaching it.
    #[test]
    fn depth_3_literal_with_wrong_inner_field_name_errors() {
        let field = menu_item_config_field();
        let events = HashSet::new();
        let mut typo_leaf = orbital_core_base::schema::SObject::new();
        typo_leaf.insert(
            "labell".to_string(), // typo: required "label" is now absent
            SExpression::String("C".to_string()),
        );
        typo_leaf.insert("subMenu".to_string(), SExpression::Array(vec![]));
        let depth3 = item("A", vec![item("B", vec![SExpression::Object(typo_leaf)])]);
        let result = check_config_default_shape(
            &field,
            &SExpression::Array(vec![depth3]),
            "t.config.subMenu",
            &EventScope {
                events: events.clone(),
                ..Default::default()
            },
            None,
        );
        assert!(
            !result.errors.is_empty(),
            "a required field missing three levels deep must be caught, not silently skipped"
        );
    }
}

/// `skip_required_members`: trait config keeps the strict (`false`) mode,
/// the entity adapter (`entity.rs`) uses the lenient (`true`) mode — a
/// struct/alias member's `!` is enforced at PERSIST time for an entity
/// default (lifecycle ruling), never at config-default time.
#[cfg(test)]
mod skip_required_members_tests {
    use super::*;

    /// `type Alias = { id : string!, kind : string! }` — a struct alias
    /// with two required members, mirroring `std-migration-job.lolo`'s
    /// `ImportSourceOption`.
    fn alias_with_required_members() -> FieldDefinition {
        let required_string = |name: &str| FieldDefinition {
            mock: None,
            merged_from: None,
            name: Some(name.to_string()),
            field_type: FieldType::String,
            required: Some(true),
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
            projected_from: None,
        };
        let mut props = HashMap::new();
        props.insert("id".to_string(), required_string("id"));
        props.insert("kind".to_string(), required_string("kind"));
        FieldDefinition {
            mock: None,
            merged_from: None,
            name: Some("source".to_string()),
            field_type: FieldType::Object,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: Some(props),
            intrinsic: None,
            description: None,
            synonyms: None,
            projected_from: None,
        }
    }

    /// `source : Alias = {}` — trait config keeps the strict literal: a
    /// config default has no later persist step to defer completeness to.
    #[test]
    fn strict_mode_rejects_incomplete_object_literal() {
        let schema = alias_with_required_members();
        let empty = SExpression::Object(orbital_core_base::schema::SObject::new());
        let result = check_field_def_shape(
            &schema,
            &empty,
            "t.config.source",
            &EventScope::default(),
            &HashMap::new(),
            false,
        );
        assert!(
            !result.errors.is_empty(),
            "trait config must still enforce required-member completeness"
        );
    }

    /// `type CliCommand = string | AnchoredEnv | AnchoredPath`: a union with a
    /// primitive member takes that primitive's value, not only records.
    #[test]
    fn a_union_with_a_primitive_member_takes_its_value() {
        let schema: FieldDefinition = serde_json::from_value(serde_json::json!({
            "type": "union",
            "values": ["string", "AnchoredEnv", "AnchoredPath"],
            "properties": {
                "string": { "name": "string", "type": "string" },
                "AnchoredEnv": { "name": "AnchoredEnv", "type": "object",
                    "properties": { "env": { "name": "env", "type": "string", "required": true } } },
                "AnchoredPath": { "name": "AnchoredPath", "type": "object",
                    "properties": { "path": { "name": "path", "type": "string", "required": true } } }
            }
        }))
        .expect("a union definition");
        let check = |value: SExpression| {
            check_field_def_shape(
                &schema,
                &value,
                "t.config.commands[0]",
                &EventScope::default(),
                &HashMap::new(),
                false,
            )
        };
        let text = check(SExpression::String("ls".to_string()));
        assert!(text.errors.is_empty(), "a string is the string variant: {:?}", text.errors);
        let mut env = orbital_core_base::schema::SObject::new();
        env.insert("env".to_string(), SExpression::String("HOME".to_string()));
        let record = check(SExpression::Object(env));
        assert!(record.errors.is_empty(), "a record variant still fits: {:?}", record.errors);
        assert!(
            !check(SExpression::Number(3.0)).errors.is_empty(),
            "control: a number fits no variant"
        );
    }

    #[test]
    fn partial_entity_defaults_keep_known_fields_without_requiring_later_members() {
        let schema = alias_with_required_members();
        let check = |kind, skip_required_members| {
            let mut fields = orbital_core_base::schema::SObject::new();
            fields.insert("kind".to_string(), kind);
            check_field_def_shape(
                &schema,
                &SExpression::Object(fields),
                "orbitals[0].entity.fields[1].default",
                &EventScope::default(),
                &HashMap::new(),
                skip_required_members,
            )
        };
        let partial = check(SExpression::String("text".to_string()), true);
        assert!(partial.errors.is_empty(), "partial entity default: {:?}", partial.errors);
        let strict = check(SExpression::String("text".to_string()), false);
        assert!(!strict.errors.is_empty(), "complete assignments still require id");
        let malformed = check(SExpression::Number(3.0), true);
        assert!(!malformed.errors.is_empty(), "partial defaults still check present fields");
    }

    /// The same `source : Alias = {}` shape, checked the way the entity
    /// adapter calls it: an entity default is an initial state, not a
    /// complete value, so `!` on a struct member is not enforced here.
    #[test]
    fn lenient_mode_accepts_incomplete_object_literal_for_entity_defaults() {
        let schema = alias_with_required_members();
        let empty = SExpression::Object(orbital_core_base::schema::SObject::new());
        let result = check_field_def_shape(
            &schema,
            &empty,
            "orbitals[0].entity.fields[1].default",
            &EventScope::default(),
            &HashMap::new(),
            true,
        );
        assert!(
            result.errors.is_empty(),
            "an entity default must not enforce struct-member completeness: {:?}",
            result.errors
        );
    }
}

#[cfg(test)]
mod effect_event_address_tests {
    use super::*;

    const REGISTRY: &str = r#"{ "services": { "test-svc": { "actions": [ { "name": "run", "params": [
        { "name": "tools", "type": "AgentTool[]", "required": true,
          "items": { "type": "AgentTool", "properties": { "event": { "type": "EventAddress" }, "read": { "type": "string" } } } }
    ] } ] } } }"#;

    fn run(tool_event: &str) -> ValidationResult {
        let json = format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [
        {{
          "name": "TaskOrbital",
          "entity": {{ "name": "Task", "persistence": "persistent", "fields": [] }},
          "traits": [{{
            "name": "TaskOrbitalTaskPersistor",
            "stateMachine": {{
              "states": [{{ "name": "idle", "isInitial": true }}],
              "events": [{{ "key": "DO_CREATE", "external": true }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "DO_CREATE", "effects": [] }}]
            }}
          }}]
        }},
        {{
          "name": "AssistantOrbital",
          "entity": {{ "name": "AssistantState", "persistence": "runtime", "fields": [] }},
          "traits": [{{
            "name": "AssistantLoop",
            "stateMachine": {{
              "states": [{{ "name": "idle", "isInitial": true }}],
              "events": [{{ "key": "ASK" }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "ASK", "effects": [
                ["call-service", "test-svc", "run", {{ "tools": [ {{ "event": "{tool_event}" }}, {{ "read": "Task" }} ] }}]
              ] }}]
            }}
          }}]
        }}
      ]
    }}"#
        );
        let schema: OrbitalSchema = serde_json::from_str(&json).expect("fixture parses");
        let mut ctx = ValidationContext::new();
        ctx.service_registry =
            crate::embedded::parse_service_registry_data(REGISTRY).expect("registry parses");
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    fn address_errors(result: &ValidationResult) -> Vec<&ValidationError> {
        result
            .errors
            .iter()
            .filter(|e| {
                matches!(
                    e.code,
                    ErrorCode::TraitEventRefUnresolved | ErrorCode::TraitEventRefNotExternalInput
                )
            })
            .collect()
    }

    #[test]
    fn a_stale_address_in_a_call_service_arg_is_an_error() {
        let result = run("TaskOrbital.TaskPersistor.DO_CREATE");
        let errs = address_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", result.errors);
        assert_eq!(errs[0].code, ErrorCode::TraitEventRefUnresolved);
    }

    #[test]
    fn control_an_address_naming_a_declared_input_passes() {
        let result = run("TaskOrbital.TaskOrbitalTaskPersistor.DO_CREATE");
        assert!(address_errors(&result).is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn edge_an_undeclared_event_on_a_real_trait_is_not_an_external_input() {
        let result = run("TaskOrbital.TaskOrbitalTaskPersistor.DO_DELETE");
        let errs = address_errors(&result);
        assert_eq!(errs.len(), 1, "{:?}", result.errors);
        assert_eq!(errs[0].code, ErrorCode::TraitEventRefNotExternalInput);
    }

    #[test]
    fn edge_a_binding_is_left_for_runtime() {
        let result = run("@config.tool");
        assert!(address_errors(&result).is_empty(), "{:?}", result.errors);
    }
}

#[cfg(test)]
mod top_level_event_address_tests {
    use super::*;

    const REGISTRY: &str = r#"{ "services": { "test-svc": { "actions": [ { "name": "watch", "params": [
        { "name": "match", "type": "string", "required": true },
        { "name": "event", "type": "EventAddress", "required": true }
    ] }, { "name": "query", "params": [
        { "name": "event", "type": "string", "required": false }
    ] } ] } } }"#;

    fn run(action: &str, event: &str) -> ValidationResult {
        let json = format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "FeedOrbital",
        "entity": {{ "name": "Seen", "persistence": "runtime", "fields": [] }},
        "traits": [{{
          "name": "FeedWatch",
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "events": [{{ "key": "INIT" }}, {{ "key": "ITEM_SEEN", "external": true }}],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [
                ["call-service", "test-svc", "{action}", {{ "match": "https://example.com/*", "event": "{event}" }}]
              ] }},
              {{ "from": "idle", "to": "idle", "event": "ITEM_SEEN", "effects": [] }}
            ]
          }}
        }}]
      }}]
    }}"#
        );
        let schema: OrbitalSchema = serde_json::from_str(&json).expect("fixture parses");
        let mut ctx = ValidationContext::new();
        ctx.service_registry =
            crate::embedded::parse_service_registry_data(REGISTRY).expect("registry parses");
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    fn codes(result: &ValidationResult) -> Vec<ErrorCode> {
        result
            .errors
            .iter()
            .filter(|e| {
                matches!(
                    e.code,
                    ErrorCode::TraitEventRefUnresolved | ErrorCode::TraitEventRefNotExternalInput
                )
            })
            .map(|e| e.code)
            .collect()
    }

    #[test]
    fn a_misspelled_top_level_event_address_is_not_an_external_input() {
        assert_eq!(
            codes(&run("watch", "FeedWatch.ITEM_SEN")),
            vec![ErrorCode::TraitEventRefNotExternalInput]
        );
    }

    #[test]
    fn control_a_top_level_address_naming_a_declared_input_passes() {
        assert!(codes(&run("watch", "FeedWatch.ITEM_SEEN")).is_empty());
    }

    #[test]
    fn edge_a_bare_top_level_event_is_rejected_like_every_event_address() {
        assert_eq!(
            codes(&run("watch", "ITEM_SEEN")),
            vec![ErrorCode::TraitEventRefUnresolved]
        );
    }

    #[test]
    fn edge_a_binding_is_left_for_runtime() {
        assert!(codes(&run("watch", "@config.itemEvent")).is_empty());
    }

    #[test]
    fn control_a_param_not_typed_event_address_is_not_checked() {
        assert!(codes(&run("query", "NOT_AN_INPUT")).is_empty());
    }
}

#[cfg(test)]
mod tuple_shape_tests {
    use super::*;

    fn nums(values: &[f64]) -> SExpression {
        SExpression::Array(values.iter().map(|n| SExpression::Number(*n)).collect())
    }

    fn slots() -> serde_json::Value {
        serde_json::json!({ "0": { "type": "number" }, "1": { "type": "number" }, "2": { "type": "number" } })
    }

    /// A tuple value has the tuple's arity and each slot's type — a struct member
    /// (`Light.position : [number, number, number]`) and a knob typed `[number, number, number]`.
    #[test]
    fn a_tuple_value_is_checked_for_arity_and_slot_types() {
        let member: FieldDefinition =
            serde_json::from_value(serde_json::json!({ "type": "tuple", "properties": slots() }))
                .unwrap();
        let check = |v: &SExpression| {
            check_field_def_shape(
                &member,
                v,
                "p",
                &EventScope::default(),
                &HashMap::new(),
                false,
            )
        };
        assert!(check(&nums(&[1.0, 2.0, 3.0])).errors.is_empty());
        assert!(!check(&nums(&[1.0, 2.0])).errors.is_empty(), "wrong arity");
        assert!(
            !check(&SExpression::Array(vec![
                SExpression::Number(1.0),
                SExpression::String("x".into()),
                SExpression::Number(3.0)
            ]))
            .errors
            .is_empty(),
            "wrong slot type"
        );
        assert!(
            !check(&SExpression::String("x".into())).errors.is_empty(),
            "not an array"
        );

        let knob: ConfigField = serde_json::from_value(
            serde_json::json!({ "type": "[number, number, number]", "properties": slots() }),
        )
        .unwrap();
        let check_knob = |v: &SExpression| {
            check_config_default_shape(&knob, v, "k", &EventScope::default(), None)
        };
        assert!(check_knob(&nums(&[4.0, 5.0, 6.0])).errors.is_empty());
        assert!(
            !check_knob(&nums(&[4.0, 5.0])).errors.is_empty(),
            "knob: wrong arity"
        );
        assert!(
            !check_knob(&SExpression::Array(vec![
                SExpression::String("a".into()),
                SExpression::Number(5.0),
                SExpression::Number(6.0)
            ]))
            .errors
            .is_empty(),
            "knob: wrong slot type"
        );
    }
}
