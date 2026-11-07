//! Config field structural shape validation against the typed declaration
//! carriers (`items` / `properties` / `values`) populated by the lolo lowerer.
//!
//! Checks that every config field's `default` value matches its declared
//! structural shape (array vs scalar, object vs primitive, enum
//! membership, per-element / per-property typing recursively).
//! Also validates config overrides in trait references (TODO: requires
//! inline resolution for the upstream declaration lookup).

use orbital_core_eval::evaluator::IsoShape;
use std::collections::{HashMap, HashSet};

use orbital_core_base::schema::{
    config_ref_event_knob, definer_knobs_for_trait, ConfigField, EntityDefinition, FieldDefinition,
    FieldType, Orbital, OrbitalSchema, PayloadField, SExpression, TagReading, TraitDefinition,
    TraitReference, UnionVariantChoice,
};
use orbital_core_base::ir::OirType as OrbitalOirType;
use orbital_core_base::type_compat::set_value_type_compatible;

use super::error::{ErrorCode, ValidationError};
use super::render_ui_binding::{resolve_binding_type, BindingContext, BindingType};
use super::{ValidationContext, ValidationResult};
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
                    events.insert(value.into_inner());
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
    if head == "call-service" {
        if let (
            Some(SExpression::String(service)),
            Some(SExpression::String(action)),
            Some(SExpression::Object(args)),
        ) = (items.get(1), items.get(2), items.get(3))
        {
            let declared = registry.params.get(service.as_str()).and_then(|a| a.get(action.as_str()));
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
    payloads: HashMap<String, Vec<PayloadField>>,
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
        Self { inputs_by_trait, payloads: crate::phases::inline::sigil_types::build_payload_index(schema) }
    }

    fn payload_of(&self, event: &str) -> Option<&[PayloadField]> {
        self.payloads.get(event).map(Vec::as_slice)
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
    /// The record aliases in scope: the orbital's own, then the program's.
    aliases: Vec<&'a std::collections::BTreeMap<String, Vec<PayloadField>>>,
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
pub(crate) fn config_declared_kind(field_type: &str) -> Option<&str> {
    match field_type {
        "event" | "field" | "slot" | "pattern" | "icon" | "component" | "secret" | "asset" => Some("string"),
        "int" | "float" => Some("number"),
        width if orbital_core_base::num_width::NumWidth::from_name(width).is_some() => Some("number"),
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
                        for (k, v) in map.iter() {
                            let SExpression::String(s) = v else { continue };
                            if s != value_str {
                                continue;
                            }
                            let Some(schema) = props.get(k.as_str()) else { continue };
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
/// "wired":true,"default":"@entity.X"}` — the `wired` mark is the structural
/// discriminator for wiring-vs-declaration (`is_declaration_form`), so the
/// binding's type was never compared against anything.
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
    let resolved = resolve_binding_type(&SExpression::string(value_str.to_string()), &render_ctx);
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

/// Validate all config fields in the schema.
pub fn validate_all_config_types(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();


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
                            aliases: orbital.types.iter().chain(schema.types.iter()).collect(),
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
                        // (not a plain wiring), validate its structural
                        // shape — the same checks that run for atom declarations.
                        // Plain wiring entries (`!is_declaration_form()`) are
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
                        if cf.is_declaration_form() {
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

/// An entity row's declared defaults seed the singleton row, so each literal default must fit its field's
/// declared type. Required-member completeness of an object default is deferred to persist, so it is not
/// checked here.
pub fn validate_all_entity_field_defaults(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let Some(entity) = orbital.entity.as_inline() else {
            continue;
        };
        let events = EventScope {
            events: orbital_event_names(orbital),
            orbital: &orbital.name,
            refs: None,
        };
        for field in &entity.fields {
            let (Some(default), Some(name)) = (&field.default, &field.name) else {
                continue;
            };
            let path = format!("orbitals[{}].entity({}).field({})", oi, entity.name, name);
            let shape = without_required_members(field);
            let found = check_field_def_shape(&shape, default, &path, &events, &HashMap::new());
            for e in found.errors {
                result.add_error(ValidationError::error(
                    ErrorCode::EntityFieldDefaultShapeMismatch,
                    e.path,
                    e.message,
                ));
            }
        }
    }
    ctx.merge(result);
}

fn without_required_members(field: &FieldDefinition) -> FieldDefinition {
    let mut out = field.clone();
    out.required = None;
    if let Some(props) = out.properties.as_mut() {
        for member in props.values_mut() {
            *member = without_required_members(member);
        }
    }
    if let Some(items) = out.items.as_mut() {
        **items = without_required_members(items);
    }
    out
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

    let ft = field.field_type.as_str();

    // Structural shape check — runs against the typed carriers
    // (`items` / `properties` / `values`) populated by the lolo lowerer.
    // Skips fields with no `default` and skips opaque types we don't
    // know how to validate (`object` / `unknown` / etc.).
    let names_fields = matches!(ft, "field" | "[field]");
    if let (Some(default), false) = (&field.default, names_fields) {
        result.merge(check_config_default_shape(
            field,
            default,
            path,
            orbital_events,
            binding_ctx,
        ));
    }

    if let (Some(bctx), Some(default)) = (binding_ctx, &field.default) {
        let knob = event_ctx.map_or(path, |(_, name)| name);
        result.merge(check_field_ref_knob(field, knob, default, path, bctx));
    }
    if names_fields {
        return result;
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

    // `event` knob defaults: membership in the trait's event set and the string-ness of the default are the std lint
    // `config-knob`; only an `Trait.EVENT` address is resolved here.
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
                    if !is_definer {
                        if let Some(refs) = orbital_events.refs {
                            result.add_errors(refs.check(s, orbital_events.orbital, path));
                        }
                    }
                }
                Some(_) => {}
            }
        }
        return result;
    }

    result
}

/// The fields of the record `R` a `field R T` knob names, with a label for messages. `None` when `R` is not known at
/// this call site (an unbound type variable, a knob that holds no literal): nothing is checked then.
fn field_ref_record_fields(
    record: Option<&str>,
    properties: Option<&orbital_core_base::collections::IndexMap<String, FieldDefinition>>,
    bctx: &ReferenceBindingCtx,
) -> Option<(String, Vec<(String, OrbitalOirType)>)> {
    use orbital_core_base::ir::types::{field_shape, payload_field_shape};
    let shaped = |defs: &[FieldDefinition]| -> Vec<(String, OrbitalOirType)> {
        defs.iter()
            .filter_map(|def| Some((def.name.clone()?, field_shape(def).unwrap_or_else(|_| OrbitalOirType::erased()))))
            .collect()
    };
    let knob_text = |knob: &str| -> Option<String> {
        match bctx.trait_def.config.as_ref()?.get(knob)?.default.as_ref()? {
            SExpression::String(text) if !text.is_empty() && !text.starts_with('@') => {
                Some(text.clone().into_inner())
            }
            _ => None,
        }
    };
    // A column an imported atom's entity merged in (`merged_from`) is not one the host declares or writes, so a
    // knob may not name it.
    let entity_fields = |name: &str| -> Option<(String, Vec<(String, OrbitalOirType)>)> {
        let entity = bctx.entity_definitions.get(name)?;
        let declared: Vec<FieldDefinition> =
            entity.fields.iter().filter(|f| f.merged_from.is_none()).cloned().collect();
        Some((name.to_string(), shaped(&declared)))
    };
    if let Some(properties) = properties.filter(|p| !p.is_empty()) {
        let fields = properties
            .iter()
            .map(|(name, def)| (name.clone(), field_shape(def).unwrap_or_else(|_| OrbitalOirType::erased())))
            .collect();
        return Some((record.unwrap_or("the record").to_string(), fields));
    }
    let record = record?;
    if record == "@entity" {
        return entity_fields(bctx.trait_def.linked_entity.as_deref()?);
    }
    if let Some(knob) = record.strip_prefix("@entity(").and_then(|r| r.strip_suffix(')')) {
        return entity_fields(&knob_text(knob)?);
    }
    if let Some(knob) = record.strip_prefix("@payload(").and_then(|r| r.strip_suffix(')')) {
        let event = knob_text(knob)?;
        let declared = bctx
            .trait_def
            .state_machine
            .events
            .as_ref()?
            .iter()
            .find(|candidate| candidate.key == event)?;
        if let Some(entity) = &declared.payload_entity {
            return entity_fields(entity);
        }
        let fields = declared
            .payload_schema
            .as_ref()?
            .iter()
            .map(|field| (field.name.clone(), payload_field_shape(field).unwrap_or_else(|_| OrbitalOirType::erased())))
            .collect();
        return Some((format!("the payload of {event}"), fields));
    }
    if record.starts_with('$') || record.starts_with('@') {
        return None;
    }
    if let Some(fields) = entity_fields(record) {
        return Some(fields);
    }
    let members = bctx.aliases.iter().find_map(|aliases| aliases.get(record))?;
    Some((
        record.to_string(),
        members
            .iter()
            .map(|member| (member.name.clone(), payload_field_shape(member).unwrap_or_else(|_| OrbitalOirType::erased())))
            .collect(),
    ))
}

/// A `field R T` knob (or a list of them): every name a call site gives is a field of `R`, and that field's type
/// satisfies `T` when the knob declares one. Bindings and computed values defer.
fn check_field_ref_knob(
    field: &ConfigField,
    knob: &str,
    value: &SExpression,
    path: &str,
    bctx: &ReferenceBindingCtx,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if is_deferred_computed(value) {
        return result;
    }
    let named: Vec<(String, &str)> = match (field.field_type.as_str(), value) {
        ("field", SExpression::String(name)) => vec![(path.to_string(), name.as_str())],
        ("[field]", SExpression::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                SExpression::String(name) => Some((format!("{path}[{i}]"), name.as_str())),
                _ => None,
            })
            .collect(),
        _ => {
            let mut nested = Vec::new();
            if let (Some(item), SExpression::Array(entries)) = (field.items.as_deref(), value) {
                for (i, entry) in entries.iter().enumerate() {
                    nested_field_refs(item, entry, &format!("{path}[{i}]"), &mut nested);
                }
            }
            for (member, def) in field.properties.iter().flat_map(|p| p.iter()) {
                if let SExpression::Object(entries) = value {
                    if let Some(entry) = entries.get(member) {
                        nested_field_refs(def, entry, &format!("{path}.{member}"), &mut nested);
                    }
                }
            }
            for (at, name, def) in nested {
                result.merge(check_field_ref_names(
                    knob,
                    vec![(at, name)],
                    def.record.as_deref(),
                    def.properties.as_ref(),
                    def.items.as_deref(),
                    bctx,
                ));
            }
            return result;
        }
    };
    let (record, properties, element) = if field.field_type == "[field]" {
        let item = field.items.as_deref();
        (
            item.and_then(|i| i.record.as_deref()),
            item.and_then(|i| i.properties.as_ref()),
            item.and_then(|i| i.items.as_deref()),
        )
    } else {
        (
            field.record.as_deref(),
            field.properties.as_ref(),
            field.items.as_deref(),
        )
    };
    result.merge(check_field_ref_names(knob, named, record, properties, element, bctx));
    result
}

/// Every `field R T` value inside a list or record value, with the definition that types it.
fn nested_field_refs<'a>(
    def: &'a FieldDefinition,
    value: &'a SExpression,
    path: &str,
    out: &mut Vec<(String, &'a str, &'a FieldDefinition)>,
) {
    match value {
        SExpression::String(name) if def.field_type == FieldType::Field => {
            out.push((path.to_string(), name.as_str(), def));
        }
        SExpression::Array(items) => {
            if let Some(item) = def.items.as_deref() {
                for (i, entry) in items.iter().enumerate() {
                    nested_field_refs(item, entry, &format!("{path}[{i}]"), out);
                }
            }
        }
        SExpression::Object(entries) => {
            for (member, member_def) in def.properties.iter().flat_map(|p| p.iter()) {
                if let Some(entry) = entries.get(member) {
                    nested_field_refs(member_def, entry, &format!("{path}.{member}"), out);
                }
            }
        }
        _ => {}
    }
}

fn check_field_ref_names(
    knob: &str,
    named: Vec<(String, &str)>,
    record: Option<&str>,
    properties: Option<&orbital_core_base::collections::IndexMap<String, FieldDefinition>>,
    element: Option<&FieldDefinition>,
    bctx: &ReferenceBindingCtx,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some((label, fields)) = field_ref_record_fields(record, properties, bctx) else {
        return result;
    };
    let expected = element.and_then(|element| orbital_core_base::ir::types::field_shape(element).ok());
    let fits = |actual: &OrbitalOirType| match &expected {
        Some(expected) => orbital_core_base::type_compat::assignable(actual, expected),
        None => true,
    };
    let mut valid: Vec<String> = fields
        .iter()
        .filter(|(_, ty)| fits(ty))
        .map(|(name, _)| name.clone())
        .collect();
    valid.sort();
    for (at, name) in named {
        if name.starts_with('@') {
            continue;
        }
        let Some((_, actual)) = fields.iter().find(|(candidate, _)| candidate == name) else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::ConfigFieldRefUnknown,
                    &at,
                    format!("`{knob}` names `{name}`, which is not a field of {label}"),
                )
                .with_suggestion(format!("name one of the fields of {label}: {}", valid.join(", ")))
                .with_valid_values(valid.clone()),
            );
            continue;
        };
        if let Some(expected) = &expected {
            if !fits(actual) {
                let actual = orbital_core_base::type_compat::oir_type_kind(actual);
                let wanted = orbital_core_base::type_compat::oir_type_kind(expected);
                result.add_error(
                    ValidationError::error(
                        ErrorCode::ConfigFieldRefTypeMismatch,
                        &at,
                        format!(
                            "`{knob}` names `{name}`, a {actual} field of {label}; the knob takes {wanted} fields"
                        ),
                    )
                    .with_suggestion(format!(
                        "name one of the {wanted} fields of {label}: {}",
                        valid.join(", ")
                    ))
                    .with_valid_values(valid.clone()),
                );
            }
        }
    }
    result
}

