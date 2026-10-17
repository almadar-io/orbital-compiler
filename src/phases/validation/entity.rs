//! Entity Validation
//!
//! Validates entity structure, fields, types, and relations.
//! Ported from `builder/packages/shared/src/orbitals/validation/entity-validator.ts`.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult, VALID_FIELD_TYPES};
use orbital_core_base::schema::{
    EntityDefinition, FieldDefinition, FieldType, OrbitalSchema, Persistence,
};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Regex for valid field names (camelCase)
static FIELD_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-zA-Z0-9]*$").unwrap());

/// Regex for valid entity names (PascalCase)
static ENTITY_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// UI component names that collide with entity names in compiled TypeScript output.
/// Sourced from the single canonical `crate::pattern_registry` embed (no longer a
/// per-phase copy of component-mapping.json).
static RESERVED_COMPONENT_NAMES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    crate::pattern_registry::component_names()
        .map(|s| s.to_string())
        .collect()
});

/// Validate all entities in a schema
pub fn validate_all_entities(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let check_naming = ctx.schema_language.should_check_ascii_naming();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if orbital.is_spec() {
            continue;
        }
        // The primary entity AND every auxiliary entity (multi-entity
        // orbitals, e.g. a relation target declared inline alongside the
        // trait-bound entity) get the SAME per-entity validation — every
        // entity-field rule, not only the primary's. An auxiliary entity was
        // previously invisible to this whole function (`orbital.entity` was
        // the only thing walked), so every entity-field rule was blind to
        // it, not just this one.
        let mut entities: Vec<(&EntityDefinition, String)> = Vec::new();
        if let Some(entity) = orbital.entity.as_inline() {
            entities.push((entity, format!("orbitals[{}].entity", i)));
        }
        for (j, aux) in orbital.auxiliary_entities.iter().flatten().enumerate() {
            if let Some(entity) = aux.as_inline() {
                entities.push((entity, format!("orbitals[{}].auxiliaryEntities[{}]", i, j)));
            }
        }

        for (entity, path) in entities {
            let result = validate_entity(entity, &path, &ctx.entity_names, check_naming);
            ctx.merge(result);

            // Defense-in-depth pair with the lolo lowering check
            // `LowerError::EntityNameShadowsTraitUnion`. The lolo rule
            // catches hand-authored `.lolo`; this validator rule catches
            // `.orb` mutated directly (factory edits, agent rewrites,
            // raw JSON patches) that bypass the lolo parse path. Both
            // emit the same diagnostic so authors and agents see one
            // consistent error message regardless of which path they
            // took. The TS codegen would otherwise produce TS2440
            // "Import declaration conflicts with local declaration".
            for t in &orbital.traits {
                let Some(trait_name) = t.trait_name() else {
                    continue;
                };
                let state_shadow = format!("{}State", trait_name);
                let event_shadow = format!("{}Event", trait_name);
                if entity.name == state_shadow {
                    ctx.add_error(
                        ValidationError::error(
                            ErrorCode::EntityNameShadowsTraitUnion,
                            &path,
                            format!(
                                "Entity '{}' name collides with trait '{}'s generated state union — TS codegen would emit both `import type {{ {} }}` and `type {} = '...'` in every handler/hook/view file (TS2440). Rename the entity (e.g. '{}Data').",
                                entity.name, trait_name, entity.name, entity.name, trait_name
                            ),
                        )
                        .with_suggestion(format!(
                            "Rename entity '{}' to '{}Data' (or any name that doesn't match a trait's <Name>State / <Name>Event union)",
                            entity.name, trait_name
                        )),
                    );
                }
                if entity.name == event_shadow {
                    ctx.add_error(
                        ValidationError::error(
                            ErrorCode::EntityNameShadowsTraitUnion,
                            &path,
                            format!(
                                "Entity '{}' name collides with trait '{}'s generated event union — TS codegen would emit both `import type {{ {} }}` and `type {} = '...'` in every handler/hook/view file (TS2440). Rename the entity (e.g. '{}Data').",
                                entity.name, trait_name, entity.name, entity.name, trait_name
                            ),
                        )
                        .with_suggestion(format!(
                            "Rename entity '{}' to '{}Data' (or any name that doesn't match a trait's <Name>State / <Name>Event union)",
                            entity.name, trait_name
                        )),
                    );
                }
            }
        }
    }

    // Trait-owned `data_entities` get the same shared/persistence-mode check
    // as the primary entity (L1 parity: `check_shared_entity_has_persistence_mode`
    // runs for both in `orbital-lolo`).
    for (trait_def, trait_path) in super::inline_traits(schema) {
        for (i, data_entity) in trait_def.data_entities.iter().flatten().enumerate() {
            if data_entity.shared && data_entity.persistence.is_none() {
                ctx.add_error(
                    ValidationError::error(
                        ErrorCode::SchemaSharedWithoutPersistenceMode,
                        format!("{}.dataEntities[{}].persistence", trait_path, i),
                        format!(
                            "Entity '{}' is [shared] but declares no persistence mode — nothing about a shared entity may be inferred.",
                            data_entity.name
                        ),
                    )
                    .with_suggestion("Add an explicit `persistent: <collection>` or `runtime` persistence tag alongside `[shared]`."),
                );
            }
        }
    }
}

