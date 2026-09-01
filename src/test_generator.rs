//! Test Generator Framework
//!
//! Generates tests alongside application code for each backend.

use orbital_core::ir::{OirEffect, OirEntity, OirModule, OirPage, OirTrait};
use orbital_core::str_utils::to_snake_case;
use std::path::PathBuf;

/// Generated file
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    /// Relative path within the generated directory
    pub path: PathBuf,
    /// File content
    pub content: String,
    /// File type for organization
    pub file_type: GeneratedFileType,
}

impl GeneratedFile {
    /// Create a new generated file
    pub fn new(
        path: impl Into<PathBuf>,
        content: impl Into<String>,
        file_type: GeneratedFileType,
    ) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            file_type,
        }
    }
}

/// Type of generated file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedFileType {
    Entity,
    Trait,
    Page,
    Route,
    Test,
    Barrel,
    Config,
}

/// Test generator trait
///
/// Each backend implements this trait to generate tests in its native test framework.
pub trait TestGenerator: Send + Sync {
    /// Generator name (e.g., "vitest", "pytest", "cargo-test")
    fn name(&self) -> &'static str;

    /// File extension for test files
    fn test_extension(&self) -> &'static str;

    /// Generate entity CRUD tests
    fn generate_entity_tests(&self, entity: &OirEntity) -> Vec<GeneratedFile>;

    /// Generate state machine transition tests
    fn generate_trait_tests(&self, trait_def: &OirTrait) -> Vec<GeneratedFile>;

    /// Generate effect execution tests
    fn generate_effect_tests(&self, effects: &[OirEffect]) -> Vec<GeneratedFile>;

    /// Generate page/UI tests
    fn generate_page_tests(&self, page: &OirPage) -> Vec<GeneratedFile>;

    /// Generate integration tests
    fn generate_integration_tests(&self, module: &OirModule) -> Vec<GeneratedFile>;

    /// Generate all tests for a module
    fn generate_all_tests(&self, module: &OirModule) -> Vec<GeneratedFile> {
        let mut files = vec![];

        // Entity tests
        for entity in &module.entities {
            files.extend(self.generate_entity_tests(entity));
        }

        // Trait tests
        for trait_def in &module.traits {
            files.extend(self.generate_trait_tests(trait_def));
        }

        // Page tests
        for page in &module.pages {
            files.extend(self.generate_page_tests(page));
        }

        // Integration tests
        files.extend(self.generate_integration_tests(module));

        files
    }
}

/// TypeScript/Vitest test generator
pub struct VitestGenerator;

impl TestGenerator for VitestGenerator {
    fn name(&self) -> &'static str {
        "vitest"
    }

    fn test_extension(&self) -> &'static str {
        "test.ts"
    }

    fn generate_entity_tests(&self, entity: &OirEntity) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"import {{ describe, it, expect }} from 'vitest';
import {{ {name} }} from '../entities/{name_lower}';

