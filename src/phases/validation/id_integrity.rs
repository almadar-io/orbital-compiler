//! Rabit V4 Phase 3 — dual-carry id integrity (`ORB_ID_*`).
//!
//! Four rules over the identity-keyed schema graph, all silent on legacy
//! id-free `.orb` (no id fields populated + no `schema.ledger` ⇒ nothing to
//! walk):
//!
//! - `ORB_ID_UNKNOWN_REF` (error) — a populated dual-carry id points at neither
//!   a declared arena node nor a `schema.ledger` entry.
//! - `ORB_ID_NAME_MISMATCH` (error) — a reference carries BOTH a name and its id
//!   sibling, but the ledger's `curName` for that id disagrees with the name.
//!   A `curName` in `@config.<key>` form is a config-forward alias: it is
//!   satisfied (no error) exactly when the owning trait's (`parent`) config
//!   `<key>` value equals the call-site name — both are legitimate views of
//!   the same event (authored forward vs resolved runtime name).
//! - `ORB_ID_KIND_MISMATCH` (error) — an id's prefix kind (`id_kind_of`)
//!   disagrees with the position it occupies.
//! - `ORB_ID_LEDGER_ORPHAN` (error) — a ledger row no node carries (stale).
//!
//! One id-index pass over the declarations builds the arena (`id → kind + name`
//! via declared nodes ∪ ledger); a single reference walk then applies rules 1–3
//! and records every id it sees; rule 4 diffs the ledger against that seen set.
//! An event ledger row is exempt from orphan detection while its owning trait id
//! (`parent`) is still live — event ids have no id-bearing declaration node, so
//! the parent trait is their carrier.

use orbital_core::identity::{id_kind_of, IdKind};
use orbital_core::schema::{
    EntityDefinition, EntityRef, FieldDefinition, ListenSource, Orbital, OrbitalSchema, PageRef,
    SExpression, TraitReference,
};
use std::collections::{HashMap, HashSet};

use super::error::{ErrorCode, ValidationError};
use super::{ValidationContext, ValidationResult};

/// Arena of declared node ids (`id → (kind, name)`) plus the ledger, shared by
/// all four rules.
struct IdIndex<'a> {
    ledger: Option<&'a orbital_core::identity::IdentityLedger>,
    declared: HashMap<String, (IdKind, String)>,
    trait_ids: HashSet<String>,
    /// Per-trait string config values (`trait id → key → value`), from
    /// `ConfigField.default` — the resolution source for `@config.<key>`
    /// event forwards.
    config_values: HashMap<String, HashMap<String, String>>,
}

impl<'a> IdIndex<'a> {
    fn build(schema: &'a OrbitalSchema) -> Self {
        let mut declared: HashMap<String, (IdKind, String)> = HashMap::new();
        let mut trait_ids: HashSet<String> = HashSet::new();
        let mut config_values: HashMap<String, HashMap<String, String>> = HashMap::new();

        for orbital in &schema.orbitals {
            if let Some(id) = &orbital.id {
                declared.insert(id.clone(), (IdKind::Orbital, orbital.name.clone()));
            }
            declare_entity(orbital.entity.as_inline(), &mut declared);
            for aux in orbital.auxiliary_entities.iter().flatten() {
                declare_entity(aux.as_inline(), &mut declared);
            }
            for tr in &orbital.traits {
                declare_trait(tr, &mut declared, &mut trait_ids, &mut config_values);
            }
            for page in orbital.pages.iter().flatten() {
                if let PageRef::Inline(p) = page {
                    if let Some(id) = &p.id {
                        declared.insert(id.clone(), (IdKind::Page, p.name.clone()));
                    }
                    for tr in p.traits.iter().flatten() {
                        declare_trait(tr, &mut declared, &mut trait_ids, &mut config_values);
                    }
                }
            }
        }

        IdIndex {
            ledger: schema.ledger.as_ref(),
            declared,
            trait_ids,
            config_values,
        }
    }

    /// An id is known if it is a declared arena node or a ledger entry.
    fn known(&self, id: &str) -> bool {
        self.declared.contains_key(id) || self.ledger.is_some_and(|l| l.entries.contains_key(id))
    }

    fn cur_name(&self, id: &str) -> Option<&str> {
        self.ledger.and_then(|l| l.cur_name(id))
    }

    /// Rule-2 alias: a ledger `curName` in `@config.<key>` form is a
    /// config-forward — the authored view of an event whose call sites carry
    /// the RESOLVED name. Accepted exactly when the owning trait's (ledger
    /// `parent`) config `<key>` value equals the call-site name; anything
    /// else is still a genuine drift.
    fn config_forward_matches(&self, id: &str, cur: &str, name: &str) -> bool {
        let Some(key) = cur.strip_prefix("@config.") else {
            return false;
        };
        let Some(parent) = self
            .ledger
            .and_then(|l| l.entries.get(id))
            .and_then(|e| e.parent.as_deref())
        else {
            return false;
        };
        self.config_values
            .get(parent)
            .and_then(|cfg| cfg.get(key))
            .is_some_and(|v| v == name)
    }
}