/// Walk the declared shape (a `ConfigField` carrying `field_type` +
/// optional `items` / `properties` / `values`) and check the actual
/// `default` value against it.
/// Whether `name` is a record alias of the program (its declared shape is a struct, not a tagged union), resolved
/// against the aliases in scope; a name that is no alias there, or whose shape is undeclared, is not.
fn names_a_record(name: &str, binding_ctx: Option<&ReferenceBindingCtx>) -> bool {
    let Some(bctx) = binding_ctx else { return false };
    let Some(members) = bctx.aliases.iter().find_map(|aliases| aliases.get(name)) else {
        return false;
    };
    matches!(orbital_core_base::ir::types::declared_alias_shape(members), Ok(OrbitalOirType::Struct { .. }))
}

/// The member types of the union a knob's alias name stands for, when the alias is in scope and is not a record.
fn union_alias_members(name: &str, binding_ctx: Option<&ReferenceBindingCtx>) -> Option<Vec<OrbitalOirType>> {
    let members = binding_ctx?.aliases.iter().find_map(|aliases| aliases.get(name))?;
    match orbital_core_base::ir::types::declared_alias_shape(members) {
        Ok(OrbitalOirType::Union { variants }) => Some(variants.iter().map(|v| v.field_type.clone()).collect()),
        _ => None,
    }
}

