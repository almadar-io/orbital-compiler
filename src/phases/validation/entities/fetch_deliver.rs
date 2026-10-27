//! `ORB_FETCH_DELIVER_IDS_*` and `ORB_FETCH_TOTAL_NONE_READ`: the authored `deliver: ids` and
//! `total: none` options of `fetch` (Almadar_Embedded_Kernel_Speed_Design.md section 5.2).
//!
//! `deliver: ids` makes the success event's `data` a row-set reference, not rows. v1 allows
//! exactly one use of it: passed whole as a `render-ui` prop value. Any other read in an arm that
//! handles the success event is an error naming the use. `total: none` skips the count, so the
//! success event's `totalCount` may not be read.

use super::value_walk::all_sources;
use super::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::bus_event::BusEventSource;
use orbital_core_base::ir::{OirBinding, OirBindingRoot, OirExpr};
use orbital_core_base::listen_match::matches_listen_source;
use orbital_core_base::schema::{
    split_event_address, Orbital, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
    ValueSlot,
};
use orbital_core_eval::evaluator::context::BindingContext;
use std::collections::{BTreeMap, HashMap, HashSet};

const DATA_REF: &str = "@payload.data";
const TOTAL_REF: &str = "@payload.totalCount";

struct FetchOptionsSeen {
    ids_events: HashSet<String>,
    no_total_events: HashSet<String>,
}

/// What a received event carries: the rows by reference, no total, or both.
#[derive(Clone, Copy, Default)]
struct Restriction {
    ids: bool,
    no_total: bool,
}

impl Restriction {
    fn union(&mut self, other: Self) {
        self.ids |= other.ids;
        self.no_total |= other.no_total;
    }

    fn any(self) -> bool {
        self.ids || self.no_total
    }
}

#[derive(Clone, Default)]
struct Annotations(BTreeMap<Vec<String>, Restriction>);

impl Annotations {
    fn delivered(restriction: Restriction) -> Self {
        let mut annotations = Self::default();
        if restriction.ids {
            annotations.insert(
                vec!["data".into()],
                Restriction { ids: true, no_total: false },
            );
        }
        if restriction.no_total {
            annotations.insert(
                vec!["totalCount".into()],
                Restriction { ids: false, no_total: true },
            );
        }
        annotations
    }

    fn insert(&mut self, path: Vec<String>, restriction: Restriction) {
        if restriction.any() {
            self.0.entry(path).or_default().union(restriction);
        }
    }

    fn union(&mut self, other: &Self) {
        for (path, restriction) in &other.0 {
            self.insert(path.clone(), *restriction);
        }
    }

    fn beneath(&self, field: &str) -> Self {
        let mut nested = Self::default();
        for (path, restriction) in &self.0 {
            let mut prefixed = vec![field.to_string()];
            prefixed.extend(path.iter().cloned());
            nested.insert(prefixed, *restriction);
        }
        nested
    }

    fn project(&self, selected: &[String]) -> Self {
        let mut projected = Self::default();
        for (path, restriction) in &self.0 {
            if selected.starts_with(path) {
                projected.insert(Vec::new(), *restriction);
            } else if path.starts_with(selected) {
                projected.insert(path[selected.len()..].to_vec(), *restriction);
            }
        }
        projected
    }

    fn all(&self) -> Restriction {
        let mut restriction = Restriction::default();
        for received in self.0.values() {
            restriction.union(*received);
        }
        restriction
    }

    fn root(&self) -> Restriction {
        self.0.get(&Vec::new()).copied().unwrap_or_default()
    }

    fn opaque(restriction: Restriction) -> Self {
        let mut annotations = Self::default();
        annotations.insert(Vec::new(), restriction);
        annotations
    }
}

#[derive(Clone)]
struct Received {
    payload: Annotations,
    original: Annotations,
}

impl Received {
    fn delivered(restriction: Restriction) -> Self {
        let payload = Annotations::delivered(restriction);
        Self {
            original: payload.clone(),
            payload,
        }
    }

    fn union(&mut self, other: &Self) {
        self.payload.union(&other.payload);
        self.original.union(&other.original);
    }
}

struct RestrictedUse {
    at: String,
    what: String,
    restriction: Restriction,
}

/// Every arm in the program that receives a restricted success event: the fetching trait's own
/// arms, plus the arms of any trait whose `listens` route subscribes to it (the route's
/// `triggers` is the receiving arm). A relay cannot carry the value on, since forwarding it in an
/// `emit` payload is itself a rejected use.
pub(crate) fn validate_fetch_deliver(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut traits: Vec<(String, &Orbital, &TraitDefinition)> = Vec::new();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(td) = trait_ref {
                let path = super::trait_path(i, &orbital.name, j, trait_ref);
                traits.push((path, orbital, td));
            }
        }
    }
    let mut receivers: HashMap<(usize, String), Received> = HashMap::new();
    for (index, (_, fetcher_orbital, td)) in traits.iter().enumerate() {
        let source = BusEventSource {
            orbital: Some(fetcher_orbital.name.clone()),
            orbital_id: fetcher_orbital.id.clone(),
            r#trait: Some(td.name.clone()),
            trait_id: td.id.clone(),
            ..Default::default()
        };
        let mut seen = FetchOptionsSeen {
            ids_events: HashSet::new(),
            no_total_events: HashSet::new(),
        };
        for transition in &td.state_machine.transitions {
            for effect in transition.effects.iter().flatten() {
                collect_fetches(effect, &mut seen);
            }
        }
        let events: HashSet<&String> = seen
            .ids_events
            .iter()
            .chain(&seen.no_total_events)
            .collect();
        for event in events {
            let restriction = Restriction {
                ids: seen.ids_events.contains(event),
                no_total: seen.no_total_events.contains(event),
            };
            let received = Received::delivered(restriction);
            receivers
                .entry((index, event.clone()))
                .and_modify(|existing| existing.union(&received))
                .or_insert_with(|| received.clone());
            for (listener, (_, listener_orbital, listener_td)) in traits.iter().enumerate() {
                for listen in listener_td.listens.iter().flatten() {
                    let (bare, declared_source) = match &listen.source {
                        Some(declared) => (listen.event.clone(), Some(declared.clone())),
                        None => split_event_address(&listen.event),
                    };
                    if bare != *event
                        || !matches_listen_source(
                            declared_source.as_ref(),
                            &listener_orbital.name,
                            &source,
                        )
                    {
                        continue;
                    }
                    let triggered = listen.triggers.clone().unwrap_or(bare);
                    let mut checker = Checker::new(&received, CheckContext::Guard);
                    if let Some(guard) = &listen.guard {
                        checker.scan(guard, "listens.guard", Consumption::All);
                    }
                    let payload = match &listen.payload_mapping {
                        Some(mapping) => {
                            let mut mapper = Checker::new(&received, CheckContext::Mapping);
                            let mut mapped = Annotations::default();
                            for (name, expr) in mapping {
                                let source = mapper.scan(
                                    expr,
                                    &format!("listens.payloadMapping.{name}"),
                                    Consumption::All,
                                );
                                mapped.union(&source.beneath(name));
                            }
                            checker.uses.extend(mapper.uses);
                            mapped
                        }
                        None => received.payload.clone(),
                    };
                    let routed = Received {
                        payload,
                        original: received.original.clone(),
                    };
                    receivers
                        .entry((listener, triggered))
                        .and_modify(|existing| existing.union(&routed))
                        .or_insert(routed);
                    report(
                        &mut result,
                        &traits[listener].0,
                        &format!("listens `{}`", listen.event),
                        checker.uses,
                    );
                }
            }
        }
    }
    let mut ordered: Vec<(&(usize, String), &Received)> = receivers.iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(b.0));
    for ((index, event), received) in ordered {
        let (path, _, td) = &traits[*index];
        for (i, transition) in td.state_machine.transitions.iter().enumerate() {
            if &transition.event != event {
                continue;
            }
            let base = format!("{path}.stateMachine.transitions[{i}]");
            if let Some(guard) = &transition.guard {
                let mut checker = Checker::new(received, CheckContext::Guard);
                checker.scan(guard, "guard", Consumption::All);
                report(&mut result, &base, &format!("`{event}`"), checker.uses);
            }
            let mut checker = Checker::new(received, CheckContext::Effects);
            for (j, effect) in transition.effects.iter().flatten().enumerate() {
                checker.scan(effect, &format!("effects[{j}]"), Consumption::All);
            }
            report(&mut result, &base, &format!("`{event}`"), checker.uses);
        }
    }
    result
}