/// Validate a single entity definition
///
/// When `check_naming` is false (non-Latin schemas), ASCII PascalCase/camelCase
/// checks are skipped (GAP-V001).
pub fn validate_entity(
    entity: &EntityDefinition,
    path: &str,
    all_entity_names: &HashSet<String>,
    check_naming: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Entity must have a name
    if entity.name.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityMissingName,
                format!("{}.name", path),
                "Entity name is required",
            )
            .with_suggestion("Add a name for your entity using PascalCase (e.g., 'Task', 'User')"),
        );
    } else if check_naming && !ENTITY_NAME_REGEX.is_match(&entity.name) {
        // GAP-V001: Only check ASCII naming for English/Latin schemas
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityInvalidFieldName,
                format!("{}.name", path),
                format!("Entity name '{}' should be PascalCase", entity.name),
            )
            .with_suggestion("Use PascalCase for entity names (e.g., 'Task', 'UserProfile')"),
        );
    }

    // Entity name must not collide with a UI component name
    if RESERVED_COMPONENT_NAMES.contains(&entity.name) {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityInvalidFieldName, // reuse closest error code
                format!("{}.name", path),
                format!(
                    "Entity name '{}' collides with a UI component name. This causes duplicate identifier errors in compiled output.",
                    entity.name
                ),
            )
            .with_suggestion(format!(
                "Rename the entity to avoid the collision (e.g., '{}Entry', '{}Record', '{}Data')",
                entity.name, entity.name, entity.name
            )),
        );
    }

    // Entity must have at least one field
    if entity.fields.is_empty() {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityNoFields,
                format!("{}.fields", path),
                "Entity must have at least one field",
            )
            .with_suggestion("Add at least one field to define the entity's data structure"),
        );
    }

    // Validate each field
    let mut field_names: HashSet<String> = HashSet::new();
    for (i, field) in entity.fields.iter().enumerate() {
        let field_path = format!("{}.fields[{}]", path, i);
        let field_result = validate_field(
            field,
            &field_path,
            all_entity_names,
            &mut field_names,
            check_naming,
        );
        result.merge(field_result);
    }

    // Check for persistent entity collection
    if let Some(persistence) = &entity.persistence {
        if *persistence == Persistence::Persistent && entity.collection.is_none() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityMissingCollection,
                    format!("{}.collection", path),
                    "Persistent entity requires a collection name",
                )
                .with_suggestion("Add a 'collection' property for database storage"),
            );
        }
    }

    // L2 twin of orbital-lolo's `check_shared_entity_has_persistence_mode`
    // (ORB_S_SHARED_WITHOUT_PERSISTENCE_MODE): nothing about a `[shared]`
    // entity may be inferred. Runs here, pre-inline, while `persistence` is
    // still the author's raw `Option` — before any downstream defaulting.
    if entity.shared && entity.persistence.is_none() {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaSharedWithoutPersistenceMode,
                format!("{}.persistence", path),
                format!(
                    "Entity '{}' is [shared] but declares no persistence mode — nothing about a shared entity may be inferred.",
                    entity.name
                ),
            )
            .with_suggestion("Add an explicit `persistent: <collection>` or `runtime` persistence tag alongside `[shared]`."),
        );
    }

    validate_browser_storage(entity, path, &mut result);

    result
}

