//! Behavior-value literals (`{behavior, trait}` and `{behavior, orbital}` objects)
//! and `unquote` placement in the schema's S-expression positions.
//!
//! Every trait value must name a trait, and every orbital value an orbital, of an
//! installed behavior; the resolution is the loader's own (the same one `uses` takes). A resolved
//! `behavior` is rewritten to `<owning package orb.prefix>/<name>`; a workspace
//! relative `./` spec is left as written. An `["unquote", …]` call outside a
//! quasiquote body is rejected here too: a quasiquote body is an encoded string,
//! so any `unquote` array reaching these positions is a stray one.

use super::InlineError;
use crate::loader::{ExternalOrbitalLoader, ImportChain};
use orbital_core::evaluator::Value;
use orbital_core::schema::behavior_value::{OrbitalValueRef, TraitValueRef};
use orbital_core::schema::quote::UNQUOTE_HEAD;
use orbital_core::schema::{
    EntityRef, FieldDefinition, OrbitalSchema, SExpression, TraitReference,
};
use std::collections::HashMap;
use std::path::Path;

struct Pass<'a> {
    loader: &'a mut ExternalOrbitalLoader,
    chain: &'a ImportChain,
    source_path: Option<&'a Path>,
    resolved: HashMap<(String, String), Result<String, String>>,
}

/// What a behavior-value object names inside its behavior.
enum Member {
    Trait(String),
    Orbital(String),
}

impl Member {
    fn literal(&self, behavior: &str) -> String {
        match self {
            Member::Trait(t) => format!("{behavior}.traits.{t}"),
            Member::Orbital(o) => format!("{behavior}.orbitals.{o}"),
        }
    }
}

