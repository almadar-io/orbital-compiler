//! `program/eval` effect validation: its `emit` config block.

use super::super::ValidationResult;
use super::{extract_emit_object_ref, DeclaredEmit};
use orbital_core_base::schema::SExpression;

pub(crate) fn validate_program_eval(
    arr: &[SExpression],
    path: &str,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    for arg in arr.iter().skip(1) {
        let SExpression::Object(options) = arg else {
            continue;
        };
        let Some(emit_obj) = extract_emit_object_ref(options) else {
            continue;
        };
    }

    result
}

#[cfg(test)]
mod tests {
    use super::super::super::ErrorCode;
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
    fn control_with_failure_is_clean() {
        let arr = call(
            r#"["program/eval", "@entity.program", {"emit": {"success": "BUILT", "failure": "BUILD_FAILED"}}]"#,
        );
        assert_eq!(codes(&arr), Vec::<ErrorCode>::new());
    }
}