fn declare_entity(
    entity: Option<&EntityDefinition>,
    declared: &mut HashMap<String, (IdKind, String)>,
) {
    let Some(entity) = entity else { return };
    if let Some(id) = &entity.id {
        declared.insert(id.clone(), (IdKind::Entity, entity.name.clone()));
    }
}

fn declare_trait(
    tr: &TraitReference,
    declared: &mut HashMap<String, (IdKind, String)>,
    trait_ids: &mut HashSet<String>,
    config_values: &mut HashMap<String, HashMap<String, String>>,
) {
    let TraitReference::Inline(td) = tr else {
        return;
    };
    if let Some(id) = &td.id {
        declared.insert(id.clone(), (IdKind::Trait, td.name.clone()));
        trait_ids.insert(id.clone());
        if let Some(config) = &td.config {
            let values: HashMap<String, String> = config
                .iter()
                .filter_map(|(k, f)| match &f.default {
                    Some(SExpression::String(v)) => Some((k.clone(), v.clone())),
                    _ => None,
                })
                .collect();
            if !values.is_empty() {
                config_values.insert(id.clone(), values);
            }
        }
    }
    // Emits are the id-bearing declaration site for events.
    for emit in td.emits.iter().flatten() {
        if let Some(id) = &emit.event_id {
            declared.insert(id.clone(), (IdKind::Event, emit.event.clone()));
        }
    }
    for de in td.data_entities.iter().flatten() {
        if let Some(id) = &de.id {
            declared.insert(id.clone(), (IdKind::Entity, de.name.clone()));
        }
    }
}

/// Collect bare `@trait.<Name>` embed tokens from an S-expression tree — the
/// same token shape `orbital_core::stamp` scans an INLINE trait's whole body
/// for at stamp time (render-ui `children`, `renderItem`/fn-form lambda bodies,
/// config defaults alike, no lambda special-casing needed since the walk is
/// unconditional). Stamp only builds that `traitEmbedIds` id side-map for
/// Inline traits, so a Reference trait's `config` carries these as bare names
/// with no id sibling — this walker recovers them for name-based resolution.
fn collect_trait_embed_names(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@trait.") {
                let name = rest.split(['.', '[']).next().unwrap_or(rest);
                if !name.is_empty() {
                    out.insert(name.to_string());
                }
            }
        }
        SExpression::Array(items) => items.iter().for_each(|i| collect_trait_embed_names(i, out)),
        SExpression::Object(map) => map.values().for_each(|v| collect_trait_embed_names(v, out)),
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
    }
}

/// Applies rules 1–3 at each reference position and records every id it sees.
struct Checker<'a> {
    index: &'a IdIndex<'a>,
    result: &'a mut ValidationResult,
    seen: &'a mut HashSet<String>,
}

