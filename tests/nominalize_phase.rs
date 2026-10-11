//! Phase-3D nominalization phase tests: anonymous `OirType::Struct`/`Union`
//! positions become `TypeRef`s registered in `OirModule::named_types` under
//! lolo-ui-style positional names, with the invariants the phase pins:
//! never overwrite a pre-existing (recursive-alias) entry, canonical field
//! order before dedup identity, `enum_values` survival, and the
//! reserve-before-recurse cycle guard.

use orbital_compiler::phases::nominalize::nominalize_module;
use orbital_core::ir::{OirEntity, OirField, OirModule, OirType};

fn module_with(entity: OirEntity) -> OirModule {
    let mut module = OirModule::new("nominalize-test", "1.0.0");
    module.entities.push(entity);
    module
}

#[test]
fn derives_positional_names_for_nested_array_and_map_shapes() {
    let mut entity = OirEntity::new("order");
    entity.fields.push(OirField::new(
        "meta",
        OirType::Struct {
            fields: vec![
                OirField::new("label", OirType::String),
                OirField::new(
                    "geo",
                    OirType::Struct {
                        fields: vec![
                            OirField::new("lat", OirType::Number),
                            OirField::new("lng", OirType::Number),
                        ],
                    },
                ),
                OirField::new("tags", OirType::array(OirType::Struct {
                    fields: vec![OirField::new("name", OirType::String)],
                })),
                OirField::new("scores", OirType::map(OirType::String, OirType::Struct {
                    fields: vec![OirField::new("grade", OirType::Number)],
                })),
            ],
        },
    ));
    let mut module = module_with(entity);
    let stats = nominalize_module(&mut module);

    assert_eq!(stats.registered, 4, "meta, geo, tags Item, scores Value");
    // lolo-ui positional scheme: `${hint}${pascal(field)}`, arrays take the
    // `Item` suffix on the ELEMENT hint, maps `Value` on the VALUE hint.
    for name in ["OrderMeta", "OrderMetaGeo", "OrderMetaTagsItem", "OrderMetaScoresValue"] {
        assert!(
            module.named_types.contains_key(name),
            "{name} must be registered, have {:?}",
            module.named_types.keys().collect::<Vec<_>>()
        );
    }
    // The use-site is a TypeRef; the nested shapes are TypeRefs too.
    let OirType::TypeRef { name } = &module.entities[0].fields[0].field_type else {
        panic!("the field must become a TypeRef");
    };
    assert_eq!(name, "OrderMeta");
    let OirType::Struct { fields } = &module.named_types["OrderMeta"] else {
        panic!("the registered entry carries the struct shape");
    };
    let geo = fields.iter().find(|f| f.name == "geo").unwrap();
    assert!(
        matches!(geo.field_type, OirType::TypeRef { ref name } if name == "OrderMetaGeo"),
        "nested struct becomes a TypeRef, got {:?}",
        geo.field_type
    );
    let tags = fields.iter().find(|f| f.name == "tags").unwrap();
    let OirType::Array { inner } = &tags.field_type else {
        panic!("array stays an array");
    };
    assert!(
        matches!(**inner, OirType::TypeRef { ref name } if name == "OrderMetaTagsItem"),
        "array element becomes a TypeRef, got {:?}",
        inner
    );
    let scores = fields.iter().find(|f| f.name == "scores").unwrap();
    let OirType::Map { value, .. } = &scores.field_type else {
        panic!("map stays a map");
    };
    assert!(
        matches!(**value, OirType::TypeRef { ref name } if name == "OrderMetaScoresValue"),
        "map value becomes a TypeRef, got {:?}",
        value
    );
}

#[test]
fn cycle_guard_resolves_a_self_recursive_struct_to_a_typeref() {
    // An empty field name re-derives the ancestor's positional name — the
    // only way a finite anonymous tree can recurse. The reserve-before-
    // recurse guard must resolve the inner shape to `TypeRef { ancestor }`
    // instead of recursing forever (lolo-emit.ts:453).
    let mut entity = OirEntity::new("node");
    entity.fields.push(OirField::new(
        "self",
        OirType::Struct {
            fields: vec![OirField::new(
                "",
                OirType::Struct {
                    fields: vec![OirField::new("value", OirType::String)],
                },
            )],
        },
    ));
    let mut module = module_with(entity);
    let stats = nominalize_module(&mut module);
    assert_eq!(stats.registered, 1, "only the outer struct registers");
    assert!(
        matches!(
            module.entities[0].fields[0].field_type,
            OirType::TypeRef { ref name } if name == "NodeSelf"
        ),
        "outer struct nominalizes to NodeSelf, got {:?}",
        module.entities[0].fields[0].field_type
    );
    let OirType::Struct { fields } = &module.named_types["NodeSelf"] else {
        panic!("NodeSelf registered");
    };
    assert!(
        matches!(
            fields[0].field_type,
            OirType::TypeRef { ref name } if name == "NodeSelf"
        ),
        "the self-recursive child is a TypeRef back to NodeSelf, got {:?}",
        fields[0].field_type
    );
}

