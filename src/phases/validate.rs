//! Validate Phase
//!
//! Validates the parsed schema for structural correctness and consistency.

use orbital_core::schema::{
    EntityDefinition, ExpectDeclaration, Orbital, OrbitalSchema, PageDefinition, StringOrArray,
    TraitDefinition, TraitReference,
};
use std::collections::HashSet;
use thiserror::Error;

/// Validation error
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Schema error at {path}: {message}")]
    SchemaError { path: String, message: String },

    #[error("Missing required field at {path}: {field}")]
    MissingField { path: String, field: String },

    #[error("Invalid reference at {path}: {reference} not found")]
    InvalidReference { path: String, reference: String },

    #[error("Duplicate definition at {path}: {name}")]
    DuplicateDefinition { path: String, name: String },

    #[error("State machine error at {path}: {message}")]
    StateMachineError { path: String, message: String },

    #[error("Multiple errors: {0:?}")]
    Multiple(Vec<ValidationError>),
}

impl ValidationError {
    fn schema_error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::SchemaError {
            path: path.into(),
            message: message.into(),
        }
    }

    fn missing_field(path: impl Into<String>, field: impl Into<String>) -> Self {
        Self::MissingField {
            path: path.into(),
            field: field.into(),
        }
    }

    fn invalid_reference(path: impl Into<String>, reference: impl Into<String>) -> Self {
        Self::InvalidReference {
            path: path.into(),
            reference: reference.into(),
        }
    }

    fn duplicate(path: impl Into<String>, name: impl Into<String>) -> Self {
        Self::DuplicateDefinition {
            path: path.into(),
            name: name.into(),
        }
    }

    fn state_machine_error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::StateMachineError {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Validation context
struct ValidationContext {
    errors: Vec<ValidationError>,
    entity_names: HashSet<String>,
    trait_names: HashSet<String>,
    page_paths: HashSet<String>,
}

impl ValidationContext {
    fn new() -> Self {
        Self {
            errors: Vec::new(),
            entity_names: HashSet::new(),
            trait_names: HashSet::new(),
            page_paths: HashSet::new(),
        }
    }

    fn add_error(&mut self, error: ValidationError) {
        self.errors.push(error);
    }

    fn into_result(self) -> Result<(), ValidationError> {
        if self.errors.is_empty() {
            Ok(())
        } else if self.errors.len() == 1 {
            Err(self.errors.into_iter().next().unwrap())
        } else {
            Err(ValidationError::Multiple(self.errors))
        }
    }
}

/// Validate a schema
pub fn validate_schema(schema: &OrbitalSchema) -> Result<(), ValidationError> {
    let mut ctx = ValidationContext::new();

    // Validate schema-level fields
    validate_schema_fields(schema, &mut ctx);

    // First pass: collect all names for reference checking
    collect_names(schema, &mut ctx);

    // Second pass: validate each orbital
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        validate_orbital(orbital, i, &mut ctx);
    }

    ctx.into_result()
}

fn validate_schema_fields(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    if schema.name.is_empty() {
        ctx.add_error(ValidationError::missing_field("schema", "name"));
    }

    // Version is optional, but if present should not be empty
    if let Some(version) = &schema.version {
        if version.is_empty() {
            ctx.add_error(ValidationError::schema_error(
                "schema.version",
                "version cannot be empty",
            ));
        }
    }
}