impl Checker<'_> {
    /// Check one populated dual-carry id field. `expected` is the id kind the
    /// position demands; `name` is the sibling name (for the dual-carry drift
    /// check), `None`/dotted-path when there is nothing comparable in file.
    fn check(&mut self, id: &Option<String>, expected: IdKind, name: Option<&str>, path: String) {
        let Some(id) = id else { return };
        self.seen.insert(id.clone());

        // Rule 3 — kind prefix vs position.
        match id_kind_of(id) {
            Some(kind) if kind == expected => {}
            other => {
                let got = other.map(|k| k.prefix()).unwrap_or("<unknown>");
                self.result.add_error(ValidationError::error(
                    ErrorCode::OrbIdKindMismatch,
                    path.clone(),
                    format!(
                        "id `{id}` has kind `{got}` but this position requires a `{}` id.",
                        expected.prefix()
                    ),
                ));
            }
        }

        // Rule 1 — dangling edge. Intrinsic stamping (`ensure_stamped` at the
        // compiler's pre-inline chokepoint) upgrades every ledger-less `.orb` on
        // load, so a schema reaching validation ALWAYS carries a ledger and no
        // ref is name-authoritative anymore. The check is therefore
        // unconditional: any populated id that matches no arena node nor ledger
        // entry is a dangling edge. (The former `ledger.is_some()` bypass —
        // G-V4-1 — is gone now that name-only inputs can't reach inline.)
        if !self.index.known(id) {
            self.result.add_error(ValidationError::error(
                ErrorCode::OrbIdUnknownRef,
                path.clone(),
                format!("id `{id}` matches no arena node nor ledger entry."),
            ));
        }

        // Rule 2 — dual-carry drift (only when the id resolves in the ledger and
        // the sibling is a bare in-file name; dotted alias paths are cross-file
        // and never carry an id under the minting contract).
        if let (Some(name), Some(cur)) =
            (name.filter(|n| !n.contains('.')), self.index.cur_name(id))
        {
            if cur != name && !self.index.config_forward_matches(id, cur, name) {
                self.result.add_error(ValidationError::error(
                    ErrorCode::OrbIdNameMismatch,
                    path,
                    format!("name `{name}` disagrees with ledger curName `{cur}` for id `{id}`."),
                ));
            }
        }
    }

    fn walk_orbital(&mut self, oi: usize, orbital: &Orbital) {
        let op = format!("orbitals[{} ({})]", oi, orbital.name);

        match &orbital.entity {
            EntityRef::Extends(e) => self.check(
                &e.extends_id,
                IdKind::Entity,
                Some(&e.reference),
                format!("{op}.entity.extends"),
            ),
            EntityRef::Inline(entity) => self.walk_entity(entity, &format!("{op}.entity")),
            EntityRef::Reference(_) => {}
        }
        for (ai, aux) in orbital.auxiliary_entities.iter().flatten().enumerate() {
            match aux {
                EntityRef::Extends(e) => self.check(
                    &e.extends_id,
                    IdKind::Entity,
                    Some(&e.reference),
                    format!("{op}.auxiliaryEntities[{ai}].extends"),
                ),
                EntityRef::Inline(entity) => {
                    self.walk_entity(entity, &format!("{op}.auxiliaryEntities[{ai}]"))
                }
                EntityRef::Reference(_) => {}
            }
        }

        for (ti, tr) in orbital.traits.iter().enumerate() {
            self.walk_trait_ref(tr, format!("{op}.traits[{ti}]"));
        }

        for (pi, page) in orbital.pages.iter().flatten().enumerate() {
            match page {
                PageRef::Inline(p) => {
                    for (ti, tr) in p.traits.iter().flatten().enumerate() {
                        self.walk_trait_ref(
                            tr,
                            format!("{op}.pages[{pi} ({})].traits[{ti}]", p.name),
                        );
                    }
                }
                PageRef::Object(o) => {
                    let pp = format!("{op}.pages[{pi}]");
                    self.check(
                        &o.ref_id,
                        IdKind::Page,
                        Some(&o.reference),
                        format!("{pp}.ref"),
                    );
                    self.check(
                        &o.linked_entity_id,
                        IdKind::Entity,
                        o.linked_entity.as_deref(),
                        format!("{pp}.linkedEntity"),
                    );
                    if let Some(ref_ids) = &o.trait_ref_ids {
                        for (ti, rid) in ref_ids.iter().enumerate() {
                            if rid.is_empty() {
                                continue;
                            }
                            let name = o
                                .traits
                                .as_ref()
                                .and_then(|t| t.get(ti))
                                .and_then(|t| t.trait_name());
                            self.check(
                                &Some(rid.clone()),
                                IdKind::Trait,
                                name,
                                format!("{pp}.traitRefIds[{ti}]"),
                            );
                        }
                    }
                    for (ti, tr) in o.traits.iter().flatten().enumerate() {
                        self.walk_trait_ref(tr, format!("{pp}.traits[{ti}]"));
                    }
                }
                PageRef::Reference(_) => {}
            }
        }
    }

    fn walk_entity(&mut self, entity: &EntityDefinition, path: &str) {
        for (fi, field) in entity.fields.iter().enumerate() {
            self.walk_field_relations(field, format!("{path}.fields[{fi}]"));
        }
    }

    fn walk_field_relations(&mut self, field: &FieldDefinition, path: String) {
        if let Some(rel) = &field.relation {
            self.check(
                &rel.entity_id,
                IdKind::Entity,
                Some(&rel.entity),
                format!("{path}.relation"),
            );
        }
        if let Some(items) = &field.items {
            self.walk_field_relations(items, format!("{path}.items"));
        }
        for (k, p) in field.properties.iter().flatten() {
            self.walk_field_relations(p, format!("{path}.properties.{k}"));
        }
    }

    fn walk_trait_ref(&mut self, tr: &TraitReference, path: String) {
        match tr {
            TraitReference::Inline(td) => {
                self.check(
                    &td.linked_entity_id,
                    IdKind::Entity,
                    td.linked_entity.as_deref(),
                    format!("{path}.linkedEntity"),
                );
                for (ei, emit) in td.emits.iter().flatten().enumerate() {
                    self.check(
                        &emit.event_id,
                        IdKind::Event,
                        Some(&emit.event),
                        format!("{path}.emits[{ei} ({})]", emit.event),
                    );
                }
                for (li, listen) in td.listens.iter().flatten().enumerate() {
                    let lp = format!("{path}.listens[{li}]");
                    self.check(
                        &listen.event_id,
                        IdKind::Event,
                        Some(&listen.event),
                        format!("{lp}.event"),
                    );
                    self.check(
                        &listen.triggers_id,
                        IdKind::Event,
                        listen.triggers.as_deref(),
                        format!("{lp}.triggers"),
                    );
                    match &listen.source {
                        Some(ListenSource::Trait {
                            trait_name,
                            trait_id,
                        }) => self.check(
                            trait_id,
                            IdKind::Trait,
                            Some(trait_name),
                            format!("{lp}.source.trait"),
                        ),
                        Some(ListenSource::Orbital {
                            orbital,
                            orbital_id,
                            trait_name,
                            trait_id,
                        }) => {
                            self.check(
                                orbital_id,
                                IdKind::Orbital,
                                Some(orbital),
                                format!("{lp}.source.orbital"),
                            );
                            self.check(
                                trait_id,
                                IdKind::Trait,
                                Some(trait_name),
                                format!("{lp}.source.trait"),
                            );
                        }
                        Some(ListenSource::Any) | None => {}
                    }
                }
                for (di, de) in td.data_entities.iter().flatten().enumerate() {
                    self.walk_entity(de, &format!("{path}.dataEntities[{di}]"));
                }
                // Render-tree carriers: `@trait.<Name>` embed tokens anywhere in
                // this trait's transition/tick effects and config defaults
                // (render-ui `children`, fn-form lambda bodies alike). Most
                // resolve via the stamp-time `traitEmbedIds` id side-map, but a
                // token introduced by INLINE's config-override substitution (a
                // `uses`-pulled Reference trait merged into an Inline body)
                // postdates that stamp and carries no id sibling — the walk
                // falls back to the same ledger name lookup the rest of it uses.
                let mut embed_names: HashSet<String> = HashSet::new();
                for transition in &td.state_machine.transitions {
                    for effect in transition.effects.iter().flatten() {
                        collect_trait_embed_names(effect, &mut embed_names);
                    }
                }
                for tick in td.ticks.iter().flatten() {
                    for effect in &tick.effects {
                        collect_trait_embed_names(effect, &mut embed_names);
                    }
                }
                for (_, cf) in td.config.iter().flatten() {
                    if let Some(default) = &cf.default {
                        collect_trait_embed_names(default, &mut embed_names);
                    }
                }
                self.note_trait_embed_names(embed_names, td.trait_embed_ids.as_ref());
            }
            TraitReference::Reference {
                reference,
                ref_id,
                linked_entity,
                linked_entity_id,
                config,
                ..
            } => {
                self.check(
                    ref_id,
                    IdKind::Trait,
                    reference.as_deref(),
                    format!("{path}.ref"),
                );
                self.check(
                    linked_entity_id,
                    IdKind::Entity,
                    linked_entity.as_deref(),
                    format!("{path}.linkedEntity"),
                );
                // A Reference trait carries no `traitEmbedIds` side-map at all
                // (stamp only scans Inline bodies) — a config default like
                // `children: ["@trait.Foo"]` is a bare NAME, resolved by the
                // ledger lookup.
                if let Some(config) = config {
                    let mut embed_names: HashSet<String> = HashSet::new();
                    for cf in config.values() {
                        if let Some(default) = &cf.default {
                            collect_trait_embed_names(default, &mut embed_names);
                        }
                    }
                    self.note_trait_embed_names(embed_names, None);
                }
            }
        }
    }

    /// Resolve a set of bare `@trait.<Name>` embed names to ids and mark them
    /// seen: the trait's own id side-map first (authoritative when present),
    /// else the ledger's name lookup (covers a name with no id sibling).
    fn note_trait_embed_names(
        &mut self,
        names: HashSet<String>,
        embed_ids: Option<&HashMap<String, String>>,
    ) {
        for id in embed_ids.iter().flat_map(|m| m.values()) {
            self.seen.insert(id.clone());
        }
        for name in names {
            if embed_ids.is_some_and(|m| m.contains_key(&name)) {
                continue;
            }
            if let Some(id) = self
                .index
                .ledger
                .and_then(|l| l.resolve_name(IdKind::Trait, &name))
            {
                self.seen.insert(id.to_string());
            }
        }
    }
}

