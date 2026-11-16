use orbital_compiler::phases::resolve::{resolve_sexpr_standalone, ResolveError};
use orbital_core_base::schema::SExpression;

#[test]
fn a_lowering_error_reaches_compiler_callers_with_the_same_message() {
    let malformed = SExpression::Array(vec![SExpression::String("if".into()), SExpression::Bool(true)].into());
    let core_message = orbital_core_eval::lower::lower_sexpr(&malformed).unwrap_err().to_string();
    let Err(ResolveError::InvalidSExpr(message)) = resolve_sexpr_standalone(&malformed) else {
        panic!("expected InvalidSExpr");
    };
    assert_eq!(core_message, format!("Failed to resolve S-expression: {message}"));
}

#[test]
fn control_a_valid_expression_lowers_identically_through_the_compiler() {
    let sum = SExpression::Array(vec![
        SExpression::String("+".into()),
        SExpression::Number(1.0),
        SExpression::String("@entity.count".into()),
    ].into());
    assert_eq!(resolve_sexpr_standalone(&sum).ok(), orbital_core_eval::lower::lower_sexpr(&sum).ok());
}
