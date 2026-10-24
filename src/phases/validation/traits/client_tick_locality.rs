//! ORB_T_CLIENT_TICK_NOT_LOCAL (error, rung 2).
//!
//! A client tick (any tick not `[background]`) advances a `[runtime]` frame
//! that exists only in the client. A trait writing that frame that is not
//! `[client]` while its own effects reach the server posts to a stateful host
//! whose never-ticked copy folds back over the live frame (riya, 2026-09-24: the body reset to its
//! spawn on every response). Writers of the frame: the ticking trait itself,
//! and for a `[shared]` entity every trait bound to it.

use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core_base::schema::{DispatchMode, EntityRef, OrbitalSchema, Persistence, TraitDefinition, TraitReference};

pub(crate) fn validate_client_tick_locality(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for orbital in &schema.orbitals {
        let entities: Vec<_> = std::iter::once(&orbital.entity)
            .chain(orbital.auxiliary_entities.iter().flatten())
            .filter_map(|e| match e {
                EntityRef::Inline(def) => Some(def),
                _ => None,
            })
            .collect();
        let primary = match &orbital.entity {
            EntityRef::Inline(def) => Some(def.name.as_str()),
            _ => None,
        };
        let traits: Vec<&TraitDefinition> = orbital
            .traits
            .iter()
            .filter_map(|t| match t {
                TraitReference::Inline(def) => Some(def),
                _ => None,
            })
            .collect();
        let ticks_on_client = |t: &TraitDefinition| {
            t.ticks
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .any(|tick| !tick.background)
        };
        for entity in entities {
            if !matches!(entity.persistence, Some(Persistence::Runtime)) {
                continue;
            }
            let bound: Vec<&TraitDefinition> = traits
                .iter()
                .copied()
                .filter(|t| t.linked_entity.as_deref().or(primary) == Some(entity.name.as_str()))
                .collect();
            let Some(ticker) = bound
                .iter()
                .find(|t| ticks_on_client(t))
                .map(|t| t.name.clone())
            else {
                continue;
            };
            for t in &bound {
                let writes_ticked_frame = entity.shared || ticks_on_client(t);
                if !writes_ticked_frame || matches!(
                    t.dispatch,
                    Some(DispatchMode::Client | DispatchMode::Optimistic)
                ) {
                    continue;
                }
                result.add_error(
                    ValidationError::error(
                        ErrorCode::ClientTickNotLocal,
                        format!("orbitals.{}.traits.{}", orbital.name, t.name),
                        format!(
                            "Trait '{}' writes `{}`, a frame `{ticker}` ticks on the client, but declares no dispatch mode: a server-run step folds the host's never-ticked copy back over the live frame",
                            t.name, entity.name
                        ),
                    )
                    .with_suggestion(format!(
                        "Declare '{}' `[client]` or `[optimistic]`, or move its server work to a trait bound to another entity",
                        t.name
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

    /// `Body` ticks the World frame on the client; `Play` is bound to it and
    /// may carry a server effect. Each trait's dispatch mode is declared (or absent).
    fn schema(
        shared: bool,
        play_fetches: bool,
        play_mode: Option<&str>,
        body_mode: Option<&str>,
        background: bool,
    ) -> OrbitalSchema {
        let mode = |m: Option<&str>| m.map_or(serde_json::Value::Null, |m| serde_json::json!(m));
        let play_effects = if play_fetches {
            serde_json::json!([["fetch", "Row", {}]])
        } else {
            serde_json::json!([])
        };
        serde_json::from_value(serde_json::json!({
            "name": "Game",
            "orbitals": [{
                "name": "Level",
                "entity": { "name": "World", "persistence": "runtime", "shared": shared,
                            "fields": [{ "name": "id", "type": "string" }, { "name": "x", "type": "number" }] },
                "traits": [
                    { "name": "Body", "linkedEntity": "World", "dispatch": mode(body_mode),
                      "ticks": [{ "name": "physics", "interval": 33, "runsInBackground": background, "effects": [["set", "@entity.x", 1]] }],
                      "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] } },
                    { "name": "Play", "linkedEntity": "World", "dispatch": mode(play_mode),
                      "stateMachine": { "states": [{ "name": "idle", "isInitial": true }],
                        "transitions": [{ "from": "idle", "to": "idle", "event": "GO", "effects": play_effects }] } }
                ]
            }]
        }))
        .unwrap()
    }

    fn flagged(s: &OrbitalSchema) -> Vec<String> {
        validate_client_tick_locality(s)
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::ClientTickNotLocal)
            .map(|e| e.path.clone())
            .collect()
    }

    #[test]
    fn traits_writing_a_shared_client_ticked_frame_without_a_declared_mode_are_errors() {
        assert_eq!(
            flagged(&schema(true, true, None, None, false)),
            vec!["orbitals.Level.traits.Body", "orbitals.Level.traits.Play"]
        );
    }

    #[test]
    fn the_rule_reads_the_declared_mode_not_the_effects() {
        assert_eq!(
            flagged(&schema(true, false, None, Some("client"), false)),
            vec!["orbitals.Level.traits.Play"],
            "a writer with no server effect still needs a declared mode"
        );
    }

    #[test]
    fn control_client_and_optimistic_traits_are_clean() {
        assert!(flagged(&schema(true, true, Some("client"), Some("client"), false)).is_empty());
        assert!(flagged(&schema(true, true, Some("optimistic"), Some("optimistic"), false)).is_empty());
    }

    #[test]
    fn an_explicit_server_mode_is_still_a_server_run_writer() {
        assert_eq!(
            flagged(&schema(true, true, Some("server"), Some("client"), false)),
            vec!["orbitals.Level.traits.Play"]
        );
    }

    #[test]
    fn control_a_non_shared_frame_is_only_the_tickers_own() {
        assert_eq!(flagged(&schema(false, true, None, None, false)), vec!["orbitals.Level.traits.Body"]);
        assert!(flagged(&schema(false, true, None, Some("client"), false)).is_empty());
    }

    #[test]
    fn control_a_background_tick_runs_on_the_host() {
        assert!(flagged(&schema(true, true, None, None, true)).is_empty());
    }

    #[test]
    fn control_a_persisted_entity_is_held_by_the_host() {
        let mut s = schema(true, true, None, None, false);
        if let EntityRef::Inline(e) = &mut s.orbitals[0].entity {
            e.persistence = Some(Persistence::Persistent);
        }
        assert!(flagged(&s).is_empty());
    }
}
