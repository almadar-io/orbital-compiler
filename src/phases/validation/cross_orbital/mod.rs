//! Cross-Orbital Validation
//!
//! Validates cross-orbital concerns: entity uniqueness, event wiring, circular dependencies.
//! Ported from `builder/packages/shared/src/orbitals/validation/orbital-validator.ts`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{OrbitalSchema, TraitDefinition, TraitReference};

mod cycles;
mod entity;
mod event_wiring;
mod listens;
mod payload;
mod relations;
mod scope;

pub(crate) use cycles::validate_circular_dependencies;
pub(crate) use entity::validate_entity_uniqueness;
pub(crate) use event_wiring::{
    extract_emit_event, transition_can_propagate, validate_event_wiring,
};
pub(crate) use listens::validate_listens_integrity;
pub(crate) use payload::validate_external_emit_payloads;
pub(crate) use relations::validate_relation_targets;
pub(crate) use scope::validate_scope_boundary;

/// Validate cross-orbital concerns
pub fn validate_cross_orbital(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();

    // Validate schema-level requirements
    if schema.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaMissingName,
                "name",
                "Schema name is required",
            )
            .with_suggestion("Add a name for your schema"),
        );
    }

    if schema.orbitals.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaNoOrbitals,
                "orbitals",
                "Schema must have at least one orbital",
            )
            .with_suggestion("Add at least one orbital to define application features"),
        );
    }

    // Check version if present
    if let Some(version) = &schema.version {
        if version.is_empty() {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::SchemaEmptyVersion,
                    "version",
                    "Schema version should not be empty",
                )
                .with_suggestion("Use semantic versioning (e.g., '1.0.0')"),
            );
        }
    }

    // Validate entity uniqueness
    result.merge(validate_entity_uniqueness(schema));

    // Validate event wiring
    result.merge(validate_event_wiring(schema));

    // Validate circular dependencies
    result.merge(validate_circular_dependencies(schema));

    // Validate external emit payloads
    result.merge(validate_external_emit_payloads(schema));

    // COMP-7: validate that cross-trait override wires only touch emits
    // declared `scope: 'external'`. Staged as a warning per Phase 2.A.
    result.merge(validate_scope_boundary(schema));

    // COMP-9: validate that every `type: 'relation'` field's target entity
    // resolves to an entity declared in the same schema. Staged as a warning
    // per Phase 2.A; the entity-scope validator still emits the blocking
    // `ORB_E_INVALID_RELATION` error for the same condition.
    result.merge(validate_relation_targets(schema));

    // Cross-orbital trait isolation (gap #13). Listens integrity (error):
    // cross-trait/cross-orbital listens must resolve to a real emit on the
    // named source trait. Backstop for the L1 (`orbital-lolo`) check;
    // covers `use`-imported behaviors that inline new listens after
    // lowering. NOTE: a transition-event collision warning was considered
    // and rejected — once Phase 4 codegen emits qualified bus keys per
    // trait (`UI:ContactCreate.EDIT` vs `UI:DealCreate.EDIT`), two
    // orbitals sharing the bare identifier `EDIT` is the expected
    // consequence of CRUD being a uniform vocabulary, not a smell.
    result.merge(validate_listens_integrity(schema));

    // Validate orbital names
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if orbital.name.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::CrossOrbitalMissingName,
                    format!("orbitals[{}].name", i),
                    "Orbital name is required",
                )
                .with_suggestion("Add a descriptive name for the orbital"),
            );
        }
    }

    ctx.merge(result);
}

