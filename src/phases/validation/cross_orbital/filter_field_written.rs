//! ORB_X_FILTER_FIELD_NEVER_WRITTEN (error).
//!
//! A `fetch`'s `filter` compares a LITERAL `(object/get @entity <field>)`
//! against a value, but no `persist create`/`update` anywhere in the schema
//! ever supplies that field for the fetched entity (or a `persistent:`-
//! collection-mate entity sharing its table) — the filter can never match a
//! real row, so the fetch always returns empty. Cross-orbital because the
//! persistor that supplies a field routinely lives in a sibling orbital (the
//! modal + persistor idiom, `@almadar/core/access/entityAccess.ts`'s
//! collection-inheritance convention).
//!
//! Ported from the JS wiring lint's `filter-field-never-written`
//! (`wiring-lint.ts:1728-1757` + `fetchFiltersOf`/`objectGetEntityFields`/
//! the schema-wide writer index at `wiring-lint.ts:1017-1090`), with ONE
//! change (F13, `docs/Almadar_Verifier_Efficacy_Findings_2026-09-12.md`): a
//! declared default on the field exempts it ONLY when the filter's
//! comparand, folded under the trait's resolved config with the core
//! `Evaluator`, is a literal EQUAL to that default. The JS predicate
//! exempted on `default !== undefined` alone, silent even when the fold
//! proves the default is unreachable (`channelKey: string = ""` vs. a filter
//! comparand that folds to `"root"`). A comparand that does not fold to a
//! literal (touches `@entity`/`@payload`/`@user`/`@state`/`?field`) stays
//! silent — undecidable is not a violation.
//!
//! An entity (and its collection-mates) that NO `persist create` ever
//! produces has its rows made outside the program — a `[identity]` roster, a
//! seed, an import, a sibling program sharing the table, ingested telemetry,
//! a substrate template entity awaiting a rebind — so "no persist supplies
//! this field" proves nothing about its rows and the rule stays silent for
//! any DECLARED field of it (the `[identity]` exemption is the same fact
//! declared by tag; a program that only `update`s such rows still did not
//! make them). A filter on an UNDECLARED column still fires: rows from
//! outside the program do not carry columns the entity does not have.
//!
//! Declared-contract comparison only: literal `(object/get @entity <f>)`
//! field names, `persist create`/`update` data-literal keys, `[identity]`/
//! `@intrinsic` exemptions, and `persistent:` collection membership. No
//! name/text matching beyond that literal grammar.

