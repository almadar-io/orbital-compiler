//! Validates `emits { @config.<knob> -> ... }` references (Option B) that
//! the inline phase could not resolve. Runs post-inline: a resolved schema
//! carries only concrete event names in `emits[*].event`; any entry still
//! starting with `@config.` at this point failed the contract — the knob is
//! not a declared string-typed config field with a default — and the inline
//! phase (`orbital-compiler/src/phases/inline/trait.rs`,
//! `resolve_config_ref_emit_names`) deliberately left it unresolved rather
//! than substituting a wrong value.

use orbital_core_base::schema::{
    config_ref_event_knob, resolve_config_ref_event_name, ConfigRefEventError, OrbitalSchema,
    TraitReference,
};

use super::error::{ErrorCode, ValidationError};
use super::{ValidationContext, ValidationResult};

pub fn validate_all_config_ref_emits(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        for (ti, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let Some(emits) = trait_def.emits.as_ref() else {
                continue;
            };
            let path_prefix = super::trait_path(oi, &orbital.name, ti, trait_ref);
            for emit in emits {
                let Some(knob) = config_ref_event_knob(&emit.event) else {
                    continue;
                };
                // Re-derive the effective config (declared defaults; the
                // inline phase has already folded any call-site override
                // into `trait_def.config[k].default` by this point) purely
                // to name the failure clause precisely.
                let mut effective_config = orbital_core_base::schema::SObject::new();
                if let Some(declared) = trait_def.config.as_ref() {
                    for (k, field) in declared.iter() {
                        if let Some(default) = field.default.as_ref() {
                            effective_config.insert(k.clone(), default.clone());
                        }
                    }
                }
                // Post-inline, a still-unresolved `@config.<knob>` reference
                // can only be an error case (a resolvable one would already
                // have been replaced by the inline phase's literal).
                let Err(err) = resolve_config_ref_event_name(
                    &emit.event,
                    &effective_config,
                    trait_def.config.as_ref(),
                ) else {
                    continue;
                };
                let message = match err {
                    ConfigRefEventError::UnknownKnob => format!(
                        "emits references `@config.{knob}` but `{knob}` is not a declared config field on this trait."
                    ),
                    ConfigRefEventError::NotString => format!(
                        "emits references `@config.{knob}` but the `{knob}` config field is not string-typed (or its call-site/default value isn't a string literal)."
                    ),
                    ConfigRefEventError::NoDefault => format!(
                        "emits references `@config.{knob}` but the `{knob}` config field has no declared default."
                    ),
                };
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EmitConfigRefInvalid,
                        format!("{path_prefix}.emits({})", emit.event),
                        message,
                    )
                    .with_suggestion(format!(
                        "Declare `config {{ {knob} : string = \"...\" }}` on this trait, or reference an existing string-typed knob with a default."
                    )),
                );
            }
        }
    }

    ctx.merge(result);
}
