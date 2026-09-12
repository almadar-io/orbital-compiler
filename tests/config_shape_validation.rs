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
            merged_from: None,
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
            merged_from: None,
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
            merged_from: None,
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
        merged_from: None,
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
        access_waivers: None,
        name: "Dashboard".to_string(),
        collection: Some("dashboards".to_string()),
        persistence: None,
        fields: vec![FieldDefinition {
            merged_from: None,
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
        effect_row: None,
        type_params: None,
        type_args: None,
        name: "KpiStats".to_string(),
        entity_rebindable: None,
        entity_contract: None,
        category: None,
        capabilities: vec![],
        local: false,
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
        theme: None,
        name: "TestApp".to_string(),
        version: Some("0.1.0".to_string()),
        description: None,
        orbitals: vec![Orbital {
            types: None,
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
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
            reference: None,
            config: None,
        }],
        metadata: None,
        ledger: None,
        schema_version: None,
        config: None,
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
            forwarded_from: None,
            payload_for: None,
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
            forwarded_from: None,
            payload_for: None,
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
                merged_from: None,
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
        merged_from: None,
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
            forwarded_from: None,
            payload_for: None,
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
            forwarded_from: None,
            payload_for: None,
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
            forwarded_from: None,
            payload_for: None,
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
            forwarded_from: None,
            payload_for: None,
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
            forwarded_from: None,
            payload_for: None,
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
            forwarded_from: None,
            payload_for: None,
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
            merged_from: None,
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
        merged_from: None,
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
            forwarded_from: None,
            payload_for: None,
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
            forwarded_from: None,
            payload_for: None,
        },
    );
    let schema = schema_with_config(config);
    let errs = shape_errors(&schema);
    assert_eq!(errs.len(), 1, "expected one error, got: {:?}", errs);
    assert!(errs[0].contains("isn't an array"), "message: {}", errs[0]);
}

/// `icon`/`slot`/`pattern`/`component` are name-carrying scalar tags — a Lucide
/// icon name, a slot key, a pattern name — and are strings in every emitted
/// program. They used to fall through `check_primitive_shape`'s catch-all
/// (`_ => true`) and accept a number or a boolean silently, which is what made
/// skipping L1's check 3.4 in dev unsafe: L1 rejected these, L2 did not.
/// ~7k knobs across the corpus carry these tags.
#[test]
fn number_on_a_name_carrying_scalar_tag_fails() {
    for tag in ["icon", "slot", "pattern", "component"] {
        let mut config = HashMap::new();
        config.insert(
            "knob".to_string(),
            ConfigField {
                field_type: tag.to_string(),
                default: Some(SExpression::Number(7.0)),
                label: None,
                description: None,
                synonyms: None,
                tier: None,
                values: None,
                items: None,
                properties: None,
                required: None,
                ref_id: None,
                forwarded_from: None,
            payload_for: None,
            },
        );
        let schema = schema_with_config(config);
        let errs = shape_errors(&schema);
        assert_eq!(errs.len(), 1, "`{tag}` must reject a number, got: {errs:?}");
        assert!(
            errs[0].contains(&format!("expected {tag}, got number")),
            "unexpected message for `{tag}`: {}",
            errs[0]
        );
    }
}

/// A string name is the legal form, and an explicitly-unset optional knob
/// (`null`) stays permitted — only L1 ever rejected null here.
#[test]
fn string_and_null_on_a_name_carrying_scalar_tag_pass() {
    for value in [SExpression::String("circle".to_string()), SExpression::Null] {
        let mut config = HashMap::new();
        config.insert(
            "knob".to_string(),
            ConfigField {
                field_type: "icon".to_string(),
                default: Some(value.clone()),
                label: None,
                description: None,
                synonyms: None,
                tier: None,
                values: None,
                items: None,
                properties: None,
                required: None,
                ref_id: None,
                forwarded_from: None,
            payload_for: None,
            },
        );
        let schema = schema_with_config(config);
        assert!(
            shape_errors(&schema).is_empty(),
            "`icon` must accept {value:?}"
        );
    }
}