/// The scalar tag `check_primitive_shape` reads for a union member, `None` for a member that is not a scalar.
fn union_member_tag(member: &OrbitalOirType) -> Option<&'static str> {
    match member {
        OrbitalOirType::String => Some("string"),
        OrbitalOirType::Number => Some("number"),
        OrbitalOirType::Integer => Some("int"),
        OrbitalOirType::Sized { width } => Some(width.name()),
        OrbitalOirType::Boolean => Some("boolean"),
        _ => None,
    }
}

fn check_union_alias_default(
    alias: &str,
    members: &[OrbitalOirType],
    default: &SExpression,
    path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let fits = matches!(default, SExpression::Null)
        || members.iter().any(|member| match union_member_tag(member) {
            Some(tag) => check_primitive_shape(tag, default, path).errors.is_empty(),
            None => matches!(default, SExpression::Object(_) | SExpression::Array(_)),
        });
    if !fits {
        let tags: Vec<&str> = members.iter().map(|m| union_member_tag(m).unwrap_or("object")).collect();
        result.add_error(ValidationError::error(
            ErrorCode::TraitConfigShapeMismatch,
            path,
            format!(
                "config default {} fits no member of '{}' ({})",
                super::sexpr::render_sexpr_brief(default),
                alias,
                tags.join(" | ")
            ),
        ));
    }
    result
}

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
            for (key, entry) in entries.iter() {
                probe.merge(check_field_def_shape(
                    value_schema,
                    entry,
                    &format!("{}.{}", path, key),
                    orbital_events,
                    &ancestors,
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
                ));
            }
        }
        return result;
    }
    if let Some(arms) = union_arms(field.values.as_ref(), field.properties.as_ref()) {
        if let SExpression::String(s) = default {
            if !s.starts_with('@') && !arms.iter().any(|(_, def)| def.field_type == FieldType::String) {
                result.add_error(ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    format!(
                        "config default '{}' fits no arm of the union ({})",
                        s,
                        arms.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
            return result;
        }
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
        if let Some(members) = union_alias_members(&field.field_type, binding_ctx) {
            result.merge(check_union_alias_default(&field.field_type, &members, default, path));
            return result;
        }
        if !matches!(default, SExpression::Object(_) | SExpression::Null) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    path,
                    format!(
                        "config default has type '{}' but the value isn't an object, got {}",
                        field.field_type,
                        super::sexpr::render_sexpr_brief(default)
                    ),
                )
                .with_suggestion("write an object literal `{ … }` with the declared members".to_string()),
            );
            return result;
        }
        if let SExpression::Object(map) = default {
            // A property can self-reference the OWNING alias (`type MenuItem
            // = { subMenu : [MenuItem] }` on a knob declared `lighting :
            // MenuItem`) — the ancestor for that case is THIS struct itself,
            // synthesized here since a `ConfigField` isn't a `FieldDefinition`
            // (they're structurally parallel but distinct wire types).
            let root_struct = FieldDefinition {
                nullable: None,
                additional_properties: None,
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
                record: None,
                derived: None,
            };
            let ancestors = root_ancestor_map(field, Some(&root_struct));
            let mut probe = ValidationResult::new();
            let is_record = field.field_type == "object" || names_a_record(&field.field_type, binding_ctx);
            for (key, prop_schema) in props {
                if map.get(key).is_none() && is_record && prop_schema.required.unwrap_or(false) {
                    probe.add_error(ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        &format!("{}.{}", path, key),
                        format!("required field '{}' missing on object literal", key),
                    ));
                }
                if let Some(prop_value) = map.get(key) {
                    probe.merge(check_field_def_shape(
                        prop_schema,
                        prop_value,
                        &format!("{}.{}", path, key),
                        orbital_events,
                        &ancestors,
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
/// A tuple value: an array of exactly the tuple's arity, each element checked against
/// its own slot.
fn check_tuple_shape(
    slots: &[&FieldDefinition],
    value: &SExpression,
    path: &str,
    orbital_events: &EventScope<'_>,
    ancestors: &HashMap<String, &FieldDefinition>,
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
        ));
    }
    result
}

/// A struct member typed `@payload(.<sibling>)`: when the sibling holds a literal event name that some trait emits,
/// the member's value is checked against that event's declared payload. A sibling that is not a literal, or an event
/// nothing declares, defers.
fn check_payload_of_member(
    member: &FieldDefinition,
    value: &SExpression,
    instance: &orbital_core_base::schema::SObject,
    path: &str,
    orbital_events: &EventScope<'_>,
    ancestors: &HashMap<String, &FieldDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let (Some(sibling), Some(refs)) = (member.payload_sibling(), orbital_events.refs) else {
        return result;
    };
    let Some(SExpression::String(raw)) = instance.get(sibling) else {
        return result;
    };
    let (event, _) = orbital_core_base::schema::split_event_address(raw);
    let Some(declared) = refs.payload_of(&event) else {
        return result;
    };
    let SExpression::Object(given) = value else {
        result.add_error(ValidationError::error(
            ErrorCode::TraitConfigShapeMismatch,
            path,
            format!("expected the payload object of event '{event}', got {}", super::sexpr::render_sexpr_brief(value)),
        ));
        return result;
    };
    let mut known: Vec<&str> = declared.iter().map(|field| field.name.as_str()).collect();
    known.sort_unstable();
    for key in given.keys() {
        if !known.contains(&key.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigShapeMismatch,
                    format!("{path}.{key}"),
                    format!("field '{key}' is not on the declared payload of event '{event}' (declared: {})", known.join(", ")),
                )
                .with_valid_values(known.iter().map(|name| name.to_string()).collect()),
            );
        }
    }
    let shape = FieldDefinition {
        properties: Some(
            declared
                .iter()
                .map(|field| (field.name.clone(), crate::phases::inline::sigil_types::field_of(field)))
                .collect(),
        ),
        ..member.clone()
    };
    let shape = FieldDefinition { record: None, ..shape };
    result.merge(check_field_def_shape(&shape, value, path, orbital_events, ancestors));
    result
}