fn collect_names(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for orbital in &schema.orbitals {
        // Collect entity names (only for inline entities, not references)
        if let Some(entity) = orbital.entity.as_inline() {
            if !ctx.entity_names.insert(entity.name.clone()) {
                ctx.add_error(ValidationError::duplicate(
                    format!("orbitals.{}", orbital.name),
                    &entity.name,
                ));
            }
        }

        // Gap #22 — auxiliary entities are imported atom entities surfaced
        // when a trait reference omits the `-> Entity` rebind at the call
        // site. Each one is a legitimate orbital-scope entity for purposes
        // of `linkedEntity` resolution + fetch/persist/set entity-arg
        // checks. Don't error on duplicates here — the inline phase already
        // dedups by name and skips collisions with the primary entity.
        if let Some(aux_list) = &orbital.auxiliary_entities {
            for aux_ref in aux_list {
                if let Some(aux_entity) = aux_ref.as_inline() {
                    ctx.entity_names.insert(aux_entity.name.clone());
                }
            }
        }

        // `expects entity X` / `expects identity X` declare a name this
        // program does NOT provide — the consumer promises it, and the
        // provider is checked at the link step once one supplies it (see
        // `validation/expectations.rs`). Relation targets, `linkedEntity`
        // bindings and fetch/persist entity args naming it are therefore
        // legitimate here. NAMES ONLY: no `EntityDefinition` is synthesized,
        // so nothing downstream can mistake an expectation for a declaration.
        if let Some(expects) = &orbital.expects {
            for expectation in expects {
                match expectation {
                    ExpectDeclaration::Entity { name, .. } => {
                        ctx.entity_names.insert(name.clone());
                    }
                    ExpectDeclaration::Identity { name: Some(name), .. } => {
                        ctx.entity_names.insert(name.clone());
                    }
                    _ => {}
                }
            }
        }

        // Collect inline trait names from trait references
        for trait_ref in &orbital.traits {
            if let TraitReference::Inline(trait_def) = trait_ref {
                if !ctx.trait_names.insert(trait_def.name.clone()) {
                    ctx.add_error(ValidationError::duplicate(
                        format!("orbitals.{}.traits", orbital.name),
                        &trait_def.name,
                    ));
                }
            }
        }

        // Collect page paths (only for inline pages, not references)
        if let Some(pages) = &orbital.pages {
            for page_ref in pages {
                if let Some(page) = page_ref.as_inline() {
                    if !ctx.page_paths.insert(page.path.clone()) {
                        ctx.add_error(ValidationError::duplicate(
                            format!("orbitals.{}.pages", orbital.name),
                            &page.path,
                        ));
                    }
                }
            }
        }
    }
}

fn validate_orbital(orbital: &Orbital, index: usize, ctx: &mut ValidationContext) {
    let path = format!("orbitals[{}]", index);

    // Validate orbital name
    if orbital.name.is_empty() {
        ctx.add_error(ValidationError::missing_field(&path, "name"));
    }

    // Validate entity (only if inline, references are validated during resolution)
    if let Some(entity) = orbital.entity.as_inline() {
        validate_entity(entity, &format!("{}.entity", path), ctx);
    }

    // Validate orbital has at least one trait
    if orbital.traits.is_empty() {
        ctx.add_error(ValidationError::schema_error(
            format!("{}.traits", path),
            "Orbital has no traits. Every orbital must have at least one interaction trait.",
        ));
    }

    // Validate trait references
    for (i, trait_ref) in orbital.traits.iter().enumerate() {
        validate_trait_reference(
            trait_ref,
            &crate::phases::validation::trait_path(index, &orbital.name, i, trait_ref),
            ctx,
        );
    }

    // Validate pages if present (only inline pages, references are validated during resolution)
    if let Some(pages) = &orbital.pages {
        for (i, page_ref) in pages.iter().enumerate() {
            if let Some(page) = page_ref.as_inline() {
                validate_page(page, &format!("{}.pages[{}]", path, i), ctx);
            }
        }
    }
}

fn validate_entity(entity: &EntityDefinition, path: &str, ctx: &mut ValidationContext) {
    // Validate entity name
    if entity.name.is_empty() {
        ctx.add_error(ValidationError::missing_field(path, "name"));
    }

    // Validate fields
    let mut field_names = HashSet::new();
    for (i, field) in entity.fields.iter().enumerate() {
        let field_path = format!("{}.fields[{}]", path, i);

        if let Some(name) = &field.name {
            if !field_names.insert(name.clone()) {
                ctx.add_error(ValidationError::duplicate(&field_path, name));
            }
        }

        // Validate relation references
        if let Some(relation) = &field.relation {
            if !ctx.entity_names.contains(&relation.entity) {
                ctx.add_error(ValidationError::invalid_reference(
                    &field_path,
                    &relation.entity,
                ));
            }
        }
    }
}

fn validate_trait_reference(trait_ref: &TraitReference, path: &str, ctx: &mut ValidationContext) {
    match trait_ref {
        TraitReference::Inline(trait_def) => {
            validate_trait(trait_def, path, ctx);
        }
        TraitReference::Reference {
            reference,
            name,
            linked_entity,
            ..
        } => {
            // Check if the referenced trait exists
            let trait_name = reference.as_ref().or(name.as_ref());
            if let Some(trait_name) = trait_name {
                if !ctx.trait_names.contains(trait_name) {
                    ctx.add_error(ValidationError::invalid_reference(path, trait_name));
                }
            }
            // Validate linked entity if specified
            if let Some(linked) = linked_entity {
                if !ctx.entity_names.contains(linked) {
                    ctx.add_error(ValidationError::invalid_reference(
                        format!("{}.linkedEntity", path),
                        linked,
                    ));
                }
            }
        }
    }
}