/// Rule 5 — `ORB_ID_DUPLICATE_EVENT_ID` (error): one event id declared by two
/// or more DIFFERENT traits of the same orbital (`stateMachine.events[].id` /
/// `emits[].eventId`). Ids are the durable node keys; a shared one makes every
/// id→name resolution over the document ambiguous (the multi-instance knob
/// clobber). Inline's event-id hygiene re-derives per instance at expansion,
/// so a hit here means the artifact was produced outside that path — the
/// producer is the bug.
fn check_duplicate_event_ids(schema: &OrbitalSchema, result: &mut ValidationResult) {
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let mut owners: HashMap<&str, Vec<&str>> = HashMap::new();
        for tr in &orbital.traits {
            let TraitReference::Inline(td) = tr else {
                continue;
            };
            let mut seen_here: HashSet<&str> = HashSet::new();
            for ev in td.state_machine.events.iter().flatten() {
                if let Some(id) = &ev.id {
                    seen_here.insert(id);
                }
            }
            for e in td.emits.iter().flatten() {
                if let Some(id) = &e.event_id {
                    seen_here.insert(id);
                }
            }
            for id in seen_here {
                owners.entry(id).or_default().push(&td.name);
            }
        }
        for (id, traits) in owners {
            if traits.len() > 1 {
                result.add_error(ValidationError::error(
                    ErrorCode::OrbIdDuplicateEventId,
                    format!("orbitals[{} ({})]", oi, orbital.name),
                    format!(
                        "event id `{id}` is declared by {} different traits ({}) — ids must be document-unique; every id→name resolution over this schema is ambiguous.",
                        traits.len(),
                        traits.join(", ")
                    ),
                ));
            }
        }
    }
}

