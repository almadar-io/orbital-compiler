//! Trait reference inlining: call-site overrides, collision detection,
//! sibling auto-pull, and source-scoped listener pruning.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::identity_normalize::rewrite_trait_embed_tokens;
use super::rewrite::{
    apply_inline_config_defaults, mark_literal_list_in_place, record_config_provenance,
    rewrite_config_bindings, rewrite_entity_field_paths, rewrite_identifiers,
    rewrite_payload_entity_refs, rewrite_trait_event_fields, stale_entity_rebind_subs,
};
use super::InlineError;
use orbital_core::schema::{
    ConfigField, ConfigProvenanceRecord, EntityDefinition, EntityRef, FieldType, OrbitalSchema,
    SExpression, SourceBehaviorMetadata, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_overrides_to_trait(
    trait_def: &mut TraitDefinition,
    linked_entity: Option<&String>,
    // Consumer-scope entity name → dual-carry id map. When a `linked_entity`
    // rebind rewrites the trait's `linkedEntity` NAME, the id sibling
    // (`linked_entity_id`) MUST be rewritten in the same operation to the id of
    // the rebind target in the consumer arena — otherwise the cloned trait keeps
    // the SOURCE file's entity id, which resolves to no node in the composed
    // schema (`ORB_ID_UNKNOWN_REF`). Empty map = no id remap (test / legacy
    // id-free callers); an unresolved target clears the id (justified: the
    // rebind target is not a node in the composed graph — a dangling foreign id
    // is never the right carry).
    entity_ids: &HashMap<String, String>,
    config: Option<&HashMap<String, ConfigField>>,
    name: Option<&str>,
    events: Option<&HashMap<String, String>>,
    // Per-key `@entity.<old>` → `@entity.<new>` field rename map. Mirrors
    // `events`: rewrites every entity-field read inside the resolved trait so a
    // DATA field the trait reads is satisfied by a differently-named field on
    // the rebind target. Also rewrites the trait's `entity_contract.requires`.
    fields: Option<&HashMap<String, String>>,
    // When true, fold call-site config overrides into the trait's declared
    // `config[k].default` (so they carry forward) but DO NOT substitute the
    // `@config.X` strings in the state machine. Used when resolving an import,
    // so a downstream consumer can still override those bindings. When false
    // (top-level target), the `@config.X` strings are baked to their effective
    // values as before.
    defer_config_subst: bool,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) {
    // 1. Direct name rename — applied first so all subsequent operations
    //    refer to the final name in any error messages.
    if let Some(new_name) = name {
        if new_name != trait_def.name {
            trait_def.name = new_name.to_string();
        }
    }

    // 2. Build the entity substitution map. The `linked_entity` arg means
    //    "the atom thinks its canonical entity is X; rebind it to Y". The
    //    pre-existing `trait_def.linked_entity` IS the from-key.
    let mut entity_subs: HashMap<String, String> = HashMap::new();
    if let Some(new_entity) = linked_entity {
        // V4 leverage-ids (id-primary, name-fallback): recover any stale
        // token name the `entity_ref_ids` side-map ties to this trait's bound
        // entity id, so a token that kept its OLD name after the bound entity
        // declaration was renamed is still rebound to `new_entity`. Inserted
        // FIRST so the name-derived `orig` mapping below can override on the
        // exact-name key (same value anyway); absent side-map = empty = the
        // name path is unchanged.
        for (from, to) in stale_entity_rebind_subs(trait_def, new_entity) {
            entity_subs.insert(from, to);
        }
        if let Some(orig) = trait_def.linked_entity.clone() {
            if &orig != new_entity {
                entity_subs.insert(orig, new_entity.clone());
            }
        }
        trait_def.linked_entity = Some(new_entity.clone());
        // Keep the dual-carry id sibling in lockstep with the rebound name.
        trait_def.linked_entity_id = entity_ids.get(new_entity).cloned();
    }

    // 3. Build the event substitution map directly from the caller args.
    //    Empty entries (or self-mapped no-ops) are pruned to keep the walker
    //    from doing redundant work.
    let mut event_subs: HashMap<String, String> = HashMap::new();
    if let Some(map) = events {
        for (from, to) in map.iter() {
            if from != to && !from.is_empty() && !to.is_empty() {
                event_subs.insert(from.clone(), to.clone());
            }
        }
    }

    // 3b. Build the entity-field rename map. Each `from → to` rewrites the
    //     trait's `@entity.<from>` reads (and `(set @entity.<from> ...)`) to
    //     `@entity.<to>` so a renamed DATA field on the rebind target satisfies
    //     the read. Self-mapped no-ops and empty entries are pruned.
    let mut field_subs: HashMap<String, String> = HashMap::new();
    if let Some(map) = fields {
        for (from, to) in map.iter() {
            if from != to && !from.is_empty() && !to.is_empty() {
                field_subs.insert(from.clone(), to.clone());
            }
        }
    }

    // 3b'. Call-site payload capture — SCOPE-AWARE. A `@payload.<field>` in this
    //      reference's call-site config is a capture from the COMPOSING effect's
    //      event ONLY when the child's own transition that renders the bound knob
    //      does not itself declare `<field>`. A render-ui body knob the child
    //      renders in its own data-load transition (e.g. `galleryBodyContent`
    //      under `BrowseItemLoaded { data }`) resolves `@payload.data` locally —
    //      blanket-tagging it `@callsitePayload` mis-validated it against the
    //      wrapper's events (G-RUI-CALLSITE-PAYLOAD-OVERRIDE). `retag_callsite_payload`
    //      re-derives the tag per field from `local_payload_fields_for_knob`,
    //      idempotent and source-agnostic (a lower-time eager tag, a baked/
    //      untagged `.orb`, both converge). The capture then flows into
    //      `effective_config` + the folded declared default below, and
    //      `contains_callsite_payload` (step 6) keeps a still-captured knob out
    //      of the child body.
    let tagged_config: Option<HashMap<String, ConfigField>> = config.map(|c| {
        c.iter()
            .map(|(k, cf)| {
                let mut cf = cf.clone();
                if let Some(default) = cf.default.as_mut() {
                    let local = local_payload_fields_for_knob(trait_def, k);
                    orbital_core::schema::retag_callsite_payload(default, &local);
                }
                (k.clone(), cf)
            })
            .collect()
    });
    let config = tagged_config.as_ref();

    // 3c. Build the effective config map (key → concrete SExpression value)
    //     EARLY — same precedence `effective_config` always uses (declared
    //     defaults, then call-site overrides) — and resolve any
    //     `@config.<knob>` emit-name reference to its concrete literal
    //     BEFORE the event-key rename pass below (step 5). Pinned contract:
    //     `events: {...}` renames target the RESOLVED emit name (e.g.
    //     ButtonRender's `emits { @config.action -> ... }` resolves to its
    //     declared default "ACTION", and a call-site `events: {ACTION:
    //     UPGRADE_WIDGET}` renames THAT). Resolving first means the later
    //     `rewrite_trait_event_fields` walk (which matches `event_subs`
    //     keys against the trait's declared event names) sees "ACTION", not
    //     the still-raw "@config.action" token. Gated the same as the body
    //     substitution below: a deferred import load leaves the reference
    //     live for the downstream consumer's own resolution pass.
    let mut effective_config: HashMap<String, SExpression> = HashMap::new();
    if let Some(declared) = trait_def.config.as_ref() {
        for (k, field) in declared.iter() {
            if let Some(default) = field.default.as_ref() {
                effective_config.insert(k.clone(), default.clone());
            }
        }
    }
    if let Some(cfg) = config {
        for (k, cf) in cfg.iter() {
            if let Some(v) = cf.default.as_ref() {
                effective_config.insert(k.clone(), v.clone());
            }
        }
    }
    // Fold an `events:` rename of a `@config.<knob>` emit name into BOTH the
    // knob's `effective_config` entry (so the step-6 body substitution emits
    // the final name everywhere `@config.<knob>` is read) AND the knob's
    // DECLARED default. The declared default is the only carrier that
    // survives a deferred import load: there the structural resolution below
    // is gated off (emits stay raw `@config.<knob>` for the consumer's own
    // pass) and the step-5 rename walk matches nothing against the raw
    // token, so without this fold the rename is silently dropped and every
    // consumer's button emits the raw pattern slot
    // (S-EVENTS-RENAME-LOST-WHEN-AN-ATOM-IS-IMPORTED).
    if !event_subs.is_empty() {
        let renames: Vec<(String, String)> = trait_def
            .emits
            .as_ref()
            .map(|emits| {
                emits
                    .iter()
                    .filter_map(|emit| {
                        let knob = orbital_core::schema::config_ref_event_knob(&emit.event)?;
                        let resolved = orbital_core::schema::resolve_config_ref_event_name(
                            &emit.event,
                            &effective_config,
                            trait_def.config.as_ref(),
                        )
                        .ok()?;
                        let renamed = event_subs.get(&resolved)?;
                        Some((knob.to_string(), renamed.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (knob, renamed) in renames {
            effective_config.insert(knob.clone(), SExpression::String(renamed.clone()));
            if let Some(cfg) = trait_def.config.as_mut() {
                if let Some(field) = cfg.get_mut(&knob) {
                    field.default = Some(SExpression::String(renamed));
                }
            }
        }
    }
    if !defer_config_subst {
        resolve_config_ref_emit_names(trait_def, &effective_config);
    }

    // 4. Walk SExpressions in transitions and ticks ONCE with both maps.
    if !entity_subs.is_empty() || !event_subs.is_empty() || !field_subs.is_empty() {
        for transition in trait_def.state_machine.transitions.iter_mut() {
            if let Some(guard) = transition.guard.as_mut() {
                rewrite_identifiers(guard, &entity_subs, &event_subs);
                rewrite_entity_field_paths(guard, &field_subs);
            }
            if let Some(effects) = transition.effects.as_mut() {
                for effect in effects.iter_mut() {
                    rewrite_identifiers(effect, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(effect, &field_subs);
                }
            }
        }
        if let Some(ticks) = trait_def.ticks.as_mut() {
            for tick in ticks.iter_mut() {
                if let Some(guard) = tick.guard.as_mut() {
                    rewrite_identifiers(guard, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(guard, &field_subs);
                }
                for effect in tick.effects.iter_mut() {
                    rewrite_identifiers(effect, &entity_subs, &event_subs);
                    rewrite_entity_field_paths(effect, &field_subs);
                }
            }
        }
    }

    // 4b. Rewrite the trait's inferred `entity_contract.requires`/`provides` so
    //     the post-inline rebind validator checks the RENAMED field names. A
    //     `fields` remap turns a `requires: ["title"]` into `requires:
    //     ["dishName"]`, matching the target entity.
    if !field_subs.is_empty() {
        if let Some(contract) = trait_def.entity_contract.as_mut() {
            for f in contract.requires.iter_mut() {
                if let Some(to) = field_subs.get(f) {
                    *f = to.clone();
                }
            }
            for f in contract.provides.iter_mut() {
                if let Some(to) = field_subs.get(f) {
                    *f = to.clone();
                }
            }
        }
    }

    // 5. Walk the structured event-key fields. These live outside SExpression
    //    payloads, so the SExpression walk above does not reach them.
    rewrite_trait_event_fields(trait_def, &event_subs);

    // 5b. Walk the typed payload schemas (emits[*].payloadSchema,
    //     state_machine.events[*].payloadSchema). Their `type` strings can
    //     reference the atom's original entity name (`[Matter]`); after a
    //     linkedEntity rebind those references would otherwise survive as
    //     dangling types in downstream handler.ts codegen.
    rewrite_payload_entity_refs(trait_def, &entity_subs);

    // 6. Phase 3.A: Substitute `@config.<param>` bindings with concrete
    //    SExpression values supplied by the caller. Unlike entity/event
    //    substitution (which only rewrites strings), config substitution can
    //    replace a whole node with an arbitrary SExpression (array, object,
    //    number, bool, null). The walk runs AFTER the identifier pass so
    //    any `@config.X` that references a renamed entity is still
    //    referenced by the entity rewriter's output (though in practice
    //    `@config.X` is a flat key so entity renames don't interact).
    //
    //    Missing params are left as-is — the downstream binding validator
    //    will flag unresolved `@config.X` so composers see a clean error.
    // Merge call-site config overrides with the atom's declared config-field
    // defaults so that `@config.X` references for params the molecule didn't
    // override still resolve to a concrete literal at compile time. Without
    // this, server-side codegen falls back to `user?.X` (no UserContext.X
    // exists) and trips TS2339; client-side falls back to `mergedConfig?.X`
    // which works at runtime but loses constant-fold opportunities. Merging
    // keeps both paths consistent and prunes dead branches via the eq-fold.
    // `effective_config` (key → concrete SExpression value, declared
    // defaults overridden by call-site values) was already built in step 3c
    // above — reused here for the body substitution below.
    //
    // Wrap string-headed array values bound for literal-array slots in the
    // `list` constructor so they are never mis-read as S-expression calls
    // (see `mark_literal_list_in_place`). Type-directed off each slot's
    // declared `field_type`.
    if let Some(declared) = trait_def.config.as_ref() {
        for (k, value) in effective_config.iter_mut() {
            if let Some(field) = declared.get(k) {
                mark_literal_list_in_place(value, &field.field_type);
            }
        }
    }
    // GAP-AG-VALUE-DRIFT: fold call-site overrides AND annotations into the
    // atom's declared config schema so the inlined trait carries the
    // organism's intent end-to-end. For each call-site `ConfigField`:
    //   - `default` → replaces the atom's declared default (existing behaviour).
    //   - `label`/`description`/`synonyms`/`tier` when `Some` → overwrite
    //     the atom's declared value (organism wins). This is the NEW behaviour
    //     for annotated call-site entries; plain wiring entries (no annotations)
    //     are unchanged.
    if let Some(cfg) = config {
        if let Some(declared) = trait_def.config.as_mut() {
            for (k, cf) in cfg.iter() {
                if let Some(field) = declared.get_mut(k) {
                    if let Some(v) = cf.default.as_ref() {
                        field.default = Some(v.clone());
                    }
                    if cf.label.is_some() {
                        field.label = cf.label.clone();
                    }
                    if cf.description.is_some() {
                        field.description = cf.description.clone();
                    }
                    if cf.synonyms.is_some() {
                        field.synonyms = cf.synonyms.clone();
                    }
                    if cf.tier.is_some() {
                        field.tier = cf.tier.clone();
                    }
                }
            }
        }
        // A reference's config block can DECLARE a brand-new knob rather
        // than override one of the wrapped trait's (e.g. an alias trait
        // adding its own `steps : [StepSpec] = [...]`). Those entries have
        // no match in `declared` above (the wrapped trait never declared
        // them) and are structurally a declaration, not a plain wiring
        // value — insert them so the resolved trait carries them as its
        // OWN declared config. This is what makes the alias's knob surface
        // visible to any downstream call site that references the ALIAS
        // (not the wrapped trait): the E1 unknown-call-site-key check reads
        // `trait_def.config` off whatever `ctx.get_trait` resolves.
        for (k, cf) in cfg.iter() {
            let already_declared = trait_def.config.as_ref().is_some_and(|m| m.contains_key(k));
            if !already_declared && cf.is_declaration_form() {
                trait_def
                    .config
                    .get_or_insert_with(HashMap::new)
                    .insert(k.clone(), cf.clone());
            }
        }
    }

    // Declared `@typeWhen` payload-field conditionals (§61 OPEN 1's
    // replacement): the GAP-AG-VALUE-DRIFT fold just above already baked the
    // call site's resolved override into `trait_def.config[<knob>].default`,
    // so this is the first point where a condition's knob value is knowable.
    apply_payload_type_conditionals(trait_def);

    // The `event`-TYPED twin of the step-3c emit-driven fold (gap #30/C1
    // root fix): a knob declared `: event` holds an event-name REFERENCE,
    // so an `events:` rename must rewrite its settled value the same way it
    // rewrites the trait's own event keys — the emit-driven fold can't see
    // these (nothing in `emits` points at the knob; the value is read from
    // a render prop). Runs AFTER the GAP-AG-VALUE-DRIFT merge above so a
    // call-site override spelling the canonical (pre-rename) name is folded
    // too, not clobbered back; before the fold, only the NEW-name spelling
    // validated (probe 2026-08-30 A/B/C). Dual-write like step 3c:
    // `effective_config` for the body substitution below, the DECLARED
    // default as the carrier that survives serialization.
    if !event_subs.is_empty() {
        // A-UNIFY: a DEFINER knob's value CREATES the event name — it is
        // never a reference into the event set, so a call-site `events:`
        // rename must not fold onto it (that would rewrite the DEFINITION,
        // not a reference to it).
        let definer_knobs = orbital_core::schema::definer_knobs_for_trait(trait_def);
        let event_knob_renames: Vec<(String, String)> = trait_def
            .config
            .as_ref()
            .map(|cfg| {
                cfg.iter()
                    .filter(|(knob, field)| {
                        field.field_type == "event" && !definer_knobs.contains(knob.as_str())
                    })
                    .filter_map(|(knob, field)| {
                        let current = match field.default.as_ref() {
                            Some(SExpression::String(s)) if !s.is_empty() => s,
                            _ => match effective_config.get(knob) {
                                Some(SExpression::String(s)) if !s.is_empty() => s,
                                _ => return None,
                            },
                        };
                        let renamed = event_subs.get(current)?;
                        Some((knob.clone(), renamed.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (knob, renamed) in event_knob_renames {
            effective_config.insert(knob.clone(), SExpression::String(renamed.clone()));
            if let Some(cfg) = trait_def.config.as_mut() {
                if let Some(field) = cfg.get_mut(&knob) {
                    field.default = Some(SExpression::String(renamed));
                }
            }
        }

        // The STRUCT-MEMBER twin of the scalar fold above: a knob declared
        // `[RecordAction]` whose element schema types a member `event` holds
        // event-name REFERENCES in exactly the same sense. The effect walk in
        // step 4 cannot reach them — a render-ui node forwards the prop as the
        // STRING `"@config.actions"` and the literal array lives here — so
        // without this an authored `events { EDIT: … }` renamed the trait's
        // transitions and emits while its own action buttons kept firing the
        // pre-rename keys, and nothing handled them
        // (SCAN-DETAIL-PANEL-ACTIONS-UNTAGGED-1).
        //
        // Same dual-write as above: `effective_config` drives the step-6 body
        // substitution, the DECLARED default is the carrier that survives a
        // deferred import load. Definer knobs are irrelevant here — a struct
        // member can never sit in a definer position, so it is always a
        // reference.
        let member_folds: Vec<(String, SExpression)> = trait_def
            .config
            .as_ref()
            .map(|cfg| {
                cfg.iter()
                    .filter_map(|(knob, field)| {
                        let items = field.items.as_deref()?;
                        let mut value = effective_config
                            .get(knob)
                            .cloned()
                            .or_else(|| field.default.clone())?;
                        crate::phases::inline::rewrite::fold_event_typed_members(
                            &mut value,
                            Some(items),
                            &event_subs,
                        )
                        .then_some((knob.clone(), value))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (knob, folded) in member_folds {
            effective_config.insert(knob.clone(), folded.clone());
            if let Some(cfg) = trait_def.config.as_mut() {
                if let Some(field) = cfg.get_mut(&knob) {
                    field.default = Some(folded);
                }
            }
        }
    }
    if !defer_config_subst && !effective_config.is_empty() {
        record_config_provenance(trait_def, &effective_config, recorder);
        // Call-site payload captures stay UNSUBSTITUTED in the child body:
        // a knob whose effective value is a `@callsitePayload.<field>` capture
        // must remain `@config.<knob>` in the child's render so each engine
        // resolves the capture against the COMPOSING effect's event payload at
        // the call site and passes the literal into the child's config (the
        // capture value is retained on the child's declared config-field default
        // above, where the call site reads it). Baking it into the child body
        // would (mis)evaluate it in the child's own INIT scope.
        let subst_config: HashMap<String, SExpression> = effective_config
            .iter()
            .filter(|(_, v)| !orbital_core::schema::contains_callsite_payload(v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let cfg = &subst_config;
        {
            for transition in trait_def.state_machine.transitions.iter_mut() {
                if let Some(guard) = transition.guard.as_mut() {
                    rewrite_config_bindings(guard, cfg);
                }
                if let Some(effects) = transition.effects.as_mut() {
                    for effect in effects.iter_mut() {
                        rewrite_config_bindings(effect, cfg);
                    }
                }
            }
            if let Some(ticks) = trait_def.ticks.as_mut() {
                for tick in ticks.iter_mut() {
                    if let Some(guard) = tick.guard.as_mut() {
                        rewrite_config_bindings(guard, cfg);
                    }
                    for effect in tick.effects.iter_mut() {
                        rewrite_config_bindings(effect, cfg);
                    }
                }
            }
        }
    }
}

/// §61 OPEN 1 replacement: resolve every emit payload field AND matching
/// `stateMachine.events[]` payload field's declared `@typeWhen` conditionals
/// (`PayloadField.type_when`) against the trait's DECLARED config (already
/// carrying the call site's resolved override via the GAP-AG-VALUE-DRIFT
/// fold just above). On a literal match, `field_type` is rewritten to the
/// condition's `ty`; a forwarded/unresolved knob matches nothing and leaves
/// the field's declared type as-is. `type_when` itself is left in place here
/// (not cleared) — `rewrite_narrowed_number_display_writes` below is the one
/// whole-schema pass that both clears it for good (the emitted `.orb` must
/// carry only the resolved truth) and needs to know a field WAS
/// knob-conditional to gate the display-buffer wrap; clearing here would
/// erase that signal before it's read. ZERO knob-name or pattern-name
/// knowledge: the conditions are authored data, this only evaluates them —
/// any future `@typeWhen` declaration on any field gets the same treatment.
fn apply_payload_type_conditionals(trait_def: &mut TraitDefinition) {
    let config = trait_def.config.clone();
    let resolve_fields = |fields: &mut [orbital_core::schema::PayloadField]| {
        for f in fields.iter_mut() {
            let Some(conditions) = f.type_when.as_ref() else {
                continue;
            };
            if let Some(ty) =
                orbital_core::schema::resolve_payload_type_when(conditions, config.as_ref())
            {
                f.field_type = ty;
            }
        }
    };
    for emit in trait_def.emits.iter_mut().flatten() {
        if let Some(fields) = emit.payload_schema.as_mut() {
            resolve_fields(fields);
        }
    }
    for ev in trait_def.state_machine.events.iter_mut().flatten() {
        if let Some(fields) = ev.payload_schema.as_mut() {
            resolve_fields(fields);
        }
    }
}

/// True for an array-shaped payload type (`[T]` or the literal `array`). The
/// display-buffer wrap below must never fire for one of these — stringifying
/// an array is display corruption, not a display fix; such a field just
/// type-checks normally downstream instead.
fn is_array_shaped_payload_type(ty: &str) -> bool {
    ty.starts_with('[') || ty == "array"
}

/// Clears every payload field's now-resolved `type_when` on `trait_def` (the
/// emitted `.orb` must carry only the resolved truth, never the conditional
/// — a still-forwarded/unresolved condition has no further chance to resolve
/// once this, the last pre-codegen schema pass, has run) and reports whether
/// the trait's own `"value"` transport field — the read `wrap_narrowed_
/// display_set` below targets — was narrowed AWAY FROM `string` to a
/// non-array scalar by `apply_payload_type_conditionals` above. Presence of
/// `type_when` (regardless of whether it matched) is exactly that signal: a
/// field never declared `@typeWhen` never carries it, so an organically
/// `number`-typed `value` field (no conditional involved) correctly reports
/// `false` here.
fn finalize_payload_type_conditionals(trait_def: &mut TraitDefinition) -> bool {
    let mut value_narrowed_to_scalar = false;
    let mut sweep = |fields: &mut [orbital_core::schema::PayloadField]| {
        for f in fields.iter_mut() {
            let was_conditional = f.type_when.take().is_some();
            if was_conditional
                && f.name == "value"
                && f.field_type != "string"
                && !is_array_shaped_payload_type(&f.field_type)
            {
                value_narrowed_to_scalar = true;
            }
        }
    };
    for emit in trait_def.emits.iter_mut().flatten() {
        if let Some(fields) = emit.payload_schema.as_mut() {
            sweep(fields);
        }
    }
    for ev in trait_def.state_machine.events.iter_mut().flatten() {
        if let Some(fields) = ev.payload_schema.as_mut() {
            sweep(fields);
        }
    }
    value_narrowed_to_scalar
}

/// §61 OPEN 1, FIX 2: the atom's OWN internal `(set @entity.<f> ?value)` —
/// its DOM display buffer, historically and honestly typed `string` on the
/// atom's own entity — must not be handed a now-non-string wire value
/// directly once its event narrowed (the pass above). Wraps the read in
/// `(str/concat ?value)` — NOT bare `concat` (the JS evaluator maps bare
/// `concat` to ARRAY concat; `str/concat` is string-concat on both the JS
/// and Rust paths) — so codegen stringifies the value back into the
/// string-typed buffer field. Only fires when `<f>`'s declared type on the
/// trait's RESOLVED linked entity is actually `string` — a non-string
/// buffer field needs no conversion.
///
/// NOT gated by the TRIGGERING event matching the narrowed (renamed)
/// definer-knob event: ui-input's own internal display-sync transition
/// listens on its fixed internal key (`ACTION`), independent of whatever
/// external name the `action` knob resolves the OUTBOUND emit to (e.g.
/// `SET_TTL`) — confirmed against the real `std-service-redis` UiInput3
/// composition, where `transitions[1].event` stays `"ACTION"` while
/// `emits[0].event` is `"SET_TTL"`. The type-validator itself (`phases/
/// validation/effect/mod.rs`'s `collect_payload_field_types`) already
/// resolves `?value`/`@payload.value` the SAME event-untethered way — by
/// FIELD NAME across the trait's declared emits, not per triggering event —
/// so this mirrors that, not a new assumption: within a trait that narrowed
/// at least one definer-knob emit's `value` field away from `string`, a
/// `set` reading the literal `value` field is the narrowed one, wherever in
/// the trait it lives.
///
/// Whole-schema, run once after `resolve_sigil_types` (mirrors its own
/// per-orbital entity-name→fields walk) so a rebind target entity declared
/// in a sibling orbital is visible the same way sigil resolution needs it to
/// be, and so it runs after every trait's own inline (hence its own
/// `apply_payload_type_conditionals` pass) has landed — this IS the point
/// that finalizes and clears every trait's `type_when`, gated trait or not.
pub(crate) fn rewrite_narrowed_number_display_writes(schema: &mut OrbitalSchema) {
    let mut entities: HashMap<String, Vec<orbital_core::schema::FieldDefinition>> = HashMap::new();
    for orbital in &schema.orbitals {
        if let EntityRef::Inline(def) = &orbital.entity {
            entities.insert(def.name.clone(), def.fields.clone());
        }
        for aux in orbital.auxiliary_entities.iter().flatten() {
            if let EntityRef::Inline(def) = aux {
                entities
                    .entry(def.name.clone())
                    .or_insert_with(|| def.fields.clone());
            }
        }
    }

    for orbital in &mut schema.orbitals {
        for trait_ref in &mut orbital.traits {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            // Always finalize (clear) `type_when` — the wrap below is an
            // ADDITIONAL step gated on the return value, not a precondition
            // for clearing.
            if !finalize_payload_type_conditionals(trait_def) {
                continue;
            }
            let Some(linked) = trait_def.linked_entity.clone() else {
                continue;
            };
            let Some(fields) = entities.get(&linked) else {
                continue;
            };
            let string_fields: HashSet<String> = fields
                .iter()
                .filter(|f| f.field_type == FieldType::String)
                .filter_map(|f| f.name.clone())
                .collect();
            if string_fields.is_empty() {
                continue;
            }
            for transition in trait_def.state_machine.transitions.iter_mut() {
                if let Some(effects) = transition.effects.as_mut() {
                    for effect in effects.iter_mut() {
                        wrap_narrowed_display_set(effect, &string_fields);
                    }
                }
            }
        }
    }
}

/// `(set @entity.<f> ?value)` (or `@payload.value`) → `(set @entity.<f>
/// (str/concat ?value))` for any `<f>` in `string_fields` — matched by the
/// FIELD the value is read from (the narrowed contract's own `value` name,
/// never a name-blind match on every `set`), not by trait/atom name.
/// Recurses into every array/object position so a `set` inside a guard-gated
/// branch is caught too.
fn wrap_narrowed_display_set(expr: &mut SExpression, string_fields: &HashSet<String>) {
    if let SExpression::Array(items) = expr {
        let is_narrowed_set = items.len() >= 3
            && matches!(&items[0], SExpression::String(op) if op == "set")
            && matches!(
                &items[2],
                SExpression::String(s) if s == "?value" || s == "@payload.value"
            )
            && set_entity_field(&items[1]).is_some_and(|f| string_fields.contains(f));
        if is_narrowed_set {
            let value = items[2].clone();
            items[2] =
                SExpression::Array(vec![SExpression::String("str/concat".to_string()), value]);
        }
        for item in items.iter_mut() {
            wrap_narrowed_display_set(item, string_fields);
        }
    } else if let SExpression::Object(map) = expr {
        for v in map.values_mut() {
            wrap_narrowed_display_set(v, string_fields);
        }
    }
}

/// Extract `<field>` from a `@entity.<field>` string target. Local twin of
/// `phases::validation::effect::set::set_entity_field` (private to that
/// module, and that file has unrelated in-flight edits) — same one-line
/// prefix-strip contract, not a divergent rule.
fn set_entity_field(target: &SExpression) -> Option<&str> {
    let SExpression::String(s) = target else {
        return None;
    };
    s.strip_prefix("@entity.").filter(|f| !f.is_empty())
}

/// Rewrite every `emits[*].event` that is a `@config.<knob>` reference to its
/// resolved literal, and mirror the rewrite onto the matching
/// `stateMachine.events[*].key` entries (L1 lowering unions emit keys into the
/// events registry, so the raw token appears there too). `listens` does not
/// share this surface yet — the parser only accepts the reference sigil where
/// an emit event name is expected in `emits {}`; `listens {}` handler dispatch
/// still requires a leading `Identifier` token (source or bare event), so a
/// config-ref there would need parser work of its own (not part of this change).
///
/// Returns the knob names that were resolved, so the caller can propagate a
/// later `events:` rename onto the SAME `effective_config` entry — a
/// render-ui body literal bound to `@config.<knob>` (e.g. ButtonRender's
/// `action: @config.action`) is substituted from `effective_config` too, and
/// must end up holding the FINAL (resolved-then-renamed) event name, not
/// just the resolved default, or `ORB_X_RENDER_UI_EVENT_LITERAL_STALE` fires.
pub(crate) fn resolve_config_ref_emit_names(
    trait_def: &mut TraitDefinition,
    effective_config: &HashMap<String, SExpression>,
) -> Vec<String> {
    let mut resolved_triples: Vec<(String, String, String)> = Vec::new();
    {
        let Some(emits) = trait_def.emits.as_ref() else {
            return Vec::new();
        };
        for emit in emits.iter() {
            let Some(knob) = orbital_core::schema::config_ref_event_knob(&emit.event) else {
                continue;
            };
            match orbital_core::schema::resolve_config_ref_event_name(
                &emit.event,
                effective_config,
                trait_def.config.as_ref(),
            ) {
                Ok(resolved) => {
                    resolved_triples.push((knob.to_string(), emit.event.clone(), resolved))
                }
                // Left unresolved on error; the validation phase's
                // EmitConfigRefInvalid check reports the specific contract
                // clause that failed (unknown knob / non-string / no default).
                Err(_) => {}
            }
        }
    }
    if resolved_triples.is_empty() {
        return Vec::new();
    }
    if let Some(emits) = trait_def.emits.as_mut() {
        for emit in emits.iter_mut() {
            if let Some((_, _, to)) = resolved_triples
                .iter()
                .find(|(_, from, _)| from == &emit.event)
            {
                emit.event = to.clone();
            }
        }
    }
    if let Some(events) = trait_def.state_machine.events.as_mut() {
        for ev in events.iter_mut() {
            if let Some((_, _, to)) = resolved_triples.iter().find(|(_, from, _)| from == &ev.key) {
                ev.key = to.clone();
            }
        }
    }
    resolved_triples
        .into_iter()
        .map(|(knob, ..)| knob)
        .collect()
}

/// Phase F.7: Apply post-substitution structural overrides to a resolved
/// `TraitDefinition`. These differ from `apply_overrides_to_trait` in that
/// they do NOT rewrite identifiers — they replace whole structural fields
/// with caller-supplied values:
///
/// - **`listens`** — replace the trait's `listens` array entirely with the
///   caller-supplied list. The common case in agent molecules is `[]`,
///   which clears the upstream's external event listeners. The inliner
///   accepts any caller list, including non-empty ones, but does not merge
///   with the upstream — it's a full replacement.
///
/// - **`emits_scope`** — set the `scope` field on every existing emit to
///   the caller-supplied value (`Internal` or `External`). The common case
///   in agent molecules is `Internal`, which confines the trait's emits to
///   the orbital's internal bus instead of broadcasting them. The inliner
///   does NOT add new emit declarations; it only modifies the scope of
///   what's already there.
///
/// Both overrides are applied AFTER `apply_overrides_to_trait` has run, so
/// any rename / linkedEntity / events substitutions are already in effect
/// by the time these structural changes apply.
fn apply_post_subst_overrides_to_trait(
    trait_def: &mut TraitDefinition,
    listens: Option<&Vec<orbital_core::schema::ListenDefinition>>,
    emits_scope: Option<&orbital_core::schema::EventScope>,
) {
    if let Some(new_listens) = listens {
        trait_def.listens = Some(new_listens.clone());
    }
    if let Some(scope) = emits_scope {
        if let Some(emits) = trait_def.emits.as_mut() {
            for emit in emits.iter_mut() {
                emit.scope = Some(scope.clone());
                // Call-site provenance: this scope is the COMPOSITION's own
                // statement, not the atom's default — the dead-control check
                // treats only this form as a consumer's deferral.
                emit.scope_overridden = Some(true);
            }
        }
    }
}

/// Walk an SExpression and collect every string literal of the exact form
/// Values for a pulled sibling's `@config.<knob>` forward defaults, taken from
/// the trait that embeds it.
///
/// `MasterListView = DataList.traits.DataListRender { config { fields:
/// @config.fields } }` inside `std-browse` means "take `fields` from whoever
/// embeds me" — the wrapper-forward contract. The embedder is known at exactly
/// one place, this pull site: a sibling is materialised PER EMBEDDER, and its
/// own declared default IS the forward string, so `apply_inline_config_defaults`'
/// self-reference guard leaves it standing and every consumer downstream reads
/// `@config.fields` where a value belongs — a populated list rendering blank.
///
/// Only whole-string forwards resolve here. A `@config.<knob>.<path>` form and
/// a knob the embedder does not declare are left alone for the binding
/// validator to judge.
fn forwarded_sibling_config(
    sibling: &TraitDefinition,
    result: &[TraitReference],
    parent_name: &str,
) -> Option<HashMap<String, ConfigField>> {
    let declared = sibling.config.as_ref()?;
    let parent = result.iter().find_map(|tr| match tr {
        TraitReference::Inline(td) if td.name == parent_name => td.config.as_ref(),
        _ => None,
    })?;
    let mut out: HashMap<String, ConfigField> = HashMap::new();
    for (key, field) in declared.iter() {
        let Some(SExpression::String(forward)) = field.default.as_ref() else {
            continue;
        };
        let Some(knob) = forward.strip_prefix("@config.") else {
            continue;
        };
        if knob.contains('.') {
            continue;
        }
        let Some(value) = parent.get(knob).and_then(|f| f.default.as_ref()) else {
            continue;
        };
        // The embedder forwards the same token upward — nothing to resolve yet;
        // its own embedder supplies the value one pull further out.
        if matches!(value, SExpression::String(v) if v == forward) {
            continue;
        }
        let mut forwarded = field.clone();
        forwarded.default = Some(value.clone());
        out.insert(key.clone(), forwarded);
    }
    (!out.is_empty()).then_some(out)
}

/// `@trait.<SiblingName>` into `out`. App atoms embed a curated variation as
/// a sibling trait and reference it inside the main trait's render-ui as the
/// literal `"@trait.SiblingView"`. The form is the full string equal to
/// `@trait.<Name>` (no trailing path segment) — that's how a render node
/// addresses a sibling trait's projected view.
///
/// Also the single owner of the `@trait.X`-collection contract for the
/// lambda-splice pass (inline/splice.rs) and the shared-entity render-
/// authority attribution (validation/entity_mutation.rs) — no second
/// `@trait` walker exists.
pub(crate) fn collect_sibling_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(name) = s.strip_prefix("@trait.") {
                if !name.is_empty() && !name.contains('.') {
                    out.insert(name.to_string());
                }
            }
        }
        SExpression::Array(items) => {
            for item in items {
                collect_sibling_trait_refs(item, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_sibling_trait_refs(v, out);
            }
        }
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
    }
}

/// Scan a resolved trait's render trees (every SExpr inside transition effects
/// + guards, and tick effects + guards) for `@trait.<Sibling>` references.
///
/// Also scans every config-field DEFAULT value. A `@trait.<Sibling>` reference
/// can live inside a config-field's default render tree (e.g. std-browse's
/// `denseBodyContent`/`feedBodyContent` bodies, selected by a `browseLook`
/// guard) rather than directly in a transition effect. When the effect that
/// consumes such a field is still an unsubstituted `@config.<field>` binding
/// — as it is during a deferred import-load inline — the transition walk alone
/// misses those refs, so the siblings never get pulled into the intermediate
/// clone and dangle once a downstream consumer substitutes the binding.
/// Scanning config defaults makes sibling discovery independent of whether the
/// config binding has been substituted yet.
fn sibling_trait_refs_of(trait_def: &TraitDefinition) -> HashSet<String> {
    let mut out = HashSet::new();
    for transition in &trait_def.state_machine.transitions {
        if let Some(guard) = transition.guard.as_ref() {
            collect_sibling_trait_refs(guard, &mut out);
        }
        if let Some(effects) = transition.effects.as_ref() {
            for effect in effects {
                collect_sibling_trait_refs(effect, &mut out);
            }
        }
    }
    if let Some(ticks) = trait_def.ticks.as_ref() {
        for tick in ticks {
            if let Some(guard) = tick.guard.as_ref() {
                collect_sibling_trait_refs(guard, &mut out);
            }
            for effect in &tick.effects {
                collect_sibling_trait_refs(effect, &mut out);
            }
        }
    }
    if let Some(config) = trait_def.config.as_ref() {
        for field in config.values() {
            if let Some(default) = field.default.as_ref() {
                collect_sibling_trait_refs(default, &mut out);
            }
        }
    }
    out
}

/// One trait Reference whose call-site `config` doesn't exactly match the
/// referenced trait's declared config knobs. Collected during inlining (the
/// only point where both the raw call-site key set and the atom's untouched
/// declaration — including cross-orbital `std/behaviors/*` atoms resolved
/// via the embedded phf registry — are visible together) and converted into
/// `ORB_T_CONFIG_UNKNOWN_CALL_SITE_KEY` / `ORB_T_CONFIG_MISSING_REQUIRED_KEY`
/// validation errors by `phases::validation::trait_ref_config`.
#[derive(Debug, Clone)]
pub struct TraitRefConfigDiagnostic {
    pub path: String,
    pub reference: String,
    pub unknown_keys: Vec<String>,
    pub missing_required_keys: Vec<String>,
    pub declared_keys: Vec<String>,
    /// `events { FROM: TO }` source keys that name no event the referenced
    /// trait declares or produces (emits ∪ transition triggers ∪
    /// `stateMachine.events[].key` ∪ effect emit-maps / render-ui event props).
    /// Collected here instead of aborting inlining: a bad rename key must not
    /// un-resolve the rest of the schema (the abort left later orbitals
    /// un-inlined and cascaded spurious ORB_T_UNDEFINED_TRAIT /
    /// ORB_X_LISTEN_SOURCE_UNRESOLVED findings). Converted to
    /// `ORB_T_EVENT_RENAME_UNKNOWN_KEY` errors by
    /// `phases::validation::trait_ref_config`.
    pub unknown_event_rename_keys: Vec<String>,
    /// The referenced trait's known event set at check time — carried so the
    /// validation error can name the valid keys.
    pub known_events: Vec<String>,
}

pub(crate) type InlinedTraits = (
    Vec<TraitReference>,
    HashMap<String, String>,
    Vec<EntityDefinition>,
    // Names of traits added by the auto-pull loop (siblings referenced via
    // `@trait.<Sibling>`). These keep their source name within an orbital so
    // the `@trait` ref resolves, but identical pulls in two orbitals collide
    // app-wide; `inline_schema` uniquifies those after all orbitals resolve.
    Vec<String>,
    Vec<TraitRefConfigDiagnostic>,
);

/// Inline trait references.
///
/// For each `TraitReference::Reference { ref, linkedEntity, config, .. }`,
/// this function resolves the imported `TraitDefinition`, applies any
/// `linkedEntity` (and, eventually, `config`) overrides via
/// `apply_overrides_to_trait`, and pushes a single
/// `TraitReference::Inline(merged_trait)` to the result. The downstream
/// `compose` and `resolve` phases only handle `TraitReference::Inline`, so
/// emitting a single merged Inline is what makes the override actually
/// reach the lowered OIR.
///
/// Phase 3.5.G.A.3 — Collision detection. After A.1 + A.2 produce the
/// merged `TraitDefinition` (with any `name` override applied), this
/// function tracks the resulting trait names in a `HashSet` and rejects
/// duplicates. The check covers three cases:
///
/// 1. Two `TraitReference::Reference` entries that both resolve to the same
///    upstream trait WITHOUT a `name` override on either: both end up with
///    the same `trait_def.name` (the upstream original) → collision.
/// 2. Two `Reference` entries where exactly one has a distinguishing `name`
///    override: the renamed one and the original-named one differ →
///    NO collision, both inlined.
/// 3. A literal `TraitReference::Inline` and a `Reference` that resolves
///    to the same name: also rejected as a collision.
///
/// On collision, the function returns
/// [`InlineError::DuplicateTraitName`] with the offending name and a
/// message that tells the molecule author exactly how to fix it: provide a
/// `name` override on at least one reference. The error fires AFTER the
/// rename has been applied, so users with two distinct `name` overrides
/// (e.g. `CartItemAddItem` and `CartItemEditItem`) are unaffected.
pub(crate) fn inline_traits(
    traits: &[TraitReference],
    ctx: &InlineContext,
    path_prefix: &str,
    // Consumer-scope entity name → id map, used to keep every rebound
    // `linkedEntity` name's dual-carry id sibling pointing at the consumer's
    // entity node (see `apply_overrides_to_trait`).
    entity_ids: &HashMap<String, String>,
    recorder: &mut Vec<ConfigProvenanceRecord>,
) -> Result<InlinedTraits, InlineError> {
    let mut result = Vec::new();
    let mut config_diagnostics: Vec<TraitRefConfigDiagnostic> = Vec::new();
    // Auxiliary entities surfaced by no-rebind sibling-pulls (see the pull loop).
    let mut pulled_aux_entities: Vec<EntityDefinition> = Vec::new();
    // Track every trait NAME that has already been pushed to `result`.
    // Names are unique across the orbital, so a second push of the same
    // name is a collision regardless of whether it came from a literal
    // Inline or a resolved Reference.
    let mut seen_names: HashSet<String> = HashSet::new();
    // Capture each `name:` override applied to a TraitReference so callers
    // (specifically `inline_orbital`) can propagate the rename through the
    // orbital's pages. Without this, `pages[].traits: [{ref: "OldName"}]`
    // entries become stale after the rename and fail validation with
    // `ORB_P_INVALID_TRAIT_REF`.
    let mut rename_map: HashMap<String, String> = HashMap::new();
    // Per-trait event renames applied at the call site (`events {OLD: NEW}`),
    // keyed by the trait's FINAL name. A pulled sibling's source-scoped listen
    // names the host's emit by its pre-rename name; Phase 9.5 chases the
    // listen through this map so the route survives the rename
    // (C-EVENTS-RENAME-MISSES-FETCH-EMIT-MAP's listener half).
    let mut event_renames_by_trait: HashMap<String, HashMap<String, String>> = HashMap::new();
    // App atoms keep a hand-rolled main trait and embed a curated variation as
    // a SIBLING trait, referencing it inside the main trait's render-ui as the
    // string literal `@trait.<SiblingName>`. When an organism composes ONLY the
    // main trait via a trait ref, the sibling is never instantiated and the
    // `@trait.<SiblingName>` dangles (`ORB_BINDING_TRAIT_UNKNOWN`). For each
    // resolved Reference trait we record the source alias + the linkedEntity
    // rebind it used, so a post-pass can auto-instantiate any same-alias
    // siblings the cloned body references — applying the SAME rebind a
    // hand-written sibling ref would have, keeping the sibling's original name
    // so `@trait.<SiblingName>` resolves.
    // (alias, sibling name, linkedEntity rebind, PULLING PARENT's final name,
    // OWNER's final name). The parent name lets a name collision rewrite exactly
    // that parent's `@trait.<Sibling>` tokens to the disambiguated pull name; the
    // owner is the top-level consumer trait that started the chain, and is the
    // unit a pulled sibling belongs to (see `pulled_as` below).
    let mut sibling_pull_work: Vec<(String, String, Option<String>, String, String)> = Vec::new();
    // Explicitly declared Reference instances keyed by (alias, SOURCE trait
    // name, linkedEntity rebind) → final (post-`name`-override) name. A
    // sibling pull that matches one of these keys is satisfied by the
    // explicit composition — regardless of the local name the consumer chose
    // — so the pull is suppressed and the owner's `@trait.<Source>` tokens
    // are rewritten to the explicit instance. This is what makes one atom
    // rebindable N times in one orbital (N entities, N explicit transport
    // rebinds, each owning its entity's companion slot).
    let mut explicit_ref_instances: HashMap<(String, String, Option<String>), String> =
        HashMap::new();
    // Per-owner source-name → final-name map for the traits materialised under
    // that owner: the owner itself (keyed by the atom's original trait name) plus
    // every sibling pulled for it. A pulled copy's source-scoped `listens` names
    // its host and co-siblings by their ATOM names; resolving them through the
    // orbital-wide `rename_map` cannot work once one atom is bound twice, because
    // that map holds one entry per source name.
    let mut owner_subs: HashMap<String, HashMap<String, String>> = HashMap::new();
    // Pulled sibling final name → the owner it was materialised for.
    let mut pulled_owner: HashMap<String, String> = HashMap::new();

    for (idx, trait_ref) in traits.iter().enumerate() {
        let trait_path = format!("{path_prefix}.traits[{idx}]");
        match trait_ref {
            TraitReference::Inline(trait_def) => {
                if !seen_names.insert(trait_def.name.clone()) {
                    return Err(InlineError::DuplicateTraitName {
                        name: trait_def.name.clone(),
                        path: trait_path,
                    });
                }
                // Atom-scope config-default substitution. When an inline
                // trait carries `@config.X` bindings (e.g. `limit:
                // @config.pageSize`) and is compiled standalone (no import
                // call site supplying overrides), the codegen path emits
                // `Number(undefined)` for those bindings — see codegen/guard.rs
                // OirBindingRoot::Config which assumes the inline phase has
                // already substituted every @config.X with a literal.
                //
                // Only do this when the orbital is NOT being loaded as an
                // import for a downstream consumer (`ctx.defer_config_subst`).
                // On an import load, folding here would substitute `@config.X`
                // with this atom's OWN declared default BEFORE the consumer's
                // `apply_overrides_to_trait` pass (which runs afterwards on the
                // caller's `TraitReference`) can apply the consumer's override
                // — permanently clobbering the override for any knob that has a
                // declared default. That is the guard-fold bug: a guard
                // `["=", "@config.browseLook", "dense"]` was baked to the atom
                // default `["=", "table", "dense"]` instead of the consumer
                // override `["=", "dense", "dense"]`. Deferring leaves
                // `@config.X` live so the override substitution reaches it; the
                // consumer's own top-level resolution (Reference arm / sibling
                // pull, both defer=false) then folds it to the effective value.
                let mut td = trait_def.clone();
                if !ctx.defer_config_subst {
                    apply_inline_config_defaults(&mut td, recorder);
                }
                result.push(TraitReference::Inline(td));
            }
            TraitReference::Reference {
                reference,
                ref_id,
                id: decl_id,
                name: name_override,
                linked_entity,
                config,
                events: events_override,
                fields: fields_override,
                listens: listens_override,
                emits_scope: emits_scope_override,
                type_args: type_args_override,
                ..
            } => {
                if let Some(ref_str) = reference {
                    let parsed = parse_reference(ref_str)?;
                    match parsed {
                        ParsedReference::Trait { alias, name } => {
                            // V4: prefer the ref's dual-carry `ref_id` when
                            // present — resolves to the same node the name
                            // path would, but disambiguates same-named
                            // traits across orbitals. Falls back to the
                            // name-based `get_trait` unchanged.
                            let inlined = match ref_id
                                .as_deref()
                                .and_then(|rid| ctx.get_trait_by_ref_id(&alias, rid))
                            {
                                Some(t) => t,
                                None => ctx.get_trait(&alias, &name)?,
                            };
                            // STUDIO-1: capture provenance BEFORE override
                            // application. `source_label` is the imported
                            // behavior's human-readable name (set by
                            // `add_alias_multi`). `source_entity_def` is
                            // the entity from the orbital that owns this
                            // trait — already plumbed for the auto-merge
                            // path, reused here so the client-side mock
                            // generator (STUDIO-2) can produce rows for
                            // the trait's linkedEntity without walking
                            // the import graph at runtime.
                            let source_label = ctx.get_entry(&alias)?.source_label.clone();
                            // §52: the trait's TRUE source entity is the one
                            // its own `->` binding names — for a source trait
                            // bound to a source aux entity, the owning
                            // orbital's slot-0 primary is the wrong answer
                            // (spurious/suppressed rebind verdicts downstream).
                            let source_entity_def = ctx.get_bound_entity_for_trait_orbital(
                                &alias,
                                &name,
                                ref_id.as_deref(),
                            );
                            let provenance = SourceBehaviorMetadata {
                                behavior: source_label,
                                alias: alias.clone(),
                                original_name: name.clone(),
                            };
                            match inlined {
                                TraitReference::Inline(mut trait_def) => {
                                    // Carry the CONSUMER's declaration id onto
                                    // the resolved trait. `Reference.id` is the
                                    // "V4 LOCAL DECLARATION id — the identity of
                                    // this trait AS DECLARED in its orbital";
                                    // after inlining, this clone IS that
                                    // declaration in the consumer's schema, so it
                                    // is the node that must carry the id.
                                    //
                                    // Keeping the SOURCE atom's id instead left
                                    // the consumer's ledger row carried by
                                    // nothing — one `ORB_ID_LEDGER_ORPHAN` per
                                    // newly-composed trait reference (reproduced
                                    // by adding a single ref to an otherwise
                                    // clean std-retail-pos). Source identity is
                                    // not lost: `source_behavior` records the
                                    // behavior/alias/original name, and the
                                    // reference's `ref_id` addressed the source
                                    // node during resolution.
                                    if let Some(decl) = decl_id.as_ref() {
                                        trait_def.id = Some(decl.clone());
                                    }
                                    // Apply name / linkedEntity / events /
                                    // config overrides to the cloned trait
                                    // so the single Inline pushed below
                                    // carries the caller's intent
                                    // end-to-end.
                                    // Capture the original trait name (the
                                    // ref's `name` segment, BEFORE override)
                                    // so the rename map below records the
                                    // before/after pair when a `name:`
                                    // override is supplied.
                                    let original_name = trait_def.name.clone();
                                    // RULE 3: validate the events-rename SOURCE
                                    // keys against the imported trait's event
                                    // set BEFORE the override is applied (after
                                    // it, the original keys are gone). A `from`
                                    // key that names nothing silently no-ops —
                                    // surface it instead. The event set is the
                                    // union of declared emits and transition
                                    // triggers (`ORB_T_EVENT_RENAME_UNKNOWN_KEY`).
                                    if let Some(map) = events_override.as_ref() {
                                        // Build the same effective per-instance
                                        // config `apply_overrides_to_trait` will
                                        // use below (declared defaults, then
                                        // call-site overrides — but only when
                                        // this isn't a deferred import load,
                                        // where the consumer's own override
                                        // pass hasn't run yet) so a rename's
                                        // source key is checked against the
                                        // RESOLVED `@config.<knob>` emit name,
                                        // not the raw unresolved token.
                                        let mut effective_config: HashMap<String, SExpression> =
                                            HashMap::new();
                                        if let Some(declared) = trait_def.config.as_ref() {
                                            for (k, field) in declared.iter() {
                                                if let Some(default) = field.default.as_ref() {
                                                    effective_config
                                                        .insert(k.clone(), default.clone());
                                                }
                                            }
                                        }
                                        if !ctx.defer_config_subst {
                                            if let Some(cfg) = config.as_ref() {
                                                for (k, cf) in cfg.iter() {
                                                    if let Some(v) = cf.default.as_ref() {
                                                        effective_config
                                                            .insert(k.clone(), v.clone());
                                                    }
                                                }
                                            }
                                        }
                                        // The known set matches what the rename
                                        // actually rewrites (`rewrite_identifiers`
                                        // covers emit arrays, emit-option maps and
                                        // render-ui event props; `rewrite_trait_event_fields`
                                        // covers the structured fields) and the
                                        // validator's `build_trait_event_sets` —
                                        // three sites, one definition. A source key
                                        // outside it is a genuine no-op rename.
                                        let mut known: HashSet<String> = HashSet::new();
                                        if let Some(events) = &trait_def.state_machine.events {
                                            for ev in events {
                                                known.insert(ev.key.clone());
                                            }
                                        }
                                        known.extend(
                                            crate::phases::validation::cross_orbital::collect_effect_produced_events(&trait_def),
                                        );
                                        if let Some(emits) = &trait_def.emits {
                                            for e in emits {
                                                if orbital_core::schema::config_ref_event_knob(
                                                    &e.event,
                                                )
                                                .is_some()
                                                {
                                                    match orbital_core::schema::resolve_config_ref_event_name(
                                                        &e.event,
                                                        &effective_config,
                                                        trait_def.config.as_ref(),
                                                    ) {
                                                        Ok(resolved) => {
                                                            known.insert(resolved);
                                                        }
                                                        // Unresolvable ref (unknown knob /
                                                        // non-string / no default): keep the
                                                        // raw token so the EmitConfigRefInvalid
                                                        // validation check (not this one)
                                                        // reports the real cause, and a rename
                                                        // targeting a genuinely different key
                                                        // still errors here.
                                                        Err(_) => {
                                                            known.insert(e.event.clone());
                                                        }
                                                    }
                                                } else {
                                                    known.insert(e.event.clone());
                                                }
                                            }
                                        }
                                        for t in &trait_def.state_machine.transitions {
                                            known.insert(t.event.clone());
                                        }
                                        // Collect — never abort. An unknown key
                                        // no-ops in the substitution walk, so
                                        // composition of this trait (and every
                                        // later orbital) stays sound; the
                                        // validator reports it as
                                        // ORB_T_EVENT_RENAME_UNKNOWN_KEY.
                                        let mut unknown_event_rename_keys: Vec<String> = map
                                            .keys()
                                            .filter(|from| {
                                                !from.is_empty() && !known.contains(from.as_str())
                                            })
                                            .cloned()
                                            .collect();
                                        if !unknown_event_rename_keys.is_empty() {
                                            unknown_event_rename_keys.sort();
                                            let mut known_events: Vec<String> =
                                                known.into_iter().collect();
                                            known_events.sort();
                                            config_diagnostics.push(TraitRefConfigDiagnostic {
                                                path: trait_path.clone(),
                                                reference: original_name.clone(),
                                                unknown_keys: Vec::new(),
                                                missing_required_keys: Vec::new(),
                                                declared_keys: Vec::new(),
                                                unknown_event_rename_keys,
                                                known_events,
                                            });
                                        }
                                    }
                                    // E1/E2: check the call-site `config`
                                    // keys against the referenced trait's
                                    // DECLARED config BEFORE
                                    // `apply_overrides_to_trait` folds
                                    // matched keys into `trait_def.config`
                                    // and silently drops unmatched ones —
                                    // this is the only point where both the
                                    // raw call-site key set and the atom's
                                    // untouched declaration are visible
                                    // together, even for cross-orbital
                                    // std/behaviors refs (resolved above via
                                    // `ctx.get_trait`, which reaches the
                                    // embedded phf registry).
                                    {
                                        let empty_cfg: HashMap<String, ConfigField> =
                                            HashMap::new();
                                        let cfg = config.as_ref().unwrap_or(&empty_cfg);
                                        let mut declared_keys: std::collections::BTreeSet<String> =
                                            trait_def
                                                .config
                                                .as_ref()
                                                .map(|m| m.keys().cloned().collect())
                                                .unwrap_or_default();
                                        // A `.lolo` alias trait can declare its OWN typed
                                        // knobs inside the reference's config block (e.g.
                                        // `trait Alias = Wrapped -> X { config { steps :
                                        // [StepSpec] = [...] @label "Steps" } }`) — those
                                        // are NEW knobs on the alias, not overrides of
                                        // `Wrapped`'s declared config, so they must not be
                                        // flagged unknown. `is_declaration_form` is the
                                        // structural signal the lowerer already uses to
                                        // distinguish a re-declaration from a plain wiring
                                        // override (`field_type == "unknown"`, no
                                        // annotations) — no key-name or value sniffing.
                                        declared_keys.extend(
                                            cfg.iter()
                                                .filter(|(_, f)| f.is_declaration_form())
                                                .map(|(k, _)| k.clone()),
                                        );
                                        let mut unknown_keys: Vec<String> = cfg
                                            .keys()
                                            .filter(|k| !declared_keys.contains(*k))
                                            .cloned()
                                            .collect();
                                        unknown_keys.sort();
                                        let mut missing_required_keys: Vec<String> = trait_def
                                            .config
                                            .as_ref()
                                            .map(|m| {
                                                m.iter()
                                                    .filter(|(k, f)| {
                                                        f.required.unwrap_or(false)
                                                            && !cfg.contains_key(k.as_str())
                                                    })
                                                    .map(|(k, _)| k.clone())
                                                    .collect()
                                            })
                                            .unwrap_or_default();
                                        missing_required_keys.sort();
                                        if !unknown_keys.is_empty()
                                            || !missing_required_keys.is_empty()
                                        {
                                            config_diagnostics.push(TraitRefConfigDiagnostic {
                                                path: trait_path.clone(),
                                                reference: original_name.clone(),
                                                unknown_keys,
                                                missing_required_keys,
                                                declared_keys: declared_keys.into_iter().collect(),
                                                unknown_event_rename_keys: Vec::new(),
                                                known_events: Vec::new(),
                                            });
                                        }
                                    }
                                    apply_overrides_to_trait(
                                        &mut trait_def,
                                        linked_entity.as_ref(),
                                        entity_ids,
                                        config.as_ref(),
                                        name_override.as_deref(),
                                        events_override.as_ref(),
                                        fields_override.as_ref(),
                                        ctx.defer_config_subst,
                                        recorder,
                                    );
                                    // Increment 3-II: stash the reference's
                                    // call-site type-parameter arguments onto
                                    // the cloned trait_def — it already
                                    // carries its OWN declared `type_params`
                                    // (kind info) from the source atom's
                                    // `.orb`, cloned verbatim above. Both
                                    // survive together to the per-orbital
                                    // `resolve_orbital_type_param_sentinels`
                                    // pass (run once `inline_orbital` fully
                                    // resolves this orbital's entities), which
                                    // needs the FULLY composed entity set to
                                    // resolve an entity-valued argument — the
                                    // same timing 3-I's `linkedEntity` default
                                    // already relies on. That pass clears this
                                    // field after consuming it.
                                    trait_def.type_args = type_args_override.clone();
                                    owner_subs
                                        .entry(trait_def.name.clone())
                                        .or_default()
                                        .insert(original_name.clone(), trait_def.name.clone());
                                    if trait_def.name != original_name {
                                        rename_map.insert(original_name, trait_def.name.clone());
                                    }
                                    if let Some(evmap) = events_override.as_ref() {
                                        let pruned: HashMap<String, String> = evmap
                                            .iter()
                                            .filter(|(from, to)| {
                                                from != to && !from.is_empty() && !to.is_empty()
                                            })
                                            .map(|(from, to)| (from.clone(), to.clone()))
                                            .collect();
                                        if !pruned.is_empty() {
                                            event_renames_by_trait
                                                .insert(trait_def.name.clone(), pruned);
                                        }
                                    }
                                    // Phase F.7: replace the trait's
                                    // listens and/or set every emit's scope
                                    // to the caller-supplied values. These
                                    // are STRUCTURAL overrides applied
                                    // after the substitution pass; they do
                                    // not require identifier rewriting.
                                    apply_post_subst_overrides_to_trait(
                                        &mut trait_def,
                                        listens_override.as_ref(),
                                        emits_scope_override.as_ref(),
                                    );
                                    // STUDIO-1/2: stamp provenance onto the
                                    // resolved trait. Runtime + codegen
                                    // ignore both fields; only Studio's L2
                                    // canvas reads them to bucket cards by
                                    // alias and populate mock rows for
                                    // imported entities.
                                    trait_def.source_behavior = Some(provenance);
                                    trait_def.source_entity_definition = source_entity_def;
                                    // Collision check happens AFTER
                                    // override application so
                                    // post-rename names are what we
                                    // compare. Two references with
                                    // distinct `name` overrides will
                                    // never collide here.
                                    if !seen_names.insert(trait_def.name.clone()) {
                                        return Err(InlineError::DuplicateTraitName {
                                            name: trait_def.name.clone(),
                                            path: trait_path.clone(),
                                        });
                                    }
                                    // Register this explicit composition so a
                                    // sibling pull targeting the same (alias,
                                    // source trait, entity) resolves to it
                                    // instead of materialising a duplicate
                                    // companion — the local name is free.
                                    explicit_ref_instances.insert(
                                        (alias.clone(), name.clone(), linked_entity.clone()),
                                        trait_def.name.clone(),
                                    );
                                    // Record this resolved ref so the sibling-pull
                                    // pass below can instantiate any same-alias
                                    // `@trait.X` siblings its body references,
                                    // rebinding them with the SAME linkedEntity.
                                    for sibling in sibling_trait_refs_of(&trait_def) {
                                        sibling_pull_work.push((
                                            alias.clone(),
                                            sibling,
                                            linked_entity.clone(),
                                            trait_def.name.clone(),
                                            trait_def.name.clone(),
                                        ));
                                    }
                                    result.push(TraitReference::Inline(trait_def));
                                }
                                other => {
                                    // Defensive: `ctx.get_trait` is
                                    // expected to return Inline. If it
                                    // returns a Reference (e.g. the
                                    // imported orbital had not been
                                    // inlined yet), pass it through
                                    // unchanged so a later pass can
                                    // resolve it. We still record the
                                    // name (via `trait_name()`) so a
                                    // subsequent collision against the
                                    // same name is rejected.
                                    if let Some(name) = other.trait_name() {
                                        if !seen_names.insert(name.to_string()) {
                                            return Err(InlineError::DuplicateTraitName {
                                                name: name.to_string(),
                                                path: trait_path.clone(),
                                            });
                                        }
                                    }
                                    result.push(other);
                                }
                            }
                        }
                        _ => return Err(InlineError::InvalidReference(ref_str.clone())),
                    }
                } else {
                    // No reference string, keep as-is (named trait
                    // reference). Track the name if available so a
                    // collision against an Inline or a resolved
                    // Reference with the same name is still rejected.
                    if let Some(name) = trait_ref.trait_name() {
                        if !seen_names.insert(name.to_string()) {
                            return Err(InlineError::DuplicateTraitName {
                                name: name.to_string(),
                                path: trait_path.clone(),
                            });
                        }
                    }
                    result.push(trait_ref.clone());
                }
            }
        }
    }

    // Sibling-trait auto-pull. Process the worklist gathered above: for each
    // `@trait.<Sibling>` a resolved trait references, instantiate the sibling
    // from the SAME source alias if it isn't already present in the consumer,
    // applying the SAME linkedEntity rebind the parent ref used. The pulled
    // sibling KEEPS its original name (so `@trait.<Sibling>` still resolves)
    // and gets NO page. Recurse: a pulled sibling's body may reference further
    // siblings. `visited` guards against cycles; `seen_names` dedups against
    // traits the consumer already declares (explicit composition) and against
    // siblings already pulled.
    let mut pulled_names: Vec<String> = Vec::new();
    {
        let mut worklist = sibling_pull_work;
        let mut visited: HashSet<(String, String, String)> = HashSet::new();
        // Final name each pulled sibling landed under, keyed by (alias, source
        // name, OWNER). Inline chrome names are numbered PER SOURCE ATOM
        // (`InlineButtonRender26` exists in many atoms), so a name-only dedup
        // silently binds the second atom's parent tree to the FIRST atom's
        // trait (cross-atom capture — the std-finance-tracker Charge button
        // firing REPRINT_RECEIPT). The owner is in the key because a sibling is
        // materialised PER EMBEDDER, not per atom: two rebinds of one atom in one
        // orbital (`ChannelRail -> Channel` and `ChatThread -> ChatMessage`, both
        // `std-browse`) carry different entity rebinds, different call-site
        // config, and different hosts to listen to, so one shared copy can only
        // serve one of them. Collisions pull under an owner-prefixed name and
        // rewrite the pulling parent's tokens; owner names are unique within the
        // orbital, so the prefixed name is unique by construction. This subsumes
        // the cross-ALIAS case — an owner pulls from exactly one alias.
        let mut pulled_as: HashMap<(String, String, String), String> = HashMap::new();
        // Consumer-declared names (explicit composition) keep first-wins skip.
        let consumer_declared: HashSet<String> = seen_names.clone();
        // Provenance of each consumer-declared trait, for the skip's gate
        // below: a pull is satisfied by a consumer trait only when that trait
        // is an instance of the SAME source trait (alias + original name). A
        // name-only match is coincidence — L1 numbers generated inline chrome
        // per FILE (`InlineStackRender8` exists in most atoms AND in any
        // consumer whose own markup reaches eight stacks), and skipping on the
        // name alone binds the pulled body's `@trait.<Sibling>` tokens to the
        // consumer's unrelated trait: a phantom `ORB_BINDING_TRAIT_CYCLE` when
        // the consumer's copy embeds the pulling parent, a silently vanished
        // affordance when the atom's copy carried the behavior (both measured,
        // std-devops-dashboard / std-marketing-campaign single-orbital drafts).
        let consumer_declared_provenance: HashMap<String, (String, String)> = result
            .iter()
            .filter_map(|tr| match tr {
                TraitReference::Inline(td) => td.source_behavior.as_ref().map(|sb| {
                    (
                        td.name.clone(),
                        (sb.alias.clone(), sb.original_name.clone()),
                    )
                }),
                _ => None,
            })
            .collect();
        // Rewrite one parent's `@trait.<from>` tokens to the disambiguated
        // name, scoped to that parent only (never schema-wide).
        let rewrite_parent =
            |result: &mut Vec<TraitReference>, parent: &str, from: &str, to: &str| {
                let subs: HashMap<String, String> =
                    HashMap::from([(from.to_string(), to.to_string())]);
                for tr in result.iter_mut() {
                    if let TraitReference::Inline(td) = tr {
                        if td.name == parent {
                            rewrite_trait_embed_tokens(td, &subs);
                        }
                    }
                }
            };
        while let Some((alias, sibling_name, linked_entity, parent_name, owner)) = worklist.pop() {
            // An explicit Reference to this same source trait, rebound to the
            // SAME entity, satisfies the pull no matter what local name the
            // consumer gave it — rewrite the pulling parent's `@trait.<Source>`
            // tokens to the explicit instance and materialise nothing. This is
            // the multi-instance case: N rebinds of one atom in one orbital,
            // each pair (transport + sim) sharing one entity under free names.
            if let Some(explicit_final) = explicit_ref_instances.get(&(
                alias.clone(),
                sibling_name.clone(),
                linked_entity.clone(),
            )) {
                if explicit_final != &sibling_name {
                    rewrite_parent(&mut result, &parent_name, &sibling_name, explicit_final);
                }
                owner_subs
                    .entry(owner.clone())
                    .or_default()
                    .insert(sibling_name.clone(), explicit_final.clone());
                continue;
            }
            // Resolve where this (alias, sibling, owner) should land.
            let mut final_name = sibling_name.clone();
            if let Some(existing) =
                pulled_as.get(&(alias.clone(), sibling_name.clone(), owner.clone()))
            {
                // Already pulled for THIS owner — shared within the owner's own
                // instantiation (two traits of one atom embedding one sibling).
                // If it landed under a disambiguated name, this parent's tokens
                // still say the source name and need the same rewrite.
                if existing != &sibling_name {
                    rewrite_parent(&mut result, &parent_name, &sibling_name, existing);
                }
                continue;
            }
            if seen_names.contains(&sibling_name) {
                if consumer_declared.contains(&sibling_name)
                    && consumer_declared_provenance
                        .get(&sibling_name)
                        .is_some_and(|(a, o)| a == &alias && o == &sibling_name)
                {
                    // Explicit composition wins (documented override surface) —
                    // but only when the consumer's trait IS this source trait;
                    // a coincidental name (no provenance, or a different
                    // source) falls through to disambiguation like any other
                    // taken name.
                    continue;
                }
                // Taken by a coincidentally-named consumer trait or ANOTHER
                // owner's pull: disambiguate under this owner.
                final_name = format!("{owner}{sibling_name}");
                if seen_names.contains(&final_name) {
                    // Even the prefixed name is taken by something that is not
                    // this pull — leave the token dangling for the validator
                    // rather than capture a wrong trait.
                    continue;
                }
            }
            if !visited.insert((alias.clone(), sibling_name.clone(), owner.clone())) {
                continue;
            }
            // The sibling must exist as a trait in the SAME source atom.
            let Ok(TraitReference::Inline(mut sibling_def)) = ctx.get_trait(&alias, &sibling_name)
            else {
                // Not a same-alias sibling (or not resolvable to an inline
                // trait) — leave the `@trait.X` for the validator to flag.
                continue;
            };
            // Apply the SAME rebind a hand-written sibling ref would: rewrite
            // every `@X.path` / `["ref", X]` / fetch/persist literal to the
            // consumer's entity. KEEP the original name (no `name` override) so
            // `@trait.<Sibling>` resolves against it.
            // Resolve the sibling's forward defaults against the embedder
            // before substitution runs, so its body reads the embedder's value
            // instead of an unresolvable `@config.X`. A deferred import load
            // leaves the forward live — the downstream consumer's own override
            // has not been applied yet and must still win.
            let forwarded_config = if ctx.defer_config_subst {
                None
            } else {
                forwarded_sibling_config(&sibling_def, &result, &parent_name)
            };
            apply_overrides_to_trait(
                &mut sibling_def,
                linked_entity.as_ref(),
                entity_ids,
                forwarded_config.as_ref(),
                None,
                None,
                None,
                ctx.defer_config_subst,
                recorder,
            );
            if final_name != sibling_name {
                // Per-owner disambiguation: rename the pulled copy and point
                // the pulling parent's tokens at it. The copy's own body still
                // references ITS atom's siblings by source name — those pulls
                // recurse below carrying THIS copy as parent, so they follow
                // the same collision handling.
                sibling_def.name = final_name.clone();
                rewrite_parent(&mut result, &parent_name, &sibling_name, &final_name);
            }
            // Provenance + source entity, mirroring the main ref path so Studio
            // and the mock generator treat the pulled sibling like any ref.
            if let Ok(entry) = ctx.get_entry(&alias) {
                sibling_def.source_behavior = Some(SourceBehaviorMetadata {
                    behavior: entry.source_label.clone(),
                    alias: alias.clone(),
                    original_name: sibling_name.clone(),
                });
            }
            // Sibling pulls come from an `@trait.<Sibling>` string literal in
            // render-ui — there is no ref object here, so no `ref_id` to try.
            sibling_def.source_entity_definition =
                ctx.get_entity_for_trait_orbital(&alias, &sibling_name, None);
            // No-rebind sibling: it keeps its OWN bound entity (e.g. a
            // step-flow's `ProgressVerticalView`). That entity is auxiliary in
            // the source behavior; carry it into the consumer so the sibling's
            // `(fetch X)` / `linkedEntity` resolve. With a rebind, the sibling
            // was retargeted to the consumer's entity and needs nothing here.
            // The pulled sibling's bound entity must EXIST in the consumer
            // orbital, or any later phase needing its SHAPE has nothing to
            // resolve against (measured: `@entity` on a `data-list` row payload
            // failed to monomorphize in 3 apps — `ORB_T_UNRESOLVED_TYPE_PARAM`
            // — because the orbital held `MasterListView -> AuditEntry` while
            // `AuditEntry` was absent from its entity set).
            //
            // Two ways the sibling ends up bound, and BOTH need the carry:
            //   - no rebind: it keeps the source behavior's own entity, which
            //     is auxiliary in that behavior;
            //   - rebind: it was retargeted to an entity the CONSUMER names,
            //     which may come from a different `uses` alias than the one the
            //     sibling was pulled from — hence the any-alias fallback.
            // Duplicates are harmless: the merge in `inline_orbital` dedupes
            // against the primary entity and existing auxiliaries.
            if let Some(bound) = &sibling_def.linked_entity {
                if let Some(aux) = ctx
                    .get_entity_by_name_for_alias(&alias, bound)
                    .or_else(|| ctx.get_entity_by_name_any_alias(bound))
                {
                    pulled_aux_entities.push(aux);
                }
            }
            // Recurse into the rebound sibling's own `@trait.Y` references,
            // with this pull (under its FINAL name) as the parent.
            for next in sibling_trait_refs_of(&sibling_def) {
                worklist.push((
                    alias.clone(),
                    next,
                    linked_entity.clone(),
                    final_name.clone(),
                    owner.clone(),
                ));
            }
            pulled_as.insert(
                (alias.clone(), sibling_name.clone(), owner.clone()),
                final_name.clone(),
            );
            owner_subs
                .entry(owner.clone())
                .or_default()
                .insert(sibling_name.clone(), final_name.clone());
            pulled_owner.insert(final_name.clone(), owner);
            seen_names.insert(final_name.clone());
            pulled_names.push(final_name);
            result.push(TraitReference::Inline(sibling_def));
        }
    }

    // Phase 9.5: drop source-scoped listeners whose source trait was not
    // imported into this orbital. When an atom's trait (e.g. the Browse trait
    // in std-agent-memory) listens for a sibling trait's emits
    // (`AgentMemoryAgent PINNED -> INIT`), and a consumer imports ONLY the
    // Browse trait via a trait ref, the sibling trait is not present in the
    // consumer's orbital, so the listen can never fire. Silently dropping it
    // prevents a false-positive validation error while preserving the
    // source-filter semantics wherever the sibling IS imported.
    //
    // `ListenSource::Orbital` entries are left alone — they point at a
    // trait in another orbital (cross-orbital filter) and are not validated
    // against the local trait set.
    let local_trait_names: HashSet<String> = result
        .iter()
        .filter_map(|tr| match tr {
            TraitReference::Inline(td) => Some(td.name.clone()),
            other => other.trait_name().map(|s| s.to_string()),
        })
        .collect();

    // Post-dedup id alignment: a skipped duplicate pull (seen_names) leaves its
    // host's listens resolving by NAME to the surviving declaration while the
    // `traitId` side-map still carries the DROPPED copy's id — the survivor's
    // ledger row then disagrees with the reference name downstream
    // (ORB_ID_NAME_MISMATCH at listens[..].source). The name is the handle
    // after dedup; the id must follow the surviving declaration.
    let local_inline_trait_ids: HashMap<String, Option<String>> = result
        .iter()
        .filter_map(|tr| match tr {
            TraitReference::Inline(td) => Some((td.name.clone(), td.id.clone())),
            _ => None,
        })
        .collect();

    for trait_ref in result.iter_mut() {
        if let TraitReference::Inline(trait_def) = trait_ref {
            // A pulled sibling resolves its listen sources against ITS OWNER's
            // map, not the orbital-wide `rename_map`: the map holds one entry per
            // source name, so when one atom is bound twice the second rebind
            // overwrites the first and every copy would point at the last host.
            let own_subs = pulled_owner
                .get(&trait_def.name)
                .and_then(|owner| owner_subs.get(owner));
            if let Some(listens) = trait_def.listens.as_mut() {
                // Rewrite listen SOURCES through the consumer's rename map
                // BEFORE the retain: a sub-trait pulled alongside a renamed
                // host still names the host's ORIGINAL trait as its source
                // (`DataGrid1 listens { Host.EVENT -> X }` with the consumer
                // binding the host as `TransactionHost`). Without the rewrite
                // the retain below sees a non-local name and silently drops
                // the route — and renaming the host is the universal case.
                for listen in listens.iter_mut() {
                    if let Some(orbital_core::schema::ListenSource::Trait { trait_name, .. }) =
                        listen.source.as_mut()
                    {
                        if let Some(renamed) = own_subs
                            .and_then(|subs| subs.get(trait_name.as_str()))
                            .or_else(|| rename_map.get(trait_name.as_str()))
                        {
                            *trait_name = renamed.clone();
                        }
                    }
                    // The listen's EVENT follows the source trait's call-site
                    // event renames too — a sibling listening for the host's
                    // `BrowseItemLoaded` must listen for `CoursesLoaded` once
                    // the host's emit was renamed, or the route (and the
                    // emitter index) dangles on the pre-rename name.
                    let source_name = match listen.source.as_ref() {
                        Some(orbital_core::schema::ListenSource::Trait { trait_name, .. }) => {
                            Some(trait_name.clone())
                        }
                        _ => None,
                    };
                    if let Some(source_name) = source_name {
                        if let Some(subs) = event_renames_by_trait.get(source_name.as_str()) {
                            if let Some(new_event) = subs.get(&listen.event) {
                                listen.event = new_event.clone();
                            }
                        }
                    }
                }
                listens.retain(|listen| match &listen.source {
                    Some(orbital_core::schema::ListenSource::Trait { trait_name, .. }) => {
                        local_trait_names.contains(trait_name)
                    }
                    _ => true,
                });
                for listen in listens.iter_mut() {
                    if let Some(orbital_core::schema::ListenSource::Trait {
                        trait_name,
                        trait_id,
                    }) = listen.source.as_mut()
                    {
                        if let Some(decl_id) = local_inline_trait_ids.get(trait_name.as_str()) {
                            if trait_id != decl_id {
                                *trait_id = decl_id.clone();
                            }
                        }
                    }
                }
            }
        }
    }

    Ok((
        result,
        rename_map,
        pulled_aux_entities,
        pulled_names,
        config_diagnostics,
    ))
}

/// True when an SExpression tree contains the bare `@config.<knob>` token
/// (`token` = `"@config.<knob>"`) as a value — the position where a whole-knob
/// value (a render-ui body, a scalar) is spliced into the child's own body.
fn sexpr_references_config(expr: &SExpression, token: &str) -> bool {
    match expr {
        SExpression::String(s) => s == token,
        SExpression::Array(items) => items.iter().any(|i| sexpr_references_config(i, token)),
        SExpression::Object(map) => map.values().any(|v| sexpr_references_config(v, token)),
        _ => false,
    }
}

/// Payload fields that resolve in the CHILD's own scope for a given call-site
/// knob: the union of `payloadSchema` fields of every event whose transition
/// (in the child's own state machine) renders `@config.<knob>`. When the child
/// renders the knob under `BrowseItemLoaded { data, totalCount }`, a
/// `@payload.data` in the override reads the child's own loaded data — local,
/// not a call-site capture. Empty when the child never renders the knob itself
/// (the knob flows up to a composing parent) — then every payload field stays a
/// call-site capture, preserving the leaf-embed error case
/// (`content={?error}` on a TypographyRender rendered under `INIT {}`).
fn local_payload_fields_for_knob(trait_def: &TraitDefinition, knob: &str) -> HashSet<String> {
    let event_fields: HashMap<&str, &Vec<orbital_core::schema::PayloadField>> = trait_def
        .state_machine
        .events
        .as_ref()
        .map(|events| {
            events
                .iter()
                .filter_map(|e| e.payload_schema.as_ref().map(|p| (e.key.as_str(), p)))
                .collect()
        })
        .unwrap_or_default();

    let token = format!("@config.{knob}");
    let mut out = HashSet::new();
    for tr in &trait_def.state_machine.transitions {
        let renders = tr
            .effects
            .as_ref()
            .is_some_and(|effs| effs.iter().any(|e| sexpr_references_config(e, &token)));
        if !renders {
            continue;
        }
        if let Some(fields) = event_fields.get(tr.event.as_str()) {
            out.extend(fields.iter().map(|f| f.name.clone()));
        }
    }
    out
}

#[cfg(test)]
mod entity_id_reader_tests {
    use super::*;
    use orbital_core::schema::{
        EventDefinition, PayloadField, StateDefinition, StateMachine, StringOrArray,
        TransitionDefinition,
    };

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string())
    }

    fn payload_field(name: &str) -> PayloadField {
        PayloadField {
            name: name.to_string(),
            field_type: "string".to_string(),
            required: None,
            description: None,
            properties: None,
            type_when: None,
        }
    }

    /// A child trait that renders `@config.<knob>` under a transition whose
    /// event declares `<payload_field>`. Mirrors std-browse rendering
    /// `galleryBodyContent` under `BrowseItemLoaded { data }`.
    fn trait_rendering_knob(
        knob: &str,
        event: &str,
        payload_field_name: Option<&str>,
    ) -> TraitDefinition {
        let events = payload_field_name.map(|f| {
            vec![EventDefinition {
                key: event.to_string(),
                id: None,
                name: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_schema: Some(vec![payload_field(f)]),
            }]
        });
        TraitDefinition {
            effect_row: None,
            type_params: None,
            type_args: None,
            name: "Body".to_string(),
            entity_rebindable: Some(true),
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "s".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events,
                transitions: vec![TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("s".to_string()),
                    to: "s".to_string(),
                    event: event.to_string(),
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        s("render-ui"),
                        s("main"),
                        s(&format!("@config.{knob}")),
                    ])]),
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    #[test]
    fn local_fields_include_rendering_transition_payload() {
        // Knob rendered under an event that declares `data` → data is local.
        let td = trait_rendering_knob("galleryBodyContent", "BrowseItemLoaded", Some("data"));
        let local = local_payload_fields_for_knob(&td, "galleryBodyContent");
        assert!(local.contains("data"), "data must be local: {local:?}");
        // A knob the child never renders itself → no local fields (flows up).
        let none = local_payload_fields_for_knob(&td, "idleContent");
        assert!(
            none.is_empty(),
            "unrendered knob has no local fields: {none:?}"
        );
    }

    #[test]
    fn leaf_render_under_init_has_no_local_payload() {
        // A leaf that renders `@config.content` under INIT (no payload) → no
        // local fields, so `@payload.error` stays a call-site capture.
        let td = trait_rendering_knob("content", "INIT", None);
        let local = local_payload_fields_for_knob(&td, "content");
        assert!(
            local.is_empty(),
            "INIT-rendered leaf has no local payload: {local:?}"
        );
    }

    /// A trait bound to entity id `ent-1` whose body references the entity by
    /// name `token` in an `@<name>.title` read, a `["ref", name]`, and a
    /// `["fetch", name]`. `linked_entity`/`entity_ref_ids` are set by the
    /// caller so the two scenarios (renamed vs not) can be expressed.
    fn trait_with_entity_token(token: &str) -> TraitDefinition {
        TraitDefinition {
            effect_row: None,
            type_params: None,
            type_args: None,
            name: "Feed".to_string(),
            entity_rebindable: Some(true),
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events: None,
                transitions: vec![TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(vec![SExpression::Array(vec![
                        s("render-ui"),
                        s("main"),
                        SExpression::Object(
                            vec![
                                ("title".to_string(), s(&format!("@{}.title", token))),
                                (
                                    "rows".to_string(),
                                    SExpression::Array(vec![s("fetch"), s(token)]),
                                ),
                                (
                                    "one".to_string(),
                                    SExpression::Array(vec![s("ref"), s(token)]),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                        ),
                    ])]),
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    fn effect_json(td: &TraitDefinition) -> String {
        serde_json::to_string(&td.state_machine.transitions[0].effects).unwrap()
    }

    #[test]
    fn rebind_rewrites_stale_token_via_entity_ref_id() {
        // The bound entity declaration was renamed `Product` -> `Item` (id
        // `ent-1` stable), so `linked_entity` is now `Item` while the body
        // tokens kept the OLD name `Product`. The `entity_ref_ids` side-map
        // ties `Product` -> `ent-1`. A rebind to `Cart` must still rewrite the
        // stale `@Product` / `(ref Product)` / `(fetch Product)` tokens.
        let mut td = trait_with_entity_token("Product");
        td.linked_entity = Some("Item".to_string());
        td.linked_entity_id = Some("ent-1".to_string());
        td.entity_ref_ids = Some(HashMap::from([(
            "Product".to_string(),
            "ent-1".to_string(),
        )]));

        let entity_ids = HashMap::from([("Cart".to_string(), "ent-9".to_string())]);
        let mut recorder = Vec::new();
        apply_overrides_to_trait(
            &mut td,
            Some(&"Cart".to_string()),
            &entity_ids,
            None,
            None,
            None,
            None,
            false,
            &mut recorder,
        );

        let json = effect_json(&td);
        assert!(
            json.contains("@Cart.title") && !json.contains("@Product.title"),
            "stale @Product token must rebind to @Cart via the side-map id: {json}"
        );
        assert!(
            !json.contains("\"Product\""),
            "stale (ref/fetch Product) literals must rebind to Cart: {json}"
        );
        assert_eq!(td.linked_entity.as_deref(), Some("Cart"));
        assert_eq!(td.linked_entity_id.as_deref(), Some("ent-9"));
    }

    #[test]
    fn rebind_without_side_map_uses_name_path() {
        // No `entity_ref_ids`: the trait's bound-entity NAME matches its body
        // tokens (`Product`), so the existing name path rebinds them. Proves
        // the id path is purely additive — absent the side-map, resolution is
        // the unchanged name-driven behavior.
        let mut td = trait_with_entity_token("Product");
        td.linked_entity = Some("Product".to_string());
        td.linked_entity_id = Some("ent-1".to_string());
        assert!(td.entity_ref_ids.is_none(), "fixture has no side-map");

        let entity_ids = HashMap::from([("Cart".to_string(), "ent-9".to_string())]);
        let mut recorder = Vec::new();
        apply_overrides_to_trait(
            &mut td,
            Some(&"Cart".to_string()),
            &entity_ids,
            None,
            None,
            None,
            None,
            false,
            &mut recorder,
        );

        let json = effect_json(&td);
        assert!(
            json.contains("@Cart.title") && !json.contains("@Product.title"),
            "name path must rebind @Product -> @Cart: {json}"
        );
        assert!(
            !json.contains("\"Product\""),
            "name path must rebind (ref/fetch Product): {json}"
        );
    }
}
