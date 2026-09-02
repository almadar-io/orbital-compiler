//! Defensive sweep for Increment 3-I's `(E : Entity)` alias parameter and
//! Increment 3-II's trait-level `(V : Type, ...)` parameters
//! (`docs/Almadar_LOLO.md` ~line 315/1387).
//!
//! The inline phase (`phases::inline::rewrite::resolve_type_param_sentinels`)
//! substitutes the `"$<param>"` / `"[$<param>]"` payload sentinel
//! `orbital-lolo` lowers — for a `type X (E : Entity) = Event { row : E }`
//! alias reference (3-I) OR a trait's own `(V : Type)` header used directly
//! in its `emits`/`listens` (3-II) — once a substitution is known:
//! the using trait's bound entity (3-I, and 3-II's Entity-kind default), or
//! an explicit call-site `:: <param> <arg>` argument (3-II). A sentinel that
//! survives inline means neither was available — blocking: the emitted
//! `.orb`/codegen would otherwise carry a literal `"$<param>"` type, which is
//! not a real type in any downstream shell.

use orbital_core::schema::{Orbital, PayloadField, TraitDefinition, TraitReference};

use super::{
    error::{ErrorCode, ValidationError},
    expectations, trait_path, ValidationContext,
};

const ENTITY_GUIDANCE: &str = "Bind the trait to a `linkedEntity` (directly, via `-> Entity`, or via a call-site rebind) so the parameter has an entity to monomorphize against.";
const TYPE_GUIDANCE: &str = "Supply an explicit call-site argument for this parameter — `:: <param> <arg>` on the trait reference (a primitive type name, or an entity name) — this Type-kind parameter has no default.";
/// A sigil type constructor that survived resolution. Each cause is named so
/// the message is actionable — an unresolved type that only says "unresolved"
/// is the silent degrade wearing a different hat.
fn unresolved_sigil_reason(field_type: &str) -> Option<&'static str> {
    if field_type == "@user" {
        return Some("this orbital declares no identity SHAPE (a bare `expects identity` declares existence only)");
    }
    if field_type.starts_with("@payload(") {
        return Some("the named knob does not hold a literal event name on this trait");
    }
    if field_type.starts_with("@entity(") {
        return Some("the named knob does not hold a literal entity name, or that entity is declared nowhere in this schema");
    }
    // Bare `@entity` on a CONFIG knob. `is_sentinel` already covers this
    // spelling for the payload walk, but the config walk keys off this
    // function alone — so without an arm here the arity-0 constructor would
    // resolve silently-or-not-at-all, which is the exact invisibility this
    // sweep gained config coverage to close.
    if field_type == "@entity" || field_type == "[@entity]" {
        return Some("the carrying trait declares no `linkedEntity`, so there is no bound row to resolve against");
    }
    None
}

const SIGIL_GUIDANCE: &str = "A sigil type resolves against something the schema must actually declare: `@payload <knob>` needs the knob typed `event` with a literal default; `@entity <knob>` needs an `entity`-typed knob naming a declared entity; `@user` needs `expects identity <Name> { … }` with a shape.";

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

/// An `expects entity <X>` (or, post-L-9, a named `expects identity <X>`)
/// TRANSFERS the shape-dependent obligation to compose: the provider is a
/// sibling the slice cannot see, Gap #22 inline surfaces it there and the
/// same sentinel resolves. Erroring here rejects a slice whose composed
/// program is valid (measured: std-customer-success CustomerAccountOrbital,
/// `InlineDataListRender` bound to a sibling's `HealthScoreSearch` — the
/// composed schema validates 0 errors while the slice failed 8×). Relation
/// targets and persist/fetch already defer this way ("a genuinely absent
/// provider is the compose link-check's job"); an absent provider stays
/// `ORB_S_EXPECTATION_UNSATISFIED` at compose. Only Entity-kind resolution
/// defers — Type-kind `:: <param> <arg>` is call-site and still errors.
fn linked_entity_is_expects_licensed(orbital: &Orbital, trait_def: &TraitDefinition) -> bool {
    match trait_def.linked_entity.as_deref() {
        Some(name) => expectations::expected_entity_names(orbital).contains(&name),
        None => false,
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
    defer_entity_kind: bool,
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
        let entity_kind = field.field_type == "@entity"
            || field.field_type == "[@entity]"
            || declared_kind == Some("Entity");
        if defer_entity_kind && entity_kind {
            // The trait IS bound — to an expects-licensed sibling entity the
            // slice cannot describe. Resolution happens at compose; see
            // `linked_entity_is_expects_licensed`.
            return;
        }
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
            check_payload_field(nested, &field_path, trait_def, defer_entity_kind, result);
        }
    }
}

/// Walk every inlined trait's `emits[].payloadSchema`,
/// `stateMachine.events[].payloadSchema` AND `config[]` for a surviving
/// sentinel. Blocking — see module docs.
///
/// Config coverage matters because it was previously absent while this
/// module's own doc comment claimed it: a sentinel left in a config field was
/// invisible to resolution AND to this sweep, and shipped into the `.orb` as a
/// meaningless literal string. The sigil type constructors (`@payload <knob>`,
/// `@entity <knob>`, `@user`) resolve in config position, so an unresolved one
/// must be reported here rather than silently emitted.
pub fn validate_type_param_sentinels(schema: &orbital_core::schema::OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = super::ValidationResult::new();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
      for (j, trait_ref) in orbital.traits.iter().enumerate() {
        let trait_def = match trait_ref {
            TraitReference::Inline(trait_def) => trait_def,
            TraitReference::Reference { .. } => continue,
        };
        let trait_path = trait_path(i, &orbital.name, j, trait_ref);
        let deferred = linked_entity_is_expects_licensed(orbital, trait_def);
        if let Some(emits) = &trait_def.emits {
            for emit in emits {
                if let Some(payload) = &emit.payload_schema {
                    let path = format!("{trait_path}.emits.{}", emit.event);
                    for field in payload {
                        check_payload_field(field, &path, trait_def, deferred, &mut result);
                    }
                }
            }
        }
        if let Some(events) = &trait_def.state_machine.events {
            for event in events {
                if let Some(payload) = &event.payload_schema {
                    let path = format!("{trait_path}.events.{}", event.key);
                    for field in payload {
                        check_payload_field(field, &path, trait_def, deferred, &mut result);
                    }
                }
            }
        }
        if let Some(config) = &trait_def.config {
            for (knob, field) in config {
                if deferred
                    && (field.field_type == "@entity" || field.field_type == "[@entity]")
                {
                    continue;
                }
                if let Some(reason) = unresolved_sigil_reason(&field.field_type) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::UnresolvedTypeParam,
                            format!("{trait_path}.config.{knob}"),
                            format!(
                                "config knob '{knob}' still carries the unresolved type `{}` — {reason}",
                                field.field_type
                            ),
                        )
                        .with_suggestion(SIGIL_GUIDANCE.to_string()),
                    );
                }
            }
        }
      }
    }

    ctx.merge(result);
}