/// `int`/`float` are numeric tags that also fell through the catch-all, so a
/// `float` knob silently accepted a string.
#[test]
fn non_number_on_int_or_float_fails() {
    for tag in ["int", "float"] {
        let mut config = HashMap::new();
        config.insert(
            "knob".to_string(),
            ConfigField {
                field_type: tag.to_string(),
                default: Some(SExpression::String("1.5".to_string())),
                label: None,
                description: None,
                synonyms: None,
                tier: None,
                values: None,
                items: None,
                properties: None,
                required: None,
                ref_id: None,
                forwarded_from: None,
            payload_for: None,
            },
        );
        let schema = schema_with_config(config);
        let errs = shape_errors(&schema);
        assert_eq!(errs.len(), 1, "`{tag}` must reject a string, got: {errs:?}");
        assert!(
            errs[0].contains(&format!("expected {tag}, got string")),
            "unexpected message for `{tag}`: {}",
            errs[0]
        );
    }
}

fn string_value_field() -> FieldDefinition {
    FieldDefinition {
        merged_from: None,
        name: None,
        field_type: FieldType::String,
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
    }
}

fn map_knob(default: SExpression) -> OrbitalSchema {
    let mut config = HashMap::new();
    config.insert(
        "labels".to_string(),
        ConfigField {
            field_type: "Map<string,string>".to_string(),
            default: Some(default),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(string_value_field())),
            properties: None,
            required: None,
            ref_id: None,
            forwarded_from: None,
            payload_for: None,
        },
    );
    schema_with_config(config)
}

/// `Map K V` is the honest type for a genuine dynamic-key dictionary — the
/// replacement for `json` on those knobs. A conforming default must pass.
#[test]
fn map_with_conforming_values_passes() {
    let mut entries: HashMap<String, SExpression> = HashMap::new();
    entries.insert(
        "draft".to_string(),
        SExpression::String("Draft".to_string()),
    );
    entries.insert("sent".to_string(), SExpression::String("Sent".to_string()));
    let schema = map_knob(SExpression::Object(entries));
    assert!(shape_errors(&schema).is_empty());
    assert!(
        shape_errors(&schema).is_empty(),
        "conforming map must not warn: {:?}",
        shape_errors(&schema)
    );
}

/// The hole this closes: `config_declared_kind` returned `None` for any tag
/// containing `<`, so a `Map<string,string>` knob fell through to
/// `check_primitive_shape`'s `_ => true` and accepted `{ a: 3 }`. Retyping
/// `json` into an UNCHECKED type would have been laundering, not typing.
#[test]
fn map_with_a_wrong_typed_value_is_reported() {
    let mut entries: HashMap<String, SExpression> = HashMap::new();
    entries.insert("draft".to_string(), SExpression::Number(3.0));
    let schema = map_knob(SExpression::Object(entries));
    let warns = shape_errors(&schema);
    assert_eq!(
        warns.len(),
        1,
        "expected one map-value finding, got: {warns:?}"
    );
    assert!(
        warns[0].contains("expected string, got number")
            && warns[0].contains("config(labels).draft"),
        "finding must name the offending KEY and both types: {}",
        warns[0]
    );
}

/// A map whose default isn't an object at all.
#[test]
fn map_with_a_non_object_default_is_reported() {
    let schema = map_knob(SExpression::String("not-a-map".to_string()));
    let warns = shape_errors(&schema);
    assert_eq!(warns.len(), 1, "expected one finding, got: {warns:?}");
    assert!(
        warns[0].contains("isn't an object"),
        "unexpected: {}",
        warns[0]
    );
}