pub(crate) fn resolve_trait_value_refs(
    schema: &mut OrbitalSchema,
    loader: &mut ExternalOrbitalLoader,
    chain: &ImportChain,
    source_path: Option<&Path>,
) -> Result<(), InlineError> {
    let mut pass = Pass {
        loader,
        chain,
        source_path,
        resolved: HashMap::new(),
    };
    for orbital in &mut schema.orbitals {
        let base = format!("orbitals[{}]", orbital.name);
        match &mut orbital.entity {
            EntityRef::Inline(ed) => {
                for f in &mut ed.fields {
                    pass.field(f, &format!("{base}.entity"))?;
                }
            }
            EntityRef::Extends(ere) => {
                for f in ere.fields.iter_mut().flatten() {
                    pass.field(f, &format!("{base}.entity"))?;
                }
            }
            EntityRef::Reference(_) => {}
        }
        for tref in &mut orbital.traits {
            match tref {
                TraitReference::Inline(td) => {
                    let tbase = format!("{base}.traits[{}]", td.name);
                    for (i, tr) in td.state_machine.transitions.iter_mut().enumerate() {
                        let p = format!("{tbase}.stateMachine.transitions[{i}]");
                        if let Some(g) = tr.guard.as_mut() {
                            pass.expr(g, &format!("{p}.guard"))?;
                        }
                        for (j, e) in tr.effects.iter_mut().flatten().enumerate() {
                            pass.expr(e, &format!("{p}.effects[{j}]"))?;
                        }
                    }
                    for (i, tick) in td.ticks.iter_mut().flatten().enumerate() {
                        let p = format!("{tbase}.ticks[{i}]");
                        if let Some(g) = tick.guard.as_mut() {
                            pass.expr(g, &format!("{p}.guard"))?;
                        }
                        for (j, e) in tick.effects.iter_mut().enumerate() {
                            pass.expr(e, &format!("{p}.effects[{j}]"))?;
                        }
                    }
                    for (k, cf) in td.config.iter_mut().flatten() {
                        if let Some(d) = cf.default.as_mut() {
                            pass.expr(d, &format!("{tbase}.config.{k}.default"))?;
                        }
                    }
                }
                TraitReference::Reference {
                    config,
                    name,
                    reference,
                    ..
                } => {
                    let label = name
                        .clone()
                        .or_else(|| reference.clone())
                        .unwrap_or_default();
                    for (k, cf) in config.iter_mut().flatten() {
                        if let Some(d) = cf.default.as_mut() {
                            pass.expr(d, &format!("{base}.traits[{label}].config.{k}.default"))?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

impl Pass<'_> {
    fn field(&mut self, f: &mut FieldDefinition, path: &str) -> Result<(), InlineError> {
        let here = format!("{path}.fields[{}]", f.name.clone().unwrap_or_default());
        if let Some(d) = f.default.as_mut() {
            self.expr(d, &format!("{here}.default"))?;
        }
        if let Some(items) = f.items.as_mut() {
            self.field(items, &here)?;
        }
        for p in f.properties.iter_mut().flat_map(|m| m.values_mut()) {
            self.field(p, &here)?;
        }
        Ok(())
    }

    fn expr(&mut self, expr: &mut SExpression, path: &str) -> Result<(), InlineError> {
        match expr {
            SExpression::Array(items) => {
                if matches!(items.first(), Some(SExpression::String(h)) if h == UNQUOTE_HEAD) {
                    return Err(InlineError::UnquoteOutsideQuasiquote {
                        path: path.to_string(),
                    });
                }
                for item in items {
                    self.expr(item, path)?;
                }
            }
            SExpression::Object(map) => {
                let as_value = Value::from(SExpression::Object(map.clone()));
                let named = TraitValueRef::from_value(&as_value)
                    .map(|r| (r.behavior, Member::Trait(r.trait_name)))
                    .or_else(|| {
                        OrbitalValueRef::from_value(&as_value)
                            .map(|r| (r.behavior, Member::Orbital(r.orbital)))
                    });
                if let Some((behavior, member)) = named {
                    let canonical = self.resolve(&behavior, &member, path)?;
                    map.insert("behavior".to_string(), SExpression::String(canonical));
                }
                for (key, v) in map.iter_mut() {
                    if key != "behavior" && key != "trait" && key != "orbital" {
                        self.expr(v, path)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn resolve(&mut self, behavior: &str, member: &Member, path: &str) -> Result<String, InlineError> {
        let key = (behavior.to_string(), member.literal(behavior));
        if !self.resolved.contains_key(&key) {
            let outcome = self.lookup(behavior, member);
            self.resolved.insert(key.clone(), outcome);
        }
        self.resolved[&key]
            .clone()
            .map_err(|reason| InlineError::BehaviorRefUnresolved {
                literal: member.literal(behavior),
                path: path.to_string(),
                reason,
            })
    }

    fn lookup(&mut self, behavior: &str, member: &Member) -> Result<String, String> {
        let mut chain = self.chain.clone_chain();
        let loaded = self
            .loader
            .load(behavior, self.source_path, &mut chain)
            .map_err(|e| e.to_string())?;
        let found = match member {
            Member::Trait(trait_name) => loaded.schema.orbitals.iter().any(|o| {
                o.traits.iter().any(|t| match t {
                    TraitReference::Inline(td) => &td.name == trait_name,
                    TraitReference::Reference { name, .. } => name.as_deref() == Some(trait_name.as_str()),
                })
            }),
            Member::Orbital(orbital) => loaded.schema.orbitals.iter().any(|o| &o.name == orbital),
        };
        if !found {
            return Err(match member {
                Member::Trait(t) => format!("behavior '{behavior}' declares no trait '{t}'"),
                Member::Orbital(o) => format!("behavior '{behavior}' declares no orbital '{o}'"),
            });
        }
        if behavior.starts_with("./") {
            return Ok(behavior.to_string());
        }
        let source = loaded.source_path.clone();
        let name = behavior.rsplit('/').next().unwrap_or(behavior);
        Ok(match self.loader.owning_prefix(&source) {
            Some(prefix) => format!("{prefix}/{name}"),
            None => behavior.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::LoaderConfig;
    use std::path::PathBuf;

    fn base_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn schema_with(default: serde_json::Value, effect: serde_json::Value) -> OrbitalSchema {
        serde_json::from_value(serde_json::json!({
            "name": "App",
            "orbitals": [{
                "name": "Holder",
                "entity": {
                    "name": "Panel",
                    "persistence": "runtime",
                    "fields": [
                        { "name": "id", "type": "string", "required": true },
                        { "name": "panel", "type": "trait", "default": default }
                    ]
                },
                "traits": [{
                    "name": "Hold",
                    "linkedEntity": "Panel",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "GO" }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": [effect] }]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("schema fixture")
    }

    fn run(schema: &mut OrbitalSchema) -> Result<(), InlineError> {
        let mut loader = ExternalOrbitalLoader::new(LoaderConfig {
            base_path: base_dir(),
            ..Default::default()
        });
        resolve_trait_value_refs(schema, &mut loader, &ImportChain::new(), None)
    }

    fn default_of(schema: &OrbitalSchema) -> SExpression {
        let EntityRef::Inline(ed) = &schema.orbitals[0].entity else {
            panic!("inline entity")
        };
        ed.fields[1].default.clone().expect("default")
    }

    fn tv(behavior: &str, trait_name: &str) -> serde_json::Value {
        serde_json::json!({ "behavior": behavior, "trait": trait_name })
    }

    #[test]
    fn a_resolving_relative_trait_value_stays_as_written() {
        let mut schema = schema_with(
            tv("./loader/fixtures/health", "HealthTrait"),
            serde_json::json!(["log", "x"]),
        );
        run(&mut schema).expect("resolves");
        assert_eq!(
            default_of(&schema),
            serde_json::from_value::<SExpression>(tv("./loader/fixtures/health", "HealthTrait"))
                .unwrap()
        );
    }

    fn ov(behavior: &str, orbital: &str) -> serde_json::Value {
        serde_json::json!({ "behavior": behavior, "orbital": orbital })
    }

    #[test]
    fn a_resolving_orbital_value_is_accepted() {
        let mut schema = schema_with(serde_json::Value::Null, serde_json::json!(["set", "@entity.id", ov("./loader/fixtures/health", "Health")]));
        run(&mut schema).expect("resolves");
    }

    #[test]
    fn control_an_unknown_orbital_of_a_known_behavior_is_unresolved() {
        let mut schema = schema_with(serde_json::Value::Null, serde_json::json!(["set", "@entity.id", ov("./loader/fixtures/health", "NoSuchOrbital")]));
        let err = run(&mut schema).unwrap_err();
        assert_eq!(err.structured_code(), "ORB_T_BEHAVIOR_REF_UNRESOLVED");
        assert!(err.to_string().contains("declares no orbital 'NoSuchOrbital'"), "{err}");
    }

    #[test]
    fn an_unknown_behavior_is_unresolved() {
        let mut schema = schema_with(
            tv("./loader/fixtures/missing", "HealthTrait"),
            serde_json::json!(["log", "x"]),
        );
        let err = run(&mut schema).unwrap_err();
        assert_eq!(err.structured_code(), "ORB_T_BEHAVIOR_REF_UNRESOLVED");
        assert!(
            err.structured_path().ends_with("fields[panel].default"),
            "{}",
            err.structured_path()
        );
    }

    #[test]
    fn an_unknown_trait_of_a_known_behavior_is_unresolved() {
        let mut schema = schema_with(
            tv("./loader/fixtures/health", "NoSuchTrait"),
            serde_json::json!(["log", "x"]),
        );
        let err = run(&mut schema).unwrap_err();
        assert_eq!(err.structured_code(), "ORB_T_BEHAVIOR_REF_UNRESOLVED");
        assert!(err.to_string().contains("declares no trait"), "{err}");
    }

    #[test]
    fn a_trait_value_inside_an_effect_is_checked_too() {
        let mut schema = schema_with(
            serde_json::Value::Null,
            serde_json::json!([
                "set",
                "@entity.panel",
                tv("./loader/fixtures/missing", "HealthTrait")
            ]),
        );
        assert!(matches!(
            run(&mut schema),
            Err(InlineError::BehaviorRefUnresolved { .. })
        ));
    }

    #[test]
    fn control_a_plain_object_and_null_default_are_untouched() {
        let mut schema = schema_with(
            serde_json::json!({ "behavior": "not a spec", "trait": "x" }),
            serde_json::json!(["set", "@entity.id", { "label": "y" }]),
        );
        run(&mut schema).expect("not trait values");
        let mut schema = schema_with(serde_json::Value::Null, serde_json::json!(["log", "x"]));
        run(&mut schema).expect("null default");
    }

    #[test]
    fn a_stray_unquote_is_rejected_with_its_code() {
        let mut schema = schema_with(
            serde_json::Value::Null,
            serde_json::json!(["set", "@entity.id", ["unquote", "@entity.id"]]),
        );
        let err = run(&mut schema).unwrap_err();
        assert_eq!(err.structured_code(), "ORB_T_UNQUOTE_OUTSIDE_QUASIQUOTE");
    }

    #[test]
    fn control_an_unquote_inside_a_quasiquote_body_is_not_a_call() {
        let body = orbital_core::schema::quote::quasiquote_expr(
            &serde_json::from_value::<SExpression>(serde_json::json!([
                "list",
                ["unquote", "@entity.id"]
            ]))
            .unwrap(),
        )
        .expect("quasiquote");
        let effect = serde_json::json!(["set", "@entity.id", serde_json::to_value(&body).unwrap()]);
        let mut schema = schema_with(serde_json::Value::Null, effect);
        run(&mut schema).expect("the encoded body hides the unquote");
    }

    #[test]
    fn inline_error_codes_match_the_validator_vocabulary() {
        use crate::phases::validation::ErrorCode;
        assert_eq!(
            ErrorCode::TraitBehaviorRefUnresolved.as_str(),
            "ORB_T_BEHAVIOR_REF_UNRESOLVED"
        );
        assert_eq!(
            ErrorCode::TraitUnquoteOutsideQuasiquote.as_str(),
            "ORB_T_UNQUOTE_OUTSIDE_QUASIQUOTE"
        );
    }
}