/// `[persistent: x, local]` / `mock` / `instances`: where an entity's rows live
/// and what they start as, each decidable on the declaration.
fn validate_browser_storage(entity: &EntityDefinition, path: &str, result: &mut ValidationResult) {
    let persistent = !matches!(entity.persistence, Some(Persistence::Runtime));
    if entity.local && !persistent {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityLocalRequiresPersistent,
                format!("{}.local", path),
                format!(
                    "Entity '{}' is [runtime, local] — a runtime entity's frame already lives in the client; `local` declares where PERSISTED rows live",
                    entity.name
                ),
            )
            .with_suggestion("Use `[persistent: <collection>, local]` for rows stored in the browser, or drop `local`."),
        );
    }
    if entity.seed_mock && !entity.local {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityMockSeedNotLocal,
                format!("{}.seedMock", path),
                format!(
                    "Entity '{}' declares `mock` seeding but its rows are not browser-stored — a server's mock data is its own deployment mode",
                    entity.name
                ),
            )
            .with_suggestion("Add `local` (`[persistent: <collection>, local, mock]`) or drop `mock`."),
        );
    }
    let has_rows = entity
        .instances
        .as_ref()
        .is_some_and(|rows| !rows.is_empty())
        || entity
            .locale_instances
            .as_ref()
            .is_some_and(|m| !m.is_empty());
    if entity.seed_mock && has_rows {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntitySeedSourceConflict,
                format!("{}.instances", path),
                format!(
                    "Entity '{}' declares both `mock` seeding and written-out `instances` — exactly one seed source",
                    entity.name
                ),
            )
            .with_suggestion("Keep the written-out rows, or drop them and keep `mock`."),
        );
    }
    if entity.identity && entity.local {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityIdentityLocal,
                format!("{}.local", path),
                format!(
                    "Entity '{}' is [identity] and `local` — the viewer directory is never browser-resident",
                    entity.name
                ),
            )
            .with_suggestion("Drop `local` from the identity entity."),
        );
    }
    check_instance_rows(
        entity,
        entity.instances.iter().flatten(),
        &format!("{}.instances", path),
        result,
    );
}

/// The checks every written-out row block gets: known fields, required fields,
/// and (browser-stored) a stable `id`.
fn check_instance_rows<'a>(
    entity: &EntityDefinition,
    rows: impl Iterator<Item = &'a orbital_core_base::schema::SObject>,
    rows_path: &str,
    result: &mut ValidationResult,
) {
    for (i, row) in rows.enumerate() {
        let row_path = format!("{}[{}]", rows_path, i);
        for key in row.keys() {
            if !entity
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some(key.as_str()))
            {
                result.add_error(ValidationError::error(
                    ErrorCode::EntityInstanceInvalid,
                    format!("{}.{}", row_path, key),
                    format!(
                        "Row {} of '{}' sets '{}', which is not a field of the entity",
                        i, entity.name, key
                    ),
                ));
            }
        }
        for field in entity
            .fields
            .iter()
            .filter(|f| f.required == Some(true) && f.default.is_none())
        {
            let Some(name) = field.name.as_deref() else {
                continue;
            };
            if !row.contains_key(name) {
                result.add_error(ValidationError::error(
                    ErrorCode::EntityInstanceInvalid,
                    row_path.clone(),
                    format!(
                        "Row {} of '{}' is missing required field '{}'",
                        i, entity.name, name
                    ),
                ));
            }
        }
        if entity.local && !row.contains_key("id") {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInstanceInvalid,
                    row_path.clone(),
                    format!("Row {} of browser-stored '{}' has no `id` — seeded rows need stable ids across reloads", i, entity.name),
                )
                .with_suggestion("Give every written-out row an `id`."),
            );
        }
    }
}

/// Per-locale rows (`instances <lang> [ … ]`): browser-stored entities only, one
/// block per declared non-default locale (the default locale's rows are
/// `instances`), each checked like the default rows.
pub fn validate_all_locale_instances(
    schema: &orbital_core_base::schema::OrbitalSchema,
    result: &mut ValidationResult,
) {
    use orbital_core_base::schema::EntityRef;
    let declared: Vec<String> = schema.locales.clone().unwrap_or_default();
    for (oi, orbital) in schema.orbitals.iter().enumerate() {
        let primary = std::iter::once((format!("orbitals[{}].entity", oi), &orbital.entity));
        let aux = orbital
            .auxiliary_entities
            .iter()
            .flatten()
            .enumerate()
            .map(|(ai, e)| (format!("orbitals[{}].auxiliaryEntities[{}]", oi, ai), e));
        for (path, entity_ref) in primary.chain(aux) {
            let EntityRef::Inline(entity) = entity_ref else {
                continue;
            };
            let Some(blocks) = entity.locale_instances.as_ref().filter(|m| !m.is_empty()) else {
                continue;
            };
            let blocks_path = format!("{}.localeInstances", path);
            if !entity.local {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityLocaleInstancesNotLocal,
                        blocks_path.clone(),
                        format!("Entity '{}' writes rows per locale but its rows are not browser-stored; a server's rows are shared by every viewer", entity.name),
                    )
                    .with_suggestion("Add `local` (`[persistent: <collection>, local]`) or keep one `instances` block."),
                );
            }
            let default = declared.first();
            for locale in blocks.keys() {
                if !declared.contains(locale) || Some(locale) == default {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EntityLocaleInstancesUnknownLocale,
                            format!("{}.{}", blocks_path, locale),
                            match default {
                                Some(d) if d == locale => format!("Entity '{}' writes `instances {}`, but '{}' is the default locale: its rows are the plain `instances` block", entity.name, locale, locale),
                                _ => format!("Entity '{}' writes `instances {}`, but the app declares locales {:?}", entity.name, locale, declared),
                            },
                        )
                        .with_suggestion("Write one block per declared locale other than the first; declare the locale in the app's `locales [ … ]`."),
                    );
                }
            }
            if !entity
                .instances
                .as_ref()
                .is_some_and(|rows| !rows.is_empty())
            {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityLocaleInstancesIncomplete,
                        format!("{}.instances", path),
                        format!("Entity '{}' writes rows per locale but no default `instances` block for its default locale", entity.name),
                    )
                    .with_suggestion("Add `instances [ … ]` with the default locale's rows."),
                );
            }
            for locale in declared.iter().skip(1).filter(|l| !blocks.contains_key(*l)) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityLocaleInstancesIncomplete,
                        blocks_path.clone(),
                        format!(
                            "Entity '{}' writes rows per locale but has no `instances {}` block",
                            entity.name, locale
                        ),
                    )
                    .with_suggestion(
                        "Write a block for every declared locale once any is written.",
                    ),
                );
            }
            for (locale, rows) in blocks {
                check_instance_rows(
                    entity,
                    rows.iter(),
                    &format!("{}.{}", blocks_path, locale),
                    result,
                );
            }
        }
    }
}

