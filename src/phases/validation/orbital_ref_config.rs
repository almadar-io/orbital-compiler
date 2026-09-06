//! Call-site rules for `orbital X = Alias.orbitals.Y { … }` references and
//! their feeding `uses A from "…" { config { … } }` overrides
//! (`docs/Almadar_Orbital_Import.md` §4.5). Mirrors `trait_ref_config.rs`:
//! the raw call-site keys are gone once inline folds/drops them into the
//! resolved schema, so these convert the [`OrbitalRefDiagnostic`]s /
//! [`UsesConfigDiagnostic`]s collected DURING inlining
//! (`phases::inline::inline_schema_with_orbital_diagnostics`) into proper
//! `ValidationError`s.
//!
//! - `ORB_O_CONFIG_UNKNOWN_KEY`: an orbital reference's call-site `config {}`
//!   names a key the upstream orbital never declared. The override silently
//!   no-ops.
//! - `ORB_O_OMIT_EMBEDDED_TRAIT`: an orbital reference's `omit {}` / `only {}`
//!   drops a trait that a surviving trait still embeds via `@trait.<X>` — the
//!   embed token has nothing left to render.
//! - `ORB_U_CONFIG_UNKNOWN_KEY`: a `uses A from "…" { config { … } }`
//!   override names a key the imported behavior's app-level `config {}`
//!   never declared.

use super::error::{ErrorCode, ValidationError};
use super::ValidationResult;
use crate::phases::inline::{OrbitalRefDiagnostic, UsesConfigDiagnostic};

/// Convert inline-collected orbital-reference diagnostics into
/// `ORB_O_CONFIG_UNKNOWN_KEY` / `ORB_O_OMIT_EMBEDDED_TRAIT` validation errors.
pub fn validate_orbital_ref_diagnostics(diagnostics: &[OrbitalRefDiagnostic]) -> ValidationResult {
    let mut result = ValidationResult::new();

    for diag in diagnostics {
        for key in &diag.unknown_config_keys {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefConfigUnknownKey,
                    format!("{}.config({})", diag.path, key),
                    format!(
                        "Call-site config key '{}' does not match any config knob declared by upstream orbital '{}'. It silently no-ops.",
                        key, diag.reference
                    ),
                )
                .with_suggestion(format!(
                    "Remove '{}' or declare it in the upstream orbital's `config {{}}` block.",
                    key
                )),
            );
        }

        for (surviving_trait, omitted_trait) in &diag.omitted_but_embedded {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefOmitEmbeddedTrait,
                    format!("{}.omit({})", diag.path, omitted_trait),
                    format!(
                        "Orbital reference '{}' omits trait '{}', but trait '{}' still embeds it via '@trait.{}'. The embed has nothing left to render.",
                        diag.reference, omitted_trait, surviving_trait, omitted_trait
                    ),
                )
                .with_suggestion(format!(
                    "Keep '{}' (drop it from omit, or add it to only), or remove the '@trait.{}' embed from '{}'.",
                    omitted_trait, omitted_trait, surviving_trait
                )),
            );
        }
    }

    result
}

/// Convert inline-collected `uses … { config }` diagnostics into
/// `ORB_U_CONFIG_UNKNOWN_KEY` validation errors.
pub fn validate_uses_config_diagnostics(diagnostics: &[UsesConfigDiagnostic]) -> ValidationResult {
    let mut result = ValidationResult::new();

    for diag in diagnostics {
        for key in &diag.unknown_keys {
            result.add_error(
                ValidationError::error(
                    ErrorCode::UsesConfigUnknownKey,
                    format!("{}.uses({}).config({})", diag.path, diag.alias, key),
                    format!(
                        "Call-site config key '{}' on `uses {} from \"…\" {{ config {{ … }} }}` does not match any app-level config knob declared by the imported behavior. It silently no-ops.",
                        key, diag.alias
                    ),
                )
                .with_suggestion(format!(
                    "Remove '{}' or declare it in the imported behavior's app-level `config {{}}` block.",
                    key
                )),
            );
        }
    }

    result
}
