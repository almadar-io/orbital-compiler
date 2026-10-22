//! Resolve the sigil type constructors — `@payload <knob>`, `@entity <knob>`
//! and `@user` — against the fully-inlined schema.
//!
//! These are the language's first DEPENDENT types: the argument is a value
//! reference (a config knob whose value names an event or an entity), so the
//! type is not knowable until the call site's overrides are folded. Hence a
//! whole-schema pass after every orbital has inlined, rather than an extension
//! of `resolve_type_param_sentinels` — that one takes a single `&mut Orbital`
//! and cannot see an event declared on a sibling orbital.
//!
//! Resolution is TOTAL by contract: a sentinel becomes a concrete named type
//! plus `properties`, or it is reported. It never degrades to `object` — partly
//! because a `ConfigField` whose `field_type` is literally `"object"` is an
//! unconditional `ORB_T_CONFIG_OBJECT_ARRAY_FORBIDDEN` regardless of
//! properties, and partly because a silent degrade is the failure mode this
//! family exists to avoid.
//!
//! "The target declares nothing" is a legal resolution producing an EMPTY
//! shape. That is what lets a generated atom validate standalone, where the
//! knob still holds its own default and nothing has declared that event.

use orbital_core_base::schema::{
    ConfigField, EntityRef, ExpectDeclaration, FieldDefinition, FieldType, OrbitalSchema,
    PayloadField, SExpression, TraitReference,
};
use std::collections::HashMap;

enum Sigil<'a> {
    EventPayload(&'a str),
    NamedEntity(&'a str),
    BoundEntity,
    Identity,
}

fn parse_sigil(field_type: &str) -> Option<Sigil<'_>> {
    if field_type == "@user" {
        return Some(Sigil::Identity);
    }
    // Bare `@entity` in CONFIG position — the arity-0 constructor, resolving
    // against the carrying trait's own `linkedEntity`. The payload-position
    // spelling is owned by `resolve_type_param_sentinels` (per-orbital, which
    // suffices there); config knobs are resolved here because the sibling
    // arity-1 arms need the whole schema. The two never see the same field.
    if field_type == "@entity" {
        return Some(Sigil::BoundEntity);
    }
    let arg = |p: &str| {
        field_type
            .strip_prefix(p)
            .and_then(|r| r.strip_suffix(')'))
            .filter(|k| !k.is_empty())
    };
    arg("@payload(")
        .map(Sigil::EventPayload)
        .or_else(|| arg("@entity(").map(Sigil::NamedEntity))
}

/// Schema-wide `event -> declared payload`, on the same traversal
/// `cross_emitter` uses and with the same anchor rule: FIRST declaration in
/// (orbital, trait, emits-array) order wins. A schema clean under
/// `ORB_X_CROSS_EMITTER_DECLARED_PAYLOAD_MISMATCH` therefore resolves to the
/// one true shape, and a schema that is not is exactly where the choice would
/// have been ambiguous.
fn build_payload_index(schema: &OrbitalSchema) -> HashMap<String, Vec<PayloadField>> {
    let mut idx: HashMap<String, Vec<PayloadField>> = HashMap::new();
    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            for emit in td.emits.as_deref().unwrap_or(&[]) {
                if let Some(fields) = &emit.payload_schema {
                    if !fields.is_empty() {
                        idx.entry(emit.event.clone())
                            .or_insert_with(|| fields.clone());
                    }
                }
            }
        }
    }
    idx
}

/// `FieldType` is `Deserialize` with lowercase renaming, so serde owns the
/// string→type mapping. Reusing it avoids a second copy of a table
/// `orbital-lolo` already has (which this crate cannot import — they are
/// siblings).
fn field_type_of(name: &str) -> FieldType {
    serde_json::from_value(serde_json::Value::String(name.to_lowercase()))
        .unwrap_or(FieldType::String)
}