fn report(result: &mut ValidationResult, base: &str, arm: &str, uses: Vec<RestrictedUse>) {
    for RestrictedUse { at, what, restriction } in uses {
        for is_total in [false, true] {
            let (code, message) = if is_total {
                if !restriction.no_total {
                    continue;
                }
                (
                    ErrorCode::FetchTotalNoneRead,
                    format!("{arm} reads `{TOTAL_REF}` ({what}), but the fetch that produces it declares `total: none`"),
                )
            } else {
                if !restriction.ids {
                    continue;
                }
                (
                    ErrorCode::FetchDeliverIdsRowsRead,
                    format!("{arm} reads `{DATA_REF}` ({what}), but the fetch that produces it declares `deliver: ids`; the reference may only be passed whole as a `render-ui` prop value"),
                )
            };
            result.add_error(
                ValidationError::error(code, format!("{base}.{at}"), message).with_suggestion(
                    "Pass it whole, e.g. `entity: ?data`, or drop the option from the fetch",
                ),
            );
        }
    }
}

fn collect_fetches(expr: &SExpression, seen: &mut FetchOptionsSeen) {
    let SExpression::Array(items) = expr else {
        return;
    };
    if matches!(items.first(), Some(SExpression::String(op)) if op == "fetch") {
        for arg in items.iter().skip(2) {
            let SExpression::Object(options) = arg else {
                continue;
            };
            let success = match options.get("emit") {
                Some(SExpression::Object(emit)) => match emit.get("success") {
                    Some(SExpression::String(s)) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                },
                _ => None,
            };
            let is = |key: &str, value: &str| matches!(options.get(key), Some(SExpression::String(s)) if s == value);
            if is("deliver", "ids") {
                if let Some(event) = &success {
                    seen.ids_events.insert(event.clone().into_inner());
                }
            }
            if is("total", "none") {
                if let Some(event) = &success {
                    seen.no_total_events.insert(event.clone().into_inner());
                }
            }
        }
    }
    for item in items {
        collect_fetches(item, seen);
    }
}

#[derive(Clone, Copy)]
enum Consumption {
    All,
    Carrier,
}

#[derive(Clone, Copy)]
enum CheckContext {
    Guard,
    Mapping,
    Effects,
}

struct Checker<'a> {
    received: &'a Received,
    locals: Vec<(String, Annotations)>,
    context: CheckContext,
    uses: Vec<RestrictedUse>,
}

impl<'a> Checker<'a> {
    fn new(received: &'a Received, context: CheckContext) -> Self {
        Self {
            received,
            locals: Vec::new(),
            context,
            uses: Vec::new(),
        }
    }

    fn binding(&self, binding: &OirBinding) -> Annotations {
        let source = match &binding.root {
            OirBindingRoot::Payload => self.received.payload.clone(),
            OirBindingRoot::Event if !matches!(self.context, CheckContext::Mapping) => {
                self.received.original.beneath("payload")
            }
            OirBindingRoot::Local(name) => self.locals.iter().rev()
                .find(|(local, _)| local == name)
                .map(|(_, source)| source.clone())
                .unwrap_or_default(),
            _ => Annotations::default(),
        };
        source.project(&binding.path)
    }

    fn consume(&mut self, source: &Annotations, at: &str, what: String, mode: Consumption) {
        let restriction = match mode {
            Consumption::All => source.all(),
            Consumption::Carrier => source.root(),
        };
        if restriction.any() {
            self.uses.push(RestrictedUse {
                at: at.to_string(),
                what,
                restriction,
            });
        }
    }

    fn string(&mut self, text: &str, at: &str, mode: Consumption) -> Annotations {
        if let Some(binding) = OirBinding::parse(text) {
            let source = self.binding(&binding);
            self.consume(&source, at, format!("binding `{text}` at `{at}`"), mode);
            return source;
        }
        for (start, end) in binding_tokens(text) {
            if let Some(binding) = OirBinding::parse(&text[start..end]) {
                let source = self.binding(&binding);
                self.consume(
                    &source,
                    at,
                    format!("interpolated into `{text}` at `{at}`"),
                    Consumption::All,
                );
            }
        }
        Annotations::default()
    }

    fn scan(&mut self, expr: &SExpression, at: &str, mode: Consumption) -> Annotations {
        if let Some(scope) = expr.sequential_bindings() {
            let depth = self.locals.len();
            for (name, initializer) in scope.bindings {
                let source = self.scan(
                    initializer,
                    &format!("{at}.let.{name}"),
                    Consumption::Carrier,
                );
                self.locals.push((name.to_string(), source));
            }
            let source = self.scan(scope.body, &format!("{at}.let.body"), mode);
            self.locals.truncate(depth);
            return source;
        }
        match expr {
            SExpression::String(text) => self.string(text, at, mode),
            SExpression::Object(fields) => {
                let mut source = Annotations::default();
                for (name, value) in fields {
                    source.union(&self.scan(value, &format!("{at}.{name}"), mode).beneath(name));
                }
                source
            }
            SExpression::Array(items) => {
                let Some(op) = orbital_core_eval::lower::recognized_call_head(expr) else {
                    let mut source = Annotations::default();
                    for (index, item) in items.iter().enumerate() {
                        source.union(
                            &self.scan(item, &format!("{at}[{index}]"), mode)
                                .beneath(&index.to_string()),
                        );
                    }
                    return source;
                };
                if op == "let" {
                    return Annotations::default();
                }
                if op == "render-ui" && matches!(self.context, CheckContext::Effects) {
                    self.render(&items[1..], at);
                    return Annotations::default();
                }
                if let Some(source) = self.lambda(expr, at, Consumption::All) {
                    return Annotations::opaque(source.all());
                }
                if let Some(projection) = crate::type_infer::signature_of(op)
                    .and_then(|signature| signature.field_projection())
                {
                    return self.projection(&items[1..], projection, at, mode);
                }
                self.call(expr, at, mode)
            }
            _ => Annotations::default(),
        }
    }

    fn render(&mut self, args: &[SExpression], at: &str) {
        for (index, arg) in args.iter().enumerate() {
            let here = format!("{at}[{}]", index + 1);
            if let SExpression::Object(props) = arg {
                for (name, value) in props {
                    let here = format!("{here}.{name}");
                    if let SExpression::String(text) = value {
                        if text == DATA_REF || text == "@event.payload.data" {
                            let mut restriction = OirBinding::parse(text)
                                .map(|binding| self.binding(&binding).all())
                                .unwrap_or_default();
                            restriction.ids = false;
                            self.consume(
                                &Annotations::opaque(restriction),
                                &here,
                                format!("binding `{text}` at `{here}`"),
                                Consumption::All,
                            );
                            continue;
                        }
                    }
                    self.scan(value, &here, Consumption::All);
                }
            } else {
                self.scan(arg, &here, Consumption::All);
            }
        }
    }

    fn lambda(&mut self, expr: &SExpression, at: &str, mode: Consumption) -> Option<Annotations> {
        let walk = all_sources();
        if !(walk.lookup)(expr.operator()?).is_some_and(|meta| meta.returns_function) {
            return None;
        }
        let (body, header) = expr.as_array()?.split_last()?;
        let mut declaration = header.to_vec();
        declaration.push(SExpression::Null);
        let declaration = SExpression::Array(declaration);
        let OirExpr::Lambda { params, .. } = orbital_core_eval::lower::lower_sexpr(&declaration).ok()? else {
            return None;
        };
        let depth = self.locals.len();
        for param in params {
            self.locals.push((param, Annotations::default()));
        }
        let source = self.scan(body, &format!("{at}.lambda.body"), mode);
        self.locals.truncate(depth);
        Some(source)
    }

