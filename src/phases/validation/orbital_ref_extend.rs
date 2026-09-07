//! `extend { … }` call-site rules for `orbital X = Alias.orbitals.Y { … }`
//! references (orbital-import Stage C, `docs/Almadar_LOLO.md` §8b). Sibling
//! of `orbital_ref_config.rs` — converts the `extend_collision`/
//! `extend_unread` fields inline collects onto [`OrbitalRefDiagnostic`]
//! (`phases::inline::orbital::resolve_orbital_reference`) into proper
//! `ValidationError`s. Kept in its own file (not `orbital_ref_config.rs`)
//! per the Stage C program's file-disjointness split — the two-site codes
//! it owns are appended to `error.rs` only, never edited there.
//!
//! - `ORB_O_EXTEND_FIELD_COLLISION` (error): an `extend {}` field name is
//!   already declared on the upstream primary entity (after `fields {}`
//!   renames), or equals a `fields {}` rename TARGET.
//! - `ORB_O_EXTEND_FIELD_UNREAD` (warning, ruled rung 2): no kept trait
//!   reads/sets/renders the field and no orbital-level config knob value
//!   forwards it — a live wire terminating in nothing.
//!
//! Path shape: `{diag.path}.extend({name})`, the SAME paren-style suffix
//! `orbital_ref_config.rs`'s other `ORB_O_*` codes already use (`.config(k)`,
//! `.roles(k)`, …) — `lower/orbital_ref.rs` records the matching
//! `{base}.extend({name})` source-map key so `orb validate --json` resolves
//! this path to the offending `.lolo` field's `line`/`column`.

use super::error::{ErrorCode, Severity, ValidationError};
use super::ValidationResult;
use crate::phases::inline::OrbitalRefDiagnostic;

/// Convert inline-collected `extend {}` diagnostics into `ORB_O_EXTEND_*`
/// validation errors/warnings. Call this from the SAME shared CLI helper
/// that already chains `validate_orbital_ref_diagnostics` →
/// `validate_uses_config_diagnostics` (C1-R1's `merge_post_inline_diagnostics`)
/// so the cascade-suppression pass sees every `ORB_O_*` row from one place.
pub fn validate_orbital_ref_extend_diagnostics(
    diagnostics: &[OrbitalRefDiagnostic],
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for diag in diagnostics {
        for (name, reason) in &diag.extend_collision {
            result.add_error(
                ValidationError::error(
                    ErrorCode::OrbitalRefExtendFieldCollision,
                    format!("{}.extend({})", diag.path, name),
                    format!(
                        "`extend {{}}` field '{}' collides with an existing field on '{}': {}",
                        name, diag.reference, reason
                    ),
                )
                .with_suggestion(format!(
                    "Rename '{}' in `extend {{}}` to a name not already on the upstream \
                     primary entity and not a `fields {{}}` rename target.",
                    name
                )),
            );
        }

        for name in &diag.extend_unread {
            // Warning severity: `ValidationError::error`'s convenience
            // `warning()` sibling was removed 2026-09-02 (every prior call
            // site was auditing a MISNAMED severity, per `error.rs`'s own
            // doc) — this is a genuine, deliberate non-blocking finding
            // (owner ruling: rung 2), built directly since `ValidationError`
            // is a plain public struct.
            result.add_error(ValidationError {
                code: ErrorCode::OrbitalRefExtendFieldUnread,
                message: format!(
                    "`extend {{}}` field '{}' on '{}' is never read, set, rendered, or \
                         forwarded by a config knob value — a live wire terminating in nothing.",
                    name, diag.reference
                ),
                path: format!("{}.extend({})", diag.path, name),
                severity: Severity::Warning,
                suggestion: Some(format!(
                    "Wire '{}' into a trait (guard/effect/render) or an orbital-level \
                         config knob (e.g. a `browseColumns`/`formFields` list entry), or \
                         remove it from `extend {{}}`.",
                    name
                )),
                valid_values: None,
                expected_shape: None,
                fix_guidance: None,
            });
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phases::inline::OrbitalRefDiagnostic;

    fn diag_with(
        extend_collision: Vec<(String, String)>,
        extend_unread: Vec<String>,
    ) -> OrbitalRefDiagnostic {
        OrbitalRefDiagnostic {
            path: "orbitals[0]".to_string(),
            reference: "Up.orbitals.UpOrbital".to_string(),
            unknown_config_keys: Vec::new(),
            omitted_but_embedded: Vec::new(),
            entity_unmapped: Vec::new(),
            entity_unknown_key: Vec::new(),
            entity_target_unknown: Vec::new(),
            role_unknown_upstream: Vec::new(),
            role_targets: Vec::new(),
            page_unknown_path: Vec::new(),
            extend_collision,
            extend_unread,
            field_rename_unapplied: Vec::new(),
            event_subs: std::collections::HashMap::new(),
            local_name: "Local".to_string(),
        }
    }

    /// `ORB_O_EXTEND_FIELD_COLLISION` is an ERROR at the paren-style path
    /// shape `orbital_ref_config.rs`'s other `ORB_O_*` codes use
    /// (`{path}.extend({name})`) — `lower/orbital_ref.rs` records the SAME
    /// shape into the source map so `orb validate --json` resolves it to
    /// the offending `.lolo` field's `line`/`column`.
    #[test]
    fn collision_diagnostic_becomes_an_error_at_the_paren_style_path() {
        let diags = vec![diag_with(
            vec![("id".to_string(), "already declared".to_string())],
            Vec::new(),
        )];
        let result = validate_orbital_ref_extend_diagnostics(&diags);
        assert_eq!(result.errors.len(), 1);
        assert!(result.warnings.is_empty());
        assert_eq!(
            result.errors[0].code,
            ErrorCode::OrbitalRefExtendFieldCollision
        );
        assert_eq!(result.errors[0].path, "orbitals[0].extend(id)");
        assert!(result.errors[0].is_error());
    }

    /// `ORB_O_EXTEND_FIELD_UNREAD` is a WARNING (owner ruling, rung 2) — it
    /// must land in `result.warnings`, never `result.errors`.
    #[test]
    fn unread_diagnostic_becomes_a_warning_at_the_paren_style_path() {
        let diags = vec![diag_with(Vec::new(), vec!["orphan".to_string()])];
        let result = validate_orbital_ref_extend_diagnostics(&diags);
        assert!(result.errors.is_empty());
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.warnings[0].code,
            ErrorCode::OrbitalRefExtendFieldUnread
        );
        assert_eq!(result.warnings[0].path, "orbitals[0].extend(orphan)");
        assert!(result.warnings[0].is_warning());
    }
}