describe('{name}', () => {{
    it('should create a new {name}', () => {{
        const entity = new {name}({{
            id: 'test-id',
{field_defaults}
        }});
        expect(entity.id).toBe('test-id');
    }});

    it('should validate required fields', () => {{
        expect(() => new {name}({{}})).toThrow();
    }});
{field_tests}
}});
"#,
            name = entity.name,
            name_lower = entity.name.to_lowercase(),
            field_defaults = entity
                .fields
                .iter()
                .filter(|f| f.required && f.name != "id")
                .map(|f| format!(
                    "            {}: {},",
                    f.name,
                    default_value_for_type(&f.field_type)
                ))
                .collect::<Vec<_>>()
                .join("\n"),
            field_tests = entity
                .fields
                .iter()
                .filter(|f| f.name != "id")
                .map(|f| format!(
                    r#"
    it('should have {} field', () => {{
        const entity = new {}({{ id: 'test' }});
        expect(entity).toHaveProperty('{}');
    }});"#,
                    f.name, entity.name, f.name
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/entities/{}.test.ts", entity.name.to_lowercase()),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_trait_tests(&self, trait_def: &OirTrait) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"import {{ describe, it, expect }} from 'vitest';
import {{ use{name} }} from '../traits/{name_lower}';

describe('{name}', () => {{
    it('should start in initial state', () => {{
        const {{ state }} = use{name}();
        expect(state).toBe('{initial_state}');
    }});
{transition_tests}
}});
"#,
            name = trait_def.name,
            name_lower = trait_def.name.to_lowercase(),
            initial_state = trait_def
                .initial_state()
                .map(|s| s.name.as_str())
                .unwrap_or("unknown"),
            transition_tests = trait_def
                .transitions
                .iter()
                .map(|t| format!(
                    r#"
    it('should transition from {} to {} on {}', async () => {{
        const {{ state, sendEvent }} = use{name}();
        await sendEvent('{}');
        // Verify transition occurred
    }});"#,
                    match &t.from {
                        orbital_core::ir::OirTransitionFrom::Single(s) => s.clone(),
                        orbital_core::ir::OirTransitionFrom::Multiple(states) => states.join("|"),
                        orbital_core::ir::OirTransitionFrom::Any => "*".to_string(),
                    },
                    t.to,
                    t.event,
                    t.event,
                    name = trait_def.name
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/traits/{}.test.ts", trait_def.name.to_lowercase()),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_effect_tests(&self, _effects: &[OirEffect]) -> Vec<GeneratedFile> {
        // Effect tests are typically part of trait tests
        vec![]
    }

    fn generate_page_tests(&self, page: &OirPage) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"import {{ describe, it, expect }} from 'vitest';
import {{ render, screen }} from '@testing-library/react';
import {{ {name}Page }} from '../pages/{name_lower}';

describe('{name}Page', () => {{
    it('should render without crashing', () => {{
        render(<{name}Page />);
    }});

    it('should match snapshot', () => {{
        const {{ container }} = render(<{name}Page />);
        expect(container).toMatchSnapshot();
    }});
{section_tests}
}});
"#,
            name = page.name,
            name_lower = page.name.to_lowercase(),
            section_tests = page
                .sections
                .iter()
                .map(|s| format!(
                    r#"
    it('should render {} section', () => {{
        render(<{name}Page />);
        // Verify section is rendered
    }});"#,
                    s.id,
                    name = page.name
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/pages/{}.test.tsx", page.name.to_lowercase()),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_integration_tests(&self, module: &OirModule) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"import {{ describe, it, expect }} from 'vitest';
import {{ createTestApp }} from '../test-utils';

describe('{name} Integration', () => {{
    it('should initialize the application', async () => {{
        const app = await createTestApp();
        expect(app).toBeDefined();
    }});

    it('should handle complete workflows', async () => {{
        const app = await createTestApp();
        // Test complete user workflows
    }});
}});
"#,
            name = module.name
        );

        vec![GeneratedFile::new(
            "tests/integration.test.ts",
            test_content,
            GeneratedFileType::Test,
        )]
    }
}

/// Rust/cargo test generator
pub struct CargoTestGenerator;

impl TestGenerator for CargoTestGenerator {
    fn name(&self) -> &'static str {
        "cargo-test"
    }

    fn test_extension(&self) -> &'static str {
        "rs"
    }

    fn generate_entity_tests(&self, entity: &OirEntity) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"//! Tests for {name} entity

use super::*;

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn test_create_{name_lower}() {{
        let entity = {name} {{
            id: "test-id".to_string(),
{field_defaults}
        }};
        assert_eq!(entity.id, "test-id");
    }}

    #[test]
    fn test_{name_lower}_serialization() {{
        let entity = {name}::default();
        let json = serde_json::to_string(&entity).unwrap();
        let deserialized: {name} = serde_json::from_str(&json).unwrap();
        assert_eq!(entity.id, deserialized.id);
    }}
{field_tests}
}}
"#,
            name = entity.name,
            name_lower = to_snake_case(&entity.name),
            field_defaults = entity
                .fields
                .iter()
                .filter(|f| f.name != "id")
                .map(|f| format!(
                    "            {}: {},",
                    to_snake_case(&f.name),
                    rust_default_value(&f.field_type)
                ))
                .collect::<Vec<_>>()
                .join("\n"),
            field_tests = entity
                .fields
                .iter()
                .filter(|f| f.name != "id")
                .map(|f| format!(
                    r#"
    #[test]
    fn test_{name_lower}_has_{field}() {{
        let entity = {name}::default();
        // Field {field} should exist
        let _ = entity.{field};
    }}"#,
                    name_lower = to_snake_case(&entity.name),
                    field = to_snake_case(&f.name),
                    name = entity.name
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/entities/{}_test.rs", to_snake_case(&entity.name)),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_trait_tests(&self, trait_def: &OirTrait) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"//! Tests for {name} trait/state machine

use super::*;

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn test_{name_lower}_initial_state() {{
        let sm = {name}::new();
        assert_eq!(sm.state(), {name}State::{initial_state});
    }}
{transition_tests}
}}
"#,
            name = trait_def.name,
            name_lower = to_snake_case(&trait_def.name),
            initial_state = trait_def
                .initial_state()
                .map(|s| orbital_core::str_utils::split_to_pascal_case(&s.name))
                .unwrap_or_else(|| "Unknown".to_string()),
            transition_tests = trait_def
                .transitions
                .iter()
                .enumerate()
                .map(|(i, t)| format!(
                    r#"
    #[test]
    fn test_{name_lower}_transition_{idx}() {{
        let mut sm = {name}::new();
        // Test transition: {from} -> {to} on {on_event}
        sm.handle_{event}(&mut entity);
        // Verify state changed
    }}"#,
                    name_lower = to_snake_case(&trait_def.name),
                    idx = i,
                    name = trait_def.name,
                    from = t.from,
                    to = t.to,
                    on_event = t.event,
                    event = to_snake_case(&t.event)
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/traits/{}_test.rs", to_snake_case(&trait_def.name)),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_effect_tests(&self, _effects: &[OirEffect]) -> Vec<GeneratedFile> {
        vec![]
    }

    fn generate_page_tests(&self, page: &OirPage) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"//! Tests for {name} page

use super::*;

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn test_{name_lower}_page_renders() {{
        // Page rendering test
    }}
{section_tests}
}}
"#,
            name = page.name,
            name_lower = to_snake_case(&page.name),
            section_tests = page
                .sections
                .iter()
                .map(|s| format!(
                    r#"
    #[test]
    fn test_{name_lower}_section_{section}() {{
        // Section {section_id} test
    }}"#,
                    name_lower = to_snake_case(&page.name),
                    section = to_snake_case(&s.id),
                    section_id = s.id
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/pages/{}_test.rs", to_snake_case(&page.name)),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_integration_tests(&self, module: &OirModule) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"//! Integration tests for {name}

use super::*;

#[cfg(test)]
mod integration_tests {{
    use super::*;

    #[tokio::test]
    async fn test_app_initialization() {{
        // Test app initialization
    }}

    #[tokio::test]
    async fn test_complete_workflow() {{
        // Test complete user workflow
    }}
}}
"#,
            name = module.name
        );

        vec![GeneratedFile::new(
            "tests/integration_test.rs",
            test_content,
            GeneratedFileType::Test,
        )]
    }
}