/// Rule family entry point. No-op on legacy id-free schemas.
pub fn validate_id_integrity(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let index = IdIndex::build(schema);
    let mut result = ValidationResult::new();
    let mut seen: HashSet<String> = index.declared.keys().cloned().collect();

    check_duplicate_event_ids(schema, &mut result);

    {
        let mut checker = Checker {
            index: &index,
            result: &mut result,
            seen: &mut seen,
        };
        for (oi, orbital) in schema.orbitals.iter().enumerate() {
            checker.walk_orbital(oi, orbital);
        }
    }

    // Rule 4 — ledger rows no node carries. An event row is carried by its
    // owning (parent) trait while that trait is live, since event ids have no
    // id-bearing declaration node of their own.
    if let Some(ledger) = &schema.ledger {
        for entry in ledger.entries.values() {
            let carried = seen.contains(&entry.id)
                || (entry.kind == IdKind::Event
                    && entry
                        .parent
                        .as_deref()
                        .is_some_and(|p| index.trait_ids.contains(p)));
            if !carried {
                result.add_error(ValidationError::error(
                    ErrorCode::OrbIdLedgerOrphan,
                    format!("ledger.entries[{}]", entry.id),
                    format!(
                        "ledger row `{}` (kind `{}`, curName `{}`) is carried by no node in the schema — stale.",
                        entry.id,
                        entry.kind.prefix().trim_end_matches('_'),
                        entry.cur_name
                    ),
                ));
            }
        }
    }

    ctx.merge(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::OrbitalSchema;

    /// Healthy stamped schema: one orbital, inline entity, one inline trait with
    /// a linked entity + emit + listen (with source), one page, full ledger.
    fn healthy_json() -> serde_json::Value {
        serde_json::json!({
            "name": "TestApp",
            "orbitals": [{
                "name": "TaskOrbital",
                "id": "orb_TASK000000000000000000000",
                "entity": {
                    "name": "Task",
                    "id": "ent_TASK000000000000000000000",
                    "fields": [{ "name": "title", "type": "string" }]
                },
                "traits": [{
                    "name": "TaskList",
                    "id": "trt_LIST000000000000000000000",
                    "linkedEntity": "Task",
                    "linkedEntityId": "ent_TASK000000000000000000000",
                    "emits": [{ "event": "REFRESH", "eventId": "evt_REFRESH0000000000000000" }],
                    "listens": [{
                        "event": "PING",
                        "eventId": "evt_PING0000000000000000000",
                        "triggers": "REFRESH",
                        "triggersId": "evt_REFRESH0000000000000000",
                        "source": { "kind": "trait", "trait": "TaskList", "traitId": "trt_LIST000000000000000000000" }
                    }],
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": []
                    }
                }],
                "pages": [{ "name": "Home", "id": "pag_HOME000000000000000000000", "path": "/" }]
            }],
            "schemaVersion": 1,
            "ledger": {
                "schemaVersion": 1,
                "entries": {
                    "orb_TASK000000000000000000000": { "id": "orb_TASK000000000000000000000", "kind": "orbital", "bakedName": "TaskOrbital", "curName": "TaskOrbital", "renames": [], "owner": "workspace" },
                    "ent_TASK000000000000000000000": { "id": "ent_TASK000000000000000000000", "kind": "entity", "bakedName": "Task", "curName": "Task", "renames": [], "owner": "workspace" },
                    "trt_LIST000000000000000000000": { "id": "trt_LIST000000000000000000000", "kind": "trait", "bakedName": "TaskList", "curName": "TaskList", "renames": [], "owner": "workspace" },
                    "evt_REFRESH0000000000000000": { "id": "evt_REFRESH0000000000000000", "kind": "event", "bakedName": "REFRESH", "curName": "REFRESH", "renames": [], "owner": "workspace", "parent": "trt_LIST000000000000000000000" },
                    "evt_PING0000000000000000000": { "id": "evt_PING0000000000000000000", "kind": "event", "bakedName": "PING", "curName": "PING", "renames": [], "owner": "workspace", "parent": "trt_LIST000000000000000000000" },
                    "pag_HOME000000000000000000000": { "id": "pag_HOME000000000000000000000", "kind": "page", "bakedName": "Home", "curName": "Home", "renames": [], "owner": "workspace" }
                }
            }
        })
    }

    fn parse(v: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(v).expect("fixture parses")
    }

    fn run(schema: &OrbitalSchema) -> ValidationResult {
        let mut ctx = ValidationContext::new();
        validate_id_integrity(schema, &mut ctx);
        ctx.into_result()
    }

    fn id_codes(result: &ValidationResult) -> Vec<ErrorCode> {
        result
            .all_issues()
            .map(|e| e.code)
            .filter(|c| c.category() == "identity")
            .collect()
    }

    #[test]
    fn healthy_schema_is_silent() {
        let result = run(&parse(healthy_json()));
        assert!(
            id_codes(&result).is_empty(),
            "healthy stamped schema must produce zero ORB_ID_* diagnostics: {:?}",
            id_codes(&result)
        );
    }

    #[test]
    fn legacy_id_free_schema_is_silent() {
        let mut v = healthy_json();
        // Strip every id field + the ledger — a pre-V4 `.orb`.
        v["orbitals"][0].as_object_mut().unwrap().remove("id");
        v["orbitals"][0]["entity"]
            .as_object_mut()
            .unwrap()
            .remove("id");
        let td = v["orbitals"][0]["traits"][0].as_object_mut().unwrap();
        td.remove("id");
        td.remove("linkedEntityId");
        td["emits"][0].as_object_mut().unwrap().remove("eventId");
        let listen = td["listens"][0].as_object_mut().unwrap();
        listen.remove("eventId");
        listen.remove("triggersId");
        listen["source"].as_object_mut().unwrap().remove("traitId");
        v["orbitals"][0]["pages"][0]
            .as_object_mut()
            .unwrap()
            .remove("id");
        v.as_object_mut().unwrap().remove("ledger");
        v.as_object_mut().unwrap().remove("schemaVersion");

        let result = run(&parse(v));
        assert!(
            id_codes(&result).is_empty(),
            "legacy id-free schema must stay silent: {:?}",
            id_codes(&result)
        );
    }

    #[test]
    fn unknown_ref_fires_on_dangling_id() {
        let mut v = healthy_json();
        // Well-formed Entity id, absent from arena + ledger.
        v["orbitals"][0]["traits"][0]["linkedEntityId"] =
            serde_json::json!("ent_GHOST00000000000000000000");
        let codes = id_codes(&run(&parse(v)));
        assert_eq!(codes, vec![ErrorCode::OrbIdUnknownRef], "codes: {codes:?}");
    }

    #[test]
    fn unknown_ref_fires_even_without_ledger() {
        // Post-intrinsic-stamping (V4 W4): the former G-V4-1 bypass is gone.
        // `ensure_stamped` upgrades every ledger-less `.orb` on load, so a schema
        // reaching validation always has a ledger and the id check is
        // unconditional. A populated id that resolves to no arena node — even
        // with the ledger stripped (a state that can no longer arise past the
        // compiler boundary) — is a dangling edge and fires ORB_ID_UNKNOWN_REF.
        // Stripping the ledger also orphans the event refs (event ids live only
        // in the ledger, not the declared arena) — so several ORB_ID_UNKNOWN_REF
        // fire, not just the ghost linkedEntity. The point is that the check no
        // longer goes SILENT without a ledger.
        let mut v = healthy_json();
        v["orbitals"][0]["traits"][0]["linkedEntityId"] =
            serde_json::json!("ent_GHOST00000000000000000000");
        v.as_object_mut().unwrap().remove("ledger");
        let codes = id_codes(&run(&parse(v)));
        assert!(
            codes.contains(&ErrorCode::OrbIdUnknownRef),
            "id check is now unconditional (no ledger bypass): {codes:?}"
        );
    }

    #[test]
    fn name_mismatch_fires_on_dual_carry_drift() {
        let mut v = healthy_json();
        // id still resolves in the ledger (curName REFRESH), but the name drifts.
        v["orbitals"][0]["traits"][0]["emits"][0]["event"] = serde_json::json!("RELOAD");
        let codes = id_codes(&run(&parse(v)));
        assert_eq!(
            codes,
            vec![ErrorCode::OrbIdNameMismatch],
            "codes: {codes:?}"
        );
    }

    #[test]
    fn config_forward_alias_is_accepted() {
        let mut v = healthy_json();
        // Authored view: the emit's ledger row keeps the `@config.<key>` forward
        // while every call site carries the RESOLVED name (the config value).
        v["ledger"]["entries"]["evt_REFRESH0000000000000000"]["curName"] =
            serde_json::json!("@config.onRefresh");
        v["orbitals"][0]["traits"][0]["config"] = serde_json::json!({
            "onRefresh": { "type": "string", "default": "REFRESH" }
        });
        let codes = id_codes(&run(&parse(v)));
        assert!(
            codes.is_empty(),
            "config-forward curName resolved by the owning trait's config must be silent (emit + listens sites): {codes:?}"
        );
    }

    #[test]
    fn config_forward_alias_rejects_unresolved_name() {
        let mut v = healthy_json();
        // Forward form in the ledger, but the call-site name does NOT equal the
        // config value — still a genuine drift.
        v["ledger"]["entries"]["evt_REFRESH0000000000000000"]["curName"] =
            serde_json::json!("@config.onRefresh");
        v["orbitals"][0]["traits"][0]["config"] = serde_json::json!({
            "onRefresh": { "type": "string", "default": "RELOAD" }
        });
        let codes = id_codes(&run(&parse(v)));
        assert!(
            codes.iter().all(|c| *c == ErrorCode::OrbIdNameMismatch) && !codes.is_empty(),
            "codes: {codes:?}"
        );
    }

    #[test]
    fn config_forward_alias_requires_the_config_key() {
        let mut v = healthy_json();
        // Forward form with no matching config entry on the owning trait.
        v["ledger"]["entries"]["evt_REFRESH0000000000000000"]["curName"] =
            serde_json::json!("@config.onRefresh");
        let codes = id_codes(&run(&parse(v)));
        assert!(
            codes.iter().all(|c| *c == ErrorCode::OrbIdNameMismatch) && !codes.is_empty(),
            "codes: {codes:?}"
        );
    }

    #[test]
    fn kind_mismatch_fires_on_wrong_prefix() {
        let mut v = healthy_json();
        // A Trait id in the entity `linkedEntityId` position, kept name-agreeing
        // via a ledger row so ONLY the kind rule fires.
        v["orbitals"][0]["traits"][0]["linkedEntityId"] =
            serde_json::json!("trt_TASKENT00000000000000000");
        v["ledger"]["entries"]["trt_TASKENT00000000000000000"] = serde_json::json!({
            "id": "trt_TASKENT00000000000000000",
            "kind": "trait",
            "bakedName": "Task",
            "curName": "Task",
            "renames": [],
            "owner": "workspace"
        });
        let codes = id_codes(&run(&parse(v)));
        assert_eq!(
            codes,
            vec![ErrorCode::OrbIdKindMismatch],
            "codes: {codes:?}"
        );
    }

    #[test]
    fn ledger_orphan_fires_as_error() {
        let mut v = healthy_json();
        v["ledger"]["entries"]["ent_GHOST00000000000000000000"] = serde_json::json!({
            "id": "ent_GHOST00000000000000000000",
            "kind": "entity",
            "bakedName": "Ghost",
            "curName": "Ghost",
            "renames": [],
            "owner": "workspace"
        });
        let result = run(&parse(v));
        assert_eq!(
            id_codes(&result),
            vec![ErrorCode::OrbIdLedgerOrphan],
            "codes: {:?}",
            id_codes(&result)
        );
        assert!(!result.is_valid(), "orphan must block validation");
        assert_eq!(result.error_count(), 1, "orphan lands in errors");
    }

    #[test]
    fn orphan_event_row_is_exempt_while_parent_trait_lives() {
        // An event ledger row with no id field carrying it, but whose parent
        // trait is present, must NOT be flagged orphan (events have no
        // id-bearing declaration node).
        let mut v = healthy_json();
        v["ledger"]["entries"]["evt_INTERNAL00000000000000"] = serde_json::json!({
            "id": "evt_INTERNAL00000000000000",
            "kind": "event",
            "bakedName": "TICK",
            "curName": "TICK",
            "renames": [],
            "owner": "workspace",
            "parent": "trt_LIST000000000000000000000"
        });
        assert!(
            id_codes(&run(&parse(v))).is_empty(),
            "event row with a live parent trait must not be an orphan"
        );
    }

    #[test]
    fn render_tree_embed_via_id_side_map_is_not_orphan() {
        // A sibling trait carried ONLY by a render-tree `@trait.Widget` embed —
        // recorded in the host's `traitEmbedIds` id side-map, no other node
        // referencing it — must not be flagged orphan.
        let mut v = healthy_json();
        v["orbitals"][0]["traits"][0]["traitEmbedIds"] =
            serde_json::json!({ "Widget": "trt_WIDGET0000000000000000000" });
        v["ledger"]["entries"]["trt_WIDGET0000000000000000000"] = serde_json::json!({
            "id": "trt_WIDGET0000000000000000000",
            "kind": "trait",
            "bakedName": "Widget",
            "curName": "Widget",
            "renames": [],
            "owner": "workspace"
        });
        let result = run(&parse(v));
        assert!(
            id_codes(&result).is_empty(),
            "id side-map embed must not orphan: {:?}",
            id_codes(&result)
        );
    }

    #[test]
    fn render_tree_embed_removed_still_orphans() {
        // Same ledger row as above, but with no `traitEmbedIds` entry carrying
        // it — rule 4 must still catch genuinely stale rows.
        let mut v = healthy_json();
        v["ledger"]["entries"]["trt_WIDGET0000000000000000000"] = serde_json::json!({
            "id": "trt_WIDGET0000000000000000000",
            "kind": "trait",
            "bakedName": "Widget",
            "curName": "Widget",
            "renames": [],
            "owner": "workspace"
        });
        let codes = id_codes(&run(&parse(v)));
        assert_eq!(
            codes,
            vec![ErrorCode::OrbIdLedgerOrphan],
            "codes: {codes:?}"
        );
    }

    #[test]
    fn duplicate_event_id_across_traits_is_an_error() {
        let mut v = healthy_json();
        // A second trait declaring the SAME event id (the multi-instance
        // expansion shape an old producer bakes) must be rejected.
        v["orbitals"][0]["traits"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "name": "TaskListCopy",
                "id": "trt_COPY000000000000000000000",
                "linkedEntity": "Task",
                "linkedEntityId": "ent_TASK000000000000000000000",
                "emits": [{ "event": "REFRESH", "eventId": "evt_REFRESH0000000000000000" }],
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": []
                }
            }));
        v["ledger"]["entries"]["trt_COPY000000000000000000000"] = serde_json::json!({
            "id": "trt_COPY000000000000000000000", "kind": "trait",
            "bakedName": "TaskListCopy", "curName": "TaskListCopy",
            "renames": [], "owner": "workspace"
        });
        let codes = id_codes(&run(&parse(v)));
        assert!(
            codes.contains(&ErrorCode::OrbIdDuplicateEventId),
            "codes: {codes:?}"
        );
    }

    #[test]
    fn same_event_id_within_one_trait_is_not_a_duplicate() {
        // One trait carrying the id in both its event list and its emit list is
        // ONE owner — the healthy fixture already has this shape and must stay
        // silent (covered by healthy_schema_is_silent), but pin it explicitly
        // with the id in both positions.
        let mut v = healthy_json();
        v["orbitals"][0]["traits"][0]["stateMachine"]["events"] = serde_json::json!([
            { "key": "REFRESH", "id": "evt_REFRESH0000000000000000" }
        ]);
        let codes = id_codes(&run(&parse(v)));
        assert!(
            !codes.contains(&ErrorCode::OrbIdDuplicateEventId),
            "codes: {codes:?}"
        );
    }

    #[test]
    fn fn_form_lambda_body_embed_is_not_orphan() {
        // `traitEmbedIds` only covers Inline-trait bodies at stamp time; a
        // Reference trait's config can carry a bare `@trait.<Name>` NAME (no id
        // sibling) nested inside a `renderItem` fn-form lambda body. Resolved by
        // name through the ledger, it must not orphan.
        let mut v = healthy_json();
        v["orbitals"][0]["traits"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "ref": "SomeLib.traits.SomeRender",
                "name": "ChildHost",
                "linkedEntity": "Task",
                "linkedEntityId": "ent_TASK000000000000000000000",
                "config": {
                    "renderItem": {
                        "type": "unknown",
                        "default": ["fn", ["item"], ["render-ui", "main",
                            { "type": "stack", "children": ["@trait.LambdaWidget"] }
                        ]]
                    }
                }
            }));
        v["ledger"]["entries"]["trt_LAMBDAWIDGET00000000000"] = serde_json::json!({
            "id": "trt_LAMBDAWIDGET00000000000",
            "kind": "trait",
            "bakedName": "LambdaWidget",
            "curName": "LambdaWidget",
            "renames": [],
            "owner": "workspace"
        });
        let result = run(&parse(v));
        assert!(
            id_codes(&result).is_empty(),
            "name-only embed inside a fn-form lambda body must not orphan: {:?}",
            id_codes(&result)
        );
    }
}