/// Collect all trait definitions from schema
/// Note: Traits are now only defined inline within orbitals, not at schema level.
pub(crate) fn collect_all_traits(schema: &OrbitalSchema) -> Vec<&TraitDefinition> {
    let mut traits: Vec<&TraitDefinition> = Vec::new();

    for orbital in &schema.orbitals {
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                traits.push(trait_def);
            }
        }
    }

    traits
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        EmitDefinition, EntityDefinition, EntityRef, EventScope, Orbital, PayloadField,
        StateMachine,
    };
    use std::collections::HashMap;

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![Orbital {
                name: "TaskOrbital".to_string(),
                uses: None,
                theme: None,
                services: None,
                entity: EntityRef::Inline(EntityDefinition {
                    name: "Task".to_string(),
                    collection: None,
                    persistence: None,
                    fields: vec![],
                    instances: None,
                }),
                auxiliary_entities: None,
                traits: vec![],
                pages: None,
                emits: None,
                listens: None,
                exposes: None,
                domain_context: None,
                design: None,
            }],
            metadata: None,
        }
    }

    #[test]
    fn test_valid_schema() {
        let schema = create_minimal_schema();
        let result = validate_entity_uniqueness(&schema);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn test_duplicate_entity() {
        let mut schema = create_minimal_schema();
        schema.orbitals.push(Orbital {
            name: "AnotherOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "Task".to_string(), // Duplicate
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        });

        let result = validate_entity_uniqueness(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::CrossOrbitalDuplicateEntity));
    }

    #[test]
    fn test_missing_schema_name() {
        let mut schema = create_minimal_schema();
        schema.name = String::new();

        let mut ctx = ValidationContext::new();
        validate_cross_orbital(&schema, &mut ctx);

        assert!(!ctx.is_valid());
        assert!(ctx
            .result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaMissingName));
    }

    #[test]
    fn test_empty_orbitals() {
        let mut schema = create_minimal_schema();
        schema.orbitals = vec![];

        let mut ctx = ValidationContext::new();
        validate_cross_orbital(&schema, &mut ctx);

        assert!(!ctx.is_valid());
        assert!(ctx
            .result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaNoOrbitals));
    }

    #[test]
    fn test_external_emit_missing_payload() {
        let mut schema = create_minimal_schema();

        // Add an inline trait with external emit but no payload
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "ORDER_PAID".to_string(),
                scope: Some(EventScope::External),
                description: Some("Emitted when order is paid".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: None, // Missing payload!
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::TraitEventExternalMissingPayload));
    }

    #[test]
    fn test_external_emit_with_payload_passes() {
        let mut schema = create_minimal_schema();

        // Add an inline trait with external emit WITH payload
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "ORDER_PAID".to_string(),
                scope: Some(EventScope::External),
                description: Some("Emitted when order is paid".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: Some(vec![PayloadField {
                    name: "orderId".to_string(),
                    field_type: "string".to_string(),
                    required: Some(true),
                    description: Some("The order ID".to_string()),
                }]),
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!!result.is_valid());
    }

    #[test]
    fn test_internal_emit_no_payload_ok() {
        let mut schema = create_minimal_schema();

        // Internal emits don't require payloads - add as inline trait
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "STATUS_CHANGED".to_string(),
                scope: Some(EventScope::Internal),
                description: Some("Internal status tracking".to_string()),
                synonyms: None,
                tier: None,
                payload_schema: None,
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_external_emit_payloads(&schema);
        assert!(!!result.is_valid());
    }

    // =========================================================================
    // COMP-2: external listen no longer counts as an orphan
    // =========================================================================

    /// Positive case for COMP-2 fix: a trait with an `external`-scoped listen
    /// and no in-schema emitter must NOT produce `ORB_X_ORPHAN_LISTENER`. The
    /// expectation is that the agent bus / another orbital is responsible for
    /// emitting this event; the sibling-only check was producing false
    /// positives.
    #[test]
    fn test_external_listen_not_reported_as_orphan() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: None,
            listens: Some(vec![orbital_core::schema::ListenDefinition {
                event: "USER_LOGGED_IN".to_string(),
                triggers: None,
                scope: Some(EventScope::External),
                source: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_mapping: None,
                guard: None,
            }]),
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_event_wiring(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "External listen should not produce ORB_X_ORPHAN_LISTENER. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case for COMP-2 fix: an `internal`-scoped listen (or one with
    /// no scope, defaulting to internal-equivalent for orphan checks — but we
    /// only check explicit External here) with no in-schema emitter SHOULD
    /// still produce the ORB_X_ORPHAN_LISTENER warning so regressions don't
    /// silently drop coverage.
    #[test]
    fn test_internal_listen_still_reports_orphan() {
        let mut schema = create_minimal_schema();
        schema.orbitals[0].traits = vec![TraitReference::Inline(TraitDefinition {
            name: "OrderTrait".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Order".to_string()),
            emits: None,
            listens: Some(vec![orbital_core::schema::ListenDefinition {
                event: "SOMETHING_WITHIN_ORBITAL".to_string(),
                triggers: None,
                scope: Some(EventScope::Internal),
                source: None,
                description: None,
                synonyms: None,
                tier: None,
                payload_mapping: None,
                guard: None,
            }]),
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        })];

        let result = validate_event_wiring(&schema);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::CrossOrbitalOrphanListener),
            "Internal listen with no emitter should still produce ORB_X_ORPHAN_LISTENER. \
             Warnings: {:?}",
            result.errors
        );
    }

    // =========================================================================
    // COMP-7: validate_scope_boundary
    // =========================================================================

    /// Helper: build an orbital that declares an atom trait with a given
    /// emit scope, and a second orbital that composes it via an
    /// `Alias.traits.Atom` reference with an `events:` rename override.
    fn build_scope_boundary_fixture(atom_scope: EventScope) -> OrbitalSchema {
        let atom = TraitDefinition {
            name: "Atom".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: Some("Task".to_string()),
            emits: Some(vec![EmitDefinition {
                event: "OPEN".to_string(),
                scope: Some(atom_scope),
                description: None,
                synonyms: None,
                tier: None,
                payload_schema: None,
            }]),
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        };
        let atom_orbital = Orbital {
            name: "AtomHost".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "AtomEntity".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![TraitReference::Inline(atom)],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        let mut events_rename = HashMap::new();
        events_rename.insert("OPEN".to_string(), "ADD_ITEM".to_string());
        let composer_ref = TraitReference::Reference {
            reference: Some("Lib.traits.Atom".to_string()),
            name: Some("Composed".to_string()),
            linked_entity: Some("Cart".to_string()),
            config: None,
            events: Some(events_rename),
            fields: None,
            listens: None,
            emits_scope: None,
        };
        let composer_orbital = Orbital {
            name: "Composer".to_string(),
            uses: Some(vec![orbital_core::schema::UseDeclaration {
                from: "local/does-not-matter".to_string(),
                alias: "Lib".to_string(),
            }]),
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "Cart".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![composer_ref],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        };

        OrbitalSchema {
            name: "CompositionApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![atom_orbital, composer_orbital],
            metadata: None,
        }
    }

    /// Positive case: the atom declares `scope: 'external'`, so remapping it
    /// is allowed. Expect no ORB_X_INTERNAL_EVENT_EXPOSED warnings.
    #[test]
    fn test_scope_boundary_external_ok() {
        let schema = build_scope_boundary_fixture(EventScope::External);
        let result = validate_scope_boundary(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::InternalEventExposed),
            "External-scoped atom events must allow remapping. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case: the atom declares `scope: 'internal'`, so a composing
    /// `events:` rename touches a private event. Expect exactly one
    /// ORB_X_INTERNAL_EVENT_EXPOSED warning on the Composer orbital.
    #[test]
    fn test_scope_boundary_internal_warns() {
        let schema = build_scope_boundary_fixture(EventScope::Internal);
        let result = validate_scope_boundary(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::InternalEventExposed)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "Exactly one ORB_X_INTERNAL_EVENT_EXPOSED expected. Warnings: {:?}",
            result.errors
        );
        assert!(hits[0].path.contains("orbitals[1].traits[0].events"));
    }

    /// Bare reference without `events:` / `listens:` overrides must not
    /// produce any COMP-7 warnings regardless of atom scope. This guards
    /// against a naive walker that fires on any `TraitReference::Reference`.
    #[test]
    fn test_scope_boundary_no_override_is_silent() {
        let mut schema = build_scope_boundary_fixture(EventScope::Internal);
        // Drop the events override from the composer trait reference.
        if let Some(TraitReference::Reference {
            events,
            listens,
            ..
        }) = schema.orbitals[1].traits.first_mut()
        {
            *events = None;
            *listens = None;
        }
        let result = validate_scope_boundary(&schema);
        assert!(
            result.errors.is_empty(),
            "References without overrides must be silent. Warnings: {:?}",
            result.errors
        );
    }

    // =========================================================================
    // COMP-9: validate_relation_targets
    // =========================================================================

    /// Positive case: a relation field whose `relation.entity` is declared in
    /// another orbital in the same schema must not produce a warning.
    #[test]
    fn test_relation_targets_in_schema_ok() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        // Add a second orbital that declares "Patient".
        schema.orbitals.push(Orbital {
            name: "PatientOrbital".to_string(),
            uses: None,
            theme: None,
            services: None,
            entity: EntityRef::Inline(EntityDefinition {
                name: "Patient".to_string(),
                collection: None,
                persistence: None,
                fields: vec![],
                instances: None,
            }),
            auxiliary_entities: None,
            traits: vec![],
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
        });
        // Point Task.patient at Patient — should resolve cleanly.
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patient".to_string()),
                field_type: FieldType::Relation,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: Some(RelationDefinition {
                    entity: "Patient".to_string(),
                    cardinality: None,
                }),
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::UnresolvedRelationTarget),
            "Resolvable relation must not warn. Warnings: {:?}",
            result.errors
        );
    }

    /// Negative case: the relation points at an entity that is not declared
    /// anywhere in the schema — expect ORB_X_UNRESOLVED_RELATION_TARGET.
    #[test]
    fn test_relation_targets_dangling_warns() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patient".to_string()),
                field_type: FieldType::Relation,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: Some(RelationDefinition {
                    entity: "Patient".to_string(),
                    cardinality: None,
                }),
                items: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedRelationTarget)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "Dangling relation must produce exactly one ORB_X_UNRESOLVED_RELATION_TARGET. \
             Warnings: {:?}",
            result.errors
        );
        assert!(hits[0].path.contains("orbitals[0].entity.fields[0]"));
    }

    /// Relations nested inside an array's `items.relation.entity` must also
    /// be validated, proving the recursive walker works.
    #[test]
    fn test_relation_targets_nested_in_array_items() {
        use orbital_core::schema::{FieldDefinition, FieldType, RelationDefinition};

        let mut schema = create_minimal_schema();
        if let EntityRef::Inline(task) = &mut schema.orbitals[0].entity {
            task.fields.push(FieldDefinition {
                name: Some("patientList".to_string()),
                field_type: FieldType::Array,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: None,
                relation: None,
                items: Some(Box::new(FieldDefinition {
                    name: None,
                    field_type: FieldType::Relation,
                    required: None,
                    primary_key: None,
                    unique: None,
                    default: None,
                    values: None,
                    relation: Some(RelationDefinition {
                        entity: "Patient".to_string(),
                        cardinality: None,
                    }),
                    items: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                })),
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            });
        }

        let result = validate_relation_targets(&schema);
        let hits: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnresolvedRelationTarget)
            .collect();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains(".items.relation.entity"));
    }
}
