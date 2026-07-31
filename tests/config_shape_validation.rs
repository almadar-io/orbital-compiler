//! Integration tests for `ORB_T_CONFIG_SHAPE_MISMATCH`.
//!
//! The shape validator runs on every trait's `config { ... }` block and
//! checks each declared field's `default` value against the typed
//! carriers populated by the lolo lowerer (`items` for `[T]` array
//! slots, `properties` for object slots, `values` for closed-string
//! enums). Bindings like `@entity.X` / `@config.X` are late-bound and
//! pass the static check.

use orbital_compiler::phases::validation::{validate_comprehensive, ErrorCode};
use orbital_core::schema::{
    ConfigField, EntityDefinition, EntityRef, FieldDefinition, FieldType, Orbital, OrbitalSchema,
    SExpression, StateDefinition, StateMachine, StringOrArray, TraitDefinition, TraitReference,
    TransitionDefinition,
};
use std::collections::HashMap;

// ============================================================================
// Helpers
// ============================================================================

fn metric_spec_items() -> FieldDefinition {
    let mut props: HashMap<String, FieldDefinition> = HashMap::new();
    props.insert(
        "label".to_string(),
        FieldDefinition {
            name: Some("label".to_string()),
            field_type: FieldType::String,
            required: Some(true),
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        },
    );
    props.insert(
        "value".to_string(),
        FieldDefinition {
            name: Some("value".to_string()),
            field_type: FieldType::Number,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        },
    );
    props.insert(
        "format".to_string(),
        FieldDefinition {
            name: Some("format".to_string()),
            field_type: FieldType::String,
            required: None,
            primary_key: None,
            unique: None,
            default: None,
            values: Some(vec![
                "number".to_string(),
                "currency".to_string(),
                "percent".to_string(),
            ]),
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        },
    );
    FieldDefinition {
        name: None,
        field_type: FieldType::Object,
        required: None,
        primary_key: None,
        unique: None,
        default: None,
        values: None,
        relation: None,
        items: None,
        properties: Some(props),
        intrinsic: None,
        description: None,
        synonyms: None,
    }
}

