use crate::phases::embed_graph::EmbedGraph;
use crate::phases::validation::fired_events::{collect_schema_fired_events, FiredEvents};
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{ListenSource, Orbital, OrbitalSchema, TraitReference};
use std::collections::{HashMap, HashSet};

/// Listens-integrity backstop (error).
///
/// Defense-in-depth duplicate of the L1 `LowerError::ListenSourceUnresolved`
/// check, run on the resolved IR so `use`-imported behaviors that inline
/// listens after lowering are also covered. Reads the typed
/// `ListenSource` enum produced by `split_listen_source`; never reparses
/// the original event string.
pub(crate) fn validate_listens_integrity(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    let mut trait_emits: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut orbitals_in_schema: HashSet<String> = HashSet::new();
    for orbital in &schema.orbitals {
        orbitals_in_schema.insert(orbital.name.clone());
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let entry = trait_emits
                .entry((orbital.name.clone(), td.name.clone()))
                .or_default();
            if let Some(emits) = &td.emits {
                for emit in emits {
                    entry.insert(emit.event.clone());
                }
            }
            // Producible-but-undeclared events: effect emit-option maps and
            // render-ui pattern event props (e.g. search-input/filter-group
            // `event` — the component fires `UI:<value>`). Same crediting as
            // the orphan-listener emitter index; without it a scoped listen on
            // a configured event name false-positives as unresolved.
            entry.extend(super::event_wiring::collect_effect_produced_events(td));
        }
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                let (source_orbital, source_trait) = match &listen.source {
                    Some(ListenSource::Trait { trait_name, .. }) => {
                        (orbital.name.clone(), trait_name.clone())
                    }
                    Some(ListenSource::Orbital {
                        orbital: o,
                        trait_name,
                        ..
                    }) => (o.clone(), trait_name.clone()),
                    Some(ListenSource::Any) | None => continue,
                };
                // Single-orbital validation context: when the listen's source
                // orbital isn't part of this schema, we can't resolve it. Skip
                // — the composed-schema validation pass catches truly unresolved
                // cross-orbital references once all sibling orbitals are loaded.
                if !orbitals_in_schema.contains(&source_orbital) {
                    continue;
                }
                let resolved = trait_emits
                    .get(&(source_orbital.clone(), source_trait.clone()))
                    .map(|events| events.contains(&listen.event))
                    .unwrap_or(false);
                if !resolved {
                    // A bare `Trait.EVENT` source resolves against the
                    // listener's OWN orbital only. When the named trait does
                    // declare the event in a SIBLING orbital, the qualified
                    // `Orbital.Trait.EVENT` form already routes there on every
                    // path (static + both runtimes) — name it.
                    let sibling_hit = matches!(&listen.source, Some(ListenSource::Trait { .. }))
                        .then(|| {
                            trait_emits.iter().find_map(|((o, t), events)| {
                                (t == &source_trait
                                    && o != &source_orbital
                                    && events.contains(&listen.event))
                                .then(|| o.clone())
                            })
                        })
                        .flatten();
                    let suggestion = match sibling_hit {
                        Some(o) => format!(
                            "Trait '{}' declares event '{}' in sibling orbital '{}' — a bare \
                             `Trait.EVENT` listen only resolves within this orbital; write the \
                             qualified form `{}.{}.{}`.",
                            source_trait, listen.event, o, o, source_trait, listen.event
                        ),
                        None => "Either declare the event in the source trait's `emits` block, or \
                                 remove the listens entry. Cross-orbital channels require the source \
                                 trait to declare the emit explicitly."
                            .to_string(),
                    };
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::CrossOrbitalListenSourceUnresolved,
                            format!("orbitals.{}.traits.{}.listens", orbital.name, td.name),
                            format!(
                                "Listen entry in '{}.{}' references source '{}.{}' event '{}' \
                                 which is not declared on the source trait.",
                                orbital.name, td.name, source_orbital, source_trait, listen.event
                            ),
                        )
                        .with_suggestion(suggestion),
                    );
                }
            }
        }
    }

    result
}