    fn call(&mut self, expr: &SExpression, at: &str, mode: Consumption) -> Annotations {
        let Some(args) = expr.arguments() else {
            return Annotations::default();
        };
        let slots = expr.value_source_slots(&all_sources());
        let mut arguments = Vec::with_capacity(args.len());
        if let Some(SExpression::String(head)) = expr.as_array().and_then(|items| items.first()) {
            self.string(head, &format!("{at}[0]"), Consumption::All);
        }
        for (index, arg) in args.iter().enumerate() {
            let returned = slots.as_ref().is_some_and(|slots| slots.iter().any(|slot| {
                matches!(slot, ValueSlot::Arg(i) | ValueSlot::LambdaBody(i) if *i == index)
            }));
            let here = format!("{at}[{}]", index + 1);
            let body_returned = slots.as_ref().is_some_and(|slots| slots.iter().any(|slot| {
                matches!(slot, ValueSlot::LambdaBody(i) if *i == index)
            }));
            let annotations = if body_returned {
                self.lambda(arg, &here, mode)
                    .unwrap_or_else(|| self.scan(arg, &here, Consumption::All))
            } else {
                self.scan(arg, &here, if returned { mode } else { Consumption::All })
            };
            arguments.push(annotations);
        }
        match slots {
            Some(slots) => {
                let mut source = Annotations::default();
                for slot in slots {
                    match slot {
                        ValueSlot::Arg(index) | ValueSlot::LambdaBody(index) => {
                            source.union(&arguments[index]);
                        }
                    }
                }
                source
            }
            None => {
                let mut possible = Restriction::default();
                for argument in arguments {
                    possible.union(argument.all());
                }
                // An opaque return cannot establish a safe field projection.
                Annotations::opaque(possible)
            }
        }
    }

    fn projection(
        &mut self,
        args: &[SExpression],
        projection: orbital_core_base::signature::FieldProjection,
        at: &str,
        mode: Consumption,
    ) -> Annotations {
        let mut source = Annotations::default();
        let mut default = Annotations::default();
        for (index, arg) in args.iter().enumerate() {
            let annotations = self.scan(
                arg,
                &format!("{at}[{}]", index + 1),
                if index == projection.source_arg {
                    Consumption::Carrier
                } else {
                    Consumption::All
                },
            );
            if index == projection.source_arg {
                source = annotations;
            } else if projection.default_arg == Some(index) {
                default = annotations;
            }
        }
        let mut selected = Annotations::default();
        match args.get(projection.key_arg).and_then(literal_keys) {
            Some(keys) => {
                for key in keys {
                    let path: Vec<String> = key.split('.').map(str::to_string).collect();
                    selected.union(&source.project(&path));
                }
                self.consume(&selected, at, format!("field projection at `{at}`"), mode);
            }
            None => {
                // A computed key may read any restricted path; no runtime key is guessed.
                self.consume(
                    &source,
                    at,
                    format!("computed field projection at `{at}`"),
                    Consumption::All,
                );
                selected = Annotations::opaque(source.all());
            }
        }
        selected.union(&default);
        selected
    }
}

fn binding_tokens(text: &str) -> Vec<(usize, usize)> {
    text.char_indices().filter_map(|(start, character)| {
        if character == '@' {
            BindingContext::binding_token_end(text, start).map(|end| (start, end))
        } else {
            None
        }
    }).collect()
}