/// Python/pytest test generator
pub struct PytestGenerator;

impl TestGenerator for PytestGenerator {
    fn name(&self) -> &'static str {
        "pytest"
    }

    fn test_extension(&self) -> &'static str {
        "py"
    }

    fn generate_entity_tests(&self, entity: &OirEntity) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"""Tests for {name} entity."""

import pytest
from ..entities.{name_lower} import {name}


class Test{name}:
    """Tests for {name} entity."""

    def test_create_{name_lower}(self):
        """Test creating a new {name}."""
        entity = {name}(
            id="test-id",
{field_defaults}
        )
        assert entity.id == "test-id"

    def test_{name_lower}_validation(self):
        """Test {name} validation."""
        with pytest.raises(ValueError):
            {name}()  # Missing required fields
{field_tests}
"#,
            name = entity.name,
            name_lower = to_snake_case(&entity.name),
            field_defaults = entity
                .fields
                .iter()
                .filter(|f| f.required && f.name != "id")
                .map(|f| format!(
                    "            {}={},",
                    to_snake_case(&f.name),
                    python_default_value(&f.field_type)
                ))
                .collect::<Vec<_>>()
                .join("\n"),
            field_tests = entity
                .fields
                .iter()
                .filter(|f| f.name != "id")
                .map(|f| format!(
                    r#"
    def test_{name_lower}_has_{field}(self):
        """Test {name} has {field} field."""
        entity = {name}(id="test")
        assert hasattr(entity, "{field}")"#,
                    name_lower = to_snake_case(&entity.name),
                    field = to_snake_case(&f.name),
                    name = entity.name
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/entities/test_{}.py", to_snake_case(&entity.name)),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_trait_tests(&self, trait_def: &OirTrait) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"""Tests for {name} trait/state machine."""

import pytest
from ..traits.{name_lower} import {name}, {name}State


class Test{name}:
    """Tests for {name} state machine."""

    def test_initial_state(self):
        """Test initial state."""
        sm = {name}()
        assert sm.state == {name}State.{initial_state}
{transition_tests}
"#,
            name = trait_def.name,
            name_lower = to_snake_case(&trait_def.name),
            initial_state = trait_def
                .initial_state()
                .map(|s| s.name.to_uppercase())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            transition_tests = trait_def
                .transitions
                .iter()
                .map(|t| format!(
                    r#"
    def test_transition_{event}(self):
        """Test transition on {event}."""
        sm = {name}()
        sm.handle_{event_lower}()
        # Verify state changed to {to}"#,
                    name = trait_def.name,
                    event = t.event,
                    event_lower = to_snake_case(&t.event),
                    to = t.to
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/traits/test_{}.py", to_snake_case(&trait_def.name)),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_effect_tests(&self, _effects: &[OirEffect]) -> Vec<GeneratedFile> {
        vec![]
    }

    fn generate_page_tests(&self, page: &OirPage) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"""Tests for {name} page."""

import pytest
from ..pages.{name_lower} import {name}Page


class Test{name}Page:
    """Tests for {name} page."""

    def test_page_renders(self):
        """Test page renders without errors."""
        page = {name}Page()
        # Verify page renders
{section_tests}
"#,
            name = page.name,
            name_lower = to_snake_case(&page.name),
            section_tests = page
                .sections
                .iter()
                .map(|s| format!(
                    r#"
    def test_section_{section}(self):
        """Test {section} section."""
        page = {name}Page()
        # Verify section renders"#,
                    name = page.name,
                    section = to_snake_case(&s.id)
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        vec![GeneratedFile::new(
            format!("tests/pages/test_{}.py", to_snake_case(&page.name)),
            test_content,
            GeneratedFileType::Test,
        )]
    }

    fn generate_integration_tests(&self, module: &OirModule) -> Vec<GeneratedFile> {
        let test_content = format!(
            r#"""Integration tests for {name}."""

import pytest
from ..main import create_app


class TestIntegration:
    """Integration tests."""

    @pytest.mark.asyncio
    async def test_app_initialization(self):
        """Test app initialization."""
        app = create_app()
        assert app is not None

    @pytest.mark.asyncio
    async def test_complete_workflow(self):
        """Test complete user workflow."""
        app = create_app()
        # Test workflow
"#,
            name = module.name
        );

        vec![GeneratedFile::new(
            "tests/test_integration.py",
            test_content,
            GeneratedFileType::Test,
        )]
    }
}

// Helper functions

fn default_value_for_type(ty: &orbital_core::ir::OirType) -> &'static str {
    use orbital_core::ir::OirType;
    match ty {
        OirType::String => "'test'",
        OirType::Number => "0",
        OirType::Integer => "0",
        OirType::Boolean => "false",
        OirType::Date | OirType::DateTime => "new Date()",
        OirType::Email => "'test@example.com'",
        OirType::Url | OirType::Image => "'https://example.com/test'",
        OirType::Phone => "'+1-555-0100'",
        OirType::Uuid => "'00000000-0000-4000-8000-000000000001'",
        OirType::Money => "0",
        OirType::File => "{ name: 'test.pdf', url: 'https://example.com/test.pdf', mimeType: 'application/pdf', sizeBytes: 1024 }",
        OirType::Json | OirType::Opaque => "{}",
        // TS renders `scalar` as the real union, so a string member is valid.
        OirType::Scalar => "'test'",
        OirType::Union { .. } => "{}",
        OirType::Array { .. } => "[]",
        OirType::Map { .. } => "{}",
        OirType::Optional { .. } => "null",
        OirType::Reference { .. } => "'ref-id'",
        OirType::Struct { .. } => "{}",
        OirType::Enum { .. } => "'value'",
        OirType::Trait => "'@trait.Mock'",
        OirType::SExpr => "'noop'",
        OirType::Node => "null",
    }
}

fn rust_default_value(ty: &orbital_core::ir::OirType) -> &'static str {
    use orbital_core::ir::OirType;
    match ty {
        OirType::String => "String::new()",
        OirType::Number => "0.0",
        OirType::Integer => "0",
        OirType::Boolean => "false",
        OirType::Date | OirType::DateTime => "Default::default()",
        OirType::Email => "\"test@example.com\".to_string()",
        OirType::Url | OirType::Image => "\"https://example.com/test\".to_string()",
        OirType::Phone => "\"+1-555-0100\".to_string()",
        OirType::Uuid => "\"00000000-0000-4000-8000-000000000001\".to_string()",
        OirType::Money => "0.0",
        OirType::File => "serde_json::Value::Null",
        OirType::Json | OirType::Opaque => "serde_json::Value::Null",
        // Rust falls back to the json() default (serde_json::Value); Null is a
        // legal member of the scalar union AND a legal Value.
        OirType::Scalar => "serde_json::Value::Null",
        OirType::Union { .. } => "serde_json::Value::Null",
        OirType::Array { .. } => "vec![]",
        OirType::Map { .. } => "Default::default()",
        OirType::Optional { .. } => "None",
        OirType::Reference { .. } => "String::new()",
        OirType::Struct { .. } => "Default::default()",
        OirType::Enum { .. } => "Default::default()",
        OirType::Trait => "String::from(\"@trait.Mock\")",
        OirType::SExpr => "String::from(\"noop\")",
        OirType::Node => "serde_json::Value::Null",
    }
}

fn python_default_value(ty: &orbital_core::ir::OirType) -> &'static str {
    use orbital_core::ir::OirType;
    match ty {
        OirType::String => "\"test\"",
        OirType::Number | OirType::Integer => "0",
        OirType::Boolean => "False",
        OirType::Date | OirType::DateTime => "datetime.now()",
        OirType::Email => "\"test@example.com\"",
        OirType::Url | OirType::Image => "\"https://example.com/test\"",
        OirType::Phone => "\"+1-555-0100\"",
        OirType::Uuid => "\"00000000-0000-4000-8000-000000000001\"",
        OirType::Money => "0",
        OirType::File => "{}",
        OirType::Json | OirType::Opaque => "{}",
        // Python falls back to the json() default; a string member is a valid
        // scalar and is what the seeder samples.
        OirType::Scalar => "\"test\"",
        OirType::Union { .. } => "{}",
        OirType::Array { .. } => "[]",
        OirType::Map { .. } => "{}",
        OirType::Optional { .. } => "None",
        OirType::Reference { .. } => "\"ref-id\"",
        OirType::Struct { .. } => "{}",
        OirType::Enum { .. } => "\"value\"",
        OirType::Trait => "\"@trait.Mock\"",
        OirType::SExpr => "\"noop\"",
        OirType::Node => "None",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::ir::{OirEntity, OirField, OirState, OirTrait, OirTransition, OirType};

    #[test]
    fn test_vitest_entity_tests() {
        let mut entity = OirEntity::new("Task");
        entity.fields.push(OirField::new("title", OirType::String));
        entity
            .fields
            .push(OirField::new("completed", OirType::Boolean));

        let generator = VitestGenerator;
        let files = generator.generate_entity_tests(&entity);

        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("describe('Task'"));
        assert!(files[0].content.contains("title"));
    }

    #[test]
    fn test_cargo_trait_tests() {
        let mut trait_def = OirTrait::new("TaskLifecycle");
        trait_def.states.push(OirState::initial("idle"));
        trait_def.states.push(OirState::new("active"));
        trait_def
            .transitions
            .push(OirTransition::new("idle", "active", "START"));

        let generator = CargoTestGenerator;
        let files = generator.generate_trait_tests(&trait_def);

        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("TaskLifecycle"));
        assert!(files[0]
            .content
            .contains("test_task_lifecycle_initial_state"));
    }
}