/// Innermost-first chain of ancestor trait NAMES for `trait_name`, walking
/// `EmbedGraph::direct_host` up to the root — the same shape as the JS
/// wiring lint's `embedHostChain` (`click-wiring-audit.ts`), cycle-guarded
/// defensively even though the graph should be acyclic by construction.
fn embed_host_name_chain(embed: &EmbedGraph, trait_name: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([trait_name.to_string()]);
    let mut current = trait_name.to_string();
    while let Some(host) = embed.direct_host(&current) {
        if !seen.insert(host.to_string()) {
            break;
        }
        chain.push(host.to_string());
        current = host.to_string();
    }
    chain
}

/// True when some OTHER trait in `orbital`, embedded (directly or
/// transitively) inside `source_trait`, itself has a live producer for
/// `event` — the JS `traitOrEmbedHostProduces` credit: an embedded child's
/// chrome fires under its embedder's scope (a JSX-hoisted inline button
/// lowers to a child trait carrying the real `action:` prop; the host's own
/// tree only holds the opaque `@trait.Child` reference).
fn embedded_child_produces(
    orbital: &Orbital,
    source_trait: &str,
    event: &str,
    fired: &FiredEvents,
) -> bool {
    let embed = EmbedGraph::build(&orbital.traits);
    for trait_ref in &orbital.traits {
        let TraitReference::Inline(td) = trait_ref else {
            continue;
        };
        if td.name == source_trait {
            continue;
        }
        if embed_host_name_chain(&embed, &td.name)
            .iter()
            .any(|host| host == source_trait)
            && fired.contains_for_trait(&td.name, event)
        {
            return true;
        }
    }
    false
}