fn validate_trait(trait_def: &TraitDefinition, path: &str, ctx: &mut ValidationContext) {
    // Validate trait name
    if trait_def.name.is_empty() {
        ctx.add_error(ValidationError::missing_field(path, "name"));
    }

    // Validate linked entity exists
    if let Some(linked) = &trait_def.linked_entity {
        if !ctx.entity_names.contains(linked) {
            ctx.add_error(ValidationError::invalid_reference(
                format!("{}.linkedEntity", path),
                linked,
            ));
        }
    }

    // Validate state machine
    validate_state_machine(trait_def, path, ctx);
}

fn validate_state_machine(trait_def: &TraitDefinition, path: &str, ctx: &mut ValidationContext) {
    let sm = &trait_def.state_machine;

    // Check for at least one state
    if sm.states.is_empty() {
        ctx.add_error(ValidationError::state_machine_error(
            path,
            "Trait must have at least one state",
        ));
        return;
    }

    // Collect state names
    let state_names: HashSet<&str> = sm.states.iter().map(|s| s.name.as_str()).collect();

    // Check for duplicate state names
    let mut seen = HashSet::new();
    for state in &sm.states {
        if !seen.insert(&state.name) {
            ctx.add_error(ValidationError::duplicate(
                format!("{}.stateMachine.states", path),
                &state.name,
            ));
        }
    }

    // Check that exactly one state is marked as initial (or none, will be inferred)
    let initial_count = sm
        .states
        .iter()
        .filter(|s| s.is_initial == Some(true))
        .count();
    if initial_count > 1 {
        ctx.add_error(ValidationError::state_machine_error(
            format!("{}.stateMachine", path),
            "Multiple states marked as initial",
        ));
    }

    // Validate transitions
    for (i, transition) in sm.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        // Validate from states
        validate_transition_from(&transition.from, &state_names, &trans_path, ctx);

        // Validate to state
        if !state_names.contains(transition.to.as_str()) {
            ctx.add_error(ValidationError::invalid_reference(
                &trans_path,
                &transition.to,
            ));
        }
    }

    // Validate modal transitions have CLOSE/CANCEL
    validate_modal_transitions(trait_def, path, ctx);
}

/// Check if an effect is a render-ui to modal or drawer slot
fn is_modal_render_effect(effect: &orbital_core::schema::SExpression) -> bool {
    if let Some(op) = effect.operator() {
        if op == "render-ui" {
            if let Some(args) = effect.arguments() {
                if let Some(slot) = args.first().and_then(|a| a.as_str()) {
                    // Check if rendering to modal or drawer slot with non-null content
                    if slot == "modal" || slot == "drawer" {
                        // Check if the second argument is not null (opening a modal)
                        if let Some(content) = args.get(1) {
                            return !content.is_null();
                        }
                    }
                }
            }
        }
    }
    false
}

/// Check if an effect clears a modal/drawer slot
fn is_modal_clear_effect(effect: &orbital_core::schema::SExpression) -> bool {
    if let Some(op) = effect.operator() {
        if op == "render-ui" {
            if let Some(args) = effect.arguments() {
                if let Some(slot) = args.first().and_then(|a| a.as_str()) {
                    if slot == "modal" || slot == "drawer" {
                        // Check if the second argument is null (clearing the modal)
                        if let Some(content) = args.get(1) {
                            return content.is_null();
                        }
                    }
                }
            }
        }
    }
    false
}