pub(crate) fn field_of(p: &PayloadField) -> FieldDefinition {
    let element = orbital_core_base::ir::types::payload_array_item_type(&p.field_type);
    FieldDefinition {
        nullable: p.nullable,
        additional_properties: p
            .additional_properties
            .as_deref()
            .map(field_of)
            .map(Box::new),
        mock: None,
        merged_from: None,
        name: Some(p.name.clone()),
        field_type: if element.is_some() {
            FieldType::Array
        } else {
            field_type_of(&p.field_type)
        },
        required: p.required,
        primary_key: None,
        unique: None,
        default: None,
        values: p.values.clone(),
        relation: None,
        items: p
            .items
            .as_deref()
            .map(field_of)
            .or_else(|| {
                element.map(|element| {
                    field_of(&PayloadField {
                        name: String::new(),
                        field_type: element.to_string(),
                        nullable: None,
                        required: None,
                        properties: p.properties.clone(),
                        items: None,
                        additional_properties: None,
                        values: p.values.clone(),
                        description: None,
                        type_when: None,
                        entity: p.entity.clone(),
                        projected_from: p.projected_from.clone(),
                    })
                })
            })
            .map(Box::new),
        properties: if element.is_some() {
            None
        } else {
            p.properties
                .as_ref()
                .map(|nested| keyed(nested.iter().map(field_of).collect()))
        },
        intrinsic: None,
        description: p.description.clone(),
        synonyms: None,
        projected_from: None,
    }
}

fn keyed(fields: Vec<FieldDefinition>) -> HashMap<String, FieldDefinition> {
    fields
        .into_iter()
        .filter_map(|f| f.name.clone().map(|n| (n, f)))
        .collect()
}

/// Post-inline, `ConfigField.default` already carries the call site's override
/// folded over the declared default — so this IS the resolved value, and no
/// separate effective-config map is needed.
fn knob_value(config: Option<&HashMap<String, ConfigField>>, knob: &str) -> Option<String> {
    match config?.get(knob)?.default.as_ref()? {
        SExpression::String(s) => Some(s.clone()),
        _ => None,
    }
}