fn literal_keys(expr: &SExpression) -> Option<Vec<&str>> {
    let leaves = expr.value_leaves(&all_sources());
    if leaves.is_empty() {
        return None;
    }
    leaves.into_iter().map(|leaf| {
        leaf.as_str().filter(|text| {
            OirBinding::parse(text).is_none() && binding_tokens(text).is_empty()
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_with(fetch_options: &str, arm_effects: &str, guard: Option<&str>) -> OrbitalSchema {
        let guard = guard.map_or(String::new(), |g| format!(r#", "guard": {g}"#));
        schema_json(&format!(
            r#"{{ "name": "T", "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [
                {{ "from": "idle", "to": "idle", "event": "INIT",
                   "effects": [["fetch", "Item", {{ {fetch_options}, "emit": {{ "success": "Loaded" }} }}]] }},
                {{ "from": "idle", "to": "idle", "event": "Loaded"{guard}, "effects": [{arm_effects}] }}
              ] }} }}"#
        ))
    }

    fn schema_json(trait_json: &str) -> OrbitalSchema {
        schema_of(&[trait_json])
    }

    fn schema_of(traits: &[&str]) -> OrbitalSchema {
        serde_json::from_str(&format!(
            r#"{{ "name": "S", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "Item", "persistence": "runtime", "fields": [{{ "name": "id", "type": "string" }}] }},
              "traits": [{}] }}] }}"#,
            traits.join(", ")
        ))
        .expect("schema json")
    }

    fn listener(source: &str, arm_effects: &str) -> String {
        format!(
            r#"{{ "name": "L", "listens": [{{ "event": "Loaded", "triggers": "GOT", "source": {source} }}],
              "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "GOT", "effects": [{arm_effects}] }}] }} }}"#
        )
    }

    fn fetcher(options: &str) -> String {
        format!(
            r#"{{ "name": "F", "stateMachine": {{ "states": [{{ "name": "idle", "isInitial": true }}],
              "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT",
                "effects": [["fetch", "Item", {{ {options}, "emit": {{ "success": "Loaded" }} }}]] }}] }} }}"#
        )
    }

    fn codes(s: &OrbitalSchema) -> Vec<ErrorCode> {
        validate_fetch_deliver(s)
            .errors
            .iter()
            .map(|e| e.code.clone())
            .collect()
    }

    fn trait_with(o: &str, a: &str, g: Option<&str>) -> OrbitalSchema {
        schema_with(o, a, g)
    }

    const IDS: &str = r#""deliver": "ids""#;
    const F_SRC: &str = r#"{ "kind": "trait", "trait": "F" }"#;

    #[test]
    fn a_cross_trait_listener_reading_a_row_is_an_error() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(F_SRC, r#"["set", "@entity.n", "@payload.data.0.name"]"#),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn a_wildcard_listener_reading_a_row_is_an_error() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(
                r#"{ "kind": "any" }"#,
                r#"["set", "@entity.n", "@payload.data.0.name"]"#,
            ),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn a_relay_emitting_the_reference_is_an_error() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(F_SRC, r#"["emit", "Again", { "rows": "@payload.data" }]"#),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn a_cross_trait_listener_passing_it_whole_to_render_ui_is_accepted() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(
                F_SRC,
                r#"["render-ui", "main", { "entity": "@payload.data" }]"#,
            ),
        ]);
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn control_a_listener_of_another_trait_is_not_restricted() {
        let s = schema_of(&[
            &fetcher(IDS),
            &listener(
                r#"{ "kind": "trait", "trait": "Other" }"#,
                r#"["set", "@entity.n", "@payload.data.0.name"]"#,
            ),
        ]);
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn control_a_cross_trait_listener_of_a_rows_fetch_may_read_rows() {
        let s = schema_of(&[
            &fetcher(r#""limit": 5"#),
            &listener(F_SRC, r#"["set", "@entity.n", "@payload.data.0.name"]"#),
        ]);
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn a_cross_trait_listener_reading_the_total_of_a_total_none_fetch_is_an_error() {
        let s = schema_of(&[
            &fetcher(r#""total": "none""#),
            &listener(F_SRC, r#"["set", "@entity.n", "@payload.totalCount"]"#),
        ]);
        assert_eq!(codes(&s), vec![ErrorCode::FetchTotalNoneRead]);
    }

    #[test]
    fn ids_passed_whole_as_a_render_ui_prop_is_accepted() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", { "type": "entity-table", "entity": "@payload.data" }]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn ids_whole_inside_a_when_wrapped_render_ui_is_accepted() {
        let t = trait_with(
            IDS,
            r#"["when", "@config.on", ["render-ui", "main", { "entity": "@payload.data" }]]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn ids_field_access_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", { "title": "@payload.data.title" }]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_array_operator_argument_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", { "n": ["array/len", "@payload.data"] }]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_set_into_the_entity_is_an_error() {
        let t = trait_with(IDS, r#"["set", "@entity.rows", "@payload.data"]"#, None);
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_in_an_emit_payload_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["emit", "Done", { "rows": "@payload.data" }]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn ids_in_a_guard_is_an_error() {
        let t = trait_with(
            IDS,
            r#"["render-ui", "main", null]"#,
            Some(r#"[">", ["array/len", "@payload.data"], 0]"#),
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchDeliverIdsRowsRead]);
    }

    #[test]
    fn control_rows_delivery_may_read_the_rows_anywhere() {
        let t = trait_with(
            r#""deliver": "rows""#,
            r#"["set", "@entity.rows", "@payload.data"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
        let t = trait_with(
            r#""limit": 20"#,
            r#"["set", "@entity.rows", "@payload.data"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn a_sibling_payload_field_is_not_the_data_reference() {
        let t = trait_with(IDS, r#"["set", "@entity.x", "@payload.dataset"]"#, None);
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn total_none_reading_the_total_is_an_error() {
        let t = trait_with(
            r#""total": "none""#,
            r#"["set", "@entity.n", "@payload.totalCount"]"#,
            None,
        );
        assert_eq!(codes(&t), vec![ErrorCode::FetchTotalNoneRead]);
    }

    #[test]
    fn control_total_none_without_reading_the_total_passes() {
        let t = trait_with(
            r#""total": "none""#,
            r#"["set", "@entity.rows", "@payload.data"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn control_exact_total_may_be_read() {
        let t = trait_with(
            r#""total": "exact""#,
            r#"["set", "@entity.n", "@payload.totalCount"]"#,
            None,
        );
        assert!(codes(&t).is_empty());
    }

    #[test]
    fn s8_unsourced_listener_inherits_restrictions_with_legal_controls() {
        for (options, effects, expected) in [
            (
                r#""deliver": "ids", "total": "none""#,
                r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                vec![ErrorCode::FetchDeliverIdsRowsRead, ErrorCode::FetchTotalNoneRead],
            ),
            (
                r#""deliver": "ids", "total": "none""#,
                r#"["render-ui", "main", { "entity": "@payload.data" }]"#,
                vec![],
            ),
            (
                r#""deliver": "rows", "total": "exact""#,
                r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                vec![],
            ),
        ] {
            let s = schema_of(&[&fetcher(options), &listener("null", effects)]);
            assert_eq!(codes(&s), expected, "options: {options}; effects: {effects}");
        }
    }

    #[test]
    fn s8_any_listener_inherits_restrictions_with_legal_controls() {
        for (options, effects, expected) in [
            (
                r#""deliver": "ids", "total": "none""#,
                r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                vec![ErrorCode::FetchDeliverIdsRowsRead, ErrorCode::FetchTotalNoneRead],
            ),
            (
                r#""deliver": "ids", "total": "none""#,
                r#"["render-ui", "main", { "entity": "@payload.data" }]"#,
                vec![],
            ),
            (
                r#""deliver": "rows", "total": "exact""#,
                r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                vec![],
            ),
        ] {
            let s = schema_of(&[
                &fetcher(options),
                &listener(r#"{ "kind": "any" }"#, effects),
            ]);
            assert_eq!(codes(&s), expected, "options: {options}; effects: {effects}");
        }
    }

    #[test]
    fn s8_name_only_source_is_local_but_qualified_source_can_cross_orbitals() {
        for (source, cross_orbital, restricted) in [
            (F_SRC, false, true),
            (F_SRC, true, false),
            (r#"{ "kind": "orbital", "orbital": "O", "trait": "F" }"#, true, true),
            (r#"{ "kind": "orbital", "orbital": "Other", "trait": "F" }"#, true, false),
        ] {
            for (options, ids) in [(IDS, true), (r#""deliver": "rows""#, false)] {
                let mut s = schema_of(&[
                    &fetcher(options),
                    &listener(source, r#"["set", "@entity.row", "@payload.data.0.name"]"#),
                ]);
                if cross_orbital {
                    let receiver = s.orbitals[0].traits.pop().unwrap();
                    let mut other = s.orbitals[0].clone();
                    other.name = "Other".into();
                    other.traits = vec![receiver];
                    s.orbitals.push(other);
                }
                let expected = if restricted && ids {
                    vec![ErrorCode::FetchDeliverIdsRowsRead]
                } else {
                    vec![]
                };
                assert_eq!(codes(&s), expected, "source: {source}; cross-orbital: {cross_orbital}; options: {options}");
            }
        }
    }

    #[test]
    fn s8_trait_id_source_survives_a_rename_but_name_only_source_does_not() {
        for cross_orbital in [false, true] {
            for (source, restricted) in [
                (r#"{ "kind": "trait", "trait": "F", "traitId": "trt_F" }"#, true),
                (F_SRC, false),
            ] {
                let mut s = schema_of(&[
                    &fetcher(IDS),
                    &listener(source, r#"["set", "@entity.row", "@payload.data.0.name"]"#),
                ]);
                let TraitReference::Inline(emitter) = &mut s.orbitals[0].traits[0] else {
                    panic!("inline fetcher fixture");
                };
                emitter.name = "RenamedF".into();
                emitter.id = Some("trt_F".into());
                if cross_orbital {
                    let receiver = s.orbitals[0].traits.pop().unwrap();
                    let mut other = s.orbitals[0].clone();
                    other.name = "Other".into();
                    other.traits = vec![receiver];
                    s.orbitals.push(other);
                }
                let expected = if restricted {
                    vec![ErrorCode::FetchDeliverIdsRowsRead]
                } else {
                    vec![]
                };
                assert_eq!(codes(&s), expected, "source: {source}; cross-orbital: {cross_orbital}");
            }
        }
    }

    #[test]
    fn s8_trait_id_source_does_not_fall_back_to_the_same_name_on_id_mismatch() {
        for (source, emitter_id, restricted) in [
            (r#"{ "kind": "trait", "trait": "F", "traitId": "trt_F" }"#, Some("trt_other"), false),
            (r#"{ "kind": "trait", "trait": "F", "traitId": "trt_F" }"#, None, false),
            (r#"{ "kind": "trait", "trait": "F", "traitId": "trt_F" }"#, Some("trt_F"), true),
            (F_SRC, Some("trt_F"), true),
        ] {
            let mut s = schema_of(&[
                &fetcher(IDS),
                &listener(source, r#"["set", "@entity.row", "@payload.data.0.name"]"#),
            ]);
            let TraitReference::Inline(emitter) = &mut s.orbitals[0].traits[0] else {
                panic!("inline fetcher fixture");
            };
            emitter.id = emitter_id.map(str::to_owned);
            let expected = if restricted {
                vec![ErrorCode::FetchDeliverIdsRowsRead]
            } else {
                vec![]
            };
            assert_eq!(codes(&s), expected, "source: {source}; emitter-id: {emitter_id:?}");
        }
    }

    #[test]
    fn s8_orbital_and_trait_ids_survive_both_display_name_renames() {
        for (source, restricted) in [
            (r#"{ "kind": "orbital", "orbital": "O", "orbitalId": "orb_O", "trait": "F", "traitId": "trt_F" }"#, true),
            (r#"{ "kind": "orbital", "orbital": "O", "trait": "F" }"#, false),
        ] {
            let mut s = schema_of(&[
                &fetcher(IDS),
                &listener(source, r#"["set", "@entity.row", "@payload.data.0.name"]"#),
            ]);
            s.orbitals[0].name = "RenamedO".into();
            s.orbitals[0].id = Some("orb_O".into());
            let TraitReference::Inline(emitter) = &mut s.orbitals[0].traits[0] else {
                panic!("inline fetcher fixture");
            };
            emitter.name = "RenamedF".into();
            emitter.id = Some("trt_F".into());
            let receiver = s.orbitals[0].traits.pop().unwrap();
            let mut other = s.orbitals[0].clone();
            other.name = "Other".into();
            other.id = Some("orb_listener".into());
            other.traits = vec![receiver];
            s.orbitals.push(other);
            let expected = if restricted {
                vec![ErrorCode::FetchDeliverIdsRowsRead]
            } else {
                vec![]
            };
            assert_eq!(codes(&s), expected, "source: {source}");
        }
    }

    #[test]
    fn s8_orbital_source_with_both_ids_rejects_different_or_missing_emitter_ids() {
        for (orbital_id, trait_id, restricted) in [
            (Some("orb_other"), Some("trt_F"), false),
            (Some("orb_O"), Some("trt_other"), false),
            (None, Some("trt_F"), false),
            (Some("orb_O"), None, false),
            (Some("orb_O"), Some("trt_F"), true),
        ] {
            let mut s = schema_of(&[
                &fetcher(IDS),
                &listener(
                    r#"{ "kind": "orbital", "orbital": "O", "orbitalId": "orb_O", "trait": "F", "traitId": "trt_F" }"#,
                    r#"["set", "@entity.row", "@payload.data.0.name"]"#,
                ),
            ]);
            s.orbitals[0].id = orbital_id.map(str::to_owned);
            let TraitReference::Inline(emitter) = &mut s.orbitals[0].traits[0] else {
                panic!("inline fetcher fixture");
            };
            emitter.id = trait_id.map(str::to_owned);
            let receiver = s.orbitals[0].traits.pop().unwrap();
            let mut other = s.orbitals[0].clone();
            other.name = "Other".into();
            other.id = Some("orb_listener".into());
            other.traits = vec![receiver];
            s.orbitals.push(other);
            let expected = if restricted {
                vec![ErrorCode::FetchDeliverIdsRowsRead]
            } else {
                vec![]
            };
            assert_eq!(codes(&s), expected, "orbital-id: {orbital_id:?}; trait-id: {trait_id:?}");
        }
    }

    #[test]
    fn s8_two_fetchers_union_ids_and_total_none_restrictions_in_both_orders() {
        for reverse in [false, true] {
            for (first, second, effects, expected) in [
                (
                    r#""deliver": "ids", "total": "exact""#,
                    r#""deliver": "rows", "total": "none""#,
                    r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                    vec![ErrorCode::FetchDeliverIdsRowsRead, ErrorCode::FetchTotalNoneRead],
                ),
                (
                    r#""deliver": "ids", "total": "exact""#,
                    r#""deliver": "rows", "total": "none""#,
                    r#"["render-ui", "main", { "entity": "@payload.data" }]"#,
                    vec![],
                ),
                (
                    r#""deliver": "rows", "total": "exact""#,
                    r#""deliver": "rows", "total": "exact""#,
                    r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                    vec![],
                ),
            ] {
                let mut s = schema_of(&[
                    &fetcher(first),
                    &fetcher(second),
                    &listener(r#"{ "kind": "any" }"#, effects),
                ]);
                let TraitReference::Inline(second_emitter) = &mut s.orbitals[0].traits[1] else {
                    panic!("inline fetcher fixture");
                };
                second_emitter.name = "F2".into();
                if reverse {
                    s.orbitals[0].traits.swap(0, 1);
                }
                assert_eq!(codes(&s), expected, "reverse: {reverse}; first: {first}; second: {second}; effects: {effects}");
            }
        }
    }

    #[test]
    fn s8_own_fetch_and_listen_route_union_restrictions_in_both_orders() {
        for reverse in [false, true] {
            for (incoming, own, effects, expected) in [
                (
                    r#""deliver": "ids", "total": "exact""#,
                    r#""deliver": "rows", "total": "none""#,
                    r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                    vec![ErrorCode::FetchDeliverIdsRowsRead, ErrorCode::FetchTotalNoneRead],
                ),
                (
                    r#""deliver": "ids", "total": "exact""#,
                    r#""deliver": "rows", "total": "none""#,
                    r#"["render-ui", "main", { "entity": "@payload.data" }]"#,
                    vec![],
                ),
                (
                    r#""deliver": "rows", "total": "exact""#,
                    r#""deliver": "rows", "total": "exact""#,
                    r#"["set", "@entity.row", "@payload.data.0.name"], ["set", "@entity.n", "@payload.totalCount"]"#,
                    vec![],
                ),
            ] {
                let own_schema = schema_of(&[&fetcher(own)]);
                let TraitReference::Inline(own_emitter) = &own_schema.orbitals[0].traits[0] else {
                    panic!("inline fetcher fixture");
                };
                let own_transition = own_emitter.state_machine.transitions[0].clone();
                let mut s = schema_of(&[
                    &fetcher(incoming),
                    &listener(F_SRC, effects),
                ]);
                let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[1] else {
                    panic!("inline listener fixture");
                };
                receiver.listens.as_mut().unwrap()[0].triggers = Some("Loaded".into());
                receiver.state_machine.transitions[0].event = "Loaded".into();
                receiver.state_machine.transitions.push(own_transition);
                if reverse {
                    s.orbitals[0].traits.swap(0, 1);
                }
                assert_eq!(codes(&s), expected, "reverse: {reverse}; incoming: {incoming}; own: {own}; effects: {effects}");
            }
        }
    }

    #[test]
    fn s8_explicit_source_does_not_reparse_a_qualified_event_name() {
        for (event, source, restricted) in [
            ("Other.Loaded", F_SRC, false),
            ("Loaded", F_SRC, true),
            ("F.Loaded", "null", true),
            ("Other.Loaded", "null", false),
        ] {
            let mut s = schema_of(&[
                &fetcher(IDS),
                &listener(source, r#"["set", "@entity.row", "@payload.data.0.name"]"#),
            ]);
            let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[1] else {
                panic!("inline listener fixture");
            };
            receiver.listens.as_mut().unwrap()[0].event = event.into();
            let expected = if restricted {
                vec![ErrorCode::FetchDeliverIdsRowsRead]
            } else {
                vec![]
            };
            assert_eq!(codes(&s), expected, "event: {event}; source: {source}");
        }
    }

    #[test]
    fn s8_alias_delivery_record_row_reads_are_restricted() {
        for effects in [
            r#"["set", "@entity.row", "@event.payload.data.0.id"]"#,
            r#"["set", "@entity.n", ["array/len", "@event.payload.data"]]"#,
            r#"["set", "@entity.row", ["object/get", "@event", "payload.data.0.id"]]"#,
            r#"["render-ui", "main", { "title": "row: @event.payload.data.0.id" }]"#,
        ] {
            let found = codes(&schema_with(IDS, effects, None));
            assert!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), "effects: {effects}; found: {found:?}");
            assert!(codes(&schema_with(r#""deliver": "rows""#, effects, None)).is_empty(), "rows control: {effects}");
        }
    }

    #[test]
    fn s8_alias_delivery_record_total_reads_are_restricted() {
        for effects in [
            r#"["set", "@entity.n", "@event.payload.totalCount"]"#,
            r#"["set", "@entity.n", ["+", "@event.payload.totalCount", 1]]"#,
            r#"["set", "@entity.n", ["object/get", "@event", "payload.totalCount"]]"#,
            r#"["render-ui", "main", { "title": "total: @event.payload.totalCount" }]"#,
        ] {
            let found = codes(&schema_with(r#""total": "none""#, effects, None));
            assert!(found.contains(&ErrorCode::FetchTotalNoneRead), "effects: {effects}; found: {found:?}");
            assert!(codes(&schema_with(r#""total": "exact""#, effects, None)).is_empty(), "exact control: {effects}");
        }
    }

    #[test]
    fn s8_alias_whole_payload_and_event_cannot_hide_row_reads() {
        for expression in [
            r#"["object/get", "@payload", "totalCount"]"#,
            r#"["object/get", "@event", "payload.totalCount"]"#,
            r#"["let", [["received", "@payload"]], ["object/get", "@received", "totalCount"]]"#,
            r#"["let", [["delivery", "@event"]], ["object/get", "@delivery", "payload.totalCount"]]"#,
        ] {
            let effects = format!(r#"["set", "@entity.n", {expression}]"#);
            assert!(codes(&schema_with(r#""deliver": "ids", "total": "exact""#, &effects, None)).is_empty(), "safe total of ids carrier: {expression}");
        }
        for expression in [
            r#"["object/get", "@payload", "data.0.id"]"#,
            r#"["object/get", "@event", "payload.data.0.id"]"#,
            r#"["let", [["received", "@payload"]], ["object/get", "@received", "data.0.id"]]"#,
            r#"["let", [["delivery", "@event"]], ["object/get", "@delivery", "payload.data.0.id"]]"#,
            r#"["array/len", ["object/get", "@payload", "data"]]"#,
            r#"["array/len", ["object/get", "@event", "payload.data"]]"#,
        ] {
            let effects = format!(r#"["set", "@entity.value", {expression}]"#);
            let found = codes(&schema_with(IDS, &effects, None));
            assert!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), "expression: {expression}; found: {found:?}");
            assert!(codes(&schema_with(r#""deliver": "rows""#, &effects, None)).is_empty(), "rows control: {expression}");
        }
    }

    #[test]
    fn s8_alias_whole_payload_and_event_cannot_hide_total_reads() {
        for expression in [
            r#"["object/get", "@payload", "data.0.id"]"#,
            r#"["object/get", "@event", "payload.data.0.id"]"#,
            r#"["let", [["received", "@payload"]], ["object/get", "@received", "data.0.id"]]"#,
            r#"["let", [["delivery", "@event"]], ["object/get", "@delivery", "payload.data.0.id"]]"#,
        ] {
            let effects = format!(r#"["set", "@entity.row", {expression}]"#);
            assert!(codes(&schema_with(r#""deliver": "rows", "total": "none""#, &effects, None)).is_empty(), "safe rows of total-none carrier: {expression}");
        }
        for expression in [
            r#"["object/get", "@payload", "totalCount"]"#,
            r#"["object/get", "@event", "payload.totalCount"]"#,
            r#"["let", [["received", "@payload"]], ["object/get", "@received", "totalCount"]]"#,
            r#"["let", [["delivery", "@event"]], ["object/get", "@delivery", "payload.totalCount"]]"#,
            r#"["+", ["object/get", "@payload", "totalCount"], 1]"#,
            r#"["+", ["object/get", "@event", "payload.totalCount"], 1]"#,
        ] {
            let effects = format!(r#"["set", "@entity.n", {expression}]"#);
            let found = codes(&schema_with(r#""total": "none""#, &effects, None));
            assert!(found.contains(&ErrorCode::FetchTotalNoneRead), "expression: {expression}; found: {found:?}");
            assert!(codes(&schema_with(r#""total": "exact""#, &effects, None)).is_empty(), "exact control: {expression}");
        }
    }

    #[test]
    fn s8_alias_whole_marker_render_props_keep_both_canonical_bindings_legal() {
        for reference in ["@payload.data", "@event.payload.data"] {
            for effects in [
                format!(r#"["render-ui", "main", {{ "entity": "{reference}" }}]"#),
                format!(r#"["when", true, ["render-ui", "main", {{ "entity": "{reference}" }}]]"#),
            ] {
                assert!(codes(&schema_with(r#""deliver": "ids", "total": "none""#, &effects, None)).is_empty(), "whole marker: {effects}");
            }
        }
    }

    #[test]
    fn s8_alias_listener_guards_read_the_raw_payload_before_constant_mapping() {
        for (guard, expected) in [
            (r#"["==", "@payload.data.0.id", "safe"]"#, ErrorCode::FetchDeliverIdsRowsRead),
            (r#"["==", "@event.payload.data.0.id", "safe"]"#, ErrorCode::FetchDeliverIdsRowsRead),
            (r#"["==", ["object/get", "@payload", "data.0.id"], "safe"]"#, ErrorCode::FetchDeliverIdsRowsRead),
            (r#"[">", "@payload.totalCount", 0]"#, ErrorCode::FetchTotalNoneRead),
            (r#"[">", "@event.payload.totalCount", 0]"#, ErrorCode::FetchTotalNoneRead),
            (r#"[">", ["object/get", "@payload", "totalCount"], 0]"#, ErrorCode::FetchTotalNoneRead),
        ] {
            for (options, restricted) in [
                (r#""deliver": "ids", "total": "none""#, true),
                (r#""deliver": "rows", "total": "exact""#, false),
            ] {
                let mut s = schema_of(&[
                    &fetcher(options),
                    &listener(F_SRC, r#"["render-ui", "main", null]"#),
                ]);
                let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[1] else {
                    panic!("inline listener fixture");
                };
                let listen = &mut receiver.listens.as_mut().unwrap()[0];
                listen.guard = Some(serde_json::from_str(guard).unwrap());
                listen.payload_mapping = Some(serde_json::from_str(r#"{ "data": { "id": "safe" }, "totalCount": 7 }"#).unwrap());
                let found = codes(&s);
                if restricted {
                    assert!(found.contains(&expected), "guard: {guard}; found: {found:?}");
                } else {
                    assert!(found.is_empty(), "rows/exact guard control: {guard}; found: {found:?}");
                }
            }
        }
    }

    #[test]
    fn s8_alias_listener_remaps_cannot_forward_restricted_values_under_new_names() {
        for (mapping, expected) in [
            (r#"{ "rows": "@payload.data" }"#, ErrorCode::FetchDeliverIdsRowsRead),
            (r#"{ "rows": ["object/get", "@payload", "data"] }"#, ErrorCode::FetchDeliverIdsRowsRead),
            (r#"{ "rows": ["let", [["received", "@payload"]], ["object/get", "@received", "data"]] }"#, ErrorCode::FetchDeliverIdsRowsRead),
            (r#"{ "count": "@payload.totalCount" }"#, ErrorCode::FetchTotalNoneRead),
            (r#"{ "count": ["object/get", "@payload", "totalCount"] }"#, ErrorCode::FetchTotalNoneRead),
            (r#"{ "envelope": "@payload" }"#, ErrorCode::FetchDeliverIdsRowsRead),
        ] {
            for (options, restricted) in [
                (r#""deliver": "ids", "total": "none""#, true),
                (r#""deliver": "rows", "total": "exact""#, false),
            ] {
                let mut s = schema_of(&[
                    &fetcher(options),
                    &listener(F_SRC, r#"["render-ui", "main", null]"#),
                ]);
                let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[1] else {
                    panic!("inline listener fixture");
                };
                receiver.listens.as_mut().unwrap()[0].payload_mapping = Some(serde_json::from_str(mapping).unwrap());
                let found = codes(&s);
                if restricted {
                    assert!(found.contains(&expected), "mapping: {mapping}; found: {found:?}");
                } else {
                    assert!(found.is_empty(), "rows/exact mapping control: {mapping}; found: {found:?}");
                }
            }
        }
    }

    #[test]
    fn s8_alias_dropped_fields_do_not_restrict_safe_mapped_payloads() {
        for (mapping, effects) in [
            (r#"{ "label": "ready" }"#, r#"["set", "@entity.label", ["str/concat", "label: ", "@payload.label"]]"#),
            (r#"{}"#, r#"["set", "@entity.copy", "@payload"]"#),
        ] {
            let mut s = schema_of(&[
                &fetcher(r#""deliver": "ids", "total": "none""#),
                &listener(F_SRC, effects),
            ]);
            let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[1] else {
                panic!("inline listener fixture");
            };
            receiver.listens.as_mut().unwrap()[0].payload_mapping = Some(serde_json::from_str(mapping).unwrap());
            assert!(codes(&s).is_empty(), "safe mapping: {mapping}; effects: {effects}");
        }
    }

    #[test]
    fn s8_alias_constant_replacements_are_not_restricted_source_fields() {
        for (mapping, effects) in [
            (r#"{ "data": { "id": "safe" }, "totalCount": 7 }"#, r#"["set", "@entity.row", "@payload.data.id"], ["set", "@entity.n", "@payload.totalCount"]"#),
            (r#"{ "data": { "id": "safe" } }"#, r#"["set", "@entity.row", "@payload.data.id"]"#),
            (r#"{ "totalCount": 7 }"#, r#"["set", "@entity.n", "@payload.totalCount"]"#),
        ] {
            let mut s = schema_of(&[
                &fetcher(r#""deliver": "ids", "total": "none""#),
                &listener(F_SRC, effects),
            ]);
            let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[1] else {
                panic!("inline listener fixture");
            };
            receiver.listens.as_mut().unwrap()[0].payload_mapping = Some(serde_json::from_str(mapping).unwrap());
            let found = codes(&s);
            assert!(found.is_empty(), "constant mapping: {mapping}; effects: {effects}; found: {found:?}");
        }
    }

    #[test]
    fn s8_alias_original_delivery_stays_restricted_after_payload_replacement() {
        for mapping in [
            r#"{ "data": { "id": "safe" }, "totalCount": 7 }"#,
            r#"{ "label": "ready" }"#,
            r#"{}"#,
        ] {
            for effects in [
                r#"["set", "@entity.row", "@event.payload.data.0.id"], ["set", "@entity.n", "@event.payload.totalCount"]"#,
                r#"["set", "@entity.row", ["object/get", "@event", "payload.data.0.id"]], ["set", "@entity.n", ["object/get", "@event", "payload.totalCount"]]"#,
            ] {
                for (options, restricted) in [
                    (r#""deliver": "ids", "total": "none""#, true),
                    (r#""deliver": "rows", "total": "exact""#, false),
                ] {
                    let mut s = schema_of(&[&fetcher(options), &listener(F_SRC, effects)]);
                    let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[1] else {
                        panic!("inline listener fixture");
                    };
                    receiver.listens.as_mut().unwrap()[0].payload_mapping = Some(serde_json::from_str(mapping).unwrap());
                    let found = codes(&s);
                    if restricted {
                        assert!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), "original delivery row; mapping: {mapping}; effects: {effects}; found: {found:?}");
                        assert!(found.contains(&ErrorCode::FetchTotalNoneRead), "original delivery total; mapping: {mapping}; effects: {effects}; found: {found:?}");
                    } else {
                        assert!(found.is_empty(), "rows/exact original-delivery control; mapping: {mapping}; effects: {effects}; found: {found:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn s8_scope_annotations_follow_sequential_binding_shadowing_and_restoration() {
        for (effects, restricted) in [
            (r#"["let", [["first", "@payload"], ["second", "@first"]], ["set", "@entity.row", ["object/get", "@second", "data.0.id"]]]"#, true),
            (r#"["let", [["received", "@payload"], ["received", { "data": { "id": "safe" } }]], ["set", "@entity.row", ["object/get", "@received", "data.id"]]]"#, false),
            (r#"["let", [["received", "@payload"]], ["do", ["let", [["received", { "data": { "id": "safe" } }]], ["set", "@entity.row", ["object/get", "@received", "data.id"]]], ["set", "@entity.row", ["object/get", "@received", "data.0.id"]]]]"#, true),
            (r#"["let", [[7, "@payload.data"], ["received", { "safe": true }, "@payload.data"]], ["set", "@entity.safe", "@received.safe"], "@payload.data"]"#, false),
        ] {
            let found = codes(&schema_with(IDS, effects, None));
            assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), restricted, "effects: {effects}; found: {found:?}");
        }
    }

    #[test]
    fn s8_scope_computed_render_values_do_not_gain_the_direct_marker_exemption() {
        for effects in [
            r#"["render-ui", "main", { "entity": ["object/get", "@payload", "data"] }]"#,
            r#"["let", [["received", "@payload"]], ["render-ui", "main", { "entity": "@received.data" }]]"#,
            r#"["render-ui", "main", { "entity": ["let", [["received", "@payload"]], "@payload.data"] }]"#,
        ] {
            let found = codes(&schema_with(IDS, effects, None));
            assert!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), "effects: {effects}; found: {found:?}");
            assert!(codes(&schema_with(r#""deliver": "rows""#, effects, None)).is_empty(), "rows control: {effects}");
        }
    }

    #[test]
    fn s8_projection_unknown_keys_and_defaults_have_paired_clean_controls() {
        for (options, expression, expected) in [
            (r#""deliver": "ids", "total": "exact""#, r#"["object/get", "@payload", "@config.field"]"#, Some(ErrorCode::FetchDeliverIdsRowsRead)),
            (r#""deliver": "rows", "total": "none""#, r#"["object/get", "@payload", "@config.field"]"#, Some(ErrorCode::FetchTotalNoneRead)),
            (r#""deliver": "rows", "total": "exact""#, r#"["object/get", "@payload", "@config.field"]"#, None),
            (IDS, r#"["object/get", { "safe": 1 }, "@config.field"]"#, None),
            (IDS, r#"["object/get", { "safe": 1 }, "@event.payload.data.0.id"]"#, Some(ErrorCode::FetchDeliverIdsRowsRead)),
            (IDS, r#"["object/get", { "safe": 1 }, "safe", "@event.payload.data"]"#, Some(ErrorCode::FetchDeliverIdsRowsRead)),
            (r#""deliver": "ids", "total": "exact""#, r#"["object/get", "@payload", "totalCount"]"#, None),
            (r#""deliver": "ids", "total": "exact""#, r#"["object/get", "@payload", "totalCount", 0]"#, None),
            (r#""deliver": "rows", "total": "none""#, r#"["object/get", "@payload", "totalCount", 0]"#, Some(ErrorCode::FetchTotalNoneRead)),
            (IDS, r#"["object/get", "@payload", "data", []]"#, Some(ErrorCode::FetchDeliverIdsRowsRead)),
        ] {
            let effects = format!(r#"["set", "@entity.value", {expression}]"#);
            let found = codes(&schema_with(options, &effects, None));
            match expected {
                Some(code) => assert!(found.contains(&code), "expression: {expression}; found: {found:?}"),
                None => assert!(found.is_empty(), "clean projection: {expression}; found: {found:?}"),
            }
        }
    }

    #[test]
    fn s8_projection_finite_key_alternatives_still_inspect_conditions() {
        for (expression, restricted) in [
            (r#"["object/get", "@payload", ["if", "@config.choose", "totalCount", "totalCount"]]"#, false),
            (r#"["object/get", "@payload", ["if", "@config.choose", "data", "totalCount"]]"#, true),
            (r#"["object/get", "@payload", ["if", ["==", "@event.payload.data.0.id", "safe"], "totalCount", "totalCount"]]"#, true),
            (r#"["object/get", "@payload", ["str/concat", "total", "Count"]]"#, true),
        ] {
            let effects = format!(r#"["set", "@entity.value", {expression}]"#);
            let found = codes(&schema_with(r#""deliver": "ids", "total": "exact""#, &effects, None));
            assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), restricted, "expression: {expression}; found: {found:?}");
        }
    }

    #[test]
    fn s8_mapping_annotations_clear_only_the_current_route_in_both_orders() {
        for reverse in [false, true] {
            for (effects, rows, total) in [
                (r#"["set", "@entity.row", "@payload.data.0.id"], ["set", "@entity.n", "@payload.totalCount"]"#, false, true),
                (r#"["set", "@entity.row", "@event.payload.data.0.id"], ["set", "@entity.n", "@event.payload.totalCount"]"#, true, true),
            ] {
                let mut s = schema_of(&[
                    &fetcher(r#""deliver": "ids", "total": "exact""#),
                    &fetcher(r#""deliver": "rows", "total": "none""#),
                    &listener(F_SRC, effects),
                ]);
                let TraitReference::Inline(second) = &mut s.orbitals[0].traits[1] else {
                    panic!("inline fetcher fixture");
                };
                second.name = "F2".into();
                let other = schema_of(&[&listener(r#"{ "kind": "trait", "trait": "F2" }"#, effects)]);
                let TraitReference::Inline(other_listener) = &other.orbitals[0].traits[0] else {
                    panic!("inline listener fixture");
                };
                let TraitReference::Inline(receiver) = &mut s.orbitals[0].traits[2] else {
                    panic!("inline listener fixture");
                };
                let listens = receiver.listens.as_mut().unwrap();
                listens[0].payload_mapping = Some(serde_json::from_str(r#"{ "data": { "id": "safe" }, "totalCount": 7 }"#).unwrap());
                listens.push(other_listener.listens.as_ref().unwrap()[0].clone());
                if reverse {
                    s.orbitals[0].traits.swap(0, 1);
                }
                let found = codes(&s);
                assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), rows, "reverse: {reverse}; effects: {effects}; found: {found:?}");
                assert_eq!(found.contains(&ErrorCode::FetchTotalNoneRead), total, "reverse: {reverse}; effects: {effects}; found: {found:?}");
            }
        }
    }

    #[test]
    fn s8_source_slots_keep_lambda_results_and_parameter_scopes_distinct() {
        for (expression, restricted) in [
            (r#"["object/get", ["array/reduce", "@config.items", {}, ["fn", ["acc", "item"], "@payload"]], "totalCount"]"#, false),
            (r#"["object/get", ["array/reduce", "@config.items", {}, ["fn", ["acc", "item"], "@payload"]], "data.0.id"]"#, true),
            (r#"["let", [["received", "@payload"]], ["array/reduce", "@config.items", {}, ["fn", ["received", "item"], ["object/get", "@received", "data"]]]]"#, false),
            (r#"["array/reduce", "@config.items", {}, ["fn", ["acc", "item"], "@event.payload.data.0.id"]]"#, true),
        ] {
            let effects = format!(r#"["set", "@entity.value", {expression}]"#);
            let found = codes(&schema_with(r#""deliver": "ids", "total": "exact""#, &effects, None));
            assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), restricted, "expression: {expression}; found: {found:?}");
            assert!(codes(&schema_with(r#""deliver": "rows", "total": "exact""#, &effects, None)).is_empty(), "rows control: {expression}");
        }
    }

    #[test]
    fn s8_unicode_direct_bindings_are_not_literal_projection_keys() {
        for (expression, restricted) in [
            (r#"["let", [["اسم", "@payload"]], ["object/get", "@اسم", "data.0.id"]]"#, true),
            (r#"["let", [["اسم", "@payload"]], ["object/get", "@اسم", "totalCount"]]"#, false),
            (r#"["let", [["اسم", "@payload"]], "@اسم.data.0.id"]"#, true),
            (r#"["let", [["مفتاح", "totalCount"]], ["object/get", "@payload", "@مفتاح"]]"#, true),
            (r#"["let", [["مفتاح", "safe"]], ["object/get", {"safe": true}, "@مفتاح"]]"#, false),
            (r#"["object/get", "@payload", "بيانات"]"#, false),
            (r#"["let", [["اسم", "@payload"]], "literal: @اسم.data.0.id"]"#, false),
        ] {
            let effects = format!(r#"["set", "@entity.value", {expression}]"#);
            let found = codes(&schema_with(r#""deliver": "ids", "total": "exact""#, &effects, None));
            assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), restricted, "expression: {expression}; found: {found:?}");
            assert!(codes(&schema_with(r#""deliver": "rows", "total": "exact""#, &effects, None)).is_empty(), "rows control: {expression}");
        }
    }

    #[test]
    fn s8_mapping_provenance_uses_only_the_roots_bound_by_the_kernel() {
        for (mapping, restricted) in [
            (r#"{ "rows": "@payload.data" }"#, true),
            (r#"{ "rows": "@event.payload.data" }"#, false),
        ] {
            let mut schema = schema_of(&[
                &fetcher(IDS),
                &listener(F_SRC, r#"["set", "@entity.rows", "@payload.rows"]"#),
            ]);
            let TraitReference::Inline(receiver) = &mut schema.orbitals[0].traits[1] else {
                panic!("inline listener fixture");
            };
            receiver.listens.as_mut().unwrap()[0].payload_mapping = Some(serde_json::from_str(mapping).unwrap());
            let found = codes(&schema);
            assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), restricted, "mapping: {mapping}; found: {found:?}");
        }
    }

    #[test]
    fn s8_lambda_header_scope_survives_runtime_ignored_let_items() {
        for (reference, restricted) in [("@original", true), ("@received", false)] {
            let effects = format!(r#"["let", [["received", "@payload"], ["original", "@payload"]],
                ["set", "@entity.value", ["array/reduce", "@config.items", {{}},
                    ["fn", ["received", "item"],
                        ["let", [[7, "@payload.data"], ["safe", true, "@payload.data"]],
                            ["object/get", "{reference}", "data.0.id"], "@payload.data"]]]]]"#);
            let found = codes(&schema_with(r#""deliver": "ids", "total": "exact""#, &effects, None));
            assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), restricted, "reference: {reference}; found: {found:?}");
            assert!(codes(&schema_with(r#""deliver": "rows", "total": "exact""#, &effects, None)).is_empty(), "rows control: {reference}");
        }
    }

    #[test]
    fn s8_literal_array_carriers_preserve_indexed_fields_without_weakening_opaque_calls() {
        for (options, rows, total) in [
            (r#""deliver": "ids", "total": "exact""#, true, false),
            (r#""deliver": "rows", "total": "none""#, false, true),
            (r#""deliver": "rows", "total": "exact""#, false, false),
        ] {
            let found = codes(&schema_with(options, r#"["set", "@entity.text", ["json/stringify", "@payload"]]"#, None));
            assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), rows, "opaque call; options: {options}; found: {found:?}");
            assert_eq!(found.contains(&ErrorCode::FetchTotalNoneRead), total, "opaque call; options: {options}; found: {found:?}");
            if !rows && !total {
                assert!(found.is_empty(), "clean opaque-call control: {found:?}");
            }
        }
        for (array, index) in [
            (r#"["ordinary literal", "@payload"]"#, 1),
            (r#"[0, "@payload"]"#, 1),
            (r#"["@payload", "ordinary literal"]"#, 0),
        ] {
            for (options, field, rows, total) in [
                (r#""deliver": "ids", "total": "exact""#, "totalCount", false, false),
                (r#""deliver": "rows", "total": "none""#, "totalCount", false, true),
                (r#""deliver": "ids", "total": "exact""#, "data", true, false),
                (r#""deliver": "rows", "total": "exact""#, "data", false, false),
            ] {
                let effects = format!(r#"["let", [["carrier", {array}]], ["set", "@entity.value", ["object/get", "@carrier", "{index}.{field}"]]]"#);
                let found = codes(&schema_with(options, &effects, None));
                assert_eq!(found.contains(&ErrorCode::FetchDeliverIdsRowsRead), rows, "array: {array}; field: {field}; options: {options}; found: {found:?}");
                assert_eq!(found.contains(&ErrorCode::FetchTotalNoneRead), total, "array: {array}; field: {field}; options: {options}; found: {found:?}");
                if !rows && !total {
                    assert!(found.is_empty(), "clean literal-array control: {found:?}");
                }
            }
        }
    }
    fn nearest_program(options: &str) -> OrbitalSchema {
        serde_json::from_str(&format!(
            r#"{{ "name": "S", "orbitals": [{{ "name": "O",
              "entity": {{ "name": "Doc", "persistence": "runtime", "fields": [
                {{ "name": "id", "type": "string" }}, {{ "name": "title", "type": "string" }},
                {{ "name": "count", "type": "number" }},
                {{ "name": "embedding", "type": "array", "items": {{ "name": "component", "type": "number" }} }}] }},
              "traits": [{{ "name": "T", "linkedEntity": "Doc", "stateMachine": {{
                "states": [{{ "name": "idle", "isInitial": true }}],
                "transitions": [{{ "from": "idle", "to": "idle", "event": "INIT",
                  "effects": [["fetch", "Doc", {{ {options} }}]] }}] }} }}] }}] }}"#
        ))
        .expect("nearest program")
    }

    fn nearest_codes(options: &str) -> Vec<ErrorCode> {
        crate::phases::validation::validate_comprehensive_without_services(&nearest_program(options))
            .errors
            .iter()
            .map(|e| e.code.clone())
            .filter(|c| matches!(c, ErrorCode::OperatorArgTypeMismatch))
            .collect()
    }

    #[test]
    fn a_well_formed_nearest_validates_and_a_mistyped_one_is_refused() {
        let nearest = |body: &str| format!(r#""nearest": {{ {body} }}"#);
        let good = nearest(r#""field": "embedding", "vector": "@entity.embedding", "k": 3"#);
        assert!(nearest_codes(&good).is_empty(), "{:?}", nearest_codes(&good));
        let with_filter = format!(r#""filter": ["=", ["object/get", "@entity", "title"], "a"], {good}"#);
        assert!(nearest_codes(&with_filter).is_empty());
        let dynamic_k = nearest(r#""field": "embedding", "vector": "@entity.embedding", "k": "@entity.count""#);
        assert!(nearest_codes(&dynamic_k).is_empty());

        for body in [
            r#""field": "embedding", "vector": "@entity.title", "k": 3"#,
            r#""field": "embedding", "vector": "@entity.embedding", "k": "@entity.title""#,
        ] {
            assert_eq!(nearest_codes(&nearest(body)), vec![ErrorCode::OperatorArgTypeMismatch], "{body}");
        }
    }
}
