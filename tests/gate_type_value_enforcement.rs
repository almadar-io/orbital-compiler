//! Phase-4 gate — the type/value-enforcement consolidation (LOLO gaps TYPENAME,
//! ENUM, SET-OP-RETURN).
//!
//! Pins the single-source layout so a future change can't silently re-fork it:
//!   - the pure compat predicates (`enum_value_is_member`,
//!     `oir_type_satisfies_field`) live exactly once, in `orbital-core`;
//!   - the `OirExpr → OirType` inferencer (`infer_oir_expr_type`) lives exactly
//!     once, in `orbital-compiler` (the validator's SET-OP-RETURN check and,
//!     once the upstream std `return_semantics` tagging lands, the codegen
//!     element-type path both consume it — never a second tree-walker).
//!
//! Walks the sibling crates' `src/` from this crate's manifest dir.

use std::fs;
use std::path::{Path, PathBuf};

fn crates_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <ws>/crates/orbital-compiler → parent = <ws>/crates
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf()
}

/// Concatenate every `.rs` under the given crate's `src/`.
fn crate_src(crate_name: &str) -> String {
    fn walk(dir: &Path, out: &mut String) {
        if !dir.exists() {
            return;
        }
        for entry in fs::read_dir(dir).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push_str(&fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut out = String::new();
    walk(&crates_root().join(crate_name).join("src"), &mut out);
    out
}

fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

#[test]
fn compat_predicates_live_only_in_core() {
    let core = crate_src("orbital-core");
    assert_eq!(
        count(&core, "pub fn enum_value_is_member"),
        1,
        "enum_value_is_member must be defined exactly once, in orbital-core"
    );
    assert_eq!(
        count(&core, "pub fn oir_type_satisfies_field"),
        1,
        "oir_type_satisfies_field must be defined exactly once, in orbital-core"
    );

    // No consumer crate re-defines these — they import from orbital_core.
    for c in ["orbital-compiler", "orbital-lolo", "orbital-shell-typescript"] {
        let src = crate_src(c);
        assert_eq!(
            count(&src, "fn enum_value_is_member"),
            0,
            "{c} re-defines enum_value_is_member — it must call orbital_core's"
        );
        assert_eq!(
            count(&src, "fn oir_type_satisfies_field"),
            0,
            "{c} re-defines oir_type_satisfies_field — it must call orbital_core's"
        );
    }
}

#[test]
fn inferencer_lives_only_in_compiler() {
    let compiler = crate_src("orbital-compiler");
    assert_eq!(
        count(&compiler, "pub fn infer_oir_expr_type"),
        1,
        "infer_oir_expr_type (the one OirExpr→OirType inferencer) must be defined \
         exactly once, in orbital-compiler"
    );

    // No sibling crate grows a second OirExpr→OirType inferencer.
    for c in ["orbital-core", "orbital-lolo", "orbital-shell-typescript"] {
        let src = crate_src(c);
        assert_eq!(
            count(&src, "fn infer_oir_expr_type"),
            0,
            "{c} defines a second OirExpr→OirType inferencer — consume \
             orbital_compiler::type_infer::infer_oir_expr_type instead"
        );
    }
}

#[test]
fn guard_evaluation_has_one_home_in_core() {
    // `test --execute` (and anything else) must evaluate guards through the one
    // canonical evaluator (`orbital_core::Evaluator::evaluate_guard_oir`), never a
    // re-grown headless walker. The shadow functions deleted from orbital-cli must
    // not come back, and the OIR→SExpression bridge that feeds the evaluator lives
    // once, in orbital-core.
    let cli = crate_src("orbital-cli");
    for shadow in ["fn evaluate_guard_headless", "fn resolve_expr_value", "fn resolve_expr_to_f64"]
    {
        assert_eq!(
            count(&cli, shadow),
            0,
            "orbital-cli re-grew a parallel guard evaluator (`{shadow}`) — evaluate \
             guards via orbital_core::Evaluator::evaluate_guard_oir instead"
        );
    }

    let core = crate_src("orbital-core");
    assert_eq!(
        count(&core, "fn to_sexpression("),
        1,
        "the OirExpr→SExpression bridge must be defined exactly once, in orbital-core"
    );
    assert_eq!(
        count(&core, "fn evaluate_guard_oir("),
        1,
        "the OirExpr-guard evaluation entry must be defined exactly once, in orbital-core"
    );
}