/// `null`, or the bare word `none` (which lowers to the string `"none"` outside trait/orbital/behavior-typed
/// fields): the unset value an optional (`T?`) field may hold at any depth.
fn is_unset_literal(value: &SExpression) -> bool {
    matches!(value, SExpression::Null)
}

pub(crate) fn check_field_def_shape(
    schema: &FieldDefinition,
    value: &SExpression,
    path: &str,
    orbital_events: &EventScope<'_>,
    ancestors: &HashMap<String, &FieldDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    // Same computed-value deferral as the top-level default: a per-element
    // or per-property value can itself be a binding or an operator call
    // (e.g. an object property whose value is `(array/map ...)`).
    if is_deferred_computed(value) {
        return result;
    }
    if schema.nullable == Some(true) && is_unset_literal(value) {
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
            let keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
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
                            .filter(|k| !props.contains_key(k.as_str()))
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
                        Some(prop_value) if prop_schema.payload_sibling().is_some() => {
                            result.merge(check_payload_of_member(
                                prop_schema,
                                prop_value,
                                map,
                                &prop_path,
                                orbital_events,
                                ancestors,
                            ));
                        }
                        Some(prop_value) => {
                            result.merge(check_field_def_shape(
                                prop_schema,
                                prop_value,
                                &prop_path,
                                orbital_events,
                                ancestors,
                            ));
                        }
                        None if prop_schema.required.unwrap_or(false) => {
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
            if let (Some(arms), SExpression::String(s)) = (union_arms(schema.values.as_ref(), schema.properties.as_ref()), value) {
                if !s.starts_with('@') && !arms.iter().any(|(_, def)| def.field_type == FieldType::String) {
                    result.add_error(ValidationError::error(
                        ErrorCode::TraitConfigShapeMismatch,
                        path,
                        format!(
                            "'{}' fits no arm of the union ({})",
                            s,
                            arms.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
                        ),
                    ));
                }
                return result;
            }
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
                            && !orbital_events.events.contains(s.as_str())
                        {
                            let mut valid: Vec<String> =
                                orbital_events.events.iter().cloned().collect();
                            valid.sort();
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::TraitConfigShapeMismatch,
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
                                ErrorCode::TraitConfigShapeMismatch,
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
                                ErrorCode::TraitConfigShapeMismatch,
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
                FieldType::Int => "int",
                FieldType::Sized(width) => width.name(),
                FieldType::Boolean => "boolean",
                FieldType::Date => "date",
                FieldType::Datetime => "datetime",
                FieldType::Timestamp => "timestamp",
                FieldType::Instant => "instant",
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

fn iso_literal_is(value: &SExpression, shape: IsoShape) -> bool {
    matches!(value, SExpression::String(s) if orbital_core_eval::evaluator::parse_iso(s).is_some_and(|(_, found)| found == shape))
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
        width if orbital_core_base::num_width::NumWidth::from_name(width).is_some() => actual == "number",
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
        "date" => iso_literal_is(value, IsoShape::Date),
        "datetime" | "instant" | "timestamp" => iso_literal_is(value, IsoShape::DateTime),
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

}

#[cfg(test)]
mod event_config_tests {
    use super::*;

    fn run(schema_json: &str) -> ValidationResult {
        let schema: OrbitalSchema = serde_json::from_str(schema_json).expect("fixture parses");
        let mut ctx = ValidationContext::new();
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    #[test]
    fn a_dotted_value_is_an_address_not_a_member_name() {
        let result = run(
            r#"{
      "name": "t", "version": "1.0.0",
      "orbitals": [{
        "name": "O",
        "entity": { "name": "W", "fields": [] },
        "traits": [{
          "name": "Picker",
          "config": { "clickEvent": { "type": "event", "default": "note.search" } },
          "emits": [{ "event": "PICK", "scope": "internal" }],
          "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "transitions": [
              { "from": "idle", "to": "idle", "event": "INIT", "effects": [] },
              { "from": "idle", "to": "idle", "event": "PICK", "effects": [] }
            ]
          }
        }]
      }]
    }"#,
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::TraitEventRefUnresolved),
            "{:?}",
            result.errors
        );
    }

    fn entity_default_fixture(birth_default: &str, age_default: &str) -> String {
        format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {{ "name": "FormEntry", "fields": [
          {{ "name": "birthDate", "type": "datetime", "default": {birth_default} }},
          {{ "name": "age", "type": "number", "default": {age_default} }}
        ] }},
        "traits": [{{
          "name": "Panel",
          "config": {{ "limit": {{ "type": "number", "default": "abc" }} }},
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }}]
          }}
        }}]
      }}]
    }}"#
        )
    }

    fn run_with_entities(schema_json: &str) -> ValidationResult {
        let schema: OrbitalSchema = serde_json::from_str(schema_json).expect("fixture parses");
        let mut ctx = ValidationContext::new();
        validate_all_config_types(&schema, &mut ctx);
        validate_all_entity_field_defaults(&schema, &mut ctx);
        ctx.result
    }

    #[test]
    fn g_orbital_core_133_every_primitive_tag_checks_its_entity_default() {
        let cases = [
            ("string", r#"1"#, r#""a""#),
            ("number", r#""abc""#, r#"1.5"#),
            ("int", r#""abc""#, r#"3"#),
            ("boolean", r#""true""#, r#"true"#),
            ("date", r#""""#, r#""2024-01-01""#),
            ("datetime", r#""""#, r#""2024-01-01T00:00:00Z""#),
            ("timestamp", r#""""#, r#""2024-01-01T00:00:00Z""#),
            ("instant", r#""""#, r#""2024-01-01T00:00:00Z""#),
        ];
        for (ty, bad, good) in cases {
            let doc = |default: &str| {
                format!(
                    r#"{{ "name": "t", "version": "1.0.0", "orbitals": [{{ "name": "O",
                    "entity": {{ "name": "Row", "fields": [{{ "name": "f", "type": "{ty}", "default": {default} }}] }},
                    "traits": [] }}] }}"#
                )
            };
            let fit = run_with_entities(&doc(good));
            assert!(fit.errors.is_empty(), "{ty}: a fitting default raises nothing: {:?}", fit.errors);
            let misfit = run_with_entities(&doc(bad));
            assert!(
                misfit.errors.iter().any(|e| e.code == ErrorCode::EntityFieldDefaultShapeMismatch),
                "{ty}: default {bad} does not fit: {:?}",
                misfit.errors
            );
        }
    }

    #[test]
    fn g_orbital_core_133_nullable_null_and_partial_object_defaults_are_accepted() {
        let doc = r#"{ "name": "t", "version": "1.0.0", "orbitals": [{ "name": "O",
            "entity": { "name": "Row", "fields": [
              { "name": "when", "type": "datetime", "nullable": true, "default": null },
              { "name": "box", "type": "object", "default": {},
                "properties": { "a": { "type": "string", "required": true } } }
            ] }, "traits": [] }] }"#;
        let result = run_with_entities(doc);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn g_orbital_core_133_optional_null_is_accepted_and_the_word_none_is_refused_at_every_depth() {
        let doc = |program_nullable: bool, stepprogram_nullable: bool, unset: &str| {
            format!(
                r#"{{ "name": "t", "version": "1.0.0", "orbitals": [{{ "name": "O",
                "entity": {{ "name": "WorkspaceView", "fields": [
                  {{ "name": "stepProgram", "type": "object", "nullable": {stepprogram_nullable}, "default": {unset},
                    "properties": {{ "name": {{ "type": "string" }} }} }},
                  {{ "name": "build", "type": "object",
                    "default": {{ "request": "", "program": {unset}, "tags": [] }},
                    "properties": {{
                      "request": {{ "type": "string" }},
                      "program": {{ "type": "object", "nullable": {program_nullable},
                                    "properties": {{ "name": {{ "type": "string" }} }} }},
                      "tags": {{ "type": "array", "items": {{ "type": "string" }} }}
                    }} }}
                ] }}, "traits": [] }}] }}"#
            )
        };
        let optional = run_with_entities(&doc(true, true, "null"));
        assert!(optional.errors.is_empty(), "optional null, top-level and nested: {:?}", optional.errors);

        let word = run_with_entities(&doc(true, true, r#""none""#));
        assert!(
            word.errors.iter().any(|e| e.path.contains("stepProgram")) && word.errors.iter().any(|e| e.path.contains("program")),
            "`none` outside a trait-typed field is the text \"none\", not an unset value: {:?}",
            word.errors
        );

    }

    #[test]
    fn g_orbital_core_133_entity_field_default_must_fit_its_declared_type() {
        let bad = run_with_entities(&entity_default_fixture(r#""""#, r#""abc""#));
        assert!(
            bad.errors.iter().any(|e| e.path.contains("config(limit)")),
            "control: the config default check runs on this fixture: {:?}",
            bad.errors
        );
        let good = run_with_entities(&entity_default_fixture(r#""2024-01-01T00:00:00Z""#, "3"));
        assert!(
            !good.errors.iter().any(|e| e.path.contains("FormEntry")),
            "control: fitting entity defaults raise nothing: {:?}",
            good.errors
        );
        for field in ["birthDate", "age"] {
            assert!(
                bad.errors.iter().any(|e| e.path.contains("FormEntry") && e.path.contains(field)),
                "entity field {field} has a default that does not fit its type and must be an error: {:?}",
                bad.errors
            );
        }
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
            .filter(|e| e.code == ErrorCode::TraitConfigShapeMismatch)
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

    fn field_knob_schema(config: &str, events: &str) -> String {
        format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {{ "name": "Contact", "fields": [
          {{ "name": "name", "type": "string" }},
          {{ "name": "email", "type": "string" }},
          {{ "name": "age", "type": "number" }},
          {{ "name": "nickname", "type": "string", "mergedFrom": "Person" }}
        ] }},
        "traits": [{{
          "name": "Panel",
          "linkedEntity": "Contact",
          "config": {{ {config} }},
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "events": [{events}],
            "transitions": [
              {{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }},
              {{ "from": "idle", "to": "idle", "event": "SELECT", "effects": [] }}
            ]
          }}
        }}]
      }}]
    }}"#
        )
    }

    fn field_ref_errors(result: &ValidationResult, code: ErrorCode) -> Vec<&ValidationError> {
        result.errors.iter().filter(|e| e.code == code).collect()
    }

    fn no_field_ref_errors(result: &ValidationResult) -> bool {
        field_ref_errors(result, ErrorCode::ConfigFieldRefUnknown).is_empty()
            && field_ref_errors(result, ErrorCode::ConfigFieldRefTypeMismatch).is_empty()
    }

    #[test]
    fn a_field_knob_names_a_field_of_the_bound_entity() {
        let strings = |names: &str| {
            run(&field_knob_schema(
                &format!(
                    r#""titleFields": {{ "type": "[field]", "items": {{ "type": "field", "record": "@entity", "items": {{ "type": "string" }} }}, "default": [{names}] }}"#
                ),
                "",
            ))
        };
        let ok = strings(r#""name", "email""#);
        assert!(no_field_ref_errors(&ok), "{:?}", ok.errors);

        let merged = strings(r#""nickname""#);
        assert_eq!(
            field_ref_errors(&merged, ErrorCode::ConfigFieldRefUnknown).len(),
            1,
            "a column an imported atom merged in is not one the host declares: {:?}",
            merged.errors
        );

        let typo = strings(r#""name", "nmae""#);
        let unknown = field_ref_errors(&typo, ErrorCode::ConfigFieldRefUnknown);
        assert_eq!(unknown.len(), 1, "{:?}", typo.errors);
        assert!(
            unknown[0]
                .message
                .contains("`titleFields` names `nmae`, which is not a field of Contact"),
            "{}",
            unknown[0].message
        );
        assert!(field_ref_errors(&typo, ErrorCode::ConfigFieldRefTypeMismatch).is_empty());

        let empty = strings(r#""name", """#);
        assert_eq!(
            field_ref_errors(&empty, ErrorCode::ConfigFieldRefUnknown).len(),
            1,
            "an empty name is not a field; an unset knob is absent: {:?}",
            empty.errors
        );

        let number = strings(r#""age""#);
        let mismatch = field_ref_errors(&number, ErrorCode::ConfigFieldRefTypeMismatch);
        assert_eq!(mismatch.len(), 1, "{:?}", number.errors);
        assert!(
            mismatch[0].message.contains(
                "`titleFields` names `age`, a number field of Contact; the knob takes string fields"
            ),
            "{}",
            mismatch[0].message
        );
        assert!(field_ref_errors(&number, ErrorCode::ConfigFieldRefUnknown).is_empty());

        // control: a knob that names any field accepts the number field, and still rejects a typo.
        let any = |name: &str| {
            run(&field_knob_schema(
                &format!(
                    r#""groupByField": {{ "type": "field", "record": "@entity", "default": "{name}" }}"#
                ),
                "",
            ))
        };
        assert!(no_field_ref_errors(&any("age")), "{:?}", any("age").errors);
        assert_eq!(
            field_ref_errors(&any("agee"), ErrorCode::ConfigFieldRefUnknown).len(),
            1
        );

        // control: an entity named as the record is checked against that entity, not the bound one.
        let other = run(&format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {{ "name": "Contact", "fields": [ {{ "name": "name", "type": "string" }} ] }},
        "traits": [{{
          "name": "Panel",
          "linkedEntity": "Contact",
          "config": {{ "k": {{ "type": "field", "record": "Company", "default": "headcount" }} }},
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT", "effects": [] }}]
          }}
        }}]
      }}, {{
        "name": "P",
        "entity": {{ "name": "Company", "fields": [ {{ "name": "headcount", "type": "number" }} ] }},
        "traits": []
      }}]
    }}"#
        ));
        assert!(no_field_ref_errors(&other), "{:?}", other.errors);
    }

    #[test]
    fn a_field_knob_names_a_field_of_an_event_payload() {
        let knob = |items: &str, name: &str| {
            run(&field_knob_schema(
                &format!(
                    r#""event": {{ "type": "event", "default": "SELECT" }}, "coalesceKey": {{ "type": "field", "record": "@payload(event)", {items} "default": "{name}" }}"#
                ),
                r#"{ "key": "SELECT", "payloadSchema": [ { "name": "title", "type": "string" }, { "name": "rank", "type": "number" } ] }"#,
            ))
        };
        assert!(no_field_ref_errors(&knob("", "title")), "{:?}", knob("", "title").errors);
        // a payload field is not an entity field: `age` belongs to Contact, not to SELECT.
        let wrong = knob("", "age");
        let unknown = field_ref_errors(&wrong, ErrorCode::ConfigFieldRefUnknown);
        assert_eq!(unknown.len(), 1, "{:?}", wrong.errors);
        assert!(unknown[0].message.contains("not a field of the payload of SELECT"), "{}", unknown[0].message);
        let typed = r#""items": { "type": "string" },"#;
        let mismatch = knob(typed, "rank");
        assert_eq!(
            field_ref_errors(&mismatch, ErrorCode::ConfigFieldRefTypeMismatch).len(),
            1,
            "{:?}",
            mismatch.errors
        );
        // control: a string payload field satisfies the string knob; any-field accepts the number one.
        assert!(no_field_ref_errors(&knob(typed, "title")));
        assert!(no_field_ref_errors(&knob("", "rank")));
    }

    #[test]
    fn a_field_knob_over_a_record_alias_is_not_shape_checked_as_the_record() {
        let point = r#""properties": { "x": { "type": "number" }, "label": { "type": "string" } },"#;
        let knob = |items: &str, name: &str| {
            run(&field_knob_schema(
                &format!(
                    r#""column": {{ "type": "field", "record": "Point", {point} {items} "default": "{name}" }}"#
                ),
                "",
            ))
        };
        let string = r#""items": { "type": "string" },"#;
        let ok = knob(string, "label");
        assert!(ok.errors.is_empty(), "{:?}", ok.errors);
        let unknown = knob(string, "z");
        assert!(field_ref_errors(&unknown, ErrorCode::ConfigFieldRefUnknown).len() == 1, "{:?}", unknown.errors);
        assert!(field_ref_errors(&unknown, ErrorCode::TraitConfigShapeMismatch).is_empty(), "{:?}", unknown.errors);
        let number = r#""items": { "type": "number" },"#;
        let wrong = knob(number, "label");
        assert!(field_ref_errors(&wrong, ErrorCode::ConfigFieldRefTypeMismatch).len() == 1, "{:?}", wrong.errors);
        assert!(field_ref_errors(&wrong, ErrorCode::TraitConfigShapeMismatch).is_empty(), "{:?}", wrong.errors);
    }

    #[test]
    fn a_field_knob_names_a_field_of_a_record_alias() {
        let point = r#""properties": { "x": { "type": "number" }, "label": { "type": "string" } },"#;
        let knob = |items: &str, name: &str| {
            run(&field_knob_schema(
                &format!(
                    r#""column": {{ "type": "field", "record": "Point", {point} {items} "default": "{name}" }}"#
                ),
                "",
            ))
        };
        let number = r#""items": { "type": "number" },"#;
        assert!(no_field_ref_errors(&knob(number, "x")), "{:?}", knob(number, "x").errors);
        let mismatch = knob(number, "label");
        let found = field_ref_errors(&mismatch, ErrorCode::ConfigFieldRefTypeMismatch);
        assert_eq!(found.len(), 1, "{:?}", mismatch.errors);
        assert!(found[0].message.contains("a string field of Point; the knob takes number fields"), "{}", found[0].message);
        let unknown = knob(number, "z");
        assert_eq!(field_ref_errors(&unknown, ErrorCode::ConfigFieldRefUnknown).len(), 1);
        // control: without a value type the string field is fine; a Contact field is not a Point field.
        assert!(no_field_ref_errors(&knob("", "label")));
        assert_eq!(field_ref_errors(&knob("", "age"), ErrorCode::ConfigFieldRefUnknown).len(), 1);
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
            nullable: None,
            additional_properties: None,
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
            record: None,
            derived: None,
        };
        let sub_menu_array = FieldDefinition {
            nullable: None,
            additional_properties: None,
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
            record: None,
            derived: None,
        };
        let label = FieldDefinition {
            nullable: None,
            additional_properties: None,
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
            record: None,
            derived: None,
        };
        let mut props = orbital_core_base::collections::IndexMap::new();
        props.insert("label".to_string(), label);
        props.insert("subMenu".to_string(), sub_menu_array);
        let menu_item = FieldDefinition {
            nullable: None,
            additional_properties: None,
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
            record: None,
            derived: None,
        };
        ConfigField {
            nullable: None,
            additional_properties: None,
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
            wired: false,
            payload_for: None,
            projected_from: None,
            record: None,
        }
    }

    fn item(label: &str, sub_menu: Vec<SExpression>) -> SExpression {
        let mut obj = orbital_core_base::schema::SObject::new();
        obj.insert("label".into(), SExpression::String(label.to_string().into()));
        obj.insert("subMenu".into(), SExpression::Array(sub_menu.into()));
        SExpression::object(obj)
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
            &SExpression::Array(vec![depth3].into()),
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
            "labell".into(), // typo: required "label" is now absent
            SExpression::String(("C".to_string()).into()),
        );
        typo_leaf.insert("subMenu".into(), SExpression::Array(vec![].into()));
        let depth3 = item("A", vec![item("B", vec![SExpression::object(typo_leaf)])]);
        let result = check_config_default_shape(
            &field,
            &SExpression::Array(vec![depth3].into()),
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

/// A config default is a complete value: a struct/alias member's `!` is enforced on it.
#[cfg(test)]
mod required_member_tests {
    use super::*;

    /// `type Alias = { id : string!, kind : string! }` — a struct alias
    /// with two required members, mirroring `std-migration-job.lolo`'s
    /// `ImportSourceOption`.
    fn alias_with_required_members() -> FieldDefinition {
        let required_string = |name: &str| FieldDefinition {
            nullable: None,
            additional_properties: None,
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
            record: None,
            derived: None,
        };
        let mut props = orbital_core_base::collections::IndexMap::new();
        props.insert("id".to_string(), required_string("id"));
        props.insert("kind".to_string(), required_string("kind"));
        FieldDefinition {
            nullable: None,
            additional_properties: None,
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
            record: None,
            derived: None,
        }
    }

    /// `source : Alias = {}` — trait config keeps the strict literal: a
    /// config default has no later persist step to defer completeness to.
    #[test]
    fn strict_mode_rejects_incomplete_object_literal() {
        let schema = alias_with_required_members();
        let empty = SExpression::object(orbital_core_base::schema::SObject::new());
        let result = check_field_def_shape(
            &schema,
            &empty,
            "t.config.source",
            &EventScope::default(),
            &HashMap::new(),
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
            )
        };
        let text = check(SExpression::String(("ls".to_string()).into()));
        assert!(text.errors.is_empty(), "a string is the string variant: {:?}", text.errors);
        let mut env = orbital_core_base::schema::SObject::new();
        env.insert("env".into(), SExpression::String(("HOME".to_string()).into()));
        let record = check(SExpression::object(env));
        assert!(record.errors.is_empty(), "a record variant still fits: {:?}", record.errors);
        assert!(
            !check(SExpression::Number(3.0)).errors.is_empty(),
            "control: a number fits no variant"
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
              "events": [{{ "key": "DO_CREATE", "input": true }}],
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
            "events": [{{ "key": "INIT" }}, {{ "key": "ITEM_SEEN", "input": true }}],
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
            )
        };
        assert!(check(&nums(&[1.0, 2.0, 3.0])).errors.is_empty());
        assert!(!check(&nums(&[1.0, 2.0])).errors.is_empty(), "wrong arity");
        assert!(
            !check(&SExpression::Array(vec![
                SExpression::Number(1.0),
                SExpression::string("x"),
                SExpression::Number(3.0)
            ].into()))
            .errors
            .is_empty(),
            "wrong slot type"
        );
        assert!(
            !check(&SExpression::string("x")).errors.is_empty(),
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
                SExpression::string("a"),
                SExpression::Number(5.0),
                SExpression::Number(6.0)
            ].into()))
            .errors
            .is_empty(),
            "knob: wrong slot type"
        );
    }
}