pub(crate) fn resolve_sigil_types(schema: &mut OrbitalSchema) {
    let payloads = build_payload_index(schema);

    let mut entities: HashMap<String, Vec<FieldDefinition>> = HashMap::new();
    let mut identity: Option<Vec<FieldDefinition>> = None;
    for orbital in &schema.orbitals {
        if let EntityRef::Inline(def) = &orbital.entity {
            entities.insert(def.name.clone(), def.fields.clone());
            if def.identity && identity.is_none() {
                identity = Some(def.fields.clone());
            }
        }
        for aux in orbital.auxiliary_entities.iter().flatten() {
            if let EntityRef::Inline(def) = aux {
                entities
                    .entry(def.name.clone())
                    .or_insert_with(|| def.fields.clone());
            }
        }
        // `expects` promises are the local type environment a standalone
        // orbital is validated against, so they are a legitimate source.
        for ex in orbital.expects.iter().flatten() {
            match ex {
                ExpectDeclaration::Entity {
                    name,
                    shape: Some(shape),
                    ..
                } => {
                    entities
                        .entry(name.clone())
                        .or_insert_with(|| shape.clone());
                }
                ExpectDeclaration::Identity {
                    shape: Some(shape), ..
                } if identity.is_none() => identity = Some(shape.clone()),
                _ => {}
            }
        }
    }

    // FALLBACK for payload-position `@entity`. The per-orbital resolver
    // (`resolve_orbital_type_param_sentinels`) looks the trait's bound entity
    // up in ITS OWN orbital's entity set, so a trait bound to an entity
    // declared in a SIBLING ORBITAL of the same app is left unresolved —
    // measured on `std-realtime-chat`, where `ChannelRail -> ChannelMember`
    // lives in `ChatMessageOrbital` while `ChannelMember` is declared in
    // `ChannelMembershipOrbital`. This pass runs whole-schema, so it can see
    // both. Only leftovers are touched: anything already resolved no longer
    // carries the sentinel.
    for orbital in &mut schema.orbitals {
        for trait_ref in &mut orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(bound) = td.linked_entity.clone() else {
                continue;
            };
            let Some(fields) = entities.get(&bound) else {
                continue;
            };
            let row: Vec<PayloadField> = fields.iter().map(payload_field_of).collect();
            for emit in td.emits.iter_mut().flatten() {
                if let Some(schema_fields) = emit.payload_schema.as_mut() {
                    substitute_entity_sentinel(schema_fields, &row, &bound);
                }
            }
            if let Some(events) = td.state_machine.events.as_mut() {
                for ev in events.iter_mut() {
                    if let Some(schema_fields) = ev.payload_schema.as_mut() {
                        substitute_entity_sentinel(schema_fields, &row, &bound);
                    }
                }
            }
        }
    }

    // Payload-position `@entity <knob>`: the row of the entity the trait's knob names (post-inline the knob's
    // default carries the call site's override).
    for orbital in &mut schema.orbitals {
        for trait_ref in &mut orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let declared = td.config.clone();
            let resolve = |knob: &str| {
                let name = knob_value(declared.as_ref(), knob)?;
                let fields = entities.get(&name)?;
                Some((
                    fields.iter().map(payload_field_of).collect::<Vec<_>>(),
                    name,
                ))
            };
            for emit in td.emits.iter_mut().flatten() {
                if let Some(schema_fields) = emit.payload_schema.as_mut() {
                    substitute_named_entity_sentinels(schema_fields, resolve);
                }
            }
            if let Some(events) = td.state_machine.events.as_mut() {
                for ev in events.iter_mut() {
                    if let Some(schema_fields) = ev.payload_schema.as_mut() {
                        substitute_named_entity_sentinels(schema_fields, resolve);
                    }
                }
            }
        }
    }

    for orbital in &mut schema.orbitals {
        for trait_ref in &mut orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let declared = td.config.clone();
            let linked = td.linked_entity.clone();
            let Some(config) = td.config.as_mut() else {
                continue;
            };
            for field in config.values_mut() {
                let Some(sigil) = parse_sigil(&field.field_type) else {
                    continue;
                };
                // Capture the definer-knob name BEFORE `sigil` is consumed
                // below — `ORB_LISTEN_ROUTE_STARVES_REQUIRED_FIELD` matches a
                // source emit's `definer_knob` against this field's
                // `payload_for` to find the emit's real payload companion
                // once `field_type` below has overwritten the `@payload(K)`
                // sentinel that used to carry this link.
                let payload_for_knob = match &sigil {
                    Sigil::EventPayload(knob) => Some((*knob).to_string()),
                    _ => None,
                };
                // An arm that cannot resolve leaves the sentinel in place; the
                // config-position walk in `type_param_sweep` is the single
                // owner of the diagnostic, so reporting here too would
                // double-count and split the message's cause taxonomy.
                let resolved = match sigil {
                    Sigil::EventPayload(knob) => knob_value(declared.as_ref(), knob).map(|event| {
                        // Nothing declares this event's payload -> EMPTY. That
                        // is an answer, not a failure: the knob guarantees no
                        // fields.
                        (
                            format!("{}Payload", pascal(&event)),
                            payloads
                                .get(&event)
                                .map(|f| f.iter().map(field_of).collect())
                                .unwrap_or_default(),
                        )
                    }),
                    Sigil::NamedEntity(knob) => {
                        knob_value(declared.as_ref(), knob).and_then(|name| {
                            entities
                                .get(&name)
                                .map(|f| (format!("{}Row", pascal(&name)), f.clone()))
                        })
                    }
                    Sigil::BoundEntity => linked.as_ref().and_then(|name| {
                        entities
                            .get(name)
                            .map(|f| (format!("{}Row", pascal(name)), f.clone()))
                    }),
                    Sigil::Identity => identity
                        .as_ref()
                        .map(|f| ("IdentityRow".to_string(), f.clone())),
                };
                if let Some((type_name, fields)) = resolved {
                    // NEVER `object` — that is an unconditional hard error for
                    // a config field regardless of `properties`.
                    field.field_type = type_name;
                    field.properties = Some(keyed(fields));
                    if let Some(knob) = payload_for_knob {
                        field.payload_for = Some(knob);
                    }
                }
            }
        }
    }
}