/// Validate that transitions opening modals have corresponding CLOSE/CANCEL transitions
fn validate_modal_transitions(
    trait_def: &TraitDefinition,
    path: &str,
    ctx: &mut ValidationContext,
) {
    let sm = &trait_def.state_machine;

    // Find all states that are targets of modal-opening transitions
    let mut modal_states: HashSet<&str> = HashSet::new();

    for transition in &sm.transitions {
        if let Some(effects) = &transition.effects {
            for effect in effects {
                if is_modal_render_effect(effect) {
                    modal_states.insert(&transition.to);
                }
            }
        }
    }

    // For each modal state, check if there's a transition that clears the modal slot.
    // Any event name is accepted — the semantic requirement is that the transition
    // actually clears the modal (has a ["render-ui", "modal", null] effect).
    // This allows schemas that rename CLOSE via the `events` override on a
    // TraitReference (e.g. "CLOSE" → "CANCEL_ADD") to still satisfy the check.
    for modal_state in modal_states {
        let has_close_transition = sm.transitions.iter().any(|t| {
            // Check if transition is from this modal state
            let from_matches = match &t.from {
                StringOrArray::Single(s) => s == modal_state,
                StringOrArray::Multiple(arr) => arr.iter().any(|s| s == modal_state),
            };

            if !from_matches {
                return false;
            }

            // Check if effects include clearing the modal — any event name is acceptable
            if let Some(effects) = &t.effects {
                effects.iter().any(is_modal_clear_effect)
            } else {
                false
            }
        });

        if !has_close_transition {
            ctx.add_error(ValidationError::state_machine_error(
                format!("{}.stateMachine", path),
                format!(
                    "State '{}' opens a modal but has no exit transition that clears it. \
                     Add a transition from '{}' with any event (e.g. 'CLOSE' or 'CANCEL') \
                     whose effects include [\"render-ui\", \"modal\", null].",
                    modal_state, modal_state
                ),
            ));
        }
    }
}

fn validate_transition_from(
    from: &StringOrArray,
    state_names: &HashSet<&str>,
    path: &str,
    ctx: &mut ValidationContext,
) {
    match from {
        StringOrArray::Single(state) => {
            if state != "*" && !state_names.contains(state.as_str()) {
                ctx.add_error(ValidationError::invalid_reference(path, state));
            }
        }
        StringOrArray::Multiple(states) => {
            for state in states {
                if state != "*" && !state_names.contains(state.as_str()) {
                    ctx.add_error(ValidationError::invalid_reference(path, state));
                }
            }
        }
    }
}

