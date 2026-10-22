use orbital_compiler::{phases, CompilerOptions};
use orbital_core_base::schema::TraitReference;
use orbital_core_eval::evaluator::{context::BindingContextBuilder, Evaluator};
use serde_json::json;

#[test]
fn checked_local_annotations_retain_literal_map_types_and_evaluate_the_initializer_once() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("checked-local.lolo");
    std::fs::write(&source, r#"
app checked-local [version: "1.0.0"]
type ValueMap = Map string number
orbital Probe {
 entity Cell [runtime] {
  id : string!
  out : [number] = []
 }
 trait Run -> Cell [integration] {
  initial: ready
  state ready {
   ASK -> ready
   (set @entity.out (let ((initial : ValueMap {}) (values (object/set @initial "answer" 42)) (sample : number (prob/uniform 0 1))) [@sample, @sample, (prob/uniform 0 1), (object/get @values "answer")]))
  }
  listens { ASK {} @description "Checked locals." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&source).expect("checked source lowers");
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&source))
            .expect("checked source resolves");
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let expression = &trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0]
        .as_array()
        .unwrap()[2];
    let context = BindingContextBuilder::new().build();
    context.prob_seed(42.0);
    let expected_context = BindingContextBuilder::new().build();
    expected_context.prob_seed(42.0);
    let first = expected_context.prob_next().unwrap();
    let second = expected_context.prob_next().unwrap();
    let actual = Evaluator::new()
        .evaluate(expression, &context)
        .expect("checked locals run");
    assert_eq!(
        actual,
        orbital_core_base::value::Value::Array(vec![
            orbital_core_base::value::Value::Number(first),
            orbital_core_base::value::Value::Number(first),
            orbital_core_base::value::Value::Number(second),
            orbital_core_base::value::Value::Number(42.0),
        ])
    );
}

#[test]
fn declared_union_fold_keeps_guarded_mixed_nodes_typed_after_source_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("declared-fold.lolo");
    std::fs::write(
        &source,
        r#"
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
"#,
    )
    .unwrap();
    let schema = orbital_test_support::lower_lolo_file(&source).expect("source lowers");
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&source))
            .expect("source resolves");
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let effect = &trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0];
    let expression = effect.as_array().expect("set effect")[2].clone();
    for (value, expected) in [
        (json!(["one", 2, true]), 3.0),
        (json!([[1, "x"], false]), 2.0),
        (json!("one"), 0.0),
        (json!({"a":[1,"x"]}), 0.0),
    ] {
        let payload = serde_json::from_value(json!({"value":value})).unwrap();
        let context = BindingContextBuilder::new().payload(payload).build();
        let result = Evaluator::new()
            .evaluate(&expression, &context)
            .expect("declared node evaluates");
        assert_eq!(result, orbital_core_base::value::Value::Number(expected));
    }
}

#[test]
fn record_branch_empty_lists_keep_their_siblings_known_element_types() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("branch-record-list-field.lolo");
    std::fs::write(&path, r#"
app branch-record-list-field [version: "1.0.0"]
orbital Probe {
 entity Cell [runtime] {
  id : string!
  out : number = 0
 }
 trait Run -> Cell [integration] {
  initial: ready
  state ready {
   ASK -> ready
   (set @entity.out (let ((rr (if ?flag { push: [], cells: [] } { push: [{ level: "x", seq: 0 }], cells: [{ kind: "x" }] }))) (array/len (array/concat (object/get @rr "push") []))))
  }
  listens { ASK { flag : boolean! } @description "Probe branch list fields." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&path).expect("source lowers");
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path))
            .expect("source resolves");
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let effect = &trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0];
    let expression = effect.as_array().unwrap()[2].clone();
    for (flag, expected) in [(true, 0.0), (false, 1.0)] {
        let payload = serde_json::from_value(json!({"flag":flag})).unwrap();
        let ctx = BindingContextBuilder::new().payload(payload).build();
        assert_eq!(
            Evaluator::new().evaluate(&expression, &ctx).unwrap(),
            orbital_core_base::value::Value::Number(expected)
        );
    }
}