/// A `FieldDefinition` rendered as the `PayloadField` a resolved row carries.
///
/// Carries the field's own SHAPE through — nested struct `properties`, an
/// array's element shape, a union's variants. Dropping them (this used to set
/// `properties: None` unconditionally) meant every struct-typed entity field
/// arrived on the bus as a shapeless `object`: an entity honestly declaring
/// `heroSpawn : Position` still published `heroSpawn : object` on `VIEW.row`,
/// and `ORB_T_GENERIC_TYPE_DEPRECATED` reported the row snapshot as untyped
/// while the entity it snapshots was fully typed. The whole `row-snapshot`
/// class of that warning was this function.
pub(crate) fn payload_field_of(f: &FieldDefinition) -> PayloadField {
    let union_items = matches!(f.field_type, FieldType::Array | FieldType::Object)
        && f.properties.is_none()
        && f.items
            .as_deref()
            .is_some_and(|item| item.field_type == FieldType::Union);
    // `Object` + `items` is a MAP (dynamic keys, uniform value): it has no
    // properties by construction and carries its value shape in the tag.
    let tag = if union_items && f.field_type == FieldType::Object {
        "object".to_string()
    } else if f.field_type == FieldType::Object && f.items.is_some() {
        let v = f
            .items
            .as_deref()
            .map(crate::phases::validation::field_type_str);
        format!("Map<string,{}>", v.unwrap_or_else(|| "string".to_string()))
    } else {
        serde_json::to_value(&f.field_type)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "string".to_string())
    };

    // Struct members / union variants, in the `Vec<PayloadField>` shape the
    // payload IR uses. A map field carries its VALUE schema in `items` and has
    // no properties by construction, so it is left alone.
    let props: Option<Vec<PayloadField>> = match f.field_type {
        // A map's value shape rides in the tag above, not in properties.
        FieldType::Object if f.items.is_some() => None,
        FieldType::Object | FieldType::Union => f.properties.as_ref().and_then(|m| {
            // `values` fixes the variant order for a union; a struct has none.
            // `m` is a `std::collections::HashMap` — its iteration order is
            // randomized per process (Rust's default hasher), NOT stable —
            // so the struct fallback sorts by name for a deterministic field
            // order across separate resolves of the same input.
            let ordered: Vec<&String> = match (&f.field_type, &f.values) {
                (FieldType::Union, Some(names)) => names.iter().collect(),
                _ => {
                    let mut keys: Vec<&String> = m.keys().collect();
                    keys.sort();
                    keys
                }
            };
            let v: Vec<PayloadField> = ordered
                .into_iter()
                .filter_map(|k| {
                    // A member's name is its key; the IR's per-member `name` is optional.
                    m.get(k).map(|member| PayloadField {
                        name: k.clone(),
                        ..payload_field_of(member)
                    })
                })
                .collect();
            (!v.is_empty()).then_some(v)
        }),
        // An array of structs: the payload convention is the `[object]` tag with
        // the ELEMENT's fields in `properties`. An array of a union keeps its element in `items` instead.
        FieldType::Array if !union_items => f
            .items
            .as_deref()
            .and_then(|it| payload_field_of(it).properties),
        _ => None,
    };

    let tag = match (&f.field_type, &props) {
        (FieldType::Array, _) if union_items => "array".to_string(),
        (FieldType::Array, Some(_)) => "[object]".to_string(),
        // A primitive-element array: carry the item's own tag instead of
        // falling back to the bare, untyped `array` string.
        (FieldType::Array, None) => match f.items.as_deref() {
            Some(item) => format!("[{}]", payload_field_of(item).field_type),
            None => tag,
        },
        _ => tag,
    };

    PayloadField {
        nullable: f.nullable,
        additional_properties: f
            .additional_properties
            .as_deref()
            .map(payload_field_of)
            .map(Box::new),
        values: f.values.clone(),
        name: f.name.clone().unwrap_or_default(),
        field_type: tag,
        required: f.required,
        description: None,
        properties: props,
        type_when: None,
        entity: None,
        projected_from: None,
        items: if f.items.is_some() {
            f.items.as_deref().map(|item| {
                Box::new(PayloadField {
                    name: item.name.clone().unwrap_or_else(|| "item".to_string()),
                    ..payload_field_of(item)
                })
            })
        } else {
            None
        },
    }
}