/// Validate a single field definition
///
/// When `check_naming` is false (non-Latin schemas), ASCII camelCase
/// checks are skipped (GAP-V001).
pub fn validate_field(
    field: &FieldDefinition,
    path: &str,
    all_entity_names: &HashSet<String>,
    seen_names: &mut HashSet<String>,
    check_naming: bool,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Field must have a name
    if let Some(name) = &field.name {
        if name.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInvalidFieldName,
                    format!("{}.name", path),
                    "Field name cannot be empty",
                )
                .with_suggestion("Provide a valid camelCase field name"),
            );
        } else {
            // Check for duplicate field names
            if !seen_names.insert(name.clone()) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityDuplicateField,
                        format!("{}.name", path),
                        format!("Duplicate field name: '{}'", name),
                    )
                    .with_suggestion("Each field must have a unique name within the entity"),
                );
            }

            // Check field name format (camelCase) — skip for non-Latin schemas (GAP-V001)
            if check_naming && !FIELD_NAME_REGEX.is_match(name) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityInvalidFieldName,
                        format!("{}.name", path),
                        format!("Field name '{}' should be camelCase", name),
                    )
                    .with_suggestion(
                        "Use camelCase for field names (e.g., 'firstName', 'createdAt')",
                    ),
                );
            }
        }
    }

    // Validate field type
    let type_str = field_type_to_string(&field.field_type);
    if !VALID_FIELD_TYPES.contains(&type_str.as_str()) {
        result.add_error(
            ValidationError::error(
                ErrorCode::EntityInvalidFieldType,
                format!("{}.type", path),
                format!("Invalid field type: '{}'", type_str),
            )
            .with_suggestion("Use a valid field type")
            .with_valid_values(VALID_FIELD_TYPES.iter().map(|s| s.to_string()).collect()),
        );
    }

    // The seeder uses an enum's `@mock` candidates as its values (they win over the
    // rotation), so each candidate must be a member.
    if let (Some(values), Some(declared)) = (field.values.as_ref(), field.mock.as_deref()) {
        if field.field_type != FieldType::Union && !values.is_empty() {
            let outside: Vec<&str> = orbital_core_base::policy::mock_candidates(declared)
                .into_iter()
                .filter(|c| !orbital_core_base::type_compat::enum_value_is_member(c, values))
                .collect();
            if !outside.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityMockNotInEnum,
                        format!("{}.mock", path),
                        format!(
                            "@mock candidate(s) {} are not values of this field",
                            outside.join(", ")
                        ),
                    )
                    .with_suggestion("Use only the field's declared values as @mock candidates")
                    .with_valid_values(values.clone()),
                );
            }
        }
    }

    // A literal default on a semantic-domain field must satisfy that domain.
    // Routes through orbital-core's single owner so L1 (lolo) and L2 cannot drift
    // on what counts as a valid email/url/phone/uuid.
    if let Some(kind) = orbital_core_base::type_compat::semantic_string_kind(&type_str) {
        if let Some(orbital_core_base::schema::SExpression::String(literal)) = field.default.as_ref() {
            if literal.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntitySemanticValueInvalid,
                        format!("{}.default", path),
                        format!("A '{}' field cannot default to an empty string", kind),
                    )
                    .with_suggestion(
                        "Drop the default — the mock generator synthesizes a valid value",
                    ),
                );
            } else {
                match orbital_core_eval::evaluator::operators::system::semantic_string_value_valid(kind, literal) {
                    Ok(true) => {}
                    Ok(false) => result.add_error(
                        ValidationError::error(
                            ErrorCode::EntitySemanticValueInvalid,
                            format!("{}.default", path),
                            format!("'{}' is not a valid {}", literal, kind),
                        )
                        .with_suggestion(format!("Provide a valid {} or drop the default", kind)),
                    ),
                    Err(reason) => result.add_error(ValidationError::error(
                        ErrorCode::EntitySemanticValueInvalid,
                        format!("{}.default", path),
                        format!("Cannot check the {} default: {}", kind, reason),
                    )),
                }
            }
        }
    }

    // An entity field's `default` must be structurally compatible with its
    // declared type — a relation field defaulting to a whole row (`seedRow
    // : Note = {}`, the defect this rule exists to catch — the right shape
    // is a typed `Map string <Row>` field like `loadedRow`) instead of an id/
    // absent, a number/boolean default on a string field, etc. Reuses the
    // one shape-vs-type walk (`check_field_def_shape`, `config_type.rs`)
    // rather than a second checker for the same question. The string-vs-
    // declared-enum-membership case is skipped here: `validate_literal_defaults`
    // (`literal_default.rs`) already reports it as `LiteralUnionDefaultMismatch`
    // — one diagnostic per defect, not two.
    if let Some(default) = &field.default {
        let enum_string_case = field.values.is_some()
            && matches!(default, orbital_core_base::schema::SExpression::String(_));
        // A non-`!` field is an OPTIONAL position, and `null` is a legal
        // initial value there for every type — the same rule
        // `render_default`'s `(OirType::Optional { .. }, OirValue::Null)`
        // arm codegens (`orbital-core/src/ir/literal.rs`). An entity default
        // is an INITIAL state, not a complete value, so this is not "every
        // type accepts null", only "an optional field's default may be
        // absent" — a required field with a null default still goes through
        // the shape check below.
        let null_on_optional = !field.required.unwrap_or(false)
            && matches!(default, orbital_core_base::schema::SExpression::Null);
        if !enum_string_case && !null_on_optional {
            // `skip_required_members: true` — a struct/alias member's `!`
            // is enforced at PERSIST time (lifecycle ruling), not on an
            // entity default; the KIND of every present member is still
            // checked. Trait config keeps the strict (`false`) mode.
            let shape = super::config_type::check_field_def_shape(
                field,
                default,
                &format!("{}.default", path),
                &super::config_type::EventScope::default(),
                &HashMap::new(),
                true,
            );
            result.add_errors(shape.errors.into_iter().map(|mut err| {
                err.code = ErrorCode::EntityDefaultTypeMismatch;
                if field.field_type == FieldType::Relation {
                    err = err.with_suggestion(
                        "a relation field's default is absent, null, or an id string; store a row in a typed `Map string <Row>` field",
                    );
                }
                err
            }));
        }
    }

    // Validate enum fields have values
    if field.field_type == FieldType::Enum {
        if let Some(values) = &field.values {
            if values.is_empty() {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityEmptyEnumValues,
                        format!("{}.values", path),
                        "Enum field must have at least one value",
                    )
                    .with_suggestion("Add values array with possible enum values"),
                );
            }
        } else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityEmptyEnumValues,
                    format!("{}.values", path),
                    "Enum field requires a 'values' array",
                )
                .with_suggestion("Add values: [\"option1\", \"option2\"] for enum fields"),
            );
        }
    }

    if field.field_type == FieldType::Array && field.items.is_none() {
        let field_label = field.name.as_deref().unwrap_or("<unnamed>");
        result.add_error(super::error::untyped_array_field(
            &format!("{}.items", path),
            field_label,
        ));
    }

    // Validate relation fields reference existing entities
    if field.field_type == FieldType::Relation {
        if let Some(relation) = &field.relation {
            if !all_entity_names.contains(&relation.entity) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EntityInvalidRelation,
                        format!("{}.relation.entity", path),
                        format!(
                            "Relation target entity '{}' does not exist",
                            relation.entity
                        ),
                    )
                    .with_suggestion("Ensure the related entity is defined in the schema")
                    .with_valid_values(all_entity_names.iter().cloned().collect()),
                );
            }
        } else {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EntityInvalidRelation,
                    format!("{}.relation", path),
                    "Relation field requires a 'relation' configuration",
                )
                .with_suggestion("Add relation: { entity: 'EntityName', cardinality: 'one' }"),
            );
        }
    }

    result
}