#[test]
fn fold_node_guards_refine_the_declared_child_result_parameter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fold-child-results.lolo");
    std::fs::write(&path, r#"
app fold-child-results [version: "1.0.0"]
type V = string | number | boolean | [V?]
orbital Probe {
 entity Cell [runtime] {
  id : string!
  out : [string] = []
 }
 trait Run -> Cell [integration] {
  initial: ready
  state ready {
   ASK -> ready
   (set @entity.out (tree/fold ?value (fn (n k) (array/concat (if (validate/string @n) [@n] []) (if (validate/array @n) (array/flatten @k) []))) V))
  }
  listens { ASK { value : V! } @description "Probe child results." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&path).expect("source lowers");
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path))
            .expect("source resolves");
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let effect = &trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0];
    let expression = effect.as_array().unwrap()[2].clone();
    for (value, expected) in [
        (json!(["a", [2, "b"], false]), json!(["a", "b"])),
        (json!("leaf"), json!(["leaf"])),
        (json!([]), json!([])),
    ] {
        let payload = serde_json::from_value(json!({"value":value})).unwrap();
        let ctx = BindingContextBuilder::new().payload(payload).build();
        assert_eq!(
            Evaluator::new().evaluate(&expression, &ctx).unwrap(),
            serde_json::from_value(expected).unwrap()
        );
    }
    let aliased_source = std::fs::read_to_string(&path)
        .unwrap()
        .replace(
            "type V = string | number | boolean | [V?]",
            "type V = string | number | boolean | [V?]\ntype Alias = V",
        )
        .replace(" V))", " Alias))");
    std::fs::write(&path, aliased_source).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&path).unwrap();
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path)).unwrap();
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "alias: {:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let expression = trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0]
        .as_array()
        .unwrap()[2]
        .clone();
    let payload = serde_json::from_value(json!({"value":["a",[2,"b"],false]})).unwrap();
    let ctx = BindingContextBuilder::new().payload(payload).build();
    assert_eq!(
        Evaluator::new()
            .evaluate(&expression, &ctx)
            .expect("a declared alias folds the same recursive children"),
        serde_json::from_value(json!(["a", "b"])).unwrap()
    );
}

#[test]
fn recursive_map_aliases_accept_their_declared_array_member() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recursive-map.lolo");
    let source = r#"app remedy [version: "1.0.0"]
type V = string | number | boolean | [V?] | Map string V?

orbital Probe {
 entity Cell [runtime] {
 id : string!
 out : string = ""
 values : Map string V? = {}
 }
 trait Run -> Cell [integration] {
 initial: ready
 state ready {
 ASK -> ready
 (set @entity.values {})
 (set @entity.values (if ?flag (object/set @entity.values "x" "one") (object/set @entity.values "x" ?many)))
 }
 listens { ASK { n : number! flag : boolean! many : [string]! } @description "Probe." }
 }
}
"#;
    for (definition, accepts) in [
        ("string | number | boolean | [V?] | Map string V?", true),
        ("string | number | boolean", false),
    ] {
        std::fs::write(
            &path,
            source.replace(
                "string | number | boolean | [V?] | Map string V?",
                definition,
            ),
        )
        .unwrap();
        let schema = orbital_test_support::lower_lolo_file(&path).expect("source lowers");
        let resolved =
            phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path))
                .expect("source resolves");
        let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
        assert_eq!(
            verdict.errors.is_empty(),
            accepts,
            "{definition}: {:?}",
            verdict.errors
        );
        if !accepts {
            continue;
        }
        let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
            panic!("inline trait")
        };
        let effect = &trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .unwrap()[1];
        let expression = effect.as_array().unwrap()[2].clone();
        for (flag, expected) in [(true, json!({"x":"one"})), (false, json!({"x":["a","b"]}))] {
            let payload =
                serde_json::from_value(json!({"flag":flag,"many":["a","b"],"n":0})).unwrap();
            let entity = serde_json::from_value(json!({"values":{}})).unwrap();
            let ctx = BindingContextBuilder::new()
                .payload(payload)
                .entity(entity)
                .build();
            assert_eq!(
                Evaluator::new().evaluate(&expression, &ctx).unwrap(),
                serde_json::from_value(expected).unwrap()
            );
        }
    }
}

#[test]
fn concatenated_record_rows_keep_their_optional_fields_through_source_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record-list-element-join.lolo");
    std::fs::write(&path, r#"app record-list-element-join [version: "1.0.0"]
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
   (set @entity.out (let ((acc {cells: [{kind: "row", kids: ["x"], keys: ["x"]}]}) (rr (if ?flag {cells: [{kind: "default", val: ?value}]} {cells: [{kind: "omit"}]}))) (array/len (array/concat (object/get @acc "cells") (object/get @rr "cells")))))
  }
  listens { ASK { flag : boolean! value : V! } @description "Probe record lists." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&path).unwrap();
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path)).unwrap();
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let expression = trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0]
        .as_array()
        .unwrap()[2]
        .clone();
    for flag in [true, false] {
        let payload = serde_json::from_value(json!({"flag":flag,"value":[1,"x"]})).unwrap();
        let ctx = BindingContextBuilder::new().payload(payload).build();
        assert_eq!(
            Evaluator::new().evaluate(&expression, &ctx).unwrap(),
            orbital_core_base::value::Value::Number(2.0)
        );
    }
}