/// Replace a WRAPPER field still typed `@entity` with the bound row's shape —
/// exactly what the per-orbital resolver (`resolve_sentinel_fields`) produces:
/// `object` / `[object]`, the row's fields in `properties`, and the `entity`
/// marker naming the row (its only surviving identity once flattened).
fn substitute_entity_sentinel(fields: &mut [PayloadField], row: &[PayloadField], entity: &str) {
    for f in fields.iter_mut() {
        let shape = match f.field_type.as_str() {
            "@entity" => "object",
            "[@entity]" => "[object]",
            _ => continue,
        };
        f.field_type = shape.to_string();
        f.properties = Some(row.to_vec());
        f.entity = Some(entity.to_string());
    }
}

/// Replace a field typed by the entity a knob names (`@entity(<knob>)` / `[@entity(<knob>)]`) with that entity's row,
/// stamped exactly like [`substitute_entity_sentinel`]. `resolve` answers a knob name with the named entity's row and
/// name; a knob that names no declared entity keeps its sentinel, which the unresolved-type sweep reports.
fn substitute_named_entity_sentinels(
    fields: &mut [PayloadField],
    resolve: impl Fn(&str) -> Option<(Vec<PayloadField>, String)>,
) {
    for f in fields.iter_mut() {
        let (knob, shape) = if let Some(k) = f
            .field_type
            .strip_prefix("[@entity(")
            .and_then(|r| r.strip_suffix(")]"))
        {
            (k, "[object]")
        } else if let Some(k) = f
            .field_type
            .strip_prefix("@entity(")
            .and_then(|r| r.strip_suffix(')'))
        {
            (k, "object")
        } else {
            continue;
        };
        let Some((row, entity)) = resolve(knob) else {
            continue;
        };
        f.field_type = shape.to_string();
        f.properties = Some(row);
        f.entity = Some(entity);
    }
}

