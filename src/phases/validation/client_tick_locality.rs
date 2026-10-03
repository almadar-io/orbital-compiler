//! ORB_T_CLIENT_TICK_NOT_LOCAL (error, rung 2).
//!
//! A client tick (any tick not `[background]`) advances a `[runtime]` frame
//! that exists only in the client. A trait writing that frame that dispatches
//! `RuntimeOptimistic` (its own effects reach the server —
//! `runtime::dispatch_mode`) posts to a stateful host whose never-ticked copy
//! folds back over the live frame (riya, 2026-09-24: the body reset to its
//! spawn on every response). Writers of the frame: the ticking trait itself,
//! and for a `[shared]` entity every trait bound to it.

use crate::phases::validation::{ErrorCode, ValidationError, ValidationResult};
use orbital_core::runtime::dispatch_mode::DispatchMode;
use orbital_core::schema::{
    EntityRef, OrbitalSchema, Persistence, TraitDefinition, TraitReference,
};

pub(crate) fn validate_client_tick_locality(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let residence = orbital_core::effects::Residence::of_entities(schema.orbitals.iter().flat_map(|o| {
        std::iter::once(&o.entity)
            .chain(o.auxiliary_entities.iter().flatten())
            .filter_map(|e| match e {
                EntityRef::Inline(def) => Some(def),
                _ => None,
            })
    }));
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
                if !writes_ticked_frame
                    || t.dispatch_mode(Some(entity), &residence) != DispatchMode::RuntimeOptimistic
                {
                    continue;
                }
                result.add_error(
                    ValidationError::error(
                        ErrorCode::ClientTickNotLocal,
                        format!("orbitals.{}.traits.{}", orbital.name, t.name),
                        format!(
                            "Trait '{}' posts to the server (its own effects reach it) but writes `{}`, a client-held frame `{ticker}` ticks on the client: the host's never-ticked copy folds back over the live frame",
                            t.name, entity.name
                        ),
                    )
                    .with_suggestion(format!(
                        "Move the server work ({}'s fetch/persist/call-service) to a trait bound to another entity, or declare '{}' local",
                        t.name, t.name
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
    /// may carry a server effect.
    fn schema(
        shared: bool,
        play_fetches: bool,
        play_local: bool,
        background: bool,
    ) -> OrbitalSchema {
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
                    { "name": "Body", "linkedEntity": "World",
                      "ticks": [{ "name": "physics", "interval": 33, "runsInBackground": background, "effects": [["set", "@entity.x", 1]] }],
                      "stateMachine": { "states": [{ "name": "idle", "isInitial": true }], "transitions": [] } },
                    { "name": "Play", "linkedEntity": "World", "local": play_local,
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
    fn a_server_bound_trait_on_a_shared_client_ticked_frame_is_an_error() {
        assert_eq!(
            flagged(&schema(true, true, false, false)),
            vec!["orbitals.Level.traits.Play"]
        );
    }

    #[test]
    fn control_client_only_traits_on_a_ticked_frame_are_clean() {
        assert!(flagged(&schema(true, false, false, false)).is_empty());
    }

    #[test]
    fn control_a_declared_local_trait_is_clean() {
        assert!(flagged(&schema(true, true, true, false)).is_empty());
    }

    #[test]
    fn control_a_non_shared_frame_is_only_the_tickers_own() {
        assert!(flagged(&schema(false, true, false, false)).is_empty());
    }

    #[test]
    fn control_a_background_tick_runs_on_the_host() {
        assert!(flagged(&schema(true, true, false, true)).is_empty());
    }

    #[test]
    fn control_a_persisted_entity_is_held_by_the_host() {
        let mut s = schema(true, true, false, false);
        if let EntityRef::Inline(e) = &mut s.orbitals[0].entity {
            e.persistence = Some(Persistence::Persistent);
        }
        assert!(flagged(&s).is_empty());
    }
}