fn schema_with_config(config: HashMap<String, ConfigField>) -> OrbitalSchema {
    let entity = EntityDefinition {
        shared: false,
        identity: false,
        read_policy: None,
        create_policy: None,
        update_policy: None,
        delete_policy: None,
        name: "Dashboard".to_string(),
        collection: Some("dashboards".to_string()),
        persistence: None,
        fields: vec![FieldDefinition {
            name: Some("id".to_string()),
            field_type: FieldType::String,
            required: Some(true),
            primary_key: Some(true),
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        }],
        instances: None,
        id: None,
    };
    let trait_def = TraitDefinition {
        name: "KpiStats".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        linked_entity: Some("Dashboard".to_string()),
        description: None,
        state_machine: StateMachine {
            states: vec![StateDefinition {
                name: "idle".to_string(),
                is_initial: Some(true),
                is_terminal: None,
                description: None,
                time_bound_ms: None,
            }],
            events: Some(vec![]),
            transitions: vec![TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: "INIT".to_string(),
                guard: None,
                effects: Some(vec![]),
            }],
        },
        emits: None,
        listens: None,
        ticks: None,
        config: Some(config),
        data_entities: None,
        scope: None,
        source_behavior: None,
        source_entity_definition: None,
        id: None,
        linked_entity_id: None,
        entity_ref_ids: None,
        trait_embed_ids: None,
    };
    OrbitalSchema {
        name: "TestApp".to_string(),
        version: Some("0.1.0".to_string()),
        description: None,
        orbitals: vec![Orbital {
            name: "TestOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(entity),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(trait_def)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }],
        metadata: None,
        ledger: None,
        schema_version: None,
    }
}

fn shape_errors(schema: &OrbitalSchema) -> Vec<String> {
    validate_comprehensive(schema)
        .errors
        .into_iter()
        .filter(|e| e.code == ErrorCode::TraitConfigShapeMismatch)
        .map(|e| format!("@ {}: {}", e.path, e.message))
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn good_array_of_metricspec_passes() {
    let mut config = HashMap::new();
    let mut good_metric: HashMap<String, SExpression> = HashMap::new();
    good_metric.insert(
        "label".to_string(),
        SExpression::String("Revenue".to_string()),
    );
    good_metric.insert("value".to_string(), SExpression::Number(142000.0));
    good_metric.insert(
        "format".to_string(),
        SExpression::String("currency".to_string()),
    );
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(SExpression::Array(vec![SExpression::Object(good_metric)])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert!(errs.is_empty(), "expected no shape errors, got: {:?}", errs);
}

#[test]
fn string_value_where_number_expected_fails() {
    let mut config = HashMap::new();
    let mut bad_metric: HashMap<String, SExpression> = HashMap::new();
    bad_metric.insert(
        "label".to_string(),
        SExpression::String("Retention".to_string()),
    );
    // LLM emitted "85%" as a string for a numeric value slot — the exact
    // bug pattern we saw in the workspace failure.
    bad_metric.insert("value".to_string(), SExpression::String("85%".to_string()));
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(SExpression::Array(vec![SExpression::Object(bad_metric)])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert_eq!(errs.len(), 1, "expected one shape error, got: {:?}", errs);
    assert!(
        errs[0].contains("expected number, got string"),
        "unexpected message: {}",
        errs[0]
    );
    assert!(
        errs[0].contains("metrics)[0].value"),
        "unexpected path: {}",
        errs[0]
    );
}

#[test]
fn bare_string_in_array_of_objects_fails() {
    // The DefaultExport workspace bug: LLM emitted `formats: ["csv", "pdf"]`
    // for a slot typed `[FormatOption]` (array of `{id, format, label}`
    // objects). Each bare string should fail the per-element shape check.
    let mut format_props: HashMap<String, FieldDefinition> = HashMap::new();
    for name in &["id", "format", "label"] {
        format_props.insert(
            (*name).to_string(),
            FieldDefinition {
                name: Some((*name).to_string()),
                field_type: FieldType::String,
                required: Some(true),
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: None,
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            },
        );
    }
    let format_items = FieldDefinition {
        name: None,
        field_type: FieldType::Object,
        required: None,
        primary_key: None,
        unique: None,
        default: None,
        values: None,
        relation: None,
        items: None,
        properties: Some(format_props),
        intrinsic: None,
        description: None,
        synonyms: None,
    };
    let mut config = HashMap::new();
    config.insert(
        "formats".to_string(),
        ConfigField {
            field_type: "[FormatOption]".to_string(),
            default: Some(SExpression::Array(vec![
                SExpression::String("csv".to_string()),
                SExpression::String("pdf".to_string()),
            ])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(format_items)),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert_eq!(errs.len(), 2, "expected two shape errors, got: {:?}", errs);
    assert!(
        errs.iter().all(|e| e.contains("expected an object")),
        "messages: {:?}",
        errs
    );
}

#[test]
fn missing_required_field_fails() {
    let mut config = HashMap::new();
    // `label` is required on MetricSpec; omit it.
    let mut no_label: HashMap<String, SExpression> = HashMap::new();
    no_label.insert("value".to_string(), SExpression::Number(10.0));
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(SExpression::Array(vec![SExpression::Object(no_label)])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert!(
        errs.iter()
            .any(|e| e.contains("required field 'label' missing")),
        "messages: {:?}",
        errs
    );
}

#[test]
fn enum_value_outside_allowed_set_fails() {
    let mut config = HashMap::new();
    let mut bad_enum: HashMap<String, SExpression> = HashMap::new();
    bad_enum.insert(
        "label".to_string(),
        SExpression::String("Revenue".to_string()),
    );
    bad_enum.insert("value".to_string(), SExpression::Number(100.0));
    // `format` is closed to ["number","currency","percent"] — anything
    // else trips the enum membership check.
    bad_enum.insert(
        "format".to_string(),
        SExpression::String("dollars".to_string()),
    );
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(SExpression::Array(vec![SExpression::Object(bad_enum)])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert!(
        errs.iter().any(|e| e.contains("isn't a legal enum value")),
        "messages: {:?}",
        errs
    );
}

#[test]
fn binding_strings_pass_the_check() {
    // `@entity.X` / `@config.X` / `@payload.X` are late-bound; the static
    // checker can't adjudicate their concrete type, so they pass.
    let mut config = HashMap::new();
    let mut binding_metric: HashMap<String, SExpression> = HashMap::new();
    binding_metric.insert(
        "label".to_string(),
        SExpression::String("Revenue".to_string()),
    );
    binding_metric.insert(
        "value".to_string(),
        SExpression::String("@entity.revenue".to_string()),
    );
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(SExpression::Array(vec![SExpression::Object(
                binding_metric,
            )])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert!(
        errs.is_empty(),
        "bindings should pass static shape check, got: {:?}",
        errs
    );
}

#[test]
fn computed_sexpr_call_for_array_items_defers() {
    // The drawable-layer idiom (std-iram / std-simple-rpg / std-kekec):
    // `items: (array/map @entity.tiles (fn [t] {...}))`. This lowers to
    // `SExpression::Array(["array/map", @entity.tiles, [fn, [t], {...}]])`
    // — the same `Array` variant as a literal list, but it's an operator
    // call whose shape only materializes at runtime. It must defer, not
    // be misread as a 3-element literal array of MetricSpec objects.
    let mut config = HashMap::new();
    let call = SExpression::Array(vec![
        SExpression::String("array/map".to_string()),
        SExpression::String("@entity.tiles".to_string()),
        SExpression::Array(vec![
            SExpression::String("fn".to_string()),
            SExpression::Array(vec![SExpression::String("t".to_string())]),
            SExpression::Object(HashMap::new()),
        ]),
    ]);
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(call),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert!(
        errs.is_empty(),
        "computed SExpr call should defer, got: {:?}",
        errs
    );
}

#[test]
fn computed_sexpr_call_nested_in_object_property_defers() {
    // A per-property value can itself be a computed call, e.g.
    // `{ label: "Revenue", value: (sum @entity.rows) }`. The per-element
    // object check must defer that property instead of type-mismatching
    // it against the declared `number` leaf.
    let mut config = HashMap::new();
    let mut metric: HashMap<String, SExpression> = HashMap::new();
    metric.insert(
        "label".to_string(),
        SExpression::String("Revenue".to_string()),
    );
    metric.insert(
        "value".to_string(),
        SExpression::Array(vec![
            SExpression::String("array/sum".to_string()),
            SExpression::String("@entity.rows".to_string()),
        ]),
    );
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(SExpression::Array(vec![SExpression::Object(metric)])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert!(
        errs.is_empty(),
        "computed SExpr call nested in an object property should defer, got: {:?}",
        errs
    );
}

#[test]
fn literal_wrong_shape_still_errors_alongside_computed_defer() {
    // A literal (non-call) array whose elements are bare strings, not
    // objects, must still fire — the deferral must not blanket-suppress
    // real literal mismatches. Reuses the same `[FormatOption]` shape as
    // `bare_string_in_array_of_objects_fails` to pin that the fix is
    // additive, not a broadening of what passes.
    let mut format_props: HashMap<String, FieldDefinition> = HashMap::new();
    format_props.insert(
        "id".to_string(),
        FieldDefinition {
            name: Some("id".to_string()),
            field_type: FieldType::String,
            required: Some(true),
            primary_key: None,
            unique: None,
            default: None,
            values: None,
            relation: None,
            items: None,
            properties: None,
            intrinsic: None,
            description: None,
            synonyms: None,
        },
    );
    let format_items = FieldDefinition {
        name: None,
        field_type: FieldType::Object,
        required: None,
        primary_key: None,
        unique: None,
        default: None,
        values: None,
        relation: None,
        items: None,
        properties: Some(format_props),
        intrinsic: None,
        description: None,
        synonyms: None,
    };
    let mut config = HashMap::new();
    config.insert(
        "formats".to_string(),
        ConfigField {
            field_type: "[FormatOption]".to_string(),
            default: Some(SExpression::Array(vec![SExpression::String(
                "csv".to_string(),
            )])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(format_items)),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert_eq!(errs.len(), 1, "expected one shape error, got: {:?}", errs);
    assert!(
        errs[0].contains("expected an object"),
        "message: {}",
        errs[0]
    );
}

#[test]
fn array_typed_slot_with_scalar_default_fails() {
    let mut config = HashMap::new();
    config.insert(
        "metrics".to_string(),
        ConfigField {
            field_type: "[MetricSpec]".to_string(),
            default: Some(SExpression::String("not an array".to_string())),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(metric_spec_items())),
            properties: None,
            required: None,
            ref_id: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert_eq!(errs.len(), 1, "expected one error, got: {:?}", errs);
    assert!(errs[0].contains("isn't an array"), "message: {}", errs[0]);
}