fn pascal(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{payload_field_of, substitute_entity_sentinel, substitute_named_entity_sentinels};
    use orbital_core_base::schema::{FieldDefinition, PayloadField};

    fn payload(json: serde_json::Value) -> PayloadField {
        serde_json::from_value(json).expect("PayloadField")
    }

    /// A sibling-orbital `@entity` resolves to the same shape and marker the per-orbital
    /// resolver stamps: `object` / `[object]` + the row + `entity`, so consumers judge it as a row.
    #[test]
    fn a_sibling_orbital_entity_sentinel_is_stamped_with_its_entity_like_the_per_orbital_resolver()
    {
        let row = vec![payload(
            serde_json::json!({ "name": "id", "type": "string" }),
        )];
        let mut fields = vec![
            payload(serde_json::json!({ "name": "row", "type": "@entity" })),
            payload(serde_json::json!({ "name": "rows", "type": "[@entity]" })),
        ];
        substitute_entity_sentinel(&mut fields, &row, "NoteTagRef");
        assert_eq!(fields[0].field_type, "object");
        assert_eq!(fields[0].entity.as_deref(), Some("NoteTagRef"));
        assert_eq!(fields[1].field_type, "[object]");
        assert_eq!(fields[1].entity.as_deref(), Some("NoteTagRef"));
    }

    #[test]
    fn control_a_non_sentinel_field_is_left_alone() {
        let row = vec![payload(
            serde_json::json!({ "name": "id", "type": "string" }),
        )];
        let mut fields = vec![payload(
            serde_json::json!({ "name": "id", "type": "string" }),
        )];
        substitute_entity_sentinel(&mut fields, &row, "NoteTagRef");
        assert_eq!(fields[0].field_type, "string");
        assert_eq!(fields[0].entity, None);
    }

    /// A payload field typed by the entity a knob names (`@entity <knob>`) takes that entity's row, stamped like the
    /// bound-entity sentinel: `object` / `[object]`, the row's fields, and the entity's name.
    #[test]
    fn a_payload_field_typed_by_the_entity_a_knob_names_takes_that_entitys_row() {
        let rows = |knob: &str| match knob {
            "sourceEntity" => Some((
                vec![payload(
                    serde_json::json!({ "name": "status", "type": "string" }),
                )],
                "Order".to_string(),
            )),
            _ => None,
        };
        let mut fields = vec![
            payload(serde_json::json!({ "name": "row", "type": "@entity(sourceEntity)" })),
            payload(serde_json::json!({ "name": "rows", "type": "[@entity(sourceEntity)]" })),
            payload(serde_json::json!({ "name": "other", "type": "@entity(missingKnob)" })),
            payload(serde_json::json!({ "name": "count", "type": "number" })),
        ];
        substitute_named_entity_sentinels(&mut fields, rows);
        assert_eq!(fields[0].field_type, "object");
        assert_eq!(fields[0].entity.as_deref(), Some("Order"));
        assert_eq!(
            fields[0].properties.as_ref().map(|p| p[0].name.as_str()),
            Some("status")
        );
        assert_eq!(fields[1].field_type, "[object]");
        assert_eq!(fields[1].entity.as_deref(), Some("Order"));
        // controls: a knob that names no entity leaves its sentinel for the pass that reports it; a plain field is untouched.
        assert_eq!(fields[2].field_type, "@entity(missingKnob)");
        assert_eq!(fields[3].field_type, "number");
        assert_eq!(fields[3].entity, None);
    }

    fn field(json: serde_json::Value) -> FieldDefinition {
        serde_json::from_value(json).expect("FieldDefinition")
    }

    fn names(p: &PayloadField) -> Vec<String> {
        p.properties
            .as_ref()
            .map(|ps| ps.iter().map(|m| m.name.clone()).collect())
            .unwrap_or_default()
    }

    /// A struct member's name is its `properties` key; the IR's per-member
    /// `name` is optional (implied by the parent).
    #[test]
    fn struct_members_are_named_by_their_key() {
        let f = field(serde_json::json!({
            "name": "address", "type": "object",
            "properties": { "street": { "type": "string" }, "zip": { "type": "string" } }
        }));
        assert_eq!(names(&payload_field_of(&f)), vec!["street", "zip"]);
    }

    #[test]
    fn array_of_struct_members_are_named_by_their_key() {
        let f = field(serde_json::json!({
            "name": "exercises", "type": "array",
            "items": { "type": "object", "properties": {
                "name": { "type": "string", "required": true },
                "sets": { "type": "number" }
            } }
        }));
        let p = payload_field_of(&f);
        assert_eq!(p.field_type, "[object]");
        assert_eq!(names(&p), vec!["name", "sets"]);
    }

    /// Control: the `.lolo` lowering stamps each member's name — unchanged.
    #[test]
    fn named_members_keep_their_name() {
        let f = field(serde_json::json!({
            "name": "address", "type": "object",
            "properties": { "street": { "name": "street", "type": "string" } }
        }));
        assert_eq!(names(&payload_field_of(&f)), vec!["street"]);
    }
}