use crate::phases::embed_graph::client_bound_traits;
use crate::phases::validation::user_identity::identity_entity_names;
use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use {orbital_core_eval::evaluator::{BindingContext, Evaluator}, orbital_core_base::value::Value};
use orbital_core_base::schema::{
    EntityDefinition, EntityRef, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// Every `["fetch", entityName, { filter, ... }]` occurrence anywhere in
/// `effects` (including nested inside `if`/`do`) that carries a `filter`
/// key, paired with the entity name and the filter S-expression. Mirrors
/// `fetchFiltersOf`.
fn fetch_filters_of(effects: &[SExpression]) -> Vec<(String, SExpression)> {
    fn scan(node: &SExpression, out: &mut Vec<(String, SExpression)>) {
        let SExpression::Array(items) = node else {
            return;
        };
        if let (Some(SExpression::String(op)), Some(SExpression::String(entity))) =
            (items.first(), items.get(1))
        {
            if op == "fetch" {
                if let Some(SExpression::Object(opts)) = items.get(2) {
                    if let Some(filter) = opts.get("filter") {
                        out.push((entity.clone(), filter.clone()));
                    }
                }
                return;
            }
        }
        for item in items {
            scan(item, out);
        }
    }
    let mut out = Vec::new();
    for effect in effects {
        scan(effect, &mut out);
    }
    out
}

/// Every LITERAL field name referenced as `(object/get @entity <f>)`
/// anywhere in `filter`. `<f>` must be a literal: an `@`/`?`-prefixed
/// dynamic selector is itself unresolved data, not a field this predicate
/// can reason about. Mirrors `objectGetEntityFields`.
fn object_get_entity_fields(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::Array(items) => {
            if let (
                Some(SExpression::String(op)),
                Some(SExpression::String(root)),
                Some(SExpression::String(field)),
            ) = (items.first(), items.get(1), items.get(2))
            {
                if items.len() == 3
                    && op == "object/get"
                    && root == "@entity"
                    && !field.is_empty()
                    && !field.starts_with('@')
                    && !field.starts_with('?')
                {
                    out.insert(field.clone());
                    return;
                }
            }
            for item in items {
                object_get_entity_fields(item, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                object_get_entity_fields(v, out);
            }
        }
        _ => {}
    }
}

/// Every comparand paired with a literal `(object/get @entity <field>)` via
/// a top-level `["=", A, B]` equality node anywhere in `filter` (through
/// `and`/`or`/`not` nesting). The only comparison shape this fold applies
/// to — any other relational operator (`in`, `!=`, `>`, …) is out of scope
/// for the fold (undecidable, not a violation; `object_get_entity_fields`
/// still finds the field name, this only feeds the F13 default-value fold).
fn object_get_equality_comparands(filter: &SExpression, field: &str, out: &mut Vec<SExpression>) {
    fn is_object_get_of(expr: &SExpression, field: &str) -> bool {
        matches!(expr, SExpression::Array(items) if items.len() == 3
            && matches!(&items[0], SExpression::String(op) if op == "object/get")
            && matches!(&items[1], SExpression::String(root) if root == "@entity")
            && matches!(&items[2], SExpression::String(f) if f == field))
    }
    if let SExpression::Array(items) = filter {
        if items.len() == 3 && matches!(&items[0], SExpression::String(op) if op == "=") {
            if is_object_get_of(&items[1], field) {
                out.push(items[2].clone());
            } else if is_object_get_of(&items[2], field) {
                out.push(items[1].clone());
            }
        }
        for item in items {
            object_get_equality_comparands(item, field, out);
        }
    }
}

/// True when `expr` reaches for a value only known at runtime (`@entity`,
/// `@payload`, `@user`, `@state`, or the `?field` payload shorthand) —
/// undecidable at validate time, so the caller stays quiet rather than
/// guess.
fn references_runtime_binding(expr: &SExpression) -> bool {
    match expr {
        SExpression::String(s) => {
            s.starts_with("@entity")
                || s.starts_with("@payload")
                || s.starts_with("@user")
                || s.starts_with("@state")
                || s.starts_with('?')
        }
        SExpression::Array(items) => items.iter().any(references_runtime_binding),
        SExpression::Object(map) => map.values().any(references_runtime_binding),
        _ => false,
    }
}

/// Fold `comparand` to a literal [`Value`] under `trait_def`'s own resolved
/// config defaults (post-inline, so call-site overrides already carry
/// through — the same folding the validator already does for guard/knob
/// defaults) — `None` when the comparand touches a runtime-only binding or
/// fails to evaluate.
fn fold_comparand_literal(comparand: &SExpression, trait_def: &TraitDefinition) -> Option<Value> {
    if references_runtime_binding(comparand) {
        return None;
    }
    let mut ctx = BindingContext::new();
    if let Some(config) = &trait_def.config {
        let mut map = orbital_core_base::value::ValueMap::new();
        for (key, field) in config {
            if let Some(default) = &field.default {
                map.insert(key.clone(), Value::from(default.clone()));
            }
        }
        ctx.set_config(Value::Object(map));
    }
    Evaluator::new().evaluate(comparand, &ctx).ok()
}

/// Record one persist write (`create`/`update`) of `entity` with data argument `data`.
fn record_persist_write(
    action: &str,
    entity: &str,
    data: Option<&SExpression>,
    entity_defs: &HashMap<&str, &EntityDefinition>,
    written: &mut HashMap<String, HashSet<String>>,
    any_data: &mut HashSet<String>,
    created: &mut HashSet<String>,
) {
    if action == "create" || action == "upsert" {
        created.insert(entity.to_string());
    }
    match data {
        Some(SExpression::Object(data)) => {
            written
                .entry(entity.to_string())
                .or_default()
                .extend(data.keys().cloned());
        }
        Some(_) => match entity_defs.get(entity) {
            Some(def) => {
                written
                    .entry(entity.to_string())
                    .or_default()
                    .extend(def.fields.iter().filter_map(|f| f.name.clone()));
            }
            None => {
                any_data.insert(entity.to_string());
            }
        },
        None => {}
    }
}

/// Walk `expr` for every `(persist create|update <entity> <data>)` node
/// (through `if`/`do`), recording which fields each call's data LITERAL
/// supplies. A bare/computed data arg supplies the columns the written
/// entity declares — rows carry no columns their entity does not have — and
/// only an entity this schema does not define may supply anything.
fn collect_persist_writes(
    expr: &SExpression,
    entity_defs: &HashMap<&str, &EntityDefinition>,
    written: &mut HashMap<String, HashSet<String>>,
    any_data: &mut HashSet<String>,
    created: &mut HashSet<String>,
) {
    match expr {
        SExpression::Array(items) => {
            if let (Some(SExpression::String(op)), Some(SExpression::String(action))) =
                (items.first(), items.get(1))
            {
                if op == "persist" && action == "batch" {
                    if let Some(ops) = items.get(2) {
                        for w in orbital_core_base::persist_batch::batch_writes(ops) {
                            if w.action != "delete" {
                                record_persist_write(
                                    w.action,
                                    w.entity,
                                    w.data,
                                    entity_defs,
                                    written,
                                    any_data,
                                    created,
                                );
                            }
                        }
                    }
                } else if let (Some(SExpression::String(entity)), true) = (
                    items.get(2),
                    op == "persist" && (action == "create" || action == "upsert" || action == "update"),
                ) {
                    record_persist_write(
                        action,
                        entity,
                        items.get(3),
                        entity_defs,
                        written,
                        any_data,
                        created,
                    );
                }
            }
            for item in items {
                collect_persist_writes(item, entity_defs, written, any_data, created);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_persist_writes(v, entity_defs, written, any_data, created);
            }
        }
        _ => {}
    }
}

/// True when SOME persist for `entity`, or for another entity sharing its
/// `persistent:` collection, supplies `field` — directly, or via a bare
/// data argument (may supply anything). Mirrors `fieldSuppliedForEntity`.
fn field_supplied_for_entity(
    entity: &str,
    field: &str,
    written: &HashMap<String, HashSet<String>>,
    any_data: &HashSet<String>,
    collection_of: &HashMap<&str, &str>,
    entities_by_collection: &HashMap<&str, Vec<&str>>,
) -> bool {
    if any_data.contains(entity) {
        return true;
    }
    if written.get(entity).is_some_and(|s| s.contains(field)) {
        return true;
    }
    let Some(collection) = collection_of.get(entity) else {
        return false;
    };
    entities_by_collection
        .get(collection)
        .into_iter()
        .flatten()
        .any(|mate| {
            *mate != entity
                && (any_data.contains(*mate)
                    || written.get(*mate).is_some_and(|s| s.contains(field)))
        })
}

/// True when SOME `persist create` targets `entity` or a `persistent:`
/// collection-mate — the program makes rows of this table.
fn entity_has_any_creator(
    entity: &str,
    created: &HashSet<String>,
    collection_of: &HashMap<&str, &str>,
    entities_by_collection: &HashMap<&str, Vec<&str>>,
) -> bool {
    if created.contains(entity) {
        return true;
    }
    let Some(collection) = collection_of.get(entity) else {
        return false;
    };
    entities_by_collection
        .get(collection)
        .into_iter()
        .flatten()
        .any(|mate| created.contains(*mate))
}

pub(crate) fn validate_filter_field_never_written(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();

    let identity_names: HashSet<&str> = identity_entity_names(schema).into_iter().collect();

    // entity name -> definition, collection, and collection-mates, across
    // every orbital's own entity + auxiliary entities.
    let mut entity_defs: HashMap<&str, &EntityDefinition> = HashMap::new();
    let mut collection_of: HashMap<&str, &str> = HashMap::new();
    let mut entities_by_collection: HashMap<&str, Vec<&str>> = HashMap::new();
    for orbital in &schema.orbitals {
        for er in
            std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten())
        {
            let EntityRef::Inline(def) = er else {
                continue;
            };
            entity_defs.insert(def.name.as_str(), def);
            if let Some(collection) = &def.collection {
                collection_of.insert(def.name.as_str(), collection.as_str());
                entities_by_collection
                    .entry(collection.as_str())
                    .or_default()
                    .push(def.name.as_str());
            }
        }
    }

    // Schema-wide persist-write index.
    let mut written_fields: HashMap<String, HashSet<String>> = HashMap::new();
    let mut any_data_entities: HashSet<String> = HashSet::new();
    let mut created_entities: HashSet<String> = HashSet::new();
    for orbital in &schema.orbitals {
        for tr in &orbital.traits {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            for t in &td.state_machine.transitions {
                for effect in t.effects.iter().flatten() {
                    collect_persist_writes(
                        effect,
                        &entity_defs,
                        &mut written_fields,
                        &mut any_data_entities,
                        &mut created_entities,
                    );
                }
            }
        }
    }

    for orbital in &schema.orbitals {
        let bound = client_bound_traits(orbital);
        for tr in &orbital.traits {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            if !bound.contains(&td.name) {
                continue;
            }
            for (ti, transition) in td.state_machine.transitions.iter().enumerate() {
                let Some(effects) = &transition.effects else {
                    continue;
                };
                for (fetched_entity, filter) in fetch_filters_of(effects) {
                    if identity_names.contains(fetched_entity.as_str()) {
                        continue; // [identity] roster — rows come from outside the program
                    }
                    let Some(entity_def) = entity_defs.get(fetched_entity.as_str()) else {
                        continue; // unresolved entity — cannot prove absence, stay quiet
                    };
                    let has_creator = entity_has_any_creator(
                        &fetched_entity,
                        &created_entities,
                        &collection_of,
                        &entities_by_collection,
                    );
                    let mut fields = HashSet::new();
                    object_get_entity_fields(&filter, &mut fields);
                    for field in &fields {
                        if field_supplied_for_entity(
                            &fetched_entity,
                            field,
                            &written_fields,
                            &any_data_entities,
                            &collection_of,
                            &entities_by_collection,
                        ) {
                            continue;
                        }
                        let declared = entity_def
                            .fields
                            .iter()
                            .find(|f| f.name.as_deref() == Some(field.as_str()));
                        if declared.is_some_and(|f| f.intrinsic == Some(true)) {
                            continue;
                        }
                        if declared.is_some() && !has_creator {
                            continue; // rows are made outside the program — nothing to prove
                        }
                        if let Some(declared_field) = declared {
                            if let Some(default) = &declared_field.default {
                                let mut comparands = Vec::new();
                                object_get_equality_comparands(&filter, field, &mut comparands);
                                if comparands.is_empty() {
                                    // No provable equality shape to fold against — stay
                                    // quiet rather than guess (mirrors the JS's original
                                    // any-default exemption for shapes this port can't fold).
                                    continue;
                                }
                                let default_literal = Value::from(default.clone());
                                let folded: Vec<Option<Value>> = comparands
                                    .iter()
                                    .map(|c| fold_comparand_literal(c, td))
                                    .collect();
                                if folded.iter().any(|f| f.is_none()) {
                                    continue; // an unfoldable branch might supply it — undecidable
                                }
                                if folded.iter().any(|f| f.as_ref() == Some(&default_literal)) {
                                    continue; // default is reachable through some branch — proven safe
                                }
                                // Every branch folds to a literal, none equal the
                                // default — F13: the default is provably unreachable.
                            }
                        }

                        let path = format!(
                            "orbitals.{}.traits.{}.stateMachine.transitions[{}].effects",
                            orbital.name, td.name, ti
                        );
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::FilterFieldNeverWritten,
                                path,
                                format!(
                                    "{} fetches {} filtered on `{}`, but no `persist create`/`update` of {} anywhere in the schema supplies `{}`{} — the filter can never match a real row, so the fetch always returns empty",
                                    td.name,
                                    fetched_entity,
                                    field,
                                    fetched_entity,
                                    field,
                                    if declared.is_none() { " (not even a declared field)" } else { "" },
                                ),
                            )
                            .with_suggestion(format!(
                                "Supply `{field}` in the `persist create`/`update {fetched_entity}` data that produces the rows this filter targets, or drop `{field}` from the filter if it is dead."
                            )),
                        );
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{
        ConfigField, FieldDefinition, FieldType, Orbital, PageDefinition, PageRef, StateMachine,
        StringOrArray,
    };
    use serde_json::json;
    use std::collections::HashMap as Map;

    fn effects_from(json_val: serde_json::Value) -> Vec<SExpression> {
        serde_json::from_value(json_val).expect("effects fixture deserializes")
    }

    fn transition_with(
        from: &str,
        to: &str,
        event: &str,
        effects: Vec<SExpression>,
    ) -> orbital_core_base::schema::TransitionDefinition {
        orbital_core_base::schema::TransitionDefinition {
            event_id: None,
            from: StringOrArray::Single(from.to_string()),
            to: to.to_string(),
            event: event.to_string(),
            guard: None,
            effects: Some(effects),
            ensures: None,
        }
    }

    fn trait_def_with(
        name: &str,
        config: Option<Map<String, ConfigField>>,
        transitions: Vec<orbital_core_base::schema::TransitionDefinition>,
    ) -> TraitReference {
        TraitReference::Inline(TraitDefinition {
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
            linked_entity: None,
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions,
            },
            ticks: None,
            config,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        })
    }

    fn field(name: &str, default: Option<SExpression>) -> FieldDefinition {
        FieldDefinition {
            mock: None,
            name: Some(name.to_string()),
            field_type: FieldType::String,
            required: None,
            primary_key: None,
            unique: None,
            default,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
            merged_from: None,
            projected_from: None,
        }
    }

    fn entity_def(
        name: &str,
        collection: Option<&str>,
        fields: Vec<FieldDefinition>,
    ) -> EntityDefinition {
        EntityDefinition {
            timestamps: None,
            soft_delete: None,
            description: None,
            visual_prompt: None,
            asset_ref: None,
            store: None,
            name: name.to_string(),
            id: None,
            collection: collection.map(String::from),
            persistence: None,
            fields,
            instances: None,
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
            invariants: None,
        }
    }

    /// A single page seeding every inline trait — client-bound-trait gating
    /// is exercised on its own in `embed_graph.rs`'s tests, so fixtures here
    /// just need every trait reachable, not a narrow page-declared subset.
    fn orbital_with(
        entity: EntityRef,
        auxiliary_entities: Option<Vec<EntityRef>>,
        traits: Vec<TraitReference>,
    ) -> Orbital {
        let page_traits: Vec<TraitReference> = traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(td) => Some(TraitReference::Reference {
                    local: false,
                    reference: Some(td.name.clone()),
                    ref_id: None,
                    id: None,
                    name: None,
                    linked_entity: None,
                    linked_entity_id: None,
                    config: None,
                    events: None,
                    event_ids: None,
                    fields: None,
                    listens: None,
                    emits_scope: None,
                    type_args: None,
                }),
                _ => None,
            })
            .collect();
        Orbital {
            spec: false,
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity,
            auxiliary_entities,
            traits,
            pages: Some(vec![PageRef::Inline(PageDefinition {
                translation_of: None,
                source_page: None,
                access: None,
                indexing: None,
                title: None,
                description: None,
                id: None,
                name: "Home".to_string(),
                path: "/".to_string(),
                view_type: None,
                primary_entity: None,
                is_initial: None,
                traits: Some(page_traits),
                sections: None,
                icon: None,
                label: None,
                roles: None,
            })]),
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

    fn schema_with(orbitals: Vec<Orbital>) -> OrbitalSchema {
        OrbitalSchema {
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals,
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

    /// F13: the program writes ThreadPost rows (so it owns them) but never
    /// stamps `channelKey`; the declared default `""` never equals the
    /// folded comparand `"root"`, so the filter is provably dead.
    #[test]
    fn hit_when_literal_comparand_differs_from_default_and_the_writer_omits_it() {
        let entity = entity_def(
            "ThreadPost",
            Some("threadposts"),
            vec![
                field("channelKey", Some(SExpression::String(String::new()))),
                field("content", None),
            ],
        );
        let fetch = effects_from(json!([
            ["fetch", "ThreadPost", { "filter": ["=", ["object/get", "@entity", "channelKey"], "root"] }]
        ]));
        let create = effects_from(json!([
            ["persist", "create", "ThreadPost", { "content": "hi" }]
        ]));
        let trait_ref = trait_def_with(
            "Browse",
            None,
            vec![
                transition_with("loading", "loading", "INIT", fetch),
                transition_with("loading", "loading", "CREATE", create),
            ],
        );
        let schema = schema_with(vec![orbital_with(
            EntityRef::Inline(entity),
            None,
            vec![trait_ref],
        )]);

        let result = validate_filter_field_never_written(&schema);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::FilterFieldNeverWritten),
            "{:?}",
            result.errors
        );
    }

    /// F13 fix: the declared default (`""`) is silent ONLY because the
    /// filter's comparand folds to that SAME literal — not merely because a
    /// default exists.
    #[test]
    fn silent_when_the_comparand_folds_to_the_same_literal_as_the_default() {
        let entity = entity_def(
            "ThreadPost",
            Some("threadposts"),
            vec![field(
                "channelKey",
                Some(SExpression::String(String::new())),
            )],
        );
        let fetch = effects_from(json!([
            ["fetch", "ThreadPost", { "filter": ["=", ["object/get", "@entity", "channelKey"], ""] }]
        ]));
        let trait_ref = trait_def_with(
            "Browse",
            None,
            vec![transition_with("loading", "loading", "INIT", fetch)],
        );
        let schema = schema_with(vec![orbital_with(
            EntityRef::Inline(entity),
            None,
            vec![trait_ref],
        )]);

        let result = validate_filter_field_never_written(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// A comparand bound to `@payload.*` cannot be folded at validate time —
    /// undecidable stays silent rather than guessing, even with a declared
    /// default present and no writer anywhere.
    #[test]
    fn silent_when_the_comparand_is_unfoldable() {
        let entity = entity_def(
            "ThreadPost",
            Some("threadposts"),
            vec![field(
                "channelKey",
                Some(SExpression::String(String::new())),
            )],
        );
        let fetch = effects_from(json!([
            ["fetch", "ThreadPost", { "filter": ["=", ["object/get", "@entity", "channelKey"], "@payload.channelKey"] }]
        ]));
        let trait_ref = trait_def_with(
            "Browse",
            None,
            vec![transition_with("loading", "loading", "INIT", fetch)],
        );
        let schema = schema_with(vec![orbital_with(
            EntityRef::Inline(entity),
            None,
            vec![trait_ref],
        )]);

        let result = validate_filter_field_never_written(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// An entity no persist ever creates has its rows made outside the
    /// program (seed / import / sibling app / template awaiting rebind) —
    /// a declared filter column stays silent, even when the program
    /// `update`s some other column of those rows.
    #[test]
    fn silent_when_nothing_creates_the_entity_and_the_column_is_declared() {
        let entity = entity_def(
            "Product",
            Some("products"),
            vec![field("title", None), field("status", None)],
        );
        let fetch = effects_from(json!([
            ["fetch", "Product", { "filter": ["str/includes", ["object/get", "@entity", "title"], "@payload.term"] }]
        ]));
        let update = effects_from(json!([
            ["persist", "update", "Product", { "id": "@payload.id", "status": "archived" }]
        ]));
        let trait_ref = trait_def_with(
            "Browse",
            None,
            vec![
                transition_with("loading", "loading", "INIT", fetch),
                transition_with("loading", "loading", "ARCHIVE", update),
            ],
        );
        let schema = schema_with(vec![orbital_with(
            EntityRef::Inline(entity),
            None,
            vec![trait_ref],
        )]);

        let result = validate_filter_field_never_written(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// Rows from outside the program still do not carry columns the entity
    /// does not declare — an undeclared filter column fires regardless.
    #[test]
    fn hit_when_the_filter_column_is_undeclared_even_with_no_creator() {
        let entity = entity_def("Product", Some("products"), vec![field("title", None)]);
        let fetch = effects_from(json!([
            ["fetch", "Product", { "filter": ["str/includes", ["object/get", "@entity", "name"], "@payload.term"] }]
        ]));
        let trait_ref = trait_def_with(
            "Browse",
            None,
            vec![transition_with("loading", "loading", "INIT", fetch)],
        );
        let schema = schema_with(vec![orbital_with(
            EntityRef::Inline(entity),
            None,
            vec![trait_ref],
        )]);

        let result = validate_filter_field_never_written(&schema);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::FilterFieldNeverWritten
                    && e.message.contains("not even a declared field")),
            "{:?}",
            result.errors
        );
    }

    /// The program CREATES the entity's rows but never supplies this
    /// column — the original defect, still a hit.
    #[test]
    fn hit_when_a_creator_exists_but_never_supplies_the_declared_column() {
        let entity = entity_def(
            "Product",
            Some("products"),
            vec![field("title", None), field("sku", None)],
        );
        let fetch = effects_from(json!([
            ["fetch", "Product", { "filter": ["str/includes", ["object/get", "@entity", "title"], "@payload.term"] }]
        ]));
        let create = effects_from(json!([
            ["persist", "create", "Product", { "sku": "A-1" }]
        ]));
        let trait_ref = trait_def_with(
            "Browse",
            None,
            vec![
                transition_with("loading", "loading", "INIT", fetch),
                transition_with("loading", "loading", "CREATE", create),
            ],
        );
        let schema = schema_with(vec![orbital_with(
            EntityRef::Inline(entity),
            None,
            vec![trait_ref],
        )]);

        let result = validate_filter_field_never_written(&schema);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::FilterFieldNeverWritten),
            "{:?}",
            result.errors
        );
    }

    /// `HubArticle` and `Article` share the `articles` collection — a
    /// persist under `Article`'s name supplies `title` for `HubArticle`'s
    /// filter too (the "view entity" idiom).
    #[test]
    fn silent_when_a_persistent_collection_mate_writes_the_field() {
        let hub_article = entity_def("HubArticle", Some("articles"), vec![field("title", None)]);
        let article = entity_def("Article", Some("articles"), vec![field("title", None)]);

        let fetch = effects_from(json!([
            ["fetch", "HubArticle", { "filter": ["=", ["object/get", "@entity", "title"], "Foo"] }]
        ]));
        let create = effects_from(json!([
            ["persist", "create", "Article", { "title": "Foo" }]
        ]));
        let trait_ref = trait_def_with(
            "Browse",
            None,
            vec![
                transition_with("loading", "loading", "INIT", fetch),
                transition_with("loading", "loading", "CREATE", create),
            ],
        );
        let schema = schema_with(vec![orbital_with(
            EntityRef::Inline(hub_article),
            Some(vec![EntityRef::Inline(article)]),
            vec![trait_ref],
        )]);

        let result = validate_filter_field_never_written(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    fn pipeline_schema(
        mate_fields: Vec<FieldDefinition>,
        own_write: Option<serde_json::Value>,
    ) -> OrbitalSchema {
        let pipeline = entity_def(
            "PipelineDeal",
            Some("deals"),
            vec![field("title", None), field("stage", None)],
        );
        let deal = entity_def("Deal", Some("deals"), mate_fields);
        let fetch = effects_from(json!([
            ["fetch", "PipelineDeal", { "filter": ["=", ["object/get", "@entity", "name"], "x"] }]
        ]));
        let mut browse = vec![transition_with("browsing", "browsing", "SEARCH", fetch)];
        if let Some(write) = own_write {
            browse.push(transition_with(
                "browsing",
                "browsing",
                "SAVE",
                effects_from(write),
            ));
        }
        let pipeline_orbital = orbital_with(
            EntityRef::Inline(pipeline),
            None,
            vec![trait_def_with("PipelineBrowse", None, browse)],
        );
        let create = effects_from(json!([
            ["persist", "create", "Deal", ["object/merge", "@payload.data", { "owner": "@user.id" }]]
        ]));
        let deal_orbital = orbital_with(
            EntityRef::Inline(deal),
            None,
            vec![trait_def_with(
                "DealPersistor",
                None,
                vec![transition_with("idle", "idle", "CREATE", create)],
            )],
        );
        schema_with(vec![pipeline_orbital, deal_orbital])
    }

    fn fires(schema: &OrbitalSchema) -> bool {
        validate_filter_field_never_written(schema)
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::FilterFieldNeverWritten)
    }

    /// G-ORB-003: a computed write to a collection-mate supplies the columns that mate
    /// declares, not any column — so a filter on a column no entity of the table
    /// declares fires on the whole program, as it does on the orbital alone.
    #[test]
    fn hit_computed_write_to_a_mate_cannot_supply_an_undeclared_column() {
        let mate = vec![
            field("title", None),
            field("stage", None),
            field("owner", None),
        ];
        assert!(fires(&pipeline_schema(mate, None)));
    }

    #[test]
    fn control_computed_write_to_a_mate_that_declares_the_column_supplies_it() {
        let mate = vec![field("title", None), field("name", None)];
        assert!(!fires(&pipeline_schema(mate, None)));
    }

    #[test]
    fn edge_computed_write_to_the_fetched_entity_itself_supplies_only_its_declared_columns() {
        let mate = vec![field("title", None)];
        let own = json!([["persist", "update", "PipelineDeal", "@payload.data"]]);
        assert!(fires(&pipeline_schema(mate, Some(own))));
    }

    #[test]
    fn edge_computed_write_to_an_entity_the_schema_does_not_define_proves_nothing() {
        let schema = pipeline_schema(vec![field("title", None)], None);
        let mut schema = schema;
        // The mate's definition is outside this program: its computed write may carry anything.
        schema.orbitals[1].entity = EntityRef::Inline(entity_def("Other", None, vec![]));
        let external = effects_from(json!([["persist", "create", "Deal", "@payload.data"]]));
        schema.orbitals[1].traits = vec![trait_def_with(
            "Ext",
            None,
            vec![transition_with("idle", "idle", "CREATE", external)],
        )];
        assert!(
            fires(&schema),
            "Deal is not defined here, so it is no collection-mate of PipelineDeal"
        );
    }
}