/// `ORB_X_LISTEN_SOURCE_NEVER_PRODUCES` — convergence of the JS wiring
/// lint's `listener-affordance-removed-by-config` into the listens-source
/// resolution above. `validate_listens_integrity`'s `resolved` gate folds a
/// trait's declared `emits[]` contract in UNCONDITIONALLY, regardless of
/// whether anything live still backs it — so a route whose source trait
/// declares the event but has since had its only producer configured away
/// (an `itemActions`/`browseItemActions` narrowed at this call site) passes
/// that check clean. This is the second half: even when the CONTRACT
/// resolves the route, is there still a LIVE producer (an effect emit, a
/// rendered `action:`/event prop, a structured `itemActions` entry, or an
/// embedded child's own producer up the source's embed chain)? Fires
/// whatever the emit's declared `scope` is — an `external` (cross-orbital
/// broadcast) declaration still needs a live broadcaster to back a route
/// that depends on it; `ORB_EMIT_DECLARED_BUT_UNFIRED`'s external exemption
/// (an unproduced declaration NOBODY routes is a latent offer) is untouched
/// — this check only fires when a `listens` route actually depends on it.
pub(crate) fn validate_listen_source_never_produces(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let fired = collect_schema_fired_events(schema);

    let mut orbitals_by_name: HashMap<&str, &Orbital> = HashMap::new();
    for orbital in &schema.orbitals {
        orbitals_by_name.insert(orbital.name.as_str(), orbital);
    }

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                let (source_orbital_name, source_trait_name) = match &listen.source {
                    Some(ListenSource::Trait { trait_name, .. }) => {
                        (orbital.name.clone(), trait_name.clone())
                    }
                    Some(ListenSource::Orbital {
                        orbital: o,
                        trait_name,
                        ..
                    }) => (o.clone(), trait_name.clone()),
                    Some(ListenSource::Any) | None => continue,
                };
                let Some(source_orbital) = orbitals_by_name.get(source_orbital_name.as_str())
                else {
                    continue; // unresolved cross-schema source: not this check's finding
                };
                let source_td = source_orbital.traits.iter().find_map(|t| match t {
                    TraitReference::Inline(td) if td.name == source_trait_name => Some(td),
                    _ => None,
                });
                let Some(source_td) = source_td else {
                    continue; // dangling trait ref: ORB_X_LISTEN_SOURCE_UNRESOLVED owns it
                };
                // Gate on the CONTRACT (declared in `emits[]`) exactly as
                // the JS check did (`emitsContractHasEvent`) — a route whose
                // source never even declares the event is a different
                // defect (ORB_X_LISTEN_SOURCE_UNRESOLVED, above), not this
                // one, and gating this way means the two checks never fire
                // twice on the same route.
                let declared = source_td
                    .emits
                    .as_ref()
                    .is_some_and(|emits| emits.iter().any(|e| e.event == listen.event));
                if !declared {
                    continue;
                }
                if fired.contains_for_trait(&source_trait_name, &listen.event) {
                    continue;
                }
                if embedded_child_produces(
                    source_orbital,
                    &source_trait_name,
                    &listen.event,
                    &fired,
                ) {
                    continue;
                }

                result.add_error(
                    ValidationError::error(
                        ErrorCode::CrossOrbitalListenSourceNeverProduces,
                        format!("orbitals.{}.traits.{}.listens", orbital.name, td.name),
                        format!(
                            "listens route {}.{} -> {}: {}'s emits contract declares {}, but no live effect, rendered affordance, or config item action at this call site produces it — a config override (e.g. itemActions/browseItemActions narrowed) likely removed the only producer",
                            source_trait_name,
                            listen.event,
                            listen.triggers.clone().unwrap_or_default(),
                            source_trait_name,
                            listen.event
                        ),
                    )
                    .with_suggestion(format!(
                        "restore {} to {}'s active affordance config, or drop the {} route if the removal is intentional",
                        listen.event, source_trait_name, td.name
                    )),
                );
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
        EntityDefinition, EntityRef, ListenDefinition, Orbital, SExpression, StateDefinition,
        StateMachine, TraitDefinition, TransitionDefinition,
    };

    fn minimal_schema(traits: Vec<TraitReference>) -> OrbitalSchema {
        OrbitalSchema {
            theme_definitions: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                types: None,
                name: "TaskOrbital".to_string(),
                uses: None,
                expects: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    shared: false,
                    identity: false,
                    local: false,
                    seed_mock: false,
                    locale_instances: None,
                    read_policy: None,
                    create_policy: None,
                    update_policy: None,
                    delete_policy: None,
                    access_waivers: None,
                    name: "Task".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                    id: None,
                }),
                auxiliary_entities: None,
                traits,
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                os_grants: None,
                design: None,
                id: None,
                reference: None,
                config: None,
            }],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        }
    }

    fn bare_trait(name: &str) -> TraitDefinition {
        TraitDefinition {
            local: false,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Task".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
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

    /// A trait whose only transition renders a `data-list` with a conditional
    /// (`if`-wrapped) `itemActions` config: the then-branch fires
    /// `EDIT_ITEM`, the else-branch fires `VIEW_ITEM`.
    fn trait_with_conditional_item_actions(name: &str) -> TraitDefinition {
        let descriptor = |event: &str| {
            let mut dm: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
            dm.insert("name".to_string(), SExpression::String("Edit".to_string()));
            dm.insert("event".to_string(), SExpression::String(event.to_string()));
            SExpression::Object(dm)
        };
        let item_actions = SExpression::Array(vec![
            SExpression::String("if".to_string()),
            SExpression::String("@user.role".to_string()),
            SExpression::Array(vec![descriptor("EDIT_ITEM")]),
            SExpression::Array(vec![descriptor("VIEW_ITEM")]),
        ]);
        let mut node: orbital_core_base::schema::SObject = orbital_core_base::schema::SObject::new();
        node.insert(
            "type".to_string(),
            SExpression::String("data-list".to_string()),
        );
        node.insert("itemActions".to_string(), item_actions);
        let render_effect = SExpression::Array(vec![
            SExpression::String("render-ui".to_string()),
            SExpression::String("main".to_string()),
            SExpression::Object(node),
        ]);

        let mut td = bare_trait(name);
        td.state_machine = StateMachine {
            states: vec![StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
            }],
            events: None,
            transitions: vec![TransitionDefinition {
                from: orbital_core_base::schema::types::StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                event_id: None,
                guard: None,
                effects: Some(vec![render_effect]),
            }],
        };
        td
    }

    fn scoped_listener(name: &str, source_trait: &str, event: &str) -> TraitDefinition {
        let mut td = bare_trait(name);
        td.listens = Some(vec![ListenDefinition {
            event: event.to_string(),
            triggers: Some("REFRESH".to_string()),
            scope: None,
            source: Some(ListenSource::Trait {
                trait_name: source_trait.to_string(),
                trait_id: None,
            }),
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            event_id: None,
            triggers_id: None,
        }]);
        td
    }

    /// A listen on an event fired only inside the else-branch of a
    /// conditional `itemActions` config resolves without
    /// `ORB_X_LISTEN_SOURCE_UNRESOLVED` — the runtime evaluates the
    /// conditional at render time, so either branch is a legal listen
    /// source.
    #[test]
    fn listen_on_conditional_branch_only_event_resolves() {
        let schema = minimal_schema(vec![
            TraitReference::Inline(trait_with_conditional_item_actions("TaskList")),
            TraitReference::Inline(scoped_listener("TaskDetail", "TaskList", "VIEW_ITEM")),
        ]);
        let result = validate_listens_integrity(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalListenSourceUnresolved),
            "listen on the conditional's else-branch-only event must resolve; got {:?}",
            result.errors
        );
    }

    // ------------------------------------------------------------------
    // Shard C: `ORB_X_LISTEN_SOURCE_NEVER_PRODUCES` — convergence of the JS
    // wiring lint's `listener-affordance-removed-by-config`. Cases 1-2
    // mirror the JS suite's two tests exactly; the last two are the new
    // coverage the merge requires (the `external`-scope ruling, and the
    // embedded-child producer credit).
    // ------------------------------------------------------------------

    fn schema_for_never_produces(traits: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "test-listen-never-produces",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "O",
                    "entity": {
                        "name": "Row",
                        "persistence": "persistent",
                        "fields": [ { "name": "id", "type": "string", "required": true } ]
                    },
                    "traits": traits,
                    "pages": []
                }
            ]
        }))
        .expect("never-produces fixture must deserialize")
    }

    fn never_produces_errors(schema: &OrbitalSchema) -> Vec<ValidationError> {
        validate_listen_source_never_produces(schema)
            .errors
            .into_iter()
            .filter(|e| e.code == ErrorCode::CrossOrbitalListenSourceNeverProduces)
            .collect()
    }

    #[test]
    fn fires_when_a_config_override_silenced_the_only_live_producer() {
        let schema = schema_for_never_produces(serde_json::json!([
            {
                "name": "Browse",
                "linkedEntity": "Row",
                "category": "interaction",
                "emits": [ { "event": "EDIT_ROW", "scope": "internal" } ],
                "config": { "itemActions": { "type": "array", "default": [ { "event": "VIEW", "label": "View" } ] } },
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "browsing", "to": "browsing", "event": "INIT", "effects": [] }
                ] }
            },
            {
                "name": "EditModal",
                "linkedEntity": "Row",
                "category": "interaction",
                "listens": [ { "event": "EDIT_ROW", "triggers": "OPEN_EDIT", "source": { "kind": "trait", "trait": "Browse" } } ],
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "idle", "to": "open", "event": "OPEN_EDIT", "effects": [] }
                ] }
            }
        ]));
        let errors = never_produces_errors(&schema);
        assert_eq!(errors.len(), 1, "{:?}", errors);
        assert!(errors[0].message.contains("EDIT_ROW"), "{:?}", errors);
    }

    #[test]
    fn silent_once_itemactions_restores_the_event_as_a_live_producer() {
        let schema = schema_for_never_produces(serde_json::json!([
            {
                "name": "Browse",
                "linkedEntity": "Row",
                "category": "interaction",
                "emits": [ { "event": "EDIT_ROW", "scope": "internal" } ],
                "config": { "itemActions": { "type": "array", "default": [
                    { "event": "VIEW", "label": "View" },
                    { "event": "EDIT_ROW", "label": "Edit" }
                ] } },
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "browsing", "to": "browsing", "event": "INIT", "effects": [] }
                ] }
            },
            {
                "name": "EditModal",
                "linkedEntity": "Row",
                "category": "interaction",
                "listens": [ { "event": "EDIT_ROW", "triggers": "OPEN_EDIT", "source": { "kind": "trait", "trait": "Browse" } } ],
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "idle", "to": "open", "event": "OPEN_EDIT", "effects": [] }
                ] }
            }
        ]));
        assert!(never_produces_errors(&schema).is_empty());
    }

    #[test]
    fn fires_on_an_external_scoped_declared_but_unproduced_event_with_a_route_on_it() {
        // `ORB_EMIT_DECLARED_BUT_UNFIRED`'s `external` exemption (an
        // unproduced declaration nobody ROUTES is a latent offer) is
        // deliberately untouched by this check — but a `listens` route DOES
        // depend on it here, so it fires regardless of scope (LOLO.md
        // §1163: external is a cross-orbital broadcast, still needs a live
        // broadcaster).
        let schema = schema_for_never_produces(serde_json::json!([
            {
                "name": "Publisher",
                "linkedEntity": "Row",
                "category": "interaction",
                "emits": [ { "event": "ROW_CHANGED", "scope": "external" } ],
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT", "effects": [] }
                ] }
            },
            {
                "name": "Subscriber",
                "linkedEntity": "Row",
                "category": "interaction",
                "listens": [ { "event": "ROW_CHANGED", "triggers": "REFRESH", "source": { "kind": "trait", "trait": "Publisher" } } ],
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "idle", "to": "idle", "event": "REFRESH", "effects": [] }
                ] }
            }
        ]));
        let errors = never_produces_errors(&schema);
        assert_eq!(errors.len(), 1, "{:?}", errors);
        assert!(errors[0].message.contains("ROW_CHANGED"), "{:?}", errors);
    }

    #[test]
    fn silent_when_an_embedded_child_produces_the_event_up_the_host_chain() {
        // `Host` declares `ROW_ACTION` in its emits contract and embeds
        // `Child` via `@trait.Child` in its own config default; `Host` has
        // no live producer of its own, but `Child`'s rendered button DOES
        // fire it — the JS `traitOrEmbedHostProduces` credit.
        let schema = schema_for_never_produces(serde_json::json!([
            {
                "name": "Host",
                "linkedEntity": "Row",
                "category": "interaction",
                "emits": [ { "event": "ROW_ACTION", "scope": "internal" } ],
                "config": { "body": { "type": "string", "default": "@trait.Child" } },
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT", "effects": [] }
                ] }
            },
            {
                "name": "Child",
                "linkedEntity": "Row",
                "category": "interaction",
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "idle", "to": "idle", "event": "INIT", "effects": [
                        ["render-ui", "main", { "type": "button", "action": "ROW_ACTION", "label": "Go" } ]
                    ] }
                ] }
            },
            {
                "name": "Watcher",
                "linkedEntity": "Row",
                "category": "interaction",
                "listens": [ { "event": "ROW_ACTION", "triggers": "ROW_ACTED", "source": { "kind": "trait", "trait": "Host" } } ],
                "stateMachine": { "states": [], "events": [], "transitions": [
                    { "from": "idle", "to": "idle", "event": "ROW_ACTED", "effects": [] }
                ] }
            }
        ]));
        assert!(never_produces_errors(&schema).is_empty());
    }
}