#[cfg(test)]
mod named_alias_required_member_tests {
    use super::*;

    fn run(types: serde_json::Value, knob: serde_json::Value) -> ValidationResult {
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "t", "version": "1.0.0",
            "orbitals": [{
                "name": "O",
                "entity": { "name": "Row", "fields": [{ "name": "id", "type": "string" }] },
                "types": types,
                "traits": [{
                    "name": "Holder",
                    "linkedEntity": "Row",
                    "config": { "p": knob },
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": [] }]
                    }
                }]
            }]
        }))
        .expect("fixture parses");
        let mut ctx = ValidationContext::new();
        crate::phases::validation::collect_names(&schema, &mut ctx);
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    fn point_members() -> serde_json::Value {
        serde_json::json!({
            "x": { "name": "x", "type": "number", "required": true },
            "y": { "name": "y", "type": "number", "required": true }
        })
    }

    fn point_alias() -> serde_json::Value {
        serde_json::json!({ "Pt": [
            { "name": "x", "type": "number", "required": true },
            { "name": "y", "type": "number", "required": true }
        ] })
    }

    fn missing(result: &ValidationResult) -> Vec<String> {
        result
            .errors
            .iter()
            .filter(|e| e.message.contains("required field") && e.message.contains("missing"))
            .map(|e| e.message.clone())
            .collect()
    }

    #[test]
    fn a_named_record_default_missing_a_required_member_is_refused() {
        let knob = |default: serde_json::Value| {
            serde_json::json!({ "type": "Pt", "properties": point_members(), "default": default })
        };
        let result = run(point_alias(), knob(serde_json::json!({})));
        assert_eq!(missing(&result).len(), 2, "both x and y are named: {:?}", result.errors);
        assert!(missing(&result).iter().any(|m| m.contains("'x'")));
        let partial = run(point_alias(), knob(serde_json::json!({ "x": 1 })));
        assert_eq!(missing(&partial).len(), 1, "{:?}", partial.errors);
        assert!(missing(&partial)[0].contains("'y'"));
        let whole = run(point_alias(), knob(serde_json::json!({ "x": 1, "y": 2 })));
        assert!(missing(&whole).is_empty(), "control: a whole record passes: {:?}", whole.errors);
    }

    #[test]
    fn an_anonymous_record_default_is_checked_the_same_way() {
        let anonymous = |default: serde_json::Value| {
            serde_json::json!({ "type": "object", "properties": point_members(), "default": default })
        };
        assert_eq!(missing(&run(serde_json::json!({}), anonymous(serde_json::json!({})))).len(), 2);
        assert!(missing(&run(serde_json::json!({}), anonymous(serde_json::json!({ "x": 1, "y": 2 })))).is_empty());
    }

    #[test]
    fn a_named_tagged_union_default_is_not_checked_as_a_record() {
        let types = serde_json::json!({ "Shape": [{
            "name": "", "type": "union", "values": ["Circle", "Square"],
            "properties": [
                { "name": "Circle", "type": "object", "properties": [{ "name": "radius", "type": "number", "required": true }] },
                { "name": "Square", "type": "object", "properties": [{ "name": "side", "type": "number", "required": true }] }
            ]
        }] });
        let variants = serde_json::json!({
            "Circle": { "name": "Circle", "type": "object", "properties": { "radius": { "name": "radius", "type": "number", "required": true } } },
            "Square": { "name": "Square", "type": "object", "properties": { "side": { "name": "side", "type": "number", "required": true } } }
        });
        let knob = |default: serde_json::Value| {
            serde_json::json!({ "type": "Shape", "values": ["Circle", "Square"], "properties": variants, "default": default })
        };
        let valid = run(types.clone(), knob(serde_json::json!({ "radius": 2 })));
        assert!(missing(&valid).is_empty(), "control: a valid variant passes: {:?}", valid.errors);
        let other = run(types, knob(serde_json::json!({ "side": 2 })));
        assert!(missing(&other).is_empty(), "control: the other variant passes: {:?}", other.errors);
    }
}