#[test]
fn repeated_guarded_dynamic_reads_keep_their_exact_source_and_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guarded-dynamic-read.lolo");
    std::fs::write(&path, r#"app guarded-dynamic-read [version: "1.0.0"]
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
   (set @entity.out (if (validate/array (object/get ?row ?key "")) (array/len (object/get ?row ?key "")) 0))
  }
  listens { ASK { row : Map string V?! key : string! } @description "Probe guarded reads." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&path).unwrap();
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path)).unwrap();
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let expression = trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0]
        .as_array()
        .unwrap()[2]
        .clone();
    for (row, expected) in [
        (json!({"x":[1,"x"]}), 2.0),
        (json!({"x":"scalar"}), 0.0),
        (json!({}), 0.0),
    ] {
        let payload = serde_json::from_value(json!({"row":row,"key":"x"})).unwrap();
        let ctx = BindingContextBuilder::new().payload(payload).build();
        assert_eq!(
            Evaluator::new()
                .evaluate(&expression, &ctx)
                .unwrap_or_else(|error| panic!("{error:?}; expression={expression}")),
            orbital_core_base::value::Value::Number(expected)
        );
    }
}

#[test]
fn record_bound_operators_refuse_a_declared_union_that_can_hold_a_scalar() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("union-record-bound.lolo");
    let source = r#"app union-record-bound [version: "1.0.0"]
type RecordCase = { name : string! }
type V = TYPE
orbital Probe {
 entity Cell [runtime] {
  id : string!
  out : string = ""
 }
 trait Run -> Cell [integration] {
  initial: ready
  state ready {
   ASK -> ready
   (set @entity.out (json/stringify (object/set ?value "x" 0)))
  }
  listens { ASK { value : V! } @description "Probe record bound." }
 }
}
"#;
    for (definition, accepted) in [
        ("RecordCase | Map string number", true),
        ("string | Map string number", false),
    ] {
        std::fs::write(&path, source.replace("TYPE", definition)).unwrap();
        let schema = orbital_test_support::lower_lolo_file(&path).unwrap();
        let resolved =
            phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path))
                .unwrap();
        let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
        assert_eq!(
            verdict.errors.is_empty(),
            accepted,
            "{definition}: {:?}",
            verdict.errors
        );
    }
}

#[test]
fn accumulator_record_empty_list_fields_get_the_callbacks_known_element_type() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reduce-record-empty-list-slot.lolo");
    std::fs::write(&path, r#"app reduce-record-empty-list-slot [version: "1.0.0"]
orbital Probe {
 entity Cell [runtime] {
  id : string!
  out : number = 0
 }
 trait Run -> Cell [integration] {
  initial: ready
  state ready {
   ASK -> ready
   (set @entity.out (let ((result (array/reduce (array/range ?n) {cells: []} (fn acc i {cells: (array/append (object/get @acc "cells") {kind: "x"})})))) (array/len (object/get @result "cells"))))
  }
  listens { ASK { n : number! } @description "Probe accumulator fields." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&path).unwrap();
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path)).unwrap();
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let expression = trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0]
        .as_array()
        .unwrap()[2]
        .clone();
    for n in [0, 3] {
        let payload = serde_json::from_value(json!({"n":n})).unwrap();
        let ctx = BindingContextBuilder::new().payload(payload).build();
        assert_eq!(
            Evaluator::new().evaluate(&expression, &ctx).unwrap(),
            orbital_core_base::value::Value::Number(f64::from(n))
        );
    }
}

#[test]
fn branch_record_lists_retain_fields_present_on_only_one_element_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("branch-list-element-field-retention.lolo");
    std::fs::write(&path, r#"app branch-list-element-field-retention [version: "1.0.0"]
orbital Probe {
 entity Cell [runtime] {
  id : string!
  out : number = 0
 }
 trait Run -> Cell [integration] {
  initial: ready
  state ready {
   ASK -> ready
   (set @entity.out (let ((result (if ?flag {cells: [{kind: "a"}]} {cells: [{kind: "b", kids: ["x"]}]}))) (array/len (object/get (array/first (object/get @result "cells")) "kids" []))))
  }
  listens { ASK { flag : boolean! } @description "Probe retained list fields." }
 }
}
"#).unwrap();
    let schema = orbital_test_support::lower_lolo_file(&path).unwrap();
    let resolved =
        phases::resolve_schema_with_path(schema, &CompilerOptions::default(), Some(&path)).unwrap();
    let verdict = phases::validation::validate_comprehensive_without_services(&resolved);
    assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
    let TraitReference::Inline(trait_def) = &resolved.orbitals[0].traits[0] else {
        panic!("inline trait")
    };
    let expression = trait_def.state_machine.transitions[0]
        .effects
        .as_ref()
        .unwrap()[0]
        .as_array()
        .unwrap()[2]
        .clone();
    for (flag, expected) in [(true, 0.0), (false, 1.0)] {
        let payload = serde_json::from_value(json!({"flag":flag})).unwrap();
        let ctx = BindingContextBuilder::new().payload(payload).build();
        assert_eq!(
            Evaluator::new().evaluate(&expression, &ctx).unwrap(),
            orbital_core_base::value::Value::Number(expected)
        );
    }
}
