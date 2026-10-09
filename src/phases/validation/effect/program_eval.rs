//! `program/eval` effect validation: the validator's errors return through the
//! failure arm, so a call without `emit.failure` drops them.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::schema::SExpression;

pub(crate) fn validate_program_eval(
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    let mut has_failure = false;

    for arg in arr.iter().skip(1) {
        let SExpression::Object(options) = arg else {
            continue;
        };
        let Some(emit_obj) = extract_emit_object_ref(options) else {
            continue;
        };
        result.merge(validate_emit_config(
            emit_obj,
            &["success", "failure"],
            declared_emits,
            path,
        ));
        if matches!(emit_obj.get("failure"), Some(SExpression::String(ev)) if !ev.is_empty()) {
            has_failure = true;
        }
    }

    if !has_failure {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectProgramEvalFailureMissing,
                path,
                "`program/eval` carries no `emit.failure` event — the validator's errors come back through it and would be dropped",
            )
            .with_suggestion(
                "Add `{ emit: { success: \"<EVENT>\", failure: \"<FAILURE_EVENT>\" } }` as the last arg and handle the failure event in an arm.",
            ),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(json: &str) -> Vec<SExpression> {
        serde_json::from_str(json).expect("sexpr json")
    }

    fn codes(arr: &[SExpression]) -> Vec<ErrorCode> {
        validate_program_eval(arr, "t", None)
            .errors
            .iter()
            .map(|e| e.code.clone())
            .collect()
    }

    #[test]
    fn missing_failure_is_an_error() {
        let arr = call(r#"["program/eval", "@entity.program", {"emit": {"success": "BUILT"}}]"#);
        assert_eq!(
            codes(&arr),
            vec![ErrorCode::EffectProgramEvalFailureMissing]
        );
    }

    #[test]
    fn no_options_object_is_an_error() {
        let arr = call(r#"["program/eval", "@entity.program"]"#);
        assert_eq!(
            codes(&arr),
            vec![ErrorCode::EffectProgramEvalFailureMissing]
        );
    }

    #[test]
    fn empty_failure_event_is_an_error() {
        let arr = call(
            r#"["program/eval", "@entity.program", {"emit": {"success": "BUILT", "failure": ""}}]"#,
        );
        assert!(codes(&arr).contains(&ErrorCode::EffectProgramEvalFailureMissing));
    }

    #[test]
    fn control_with_failure_is_clean() {
        let arr = call(
            r#"["program/eval", "@entity.program", {"emit": {"success": "BUILT", "failure": "BUILD_FAILED"}}]"#,
        );
        assert_eq!(codes(&arr), Vec::<ErrorCode>::new());
    }
}