#[test]
fn never_overwrites_a_preexisting_recursive_alias_entry() {
    // Seed named_types the way resolve.rs does for a recursive lolo alias —
    // under the SAME positional name the anonymous shape would claim.
    let mut entity = OirEntity::new("menu");
    entity.fields.push(OirField::new(
        "root",
        OirType::Struct {
            fields: vec![OirField::new("label", OirType::String)],
        },
    ));
    let mut module = module_with(entity);
    module.named_types.insert(
        "MenuRoot".to_string(),
        OirType::Struct {
            fields: vec![OirField::new(
                "label",
                // A DIFFERENT shape than the anonymous one — the collision
                // path, not the dedup path.
                OirType::Optional {
                    inner: Box::new(OirType::String),
                },
            )],
        },
    );
    let stats = nominalize_module(&mut module);

    assert_eq!(stats.skipped_collisions, 1);
    assert_eq!(stats.registered, 0);
    // The pre-existing entry is untouched (first-registration-wins).
    let OirType::Struct { fields } = &module.named_types["MenuRoot"] else {
        panic!("the pre-existing entry survives");
    };
    assert!(
        matches!(fields[0].field_type, OirType::Optional { .. }),
        "the pre-existing entry's shape must not be overwritten"
    );
    // The colliding use-site stays anonymous (its children nominalized).
    assert!(
        matches!(
            module.entities[0].fields[0].field_type,
            OirType::Struct { .. }
        ),
        "a colliding anonymous shape stays anonymous, got {:?}",
        module.entities[0].fields[0].field_type
    );
}

#[test]
fn same_shape_from_both_resolve_paths_dedups_to_one_entry() {
    // The entity path sorts struct fields (resolve.rs:797); the payload path
    // preserves source order (resolve.rs:2827-2842). After canonicalization
    // both orders are ONE identity — one entry, both use-sites TypeRef it.
    let shape_a = || {
        OirType::Struct {
            fields: vec![
                OirField::new("alpha", OirType::String),
                OirField::new("beta", OirType::Number),
            ],
        }
    };
    let shape_b = || {
        OirType::Struct {
            fields: vec![
                OirField::new("beta", OirType::Number),
                OirField::new("alpha", OirType::String),
            ],
        }
    };
    let mut entity = OirEntity::new("doc");
    entity.fields.push(OirField::new("meta", shape_a()));
    let mut module = module_with(entity);
    module.traits.push({
        let mut t = orbital_core::ir::OirTrait::new("viewer");
        let mut event = orbital_core::ir::OirEvent::new("LOAD");
        event.payload_schema = Some(orbital_core::ir::OirTypeMap::from([(
            "meta".to_string(),
            shape_b(),
        )]));
        t.events.push(event);
        t
    });
    let stats = nominalize_module(&mut module);

    assert_eq!(stats.registered, 1, "one canonical entry for both orders");
    assert_eq!(stats.reused_dedup, 1);
    assert_eq!(module.named_types.len(), 1);
    let shared = module.named_types.keys().next().unwrap().clone();
    assert!(
        matches!(
            module.entities[0].fields[0].field_type,
            OirType::TypeRef { name: ref n } if *n == shared
        ),
        "the entity use-site TypeRefs the shared entry"
    );
    let payload_ty = module.traits[0].events[0]
        .payload_schema
        .as_ref()
        .unwrap()["meta"]
        .clone();
    assert!(
        matches!(payload_ty, OirType::TypeRef { name: ref n } if *n == shared),
        "the payload use-site TypeRefs the same entry, got {payload_ty:?}"
    );
    // The registered entry is in canonical (sorted) order.
    let OirType::Struct { fields } = &module.named_types[&shared] else {
        panic!("entry is a struct");
    };
    assert_eq!(
        fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
}

#[test]
fn enum_values_survive_into_the_registered_entry() {
    let mut entity = OirEntity::new("task");
    entity.fields.push(OirField::new(
        "status",
        OirType::Struct {
            fields: vec![{
                let mut f = OirField::new("level", OirType::String);
                f.enum_values = Some(vec!["low".to_string(), "high".to_string()]);
                f
            }],
        },
    ));
    let mut module = module_with(entity);
    nominalize_module(&mut module);

    let OirType::Struct { fields } = &module.named_types["TaskStatus"] else {
        panic!("entry registered");
    };
    assert_eq!(
        fields[0].enum_values,
        Some(vec!["low".to_string(), "high".to_string()]),
        "the enum_values sidecar rides the field into the registered entry"
    );
}

#[test]
fn second_run_is_a_noop_reuse() {
    let mut entity = OirEntity::new("order");
    entity.fields.push(OirField::new(
        "meta",
        OirType::Struct {
            fields: vec![OirField::new("label", OirType::String)],
        },
    ));
    let mut module = module_with(entity);
    let first = nominalize_module(&mut module);
    assert_eq!(first.registered, 1);
    let snapshot = serde_json::to_string(&module).unwrap();
    let second = nominalize_module(&mut module);
    assert_eq!(second.registered, 0, "positional names are all claimed");
    assert_eq!(
        serde_json::to_string(&module).unwrap(),
        snapshot,
        "a second run must not change the module"
    );
}

#[test]
fn unions_nominalize_and_register() {
    let mut entity = OirEntity::new("item");
    entity.fields.push(OirField::new(
        "kind",
        OirType::Union {
            variants: vec![
                OirField::new(
                    "book",
                    OirType::Struct {
                        fields: vec![OirField::new("title", OirType::String)],
                    },
                ),
                OirField::new(
                    "movie",
                    OirType::Struct {
                        fields: vec![OirField::new("runtime", OirType::Number)],
                    },
                ),
            ],
        },
    ));
    let mut module = module_with(entity);
    let stats = nominalize_module(&mut module);
    assert_eq!(stats.registered, 3, "the union + both variant structs");
    assert!(
        matches!(
            module.entities[0].fields[0].field_type,
            OirType::TypeRef { ref name } if name == "ItemKind"
        ),
        "the union field becomes a TypeRef, got {:?}",
        module.entities[0].fields[0].field_type
    );
    assert!(module.named_types.contains_key("ItemKindBook"));
    assert!(module.named_types.contains_key("ItemKindMovie"));
}