/// `[MapViewMarkersItem]` starts with the letters "Map" but is an ARRAY. The
/// guard is on the `Map<` prefix precisely so this stays on the array path.
#[test]
fn an_array_type_merely_starting_with_map_is_not_treated_as_a_map() {
    let mut config = HashMap::new();
    config.insert(
        "markers".to_string(),
        ConfigField {
            field_type: "[MapViewMarkersItem]".to_string(),
            default: Some(SExpression::Array(vec![])),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(string_value_field())),
            properties: None,
            required: None,
            ref_id: None,
            forwarded_from: None,
            payload_for: None,
        },
    );
    let schema = schema_with_config(config);
    assert!(shape_errors(&schema).is_empty());
    assert!(shape_errors(&schema).is_empty());
}

fn scalar_knob(default: SExpression) -> OrbitalSchema {
    let mut config = HashMap::new();
    config.insert(
        "cell".to_string(),
        ConfigField {
            field_type: "scalar".to_string(),
            default: Some(default),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: None,
            properties: None,
            required: None,
            ref_id: None,
            forwarded_from: None,
            payload_for: None,
        },
    );
    schema_with_config(config)
}

/// `scalar` admits exactly string | number | boolean | null.
#[test]
fn scalar_accepts_every_primitive_member() {
    for v in [
        SExpression::String("x".to_string()),
        SExpression::Number(3.0),
        SExpression::Bool(true),
        SExpression::Null,
    ] {
        let schema = scalar_knob(v.clone());
        let errs = shape_errors(&schema);
        assert!(errs.is_empty(), "scalar must accept {v:?}, got: {errs:?}");
    }
}

/// The whole reason `scalar` replaces `json`: a value that NESTS is rejected.
/// `json` accepted both of these silently.
#[test]
fn scalar_rejects_objects_and_arrays() {
    for v in [
        SExpression::Object(HashMap::new()),
        SExpression::Array(vec![]),
    ] {
        let schema = scalar_knob(v.clone());
        let errs = shape_errors(&schema);
        assert_eq!(errs.len(), 1, "scalar must reject {v:?}, got: {errs:?}");
        assert!(
            errs[0].contains("expected scalar"),
            "message must name the declared type: {}",
            errs[0]
        );
    }
}

/// `Map string scalar` is the shape `loadedRow`-style dynamic-key value bags
/// actually have — dynamic keys, primitive values. The map arm and the scalar
/// arm have to compose, or neither is usable for the json burndown.
#[test]
fn map_of_scalar_accepts_mixed_primitive_values_but_not_nesting() {
    let mut ok: HashMap<String, SExpression> = HashMap::new();
    ok.insert("title".to_string(), SExpression::String("Hi".to_string()));
    ok.insert("count".to_string(), SExpression::Number(2.0));
    ok.insert("done".to_string(), SExpression::Bool(false));

    let mut config = HashMap::new();
    let value_schema = FieldDefinition {
        merged_from: None,
        name: None,
        field_type: FieldType::Scalar,
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
    };
    config.insert(
        "loadedRow".to_string(),
        ConfigField {
            field_type: "Map<string,scalar>".to_string(),
            default: Some(SExpression::Object(ok)),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(value_schema.clone())),
            properties: None,
            required: None,
            ref_id: None,
            forwarded_from: None,
            payload_for: None,
        },
    );
    let schema = schema_with_config(config);
    assert!(shape_errors(&schema).is_empty());
    assert!(
        shape_errors(&schema).is_empty(),
        "mixed primitives are legal in Map string scalar: {:?}",
        shape_errors(&schema)
    );

    let mut bad: HashMap<String, SExpression> = HashMap::new();
    bad.insert("nested".to_string(), SExpression::Object(HashMap::new()));
    let mut config2 = HashMap::new();
    config2.insert(
        "loadedRow".to_string(),
        ConfigField {
            field_type: "Map<string,scalar>".to_string(),
            default: Some(SExpression::Object(bad)),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            values: None,
            items: Some(Box::new(value_schema)),
            properties: None,
            required: None,
            ref_id: None,
            forwarded_from: None,
            payload_for: None,
        },
    );
    let schema2 = schema_with_config(config2);
    assert_eq!(
        shape_errors(&schema2).len(),
        1,
        "a nested value is not a scalar: {:?}",
        shape_errors(&schema2)
    );
}
