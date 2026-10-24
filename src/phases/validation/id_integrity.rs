//! `ORB_ID_AUTHORED_WITHOUT_LEDGER`: a load-time warning for ids `stamp::ensure_stamped` replaced in a ledger-less
//! `.orb`. The dual-carry id integrity rules (`ORB_ID_UNKNOWN_REF`, `_NAME_MISMATCH`, `_KIND_MISMATCH`,
//! `_LEDGER_ORPHAN`) are the std lint std-orb-lint-id-refs.

use super::error::{ErrorCode, ValidationError};
use super::ValidationResult;

/// `ORB_ID_AUTHORED_WITHOUT_LEDGER`: the authored ids `stamp::ensure_stamped`
/// replaced while loading a ledger-less `.orb`.
pub fn authored_ids_replaced_on_load(
    replaced: &[orbital_core_base::stamp::ReplacedAuthoredId],
) -> ValidationResult {
    let mut result = ValidationResult::new();
    for r in replaced {
        let mut warning = ValidationError::error(
            ErrorCode::OrbIdAuthoredWithoutLedger,
            r.path.clone(),
            format!(
                "{} \"{}\" is replaced on load: the .orb has no ledger",
                r.path, r.id
            ),
        )
        .with_suggestion("add a ledger, or drop the id");
        warning.severity = super::Severity::Warning;
        result.add_error(warning);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::OrbitalSchema;

    fn ledgerless_program(trait_id: Option<&str>) -> OrbitalSchema {
        let mut t = serde_json::json!({ "name": "A", "linkedEntity": "E", "stateMachine": {
            "states": [{ "name": "idle", "isInitial": true }],
            "events": [{ "key": "INIT", "name": "INIT" }],
            "transitions": [{ "from": "idle", "to": "idle", "event": "INIT" }] } });
        if let Some(id) = trait_id {
            t["id"] = serde_json::json!(id);
        }
        serde_json::from_value(serde_json::json!({
            "name": "P",
            "orbitals": [{ "name": "O", "entity": { "name": "E", "persistence": "runtime",
                "fields": [{ "name": "id", "type": "string" }] },
                "traits": [t], "pages": [] }]
        }))
        .expect("schema")
    }

    fn load_warnings(mut schema: OrbitalSchema) -> ValidationResult {
        authored_ids_replaced_on_load(&orbital_core_base::stamp::ensure_stamped(&mut schema))
    }

    #[test]
    fn authored_id_replaced_on_ledgerless_load_warns() {
        let result = load_warnings(ledgerless_program(Some("t-1")));
        assert!(result.errors.is_empty());
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.warnings[0].code,
            ErrorCode::OrbIdAuthoredWithoutLedger
        );
        assert_eq!(
            result.warnings[0].message,
            "orbitals[0].traits[0].id \"t-1\" is replaced on load: the .orb has no ledger"
        );
        assert_eq!(
            result.warnings[0].suggestion.as_deref(),
            Some("add a ledger, or drop the id")
        );
    }

    #[test]
    fn control_ledgerless_load_without_authored_ids_does_not_warn() {
        let result = load_warnings(ledgerless_program(None));
        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
    }
}
