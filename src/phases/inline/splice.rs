//! Lambda-scope splice for render-only trait references.
//!
//! A `@trait.X` reference inside a `fn`-lambda render subtree (`renderItem`
//! and any function-typed render prop) cannot survive as a reference: the
//! referenced trait renders in its OWN context, so the author's prop
//! expressions (already substituted into its render template by
//! `apply_overrides_to_trait`) would resolve outside the lambda's per-item
//! scope. When the referenced trait is structurally render-only, this pass
//! splices its render template in place of the `@trait.X` string, so the
//! expressions end up physically inside the lambda and resolve in its scope.
//! The wrapper's `emits {}` declarations are merged into the host trait.
//!
//! Eligibility is structural (never name-based): at most one state, all
//! transitions guard-free self-loops whose effects are exclusively
//! `render-ui`, exactly one `render-ui` across the trait, no `listens`, no
//! `ticks`. `emits` declarations do NOT disqualify. A stateful trait
//! referenced inside a lambda is a hard error — per-item interactive traits
//! are not supported.
//!
//! Outside lambda scope, references are left untouched (normal trait
//! composition + render-authority attribution).

use super::r#trait::collect_sibling_trait_refs;
use super::rewrite::trait_ref_name;
use super::InlineError;
use crate::phases::validation::render_ui::is_render_ui_effect;
use orbital_core_base::schema::{
    EmitDefinition, Orbital, PageRef, SExpression, StringOrArray, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Eligible render template + the wrapper's emit declarations, or `None`
/// when the trait is present but not render-only.
type SpliceEntry = Option<(SExpression, Vec<EmitDefinition>)>;

/// Splice lambda-scoped `@trait.X` references throughout an orbital's
/// (already inlined) traits, then drop wrapper traits that were fully
/// consumed by the splice.
pub(crate) fn splice_lambda_trait_refs(
    orbital: &mut Orbital,
    orbital_path: &str,
) -> Result<(), InlineError> {
    let mut templates: HashMap<String, SpliceEntry> = HashMap::new();
    // V4 leverage-ids side indexes, built once from the sibling set:
    //   `id_to_name`      — embedded trait id → its CURRENT name (the
    //                       `templates` key), so a token whose embed id maps
    //                       to a renamed sibling still finds the template.
    //   `embed_ids_by_name` — trait name → its own `trait_embed_ids`, so
    //                       nested splices switch to the wrapper's side-map
    //                       when recursing into its expanded template.
    let mut id_to_name: orbital_core_base::collections::IndexMap<String, String> = orbital_core_base::collections::IndexMap::new();
    let mut embed_ids_by_name: HashMap<String, Option<orbital_core_base::collections::IndexMap<String, String>>> = HashMap::new();
    for tr in &orbital.traits {
        if let TraitReference::Inline(td) = tr {
            templates.insert(
                td.name.clone(),
                render_only_template(td).map(|t| (t, td.emits.clone().unwrap_or_default())),
            );
            if let Some(id) = td.id.as_deref() {
                id_to_name.insert(id.to_string(), td.name.clone());
            }
            embed_ids_by_name.insert(td.name.clone(), td.trait_embed_ids.clone());
        }
    }
    if templates.is_empty() {
        return Ok(());
    }

    let mut spliced: HashSet<String> = HashSet::new();
    for tr in orbital.traits.iter_mut() {
        let TraitReference::Inline(td) = tr else {
            continue;
        };
        let path = format!("{}.traits[name={}]", orbital_path, td.name);
        // The host's own embed side-map drives id-primary resolution for the
        // `@trait.X` tokens in ITS body. Cloned so the transitions/ticks below
        // can borrow `td` mutably while this read-only map is in scope.
        let host_embed = td.trait_embed_ids.clone();
        // Seed with the host itself so a self-reference never self-splices.
        let mut visiting: Vec<String> = vec![td.name.clone()];
        let mut merged: Vec<(String, EmitDefinition)> = Vec::new();
        for transition in td.state_machine.transitions.iter_mut() {
            if let Some(effects) = transition.effects.as_mut() {
                for effect in effects.iter_mut() {
                    splice_expr(
                        effect,
                        false,
                        &templates,
                        &embed_ids_by_name,
                        &id_to_name,
                        host_embed.as_ref(),
                        &path,
                        &mut visiting,
                        &mut merged,
                        &mut spliced,
                    )?;
                }
            }
        }
        if let Some(ticks) = td.ticks.as_mut() {
            for tick in ticks.iter_mut() {
                for effect in tick.effects.iter_mut() {
                    splice_expr(
                        effect,
                        false,
                        &templates,
                        &embed_ids_by_name,
                        &id_to_name,
                        host_embed.as_ref(),
                        &path,
                        &mut visiting,
                        &mut merged,
                        &mut spliced,
                    )?;
                }
            }
        }
        if !merged.is_empty() {
            merge_wrapper_emits(td, merged, &path)?;
        }
    }

    if !spliced.is_empty() {
        remove_consumed_wrappers(orbital, &spliced);
    }
    Ok(())
}

/// The structural render-only test. Returns the single render-ui pattern
/// subtree when the trait qualifies for lambda splicing.
fn render_only_template(td: &TraitDefinition) -> Option<SExpression> {
    if td.listens.as_ref().is_some_and(|l| !l.is_empty()) {
        return None;
    }
    if td.ticks.as_ref().is_some_and(|t| !t.is_empty()) {
        return None;
    }
    if td.state_machine.states.len() > 1 {
        return None;
    }
    let mut template: Option<SExpression> = None;
    for transition in &td.state_machine.transitions {
        let self_loop = match &transition.from {
            StringOrArray::Single(f) => *f == transition.to,
            StringOrArray::Multiple(fs) => fs.iter().all(|f| *f == transition.to),
        };
        if !self_loop || transition.guard.is_some() {
            return None;
        }
        for effect in transition.effects.iter().flatten() {
            if !is_render_ui_effect(effect) {
                return None;
            }
            let SExpression::Array(arr) = effect else {
                return None;
            };
            let pattern = arr.get(2)?;
            if template.is_some() {
                // Two render-ui effects — one canonical template does not
                // exist, so the trait keeps reference semantics.
                return None;
            }
            template = Some(pattern.clone());
        }
    }
    template
}

/// Recursive in-place splice walker. `in_lambda` becomes true inside the
/// body of any `["fn", params, body]` form and stays true for the
/// whole subtree.
#[allow(clippy::too_many_arguments)]
fn splice_expr(
    expr: &mut SExpression,
    in_lambda: bool,
    templates: &HashMap<String, SpliceEntry>,
    embed_ids_by_name: &HashMap<String, Option<orbital_core_base::collections::IndexMap<String, String>>>,
    id_to_name: &orbital_core_base::collections::IndexMap<String, String>,
    // The embed side-map of the trait whose body this subtree belongs to
    // (`None` when it has no `trait_embed_ids`). Switched to the wrapper's own
    // map when recursing into its expanded template.
    cur_embed_ids: Option<&orbital_core_base::collections::IndexMap<String, String>>,
    path: &str,
    visiting: &mut Vec<String>,
    merged: &mut Vec<(String, EmitDefinition)>,
    spliced: &mut HashSet<String>,
) -> Result<(), InlineError> {
    match expr {
        SExpression::String(s) if in_lambda => {
            let Some(token) = s.strip_prefix("@trait.") else {
                return Ok(());
            };
            if token.is_empty() || token.contains('.') {
                return Ok(());
            }
            // V4 leverage-ids (id-primary, name-fallback): resolve the embed
            // token by the owning trait's `trait_embed_ids` id first — token
            // → embed id → the renamed sibling's CURRENT name — so a token that
            // kept its old name after the embedded declaration was renamed
            // still splices. Absent side-map / unknown id → the token name is
            // used verbatim, exactly as before.
            let name: &str = cur_embed_ids
                .and_then(|m| m.get(token))
                .and_then(|id| id_to_name.get(id))
                .map(String::as_str)
                .unwrap_or(token);
            match templates.get(name) {
                Some(Some((template, emits))) => {
                    if visiting.iter().any(|v| v == name) {
                        // Reference cycle — leave the string for the
                        // validator rather than looping.
                        return Ok(());
                    }
                    let mut expanded = template.clone();
                    visiting.push(name.to_string());
                    // The wrapper's template may itself contain nested
                    // inline refs (JSX children); they are now inside the
                    // lambda too, so splice recursively — under the WRAPPER's
                    // own embed side-map so nested tokens resolve by its ids.
                    let child_embed = embed_ids_by_name.get(name).and_then(Option::as_ref);
                    splice_expr(
                        &mut expanded,
                        true,
                        templates,
                        embed_ids_by_name,
                        id_to_name,
                        child_embed,
                        path,
                        visiting,
                        merged,
                        spliced,
                    )?;
                    visiting.pop();
                    for emit in emits {
                        merged.push((name.to_string(), emit.clone()));
                    }
                    spliced.insert(name.to_string());
                    *expr = expanded;
                    Ok(())
                }
                Some(None) => Err(InlineError::LambdaStatefulTrait {
                    name: name.to_string(),
                    path: path.to_string(),
                }),
                // Unknown reference — the binding validator owns that error.
                None => Ok(()),
            }
        }
        SExpression::Array(arr) => {
            let is_lambda_form = arr.len() == 3
                && matches!(&arr[0], SExpression::String(op) if op == "fn");
            if is_lambda_form {
                splice_expr(
                    &mut arr[2],
                    true,
                    templates,
                    embed_ids_by_name,
                    id_to_name,
                    cur_embed_ids,
                    path,
                    visiting,
                    merged,
                    spliced,
                )
            } else {
                for item in arr.iter_mut() {
                    splice_expr(
                        item,
                        in_lambda,
                        templates,
                        embed_ids_by_name,
                        id_to_name,
                        cur_embed_ids,
                        path,
                        visiting,
                        merged,
                        spliced,
                    )?;
                }
                Ok(())
            }
        }
        SExpression::Object(map) => {
            for v in map.values_mut() {
                splice_expr(
                    v,
                    in_lambda,
                    templates,
                    embed_ids_by_name,
                    id_to_name,
                    cur_embed_ids,
                    path,
                    visiting,
                    merged,
                    spliced,
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Merge spliced wrappers' emit declarations into the host trait, deduped by
/// event name. A same-name emit with a structurally different definition is
/// a hard conflict, never a silent union.
fn merge_wrapper_emits(
    host: &mut TraitDefinition,
    incoming: Vec<(String, EmitDefinition)>,
    path: &str,
) -> Result<(), InlineError> {
    let mut emits = host.emits.take().unwrap_or_default();
    for (wrapper, emit) in incoming {
        if let Some(existing) = emits.iter().find(|e| e.event == emit.event) {
            let same = serde_json::to_value(existing).ok() == serde_json::to_value(&emit).ok();
            if !same {
                let event = emit.event.clone();
                host.emits = Some(emits);
                return Err(InlineError::EmitMergeConflict {
                    event,
                    name: wrapper,
                    host: host.name.clone(),
                    path: path.to_string(),
                });
            }
        } else {
            emits.push(emit);
        }
    }
    if !emits.is_empty() {
        host.emits = Some(emits);
    }
    Ok(())
}

/// Drop spliced wrapper traits that are no longer referenced anywhere: no
/// remaining `@trait.<name>` string in any trait body or config default, and
/// no page lists them. A wrapper still referenced outside a lambda keeps its
/// declaration (normal composition continues to own it).
fn remove_consumed_wrappers(orbital: &mut Orbital, spliced: &HashSet<String>) {
    // Fixpoint: removing an outer wrapper can strand the reference that kept
    // an inner wrapper alive (nested JSX children), so re-scan survivors
    // until nothing more drops.
    loop {
        let mut still_referenced: HashSet<String> = HashSet::new();
        for tr in &orbital.traits {
            if let TraitReference::Inline(td) = tr {
                for transition in &td.state_machine.transitions {
                    if let Some(guard) = &transition.guard {
                        collect_sibling_trait_refs(guard, &mut still_referenced);
                    }
                    for effect in transition.effects.iter().flatten() {
                        collect_sibling_trait_refs(effect, &mut still_referenced);
                    }
                }
                for tick in td.ticks.iter().flatten() {
                    if let Some(guard) = &tick.guard {
                        collect_sibling_trait_refs(guard, &mut still_referenced);
                    }
                    for effect in &tick.effects {
                        collect_sibling_trait_refs(effect, &mut still_referenced);
                    }
                }
                for field in td.config.iter().flat_map(|c| c.values()) {
                    if let Some(default) = &field.default {
                        collect_sibling_trait_refs(default, &mut still_referenced);
                    }
                }
            }
        }

        let mut removable: HashSet<&str> = orbital
            .traits
            .iter()
            .filter_map(trait_ref_name)
            .filter(|n| spliced.contains(*n) && !still_referenced.contains(*n))
            .collect();
        if removable.is_empty() {
            break;
        }

        // The lowerer lists hoisted synthetics on the authoring trait's page;
        // those entries are shadows of the (now spliced-away) authoring site
        // and travel with the trait. Never empty a page though: if removal
        // would leave a page with zero traits, the trait it lists survives.
        for page in orbital.pages.iter().flatten() {
            if let PageRef::Inline(def) = page {
                let Some(page_traits) = def.traits.as_ref() else {
                    continue;
                };
                let survivors = page_traits
                    .iter()
                    .filter_map(trait_ref_name)
                    .filter(|n| !removable.contains(n))
                    .count();
                if survivors == 0 {
                    for tr in page_traits {
                        if let Some(name) = trait_ref_name(tr) {
                            removable.remove(name);
                        }
                    }
                }
            }
        }
        if removable.is_empty() {
            break;
        }
        let removable: HashSet<String> = removable.into_iter().map(String::from).collect();

        for page in orbital.pages.iter_mut().flatten() {
            if let PageRef::Inline(def) = page {
                if let Some(page_traits) = def.traits.as_mut() {
                    page_traits
                        .retain(|tr| trait_ref_name(tr).is_none_or(|n| !removable.contains(n)));
                }
            }
        }
        orbital
            .traits
            .retain(|tr| trait_ref_name(tr).is_none_or(|n| !removable.contains(n)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
        EntityDefinition, EntityRef, PayloadField, StateDefinition, StateMachine,
        TransitionDefinition,
    };

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string().into())
    }
    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items)
    }
    fn obj(pairs: Vec<(&str, SExpression)>) -> SExpression {
        SExpression::object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn render_only_trait(name: &str, pattern: SExpression) -> TraitDefinition {
        TraitDefinition {
            dispatch: None,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            description: None,
            linked_entity: Some("Item".to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                    holds: None,
                }],
                events: None,
                transitions: vec![TransitionDefinition {
                    event_id: None,
                    from: StringOrArray::Single("idle".to_string()),
                    to: "idle".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(vec![arr(vec![s("render-ui"), s("main"), pattern])]),
                    ensures: None,
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            source_declaration: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    fn emit(event: &str, field: &str) -> EmitDefinition {
        EmitDefinition {
            definer_knob: None,
            scope_overridden: None,
            event: event.to_string(),
            description: None,
            synonyms: None,
            tier: None,
            scope: None,
            payload_schema: Some(vec![PayloadField {
                nullable: None,
                additional_properties: None,
                values: None,
                name: field.to_string(),
                field_type: "string".to_string(),
                required: None,
                description: None,
                properties: None,
                type_when: None,
                entity: None,
                projected_from: None,
                items: None,
            }]),
            event_id: None,
            payload_entity: None,
        }
    }

    fn host_with_render(name: &str, pattern: SExpression) -> TraitDefinition {
        render_only_trait(name, pattern)
    }

    fn make_orbital(traits: Vec<TraitDefinition>) -> Orbital {
        Orbital {
            spec: false,
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                timestamps: None,
                soft_delete: None,
                description: None,
                visual_prompt: None,
                asset_ref: None,
                store: None,
                shared: false,
                identity: false,
                browser: false,
                locale_instances: None,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Item".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
                id: None,
                invariants: None,
                indexes: None,
            }),
            auxiliary_entities: None,
            traits: traits.into_iter().map(TraitReference::Inline).collect(),
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
        }
    }

    fn lambda_render_pattern(body_children: Vec<SExpression>) -> SExpression {
        obj(vec![
            ("type", s("data-list")),
            ("items", s("@entity.parties")),
            (
                "renderItem",
                arr(vec![
                    s("fn"),
                    s("party"),
                    obj(vec![
                        ("type", s("stack")),
                        ("children", SExpression::Array(body_children)),
                    ]),
                ]),
            ),
        ])
    }

    fn host_pattern_of(orbital: &Orbital, name: &str) -> SExpression {
        for tr in &orbital.traits {
            if let TraitReference::Inline(td) = tr {
                if td.name == name {
                    let effects = td.state_machine.transitions[0].effects.as_ref().unwrap();
                    let SExpression::Array(arr) = &effects[0] else {
                        panic!("render-ui effect")
                    };
                    return arr[2].clone();
                }
            }
        }
        panic!("trait {name} not found");
    }

    #[test]
    fn eligibility_accepts_render_only_with_emits() {
        let mut w = render_only_trait("W", obj(vec![("type", s("badge"))]));
        w.emits = Some(vec![emit("ACTION", "id")]);
        assert!(
            render_only_template(&w).is_some(),
            "emits must not disqualify"
        );
    }

    #[test]
    fn eligibility_rejects_multi_state_listens_ticks_guard() {
        let base = render_only_trait("W", obj(vec![("type", s("badge"))]));

        let mut multi = base.clone();
        multi.state_machine.states.push(StateDefinition {
            name: "open".to_string(),
            is_initial: None,
            is_terminal: None,
            description: None,
            time_bound_ms: None,
            holds: None,
        });
        assert!(
            render_only_template(&multi).is_none(),
            "multi-state must disqualify"
        );

        let mut listening = base.clone();
        listening.listens = Some(vec![orbital_core_base::schema::ListenDefinition {
            event: "E".to_string(),
            triggers: None,
            scope: None,
            description: None,
            synonyms: None,
            tier: None,
            payload_mapping: None,
            guard: None,
            source: None,
            event_id: None,
            triggers_id: None,
        }]);
        assert!(
            render_only_template(&listening).is_none(),
            "listens must disqualify"
        );

        let mut guarded = base.clone();
        guarded.state_machine.transitions[0].guard = Some(s("@entity.x"));
        assert!(
            render_only_template(&guarded).is_none(),
            "guard must disqualify"
        );

        let mut non_render = base;
        non_render.state_machine.transitions[0].effects = Some(vec![arr(vec![
            s("set"),
            s("@entity.x"),
            SExpression::Number(1.into()),
        ])]);
        assert!(
            render_only_template(&non_render).is_none(),
            "non-render effect must disqualify"
        );
    }

    #[test]
    fn splices_wrapper_into_lambda_and_merges_emits() {
        let mut wrapper = render_only_trait(
            "InlineW1",
            obj(vec![("type", s("badge")), ("label", s("@party.title"))]),
        );
        wrapper.emits = Some(vec![emit("ACTION", "id")]);
        let host = host_with_render("Host", lambda_render_pattern(vec![s("@trait.InlineW1")]));
        let mut orbital = make_orbital(vec![host, wrapper]);

        splice_lambda_trait_refs(&mut orbital, "orbitals[0]").expect("splice must succeed");

        let pattern = host_pattern_of(&orbital, "Host");
        let json = serde_json::to_string(&pattern).unwrap();
        assert!(
            json.contains("@party.title"),
            "expression must live in the host lambda: {json}"
        );
        assert!(
            !json.contains("@trait.InlineW1"),
            "reference must be spliced away: {json}"
        );

        // Wrapper removed; emits merged onto host.
        let names: Vec<&str> = orbital.traits.iter().filter_map(trait_ref_name).collect();
        assert_eq!(names, vec!["Host"], "consumed wrapper must be removed");
        let TraitReference::Inline(host_td) = &orbital.traits[0] else {
            panic!()
        };
        let emits = host_td.emits.as_ref().expect("merged emits");
        assert!(emits.iter().any(|e| e.event == "ACTION"));
    }

    #[test]
    fn nested_wrapper_children_splice_recursively() {
        let inner = render_only_trait("InlineInner", obj(vec![("type", s("icon"))]));
        let outer = render_only_trait(
            "InlineOuter",
            obj(vec![
                ("type", s("badge")),
                (
                    "children",
                    SExpression::Array(vec![s("@trait.InlineInner")]),
                ),
            ]),
        );
        let host = host_with_render("Host", lambda_render_pattern(vec![s("@trait.InlineOuter")]));
        let mut orbital = make_orbital(vec![host, outer, inner]);

        splice_lambda_trait_refs(&mut orbital, "orbitals[0]").expect("splice must succeed");

        let json = serde_json::to_string(&host_pattern_of(&orbital, "Host")).unwrap();
        assert!(
            !json.contains("@trait."),
            "nested refs must splice too: {json}"
        );
        assert!(
            json.contains("icon"),
            "inner template must be present: {json}"
        );
        assert_eq!(orbital.traits.len(), 1, "both wrappers consumed");
    }

    #[test]
    fn stateful_wrapper_in_lambda_errors() {
        let mut stateful = render_only_trait("Modal", obj(vec![("type", s("modal"))]));
        stateful.state_machine.states.push(StateDefinition {
            name: "open".to_string(),
            is_initial: None,
            is_terminal: None,
            description: None,
            time_bound_ms: None,
            holds: None,
        });
        let host = host_with_render("Host", lambda_render_pattern(vec![s("@trait.Modal")]));
        let mut orbital = make_orbital(vec![host, stateful]);

        let err = splice_lambda_trait_refs(&mut orbital, "orbitals[0]").unwrap_err();
        match err {
            InlineError::LambdaStatefulTrait { name, .. } => assert_eq!(name, "Modal"),
            other => panic!("expected LambdaStatefulTrait, got {other:?}"),
        }
    }

    #[test]
    fn splices_renamed_embed_via_trait_embed_id() {
        // V4 leverage-ids: the embedded declaration was renamed
        // `Card` -> `ProductCard` (its id stayed stable), but the host's
        // lambda token still says `@trait.Card`. The host's `trait_embed_ids`
        // maps the stale token `Card` -> the sibling's id, so the splice must
        // resolve the token by id and find the RENAMED sibling — a token that
        // matches NO sibling by name.
        let mut wrapper = render_only_trait(
            "ProductCard",
            obj(vec![("type", s("badge")), ("label", s("@party.title"))]),
        );
        wrapper.id = Some("trait-card".to_string());
        let mut host = host_with_render("Host", lambda_render_pattern(vec![s("@trait.Card")]));
        host.trait_embed_ids = Some(orbital_core_base::collections::IndexMap::from([(
            "Card".to_string(),
            "trait-card".to_string(),
        )]));
        let mut orbital = make_orbital(vec![host, wrapper]);

        splice_lambda_trait_refs(&mut orbital, "orbitals[0]").expect("splice must succeed");

        let json = serde_json::to_string(&host_pattern_of(&orbital, "Host")).unwrap();
        assert!(
            json.contains("@party.title"),
            "renamed embed template must splice in via id: {json}"
        );
        assert!(
            !json.contains("@trait.Card"),
            "stale token must resolve away via id: {json}"
        );
        let names: Vec<&str> = orbital.traits.iter().filter_map(trait_ref_name).collect();
        assert_eq!(names, vec!["Host"], "renamed wrapper must be consumed");
    }

    #[test]
    fn embed_without_side_map_falls_back_to_name() {
        // No `trait_embed_ids` on the host: a token whose name matches the
        // sibling name still splices via the unchanged name path.
        let wrapper = render_only_trait("InlineW1", obj(vec![("type", s("badge"))]));
        let host = host_with_render("Host", lambda_render_pattern(vec![s("@trait.InlineW1")]));
        assert!(host.trait_embed_ids.is_none(), "fixture has no side-map");
        let mut orbital = make_orbital(vec![host, wrapper]);

        splice_lambda_trait_refs(&mut orbital, "orbitals[0]").expect("splice must succeed");

        let json = serde_json::to_string(&host_pattern_of(&orbital, "Host")).unwrap();
        assert!(
            !json.contains("@trait.InlineW1"),
            "name-path splice must still fire without a side-map: {json}"
        );
        assert!(json.contains("badge"), "template must be present: {json}");
    }

    #[test]
    fn outside_lambda_reference_is_untouched() {
        let wrapper = render_only_trait("InlineW1", obj(vec![("type", s("badge"))]));
        let host = host_with_render(
            "Host",
            obj(vec![
                ("type", s("stack")),
                ("children", SExpression::Array(vec![s("@trait.InlineW1")])),
            ]),
        );
        let mut orbital = make_orbital(vec![host, wrapper]);

        splice_lambda_trait_refs(&mut orbital, "orbitals[0]").expect("splice must succeed");

        let json = serde_json::to_string(&host_pattern_of(&orbital, "Host")).unwrap();
        assert!(
            json.contains("@trait.InlineW1"),
            "non-lambda positions keep reference semantics: {json}"
        );
        assert_eq!(orbital.traits.len(), 2, "wrapper must not be removed");
    }

    #[test]
    fn emit_merge_conflict_is_rejected() {
        let mut wrapper = render_only_trait("InlineW1", obj(vec![("type", s("badge"))]));
        wrapper.emits = Some(vec![emit("ACTION", "id")]);
        let mut host = host_with_render("Host", lambda_render_pattern(vec![s("@trait.InlineW1")]));
        host.emits = Some(vec![emit("ACTION", "differentField")]);
        let mut orbital = make_orbital(vec![host, wrapper]);

        let err = splice_lambda_trait_refs(&mut orbital, "orbitals[0]").unwrap_err();
        match err {
            InlineError::EmitMergeConflict { event, host, .. } => {
                assert_eq!(event, "ACTION");
                assert_eq!(host, "Host");
            }
            other => panic!("expected EmitMergeConflict, got {other:?}"),
        }
    }

    #[test]
    fn wrapper_cycle_terminates_without_splice_loop() {
        let a = render_only_trait(
            "CycleA",
            obj(vec![
                ("type", s("stack")),
                ("children", SExpression::Array(vec![s("@trait.CycleB")])),
            ]),
        );
        let b = render_only_trait(
            "CycleB",
            obj(vec![
                ("type", s("stack")),
                ("children", SExpression::Array(vec![s("@trait.CycleA")])),
            ]),
        );
        let host = host_with_render("Host", lambda_render_pattern(vec![s("@trait.CycleA")]));
        let mut orbital = make_orbital(vec![host, a, b]);

        // Must terminate; the cycle edge stays a reference for the validator.
        splice_lambda_trait_refs(&mut orbital, "orbitals[0]").expect("cycle must not loop");
    }
}