/// Convert FieldType to string for validation
fn field_type_to_string(field_type: &FieldType) -> String {
    match field_type {
        FieldType::String => "string".to_string(),
        FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Date => "date".to_string(),
        FieldType::Datetime => "datetime".to_string(),
        FieldType::Timestamp => "timestamp".to_string(),
        FieldType::Email => "email".to_string(),
        FieldType::Url => "url".to_string(),
        FieldType::Phone => "phone".to_string(),
        FieldType::Uuid => "uuid".to_string(),
        FieldType::Image => "image".to_string(),
        FieldType::Money => "money".to_string(),
        FieldType::File => "file".to_string(),
        FieldType::Bytes => "bytes".to_string(),
        FieldType::CallKey => "CallKey".to_string(),
        FieldType::Array | FieldType::Tuple => "array".to_string(),
        FieldType::Object => "object".to_string(),
        FieldType::Enum => "enum".to_string(),
        FieldType::Relation => "relation".to_string(),
        FieldType::Trait => "trait".to_string(),
        FieldType::Orbital => "orbital".to_string(),
        FieldType::Behavior => "behavior".to_string(),
        FieldType::Slot => "slot".to_string(),
        FieldType::Pattern => "pattern".to_string(),
        FieldType::Node => "node".to_string(),
        FieldType::Event => "event".to_string(),
        FieldType::EventAddress => "EventAddress".to_string(),
        FieldType::SExpr => "SExpr".to_string(),
        FieldType::Union => "union".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{Cardinality, RelationDefinition, SExpression};

    fn create_valid_entity() -> EntityDefinition {
        EntityDefinition {
            shared: false,
            identity: false,
            local: false,
            seed_mock: false,
            locale_instances: None,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "Task".to_string(),
            collection: Some("tasks".to_string()),
            persistence: Some(Persistence::Persistent),
            fields: vec![FieldDefinition {
                mock: None,
                merged_from: None,
                name: Some("title".to_string()),
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
                projected_from: None,
            }],
            instances: None,
            id: None,
        }
    }

    fn create_field(name: &str, field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            mock: None,
            merged_from: None,
            name: Some(name.to_string()),
            field_type,
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
            projected_from: None,
        }
    }

    #[test]
    fn test_valid_entity() {
        let entity = create_valid_entity();
        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn a_semantic_default_is_checked_against_the_one_pattern_table() {
        let with_default = |kind: FieldType, literal: &str| {
            let mut entity = create_valid_entity();
            let mut field = create_field("link", kind);
            field.default = Some(SExpression::String(literal.to_string()));
            entity.fields.push(field);
            validate_entity(&entity, "test", &HashSet::new(), true)
        };
        let invalid = |r: &ValidationResult| r.errors.iter().any(|e| e.code == ErrorCode::EntitySemanticValueInvalid);
        assert!(!invalid(&with_default(FieldType::Url, "http://x.y")), "a short host is a URL, as validate/url says");
        assert!(invalid(&with_default(FieldType::Url, "Cover Image")), "control: not a URL");
        assert!(!invalid(&with_default(FieldType::Email, "ann@example.com")));
        assert!(invalid(&with_default(FieldType::Email, "ann@localhost")), "control: the domain needs a dot");
    }

    #[test]
    fn test_entity_missing_name() {
        let mut entity = create_valid_entity();
        entity.name = String::new();

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityMissingName));
    }

    #[test]
    fn test_entity_no_fields() {
        let mut entity = create_valid_entity();
        entity.fields = vec![];

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityNoFields));
    }

    #[test]
    fn test_entity_duplicate_field() {
        let mut entity = create_valid_entity();
        entity.fields.push(create_field("title", FieldType::String)); // Duplicate

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityDuplicateField));
    }

    #[test]
    fn test_entity_enum_no_values() {
        let mut entity = create_valid_entity();
        entity.fields.push(create_field("status", FieldType::Enum));

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityEmptyEnumValues));
    }

    #[test]
    fn test_entity_untyped_array_field_errors_typed_array_field_does_not() {
        let mut entity = create_valid_entity();
        entity
            .fields
            .push(create_field("attachments", FieldType::Array));
        let mut typed_array = create_field("tags", FieldType::Array);
        typed_array.items = Some(Box::new(create_field("", FieldType::String)));
        entity.fields.push(typed_array);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        let untyped_array_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UntypedArrayField)
            .collect();
        assert_eq!(untyped_array_errors.len(), 1, "Errors: {:?}", result.errors);
        assert!(untyped_array_errors[0].path.ends_with(".fields[1].items"));
    }

    #[test]
    fn test_entity_invalid_relation() {
        let mut entity = create_valid_entity();
        let mut field = create_field("owner", FieldType::Relation);
        field.relation = Some(RelationDefinition {
            entity: "NonExistent".to_string(),
            cardinality: None,
            entity_id: None,
        });
        entity.fields.push(field);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityInvalidRelation));
    }

    #[test]
    fn test_entity_valid_relation() {
        let mut entity = create_valid_entity();
        let mut field = create_field("owner", FieldType::Relation);
        field.relation = Some(RelationDefinition {
            entity: "User".to_string(),
            cardinality: None,
            entity_id: None,
        });
        entity.fields.push(field);

        let known_entities: HashSet<String> = ["Task".to_string(), "User".to_string()]
            .into_iter()
            .collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(result.is_valid());
    }

    #[test]
    fn test_entity_default_relation_holds_object_rejected() {
        // The defect this rule exists to catch: `seedRow : Note = {}` — a
        // whole row where only an id/absent belongs.
        let mut entity = create_valid_entity();
        let mut field = create_field("seedRow", FieldType::Relation);
        field.relation = Some(RelationDefinition {
            entity: "Task".to_string(),
            cardinality: None,
            entity_id: None,
        });
        field.default = Some(SExpression::Object(orbital_core_base::schema::SObject::new()));
        entity.fields.push(field);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_relation_id_string_or_absent_accepted() {
        let mut entity = create_valid_entity();
        let mut id_default = create_field("owner", FieldType::Relation);
        id_default.relation = Some(RelationDefinition {
            entity: "Task".to_string(),
            cardinality: None,
            entity_id: None,
        });
        id_default.default = Some(SExpression::String("task-1".to_string()));
        entity.fields.push(id_default);

        let mut no_default = create_field("assignee", FieldType::Relation);
        no_default.relation = Some(RelationDefinition {
            entity: "Task".to_string(),
            cardinality: None,
            entity_id: None,
        });
        entity.fields.push(no_default);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    fn many_relation_field(name: &str, default: SExpression) -> FieldDefinition {
        let mut field = create_field(name, FieldType::Relation);
        field.relation = Some(RelationDefinition {
            entity: "Task".to_string(),
            cardinality: Some(Cardinality::Many),
            entity_id: None,
        });
        field.default = Some(default);
        field
    }

    #[test]
    fn test_entity_default_many_relation_empty_array_accepted() {
        let mut entity = create_valid_entity();
        entity
            .fields
            .push(many_relation_field("children", SExpression::Array(vec![])));

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_many_relation_id_array_accepted() {
        let mut entity = create_valid_entity();
        entity.fields.push(many_relation_field(
            "children",
            SExpression::Array(vec![SExpression::String("task-1".to_string())]),
        ));

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_many_relation_row_in_array_rejected() {
        // "a row in an id slot" — an object element where only id strings
        // belong, the array-cardinality twin of `seedRow : Note = {}`.
        let mut entity = create_valid_entity();
        entity.fields.push(many_relation_field(
            "children",
            SExpression::Array(vec![SExpression::Object(orbital_core_base::schema::SObject::new())]),
        ));

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_many_relation_bare_object_rejected() {
        let mut entity = create_valid_entity();
        entity.fields.push(many_relation_field(
            "children",
            SExpression::Object(orbital_core_base::schema::SObject::new()),
        ));

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_one_relation_array_rejected() {
        let mut entity = create_valid_entity();
        let mut field = create_field("owner", FieldType::Relation);
        field.relation = Some(RelationDefinition {
            entity: "Task".to_string(),
            cardinality: Some(Cardinality::One),
            entity_id: None,
        });
        field.default = Some(SExpression::Array(vec![]));
        entity.fields.push(field);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_number_string_mismatch_rejected() {
        let mut entity = create_valid_entity();
        let mut field = create_field("count", FieldType::Number);
        field.default = Some(SExpression::String("not-a-number".to_string()));
        entity.fields.push(field);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_boolean_number_mismatch_rejected() {
        let mut entity = create_valid_entity();
        let mut field = create_field("active", FieldType::Boolean);
        field.default = Some(SExpression::Number(1.0));
        entity.fields.push(field);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_enum_non_string_rejected() {
        let mut entity = create_valid_entity();
        let mut field = create_field("status", FieldType::Enum);
        field.values = Some(vec!["open".to_string(), "closed".to_string()]);
        field.default = Some(SExpression::Number(1.0));
        entity.fields.push(field);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_enum_unknown_string_member_not_double_reported() {
        // `literal_default.rs`'s `LiteralUnionDefaultMismatch` owns this
        // case (a string default naming no declared enum member); the
        // adapter here must skip it, not report a second diagnostic.
        let mut entity = create_valid_entity();
        let mut field = create_field("status", FieldType::Enum);
        field.values = Some(vec!["open".to_string(), "closed".to_string()]);
        field.default = Some(SExpression::String("archived".to_string()));
        entity.fields.push(field);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_null_on_optional_field_of_any_type_accepted() {
        // `render_default`'s `(OirType::Optional, OirValue::Null)` arm
        // (`orbital-core/src/ir/literal.rs`) accepts null for every inner
        // type; a non-`!` entity field must not re-reject it — for a
        // relation field or a struct-typed field alike.
        let mut entity = create_valid_entity();

        let mut relation_field = create_field("owner", FieldType::Relation);
        relation_field.relation = Some(RelationDefinition {
            entity: "Task".to_string(),
            cardinality: None,
            entity_id: None,
        });
        relation_field.default = Some(SExpression::Null);
        entity.fields.push(relation_field);

        let mut label_prop = create_field("label", FieldType::String);
        label_prop.required = Some(true);
        let mut props = HashMap::new();
        props.insert("label".to_string(), label_prop);
        let mut object_field = create_field("meta", FieldType::Object);
        object_field.properties = Some(props);
        object_field.default = Some(SExpression::Null);
        entity.fields.push(object_field);

        let known_entities: HashSet<String> = ["Task".to_string()].into_iter().collect();
        let result = validate_entity(&entity, "test", &known_entities, true);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_null_on_required_field_still_checked() {
        let mut entity = create_valid_entity();
        let mut field = create_field("count", FieldType::Number);
        field.required = Some(true);
        field.default = Some(SExpression::Null);
        entity.fields.push(field);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_entity_default_incomplete_struct_literal_accepted_required_members_deferred_to_persist()
    {
        // `!` on a struct/alias member is enforced at PERSIST time
        // (lifecycle ruling), not on an entity default — `source : Alias =
        // {}` is legal here even though `Alias` declares required members;
        // only each PRESENT member's kind is checked (config_type.rs's
        // `skip_required_members_tests` proves trait config keeps the
        // strict check).
        let mut entity = create_valid_entity();
        let mut id_prop = create_field("id", FieldType::String);
        id_prop.required = Some(true);
        let mut kind_prop = create_field("kind", FieldType::String);
        kind_prop.required = Some(true);
        let mut props = HashMap::new();
        props.insert("id".to_string(), id_prop);
        props.insert("kind".to_string(), kind_prop);

        let mut field = create_field("source", FieldType::Object);
        field.properties = Some(props);
        field.default = Some(SExpression::Object(orbital_core_base::schema::SObject::new()));
        entity.fields.push(field);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::EntityDefaultTypeMismatch),
            "Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_persistent_entity_missing_collection() {
        let mut entity = create_valid_entity();
        entity.collection = None;

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EntityMissingCollection));
    }

    #[test]
    fn test_shared_entity_without_persistence_mode_errors() {
        let mut entity = create_valid_entity();
        entity.shared = true;
        entity.persistence = None;

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaSharedWithoutPersistenceMode));
    }

    #[test]
    fn test_shared_entity_with_explicit_persistence_mode_passes() {
        let mut entity = create_valid_entity();
        entity.shared = true;
        entity.persistence = Some(Persistence::Runtime);

        let result = validate_entity(&entity, "test", &HashSet::new(), true);
        assert!(!result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::SchemaSharedWithoutPersistenceMode));
    }
}

#[cfg(test)]
mod mock_enum_membership_tests {
    use super::*;

    // The seeder uses an enum's `@mock` candidates as its values, so a candidate that
    // is not a member would seed an illegal row.
    fn field(value: serde_json::Value) -> FieldDefinition {
        serde_json::from_value(value).expect("field fixture must deserialize")
    }

    fn codes(f: &FieldDefinition) -> Vec<ErrorCode> {
        validate_field(
            f,
            "entity.fields[0]",
            &HashSet::new(),
            &mut HashSet::new(),
            true,
        )
        .errors
        .into_iter()
        .map(|e| e.code)
        .collect()
    }

    #[test]
    fn a_mock_candidate_outside_the_enum_is_an_error_naming_it() {
        let f = field(serde_json::json!({
            "name": "status", "type": "string", "values": ["draft", "paid"], "mock": "paid, refunded"
        }));
        let result = validate_field(
            &f,
            "entity.fields[0]",
            &HashSet::new(),
            &mut HashSet::new(),
            true,
        );
        let err = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::EntityMockNotInEnum);
        assert!(err.is_some(), "{:?}", result.errors);
        assert!(err.map(|e| e.message.contains("refunded")).unwrap_or(false));
    }

    #[test]
    fn control_member_candidates_are_accepted() {
        let f = field(serde_json::json!({
            "name": "status", "type": "string", "values": ["draft", "paid"], "mock": "paid, paid, draft"
        }));
        assert!(!codes(&f).contains(&ErrorCode::EntityMockNotInEnum));
    }

    #[test]
    fn control_a_non_enum_field_keeps_free_form_mock() {
        let f = field(
            serde_json::json!({ "name": "client", "type": "string", "mock": "Acme Corp, Globex" }),
        );
        assert!(!codes(&f).contains(&ErrorCode::EntityMockNotInEnum));
    }
}