fn validate_page(page: &PageDefinition, path: &str, ctx: &mut ValidationContext) {
    // Validate page name
    if page.name.is_empty() {
        ctx.add_error(ValidationError::missing_field(path, "name"));
    }

    // Validate page path
    if page.path.is_empty() {
        ctx.add_error(ValidationError::missing_field(path, "path"));
    }

    // Validate trait references on the page
    if let Some(traits) = &page.traits {
        for (i, trait_ref) in traits.iter().enumerate() {
            let trait_name = trait_ref.trait_name();
            if let Some(name) = trait_name {
                if !ctx.trait_names.contains(name) {
                    ctx.add_error(ValidationError::invalid_reference(
                        format!("{}.traits[{} ({})]", path, i, name),
                        name,
                    ));
                }
            }
        }
    }

    // Validate primary entity reference
    if let Some(entity) = &page.primary_entity {
        if !ctx.entity_names.contains(entity) {
            ctx.add_error(ValidationError::invalid_reference(
                format!("{}.primaryEntity", path),
                entity,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{
        SExpression, StateDefinition, StateMachine, TraitDefinition, TransitionDefinition,
    };

    fn create_minimal_schema() -> OrbitalSchema {
        OrbitalSchema {
            theme: None,
            name: "TestApp".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            orbitals: vec![],
            metadata: None,
            ledger: None,
            schema_version: None,
        }
    }

    #[test]
    fn test_validate_minimal_schema() {
        let schema = create_minimal_schema();
        let result = validate_schema(&schema);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_missing_name() {
        let mut schema = create_minimal_schema();
        schema.name = String::new();

        let result = validate_schema(&schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_modal_render_effect_detection() {
        // Test render-ui to modal with content
        let effect: SExpression = serde_json::from_str(
            r#"["render-ui", "modal", { "type": "form-section", "title": "Test" }]"#,
        )
        .unwrap();
        assert!(is_modal_render_effect(&effect));

        // Test render-ui to modal with null (clearing)
        let effect: SExpression = serde_json::from_str(r#"["render-ui", "modal", null]"#).unwrap();
        assert!(!is_modal_render_effect(&effect));

        // Test render-ui to main slot
        let effect: SExpression = serde_json::from_str(
            r#"["render-ui", "main", { "type": "page-header", "title": "Test" }]"#,
        )
        .unwrap();
        assert!(!is_modal_render_effect(&effect));

        // Test render-ui to drawer
        let effect: SExpression = serde_json::from_str(
            r#"["render-ui", "drawer", { "type": "detail-panel", "entity": "Task" }]"#,
        )
        .unwrap();
        assert!(is_modal_render_effect(&effect));
    }

    #[test]
    fn test_modal_clear_effect_detection() {
        // Test clearing modal
        let effect: SExpression = serde_json::from_str(r#"["render-ui", "modal", null]"#).unwrap();
        assert!(is_modal_clear_effect(&effect));

        // Test clearing drawer
        let effect: SExpression = serde_json::from_str(r#"["render-ui", "drawer", null]"#).unwrap();
        assert!(is_modal_clear_effect(&effect));

        // Test rendering to modal (not clearing)
        let effect: SExpression =
            serde_json::from_str(r#"["render-ui", "modal", { "type": "form-section" }]"#).unwrap();
        assert!(!is_modal_clear_effect(&effect));
    }

    fn create_state(name: &str, is_initial: bool) -> StateDefinition {
        StateDefinition {
            name: name.to_string(),
            is_initial: if is_initial { Some(true) } else { None },
            is_terminal: None,
            description: None,
            time_bound_ms: None,
        }
    }

    fn create_transition(
        from: &str,
        to: &str,
        event: &str,
        effects_json: Option<&str>,
    ) -> TransitionDefinition {
        TransitionDefinition {
            event_id: None,
            from: StringOrArray::Single(from.to_string()),
            to: to.to_string(),
            event: event.to_string(),
            guard: None,
            effects: effects_json.map(|json| vec![serde_json::from_str(json).unwrap()]),
        }
    }

    fn create_test_trait(
        name: &str,
        states: Vec<StateDefinition>,
        transitions: Vec<TransitionDefinition>,
    ) -> TraitDefinition {
        TraitDefinition {
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: None,
            emits: None,
            listens: None,
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            state_machine: StateMachine {
                states,
                events: None,
                transitions,
            },
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    #[test]
    fn test_validate_modal_with_close_transition() {
        // Create a trait with proper modal transitions
        let trait_def = create_test_trait(
            "TestTrait",
            vec![
                create_state("browsing", true),
                create_state("creating", false),
            ],
            vec![
                // Open modal
                create_transition(
                    "browsing",
                    "creating",
                    "CREATE",
                    Some(r#"["render-ui", "modal", { "type": "form-section" }]"#),
                ),
                // Close modal
                create_transition(
                    "creating",
                    "browsing",
                    "CLOSE",
                    Some(r#"["render-ui", "modal", null]"#),
                ),
            ],
        );

        let mut ctx = ValidationContext::new();
        validate_modal_transitions(&trait_def, "test.trait", &mut ctx);
        assert!(
            ctx.errors.is_empty(),
            "Expected no errors for proper modal transitions"
        );
    }

    #[test]
    fn test_validate_modal_without_close_transition() {
        // Create a trait with modal but no CLOSE transition
        let trait_def = create_test_trait(
            "TestTrait",
            vec![
                create_state("browsing", true),
                create_state("creating", false),
            ],
            vec![
                // Open modal but no CLOSE transition
                create_transition(
                    "browsing",
                    "creating",
                    "CREATE",
                    Some(r#"["render-ui", "modal", { "type": "form-section" }]"#),
                ),
            ],
        );

        let mut ctx = ValidationContext::new();
        validate_modal_transitions(&trait_def, "test.trait", &mut ctx);
        assert_eq!(
            ctx.errors.len(),
            1,
            "Expected error for missing CLOSE transition"
        );

        let error = &ctx.errors[0];
        match error {
            ValidationError::StateMachineError { message, .. } => {
                assert!(message.contains("creating"));
                assert!(message.contains("CLOSE"));
            }
            _ => panic!("Expected StateMachineError"),
        }
    }

    #[test]
    fn test_validate_modal_with_cancel_transition() {
        // CANCEL is also acceptable for closing modals
        let trait_def = create_test_trait(
            "TestTrait",
            vec![
                create_state("browsing", true),
                create_state("editing", false),
            ],
            vec![
                create_transition(
                    "browsing",
                    "editing",
                    "EDIT",
                    Some(r#"["render-ui", "modal", { "type": "form-section" }]"#),
                ),
                create_transition(
                    "editing",
                    "browsing",
                    "CANCEL",
                    Some(r#"["render-ui", "modal", null]"#),
                ),
            ],
        );

        let mut ctx = ValidationContext::new();
        validate_modal_transitions(&trait_def, "test.trait", &mut ctx);
        assert!(
            ctx.errors.is_empty(),
            "Expected no errors when CANCEL transition exists"
        );
    }
}
