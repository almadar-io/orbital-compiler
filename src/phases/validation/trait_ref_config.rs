//! Call-site config rules for trait References (E1 / E2).
//!
//! These do NOT walk the post-inline schema — by the time `inline_schema`
//! finishes, `apply_overrides_to_trait` has already folded every matched
//! call-site config key into the referenced trait's declared config and
//! silently dropped the unmatched ones, so the raw call-site key set is
//! gone. Instead this module converts the [`TraitRefConfigDiagnostic`]s
//! collected DURING inlining (`phases::inline::inline_schema_with_config_diagnostics`,
//! the only point where both the raw call-site keys and the referenced
//! trait's untouched declaration — including cross-orbital `std/behaviors/*`
//! atoms resolved from the embedded phf registry — are visible together)
//! into proper `ValidationError`s.
//!
//! - **E1** (`ORB_T_CONFIG_UNKNOWN_CALL_SITE_KEY`): a call-site config key
//!   matches no declared knob on the referenced trait. The override
//!   silently no-ops today — e.g. a dead `entity=` left on a call site
//!   after `ui-data-grid` never declared that knob, or a generator
//!   regression that deletes a knob (`content` on `ui-typography`) going
//!   unnoticed because every call site passing `content=` still validates
//!   clean.
//! - **E2** (`ORB_T_CONFIG_MISSING_REQUIRED_KEY`): the referenced trait
//!   declares a knob `!`-required and the call site's config block omits
//!   the key entirely. Any value form (literal, `@config.X` forward,
//!   `@entity.*`, payload ref) satisfies the requirement.
//!
//! If the referenced trait itself could not be resolved, `inline_schema`
//! already failed with an `InlineError` before these diagnostics could even
//! be collected — E1/E2 never double-fire on an unresolvable reference.

use super::error::{ErrorCode, ValidationError};
use super::ValidationResult;
use crate::phases::inline::TraitRefConfigDiagnostic;

/// Convert inline-collected call-site config diagnostics into E1/E2
/// validation errors.
pub fn validate_trait_ref_config_diagnostics(
    diagnostics: &[TraitRefConfigDiagnostic],
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for diag in diagnostics {
        let declared = if diag.declared_keys.is_empty() {
            "(none)".to_string()
        } else {
            diag.declared_keys.join(", ")
        };

        for key in &diag.unknown_keys {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigUnknownCallSiteKey,
                    format!("{}.config({})", diag.path, key),
                    format!(
                        "Call-site config key '{}' does not match any declared config knob on referenced trait '{}'. It silently no-ops. Declared knobs: {}.",
                        key, diag.reference, declared
                    ),
                )
                .with_valid_values(diag.declared_keys.clone()),
            );
        }

        for key in &diag.missing_required_keys {
            result.add_error(
                ValidationError::error(
                    ErrorCode::TraitConfigMissingRequiredKey,
                    &diag.path,
                    format!(
                        "Referenced trait '{}' declares config knob '{}' as required (`!`), but this call site's config block does not supply it. Declared knobs: {}.",
                        diag.reference, key, declared
                    ),
                )
                .with_suggestion(format!(
                    "Supply `{}` in this trait reference's config block (a literal, an `@config.{}` forward, or another binding).",
                    key, key
                )),
            );
        }
    }

    result
}
