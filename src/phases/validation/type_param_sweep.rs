//! Defensive sweep for Increment 3-I's `(E : Entity)` alias parameter and
//! Increment 3-II's trait-level `(V : Type, ...)` parameters
//! (`docs/Almadar_LOLO.md` ~line 315/1387).
//!
//! The inline phase (`phases::inline::rewrite::resolve_type_param_sentinels`)
//! substitutes the `"$<param>"` / `"[$<param>]"` payload sentinel
//! `orbital-lolo` lowers — for a `type X (E : Entity) = Event { row : E }`
//! alias reference (3-I) OR a trait's own `(V : Type)` header used directly
//! in its `emits`/`listens`/`config` (3-II) — once a substitution is known:
//! the using trait's bound entity (3-I, and 3-II's Entity-kind default), or
//! an explicit call-site `:: <param> <arg>` argument (3-II). A sentinel that
//! survives inline means neither was available — blocking: the emitted
//! `.orb`/codegen would otherwise carry a literal `"$<param>"` type, which is
//! not a real type in any downstream shell.

use orbital_core::schema::{PayloadField, TraitDefinition};

use super::{
    error::{ErrorCode, ValidationError},
    inline_traits, ValidationContext,
};

const ENTITY_GUIDANCE: &str = "Bind the trait to a `linkedEntity` (directly, via `-> Entity`, or via a call-site rebind) so the parameter has an entity to monomorphize against.";
const TYPE_GUIDANCE: &str = "Supply an explicit call-site argument for this parameter — `:: <param> <arg>` on the trait reference (a primitive type name, or an entity name) — this Type-kind parameter has no default.";
const GENERIC_GUIDANCE: &str = "Bind the trait to a `linkedEntity` so an Entity-kind parameter can default to it, or supply an explicit call-site `:: <param> <arg>` argument.";

fn is_sentinel(field_type: &str) -> bool {
    field_type.starts_with('$')
        || (field_type.starts_with("[$") && field_type.ends_with(']'))
        // The `@entity` bound-row self-reference is the param-less sibling of
        // an Entity-kind default: unresolved means the trait has no
        // linkedEntity to monomorphize against.
        || field_type == "@entity"
        || field_type == "[@entity]"
}

fn sentinel_param_name(field_type: &str) -> &str {
    if let Some(inner) = field_type.strip_prefix("[$").and_then(|s| s.strip_suffix(']')) {
        inner
    } else {
        field_type.trim_start_matches('$')
    }
}

/// Item 6: distinguish "no linked entity" (Entity-kind param) from "no
/// `:: <param> <arg>` supplied" (Type-kind param) when the carrying trait's
/// own declared `type_params` (Increment 3-II) name this sentinel's param —
/// otherwise (the 3-I alias-level case, no declared trait param of this
/// name) falls back to the original combined message.
fn check_payload_field(
    field: &PayloadField,
    path: &str,
    trait_def: &TraitDefinition,
    result: &mut super::ValidationResult,
) {
    let field_path = format!("{path}.{}", field.name);
    if is_sentinel(&field.field_type) {
        let param_name = sentinel_param_name(&field.field_type);
        let declared_kind = trait_def
            .type_params
            .as_ref()
            .and_then(|params| params.iter().find(|p| p.name == param_name))
            .map(|p| p.kind.as_str());
        let (reason, guidance) = match declared_kind {
            Some("Entity") => (
                format!(
                    "payload field '{}' carries the unresolved type-parameter sentinel '{}' — \
                     Entity-kind parameter '{param_name}' has no `linkedEntity` to monomorphize against",
                    field.name, field.field_type
                ),
                ENTITY_GUIDANCE,
            ),
            Some(_) => (
                format!(
                    "payload field '{}' carries the unresolved type-parameter sentinel '{}' — \
                     parameter '{param_name}' was not supplied a `:: {param_name} <arg>` \
                     call-site argument",
                    field.name, field.field_type
                ),
                TYPE_GUIDANCE,
            ),
            None => (
                format!(
                    "payload field '{}' carries the unresolved type-parameter sentinel '{}' — \
                     the compiler could not monomorphize the parameter",
                    field.name, field.field_type
                ),
                GENERIC_GUIDANCE,
            ),
        };
        result.add_error(
            ValidationError::error(ErrorCode::UnresolvedTypeParam, &field_path, reason)
                .with_suggestion(guidance.to_string()),
        );
        // A sentinel field carries no `properties` pre-substitution — nothing
        // to recurse into.
        return;
    }
    if let Some(props) = &field.properties {
        for nested in props {
            check_payload_field(nested, &field_path, trait_def, result);
        }
    }
}

/// Walk every inlined trait's `emits[].payloadSchema` and
/// `stateMachine.events[].payloadSchema` for a surviving `$`-prefixed
/// sentinel. Blocking — see module docs.
pub fn validate_type_param_sentinels(schema: &orbital_core::schema::OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = super::ValidationResult::new();

    for (trait_def, trait_path) in inline_traits(schema) {
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                if let Some(payload) = &emit.payload_schema {
                    let path = format!("{trait_path}.emits.{}", emit.event);
                    for field in payload {
                        check_payload_field(field, &path, trait_def, &mut result);
                    }
                }
            }
        }
        if let Some(events) = &trait_def.state_machine.events {
            for event in events {
                if let Some(payload) = &event.payload_schema {
                    let path = format!("{trait_path}.events.{}", event.key);
                    for field in payload {
                        check_payload_field(field, &path, trait_def, &mut result);
                    }
                }
            }
        }
    }

    ctx.merge(result);
}