#[cfg(test)]
mod payload_of_member_tests {
    use super::*;

    fn run(action: &str, payload: &str) -> ValidationResult {
        let json = format!(
            r#"{{
      "name": "t", "version": "1.0.0",
      "orbitals": [{{
        "name": "O",
        "entity": {{ "name": "Row", "fields": [{{ "name": "id", "type": "string" }}] }},
        "traits": [{{
          "name": "Items",
          "linkedEntity": "Row",
          "emits": [{{ "event": "SAVE", "payloadSchema": [
            {{ "name": "amount", "type": "number", "required": true }},
            {{ "name": "label", "type": "string" }}
          ] }}],
          "config": {{ "items": {{
            "type": "[Item]",
            "items": {{ "type": "object", "properties": {{
              "id": {{ "name": "id", "type": "string" }},
              "action": {{ "name": "action", "type": "event" }},
              "actionPayload": {{ "name": "actionPayload", "type": "object", "record": "@payload(.action)" }}
            }} }},
            "default": [{{ "id": "a", "action": "{action}", "actionPayload": {payload} }}]
          }} }},
          "stateMachine": {{
            "states": [{{ "name": "idle", "isInitial": true }}],
            "transitions": [{{ "from": "idle", "to": "idle", "event": "SAVE", "effects": [] }}]
          }}
        }}]
      }}]
    }}"#
        );
        let schema: OrbitalSchema = serde_json::from_str(&json).expect("fixture parses");
        let mut ctx = ValidationContext::new();
        crate::phases::validation::collect_names(&schema, &mut ctx);
        validate_all_config_types(&schema, &mut ctx);
        ctx.result
    }

    fn mismatches(result: &ValidationResult) -> Vec<String> {
        result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::TraitConfigShapeMismatch)
            .map(|e| format!("{}: {}", e.path, e.message))
            .collect()
    }

    #[test]
    fn a_payload_that_fits_the_named_event_is_clean() {
        let result = run("SAVE", r#"{ "amount": 1, "label": "x" }"#);
        assert_eq!(mismatches(&result), Vec::<String>::new());
    }

    #[test]
    fn a_field_the_named_event_does_not_declare_is_reported() {
        let result = run("SAVE", r#"{ "amount": 1, "bogus": 2 }"#);
        let found = mismatches(&result);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("bogus") && found[0].contains("SAVE"), "{found:?}");
    }

    #[test]
    fn a_value_of_the_wrong_type_is_reported() {
        let result = run("SAVE", r#"{ "amount": "many" }"#);
        assert_eq!(mismatches(&result).len(), 1, "{:?}", result.errors);
    }

    #[test]
    fn a_missing_required_field_is_reported() {
        let result = run("SAVE", r#"{ "label": "x" }"#);
        let found = mismatches(&result);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("amount"), "{found:?}");
    }

    #[test]
    fn control_an_event_nothing_declares_defers() {
        let result = run("OTHER", r#"{ "bogus": 2 }"#);
        assert!(mismatches(&result).iter().all(|m| !m.contains("bogus")), "{:?}", result.errors);
    }

    #[test]
    fn control_a_sibling_that_is_not_a_literal_defers() {
        let result = run("@config.action", r#"{ "bogus": 2 }"#);
        assert!(mismatches(&result).iter().all(|m| !m.contains("bogus")), "{:?}", result.errors);
    }
}

/// The arms of a union knob or field: `values` names them and `properties` carries each one's shape (the union
/// carrier); a closed-string enum has `values` alone and is not a union.
fn union_arms<'a>(
    values: Option<&'a Vec<String>>,
    properties: Option<&'a orbital_core_base::collections::IndexMap<String, FieldDefinition>>,
) -> Option<Vec<(&'a String, &'a FieldDefinition)>> {
    let (values, properties) = (values?, properties?);
    let arms: Vec<_> = values.iter().filter_map(|name| properties.get(name).map(|def| (name, def))).collect();
    (!arms.is_empty()).then_some(arms)
}
