use orbital_compiler::{phases, CompilerOptions};
use orbital_core_base::schema::TraitReference;
use orbital_core_eval::evaluator::{context::BindingContextBuilder, Evaluator};
use serde_json::json;

#[test]
fn declared_union_fold_keeps_guarded_mixed_nodes_typed_after_source_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("declared-fold.lolo");
    std::fs::write(&source, r#"
app fold-node-probe [version: "1.0.0"]
type V = string | number | boolean | [V?] | Map string V?
orbital Probe {
 entity Cell [runtime] {
 id : string!
 out : number = 0
 }
 trait Run -> Cell [integration] {
  initial: ready
  state ready {
   ASK -> ready
   (set @entity.out (tree/fold ?value (fn (n k) (if (validate/array @n) (array/len @n) 0)) V))
  }
  listens { ASK { value : V! } @description "Probe." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&source).expect("source lowers");
    let resolved = phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&source)).expect("source resolves");
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else { panic!("inline trait") };
    let effect = &trait_def.state_machine.transitions[0].effects.as_ref().unwrap()[0];
    let expression = effect.as_array().expect("set effect")[2].clone();
    for (value, expected) in [(json!(["one", 2, true]), 3.0), (json!([[1,"x"], false]), 2.0), (json!("one"), 0.0), (json!({"a":[1,"x"]}), 0.0)] {
        let payload = serde_json::from_value(json!({"value":value})).unwrap();
        let context = BindingContextBuilder::new().payload(payload).build();
        let result = Evaluator::new().evaluate(&expression, &context).expect("declared node evaluates");
        assert_eq!(result, orbital_core_base::value::Value::Number(expected));
    }
}
