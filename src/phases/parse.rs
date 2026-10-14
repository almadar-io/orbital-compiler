//! Parse Phase
//!
//! Reads schema files and deserializes them into OrbitalSchema types.

use orbital_core::schema::OrbitalSchema;
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Parse error
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("Failed to read file: {0}")]
    FileRead(#[from] std::io::Error),

    #[error("Failed to parse JSON: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Unsupported file format: {0}")]
    UnsupportedFormat(String),

    #[error("Failed to lower {0}: {1}")]
    SourceLowering(String, String),
}

/// Parse a schema file
///
/// Supports .json and .orb (JSON) files, and .lolo through the registered source lowerer.
pub fn parse_file(path: impl AsRef<Path>) -> Result<OrbitalSchema, ParseError> {
    let path = path.as_ref();
    let content = fs::read_to_string(path)?;

    // Determine format from extension
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("json");

    match ext {
        "json" | "orb" => parse_json(&content),
        "lolo" => crate::loader::lower_source(path)
            .map_err(|e| ParseError::SourceLowering(path.display().to_string(), e)),
        other => Err(ParseError::UnsupportedFormat(other.to_string())),
    }
}

/// Parse a JSON string into OrbitalSchema
pub fn parse_json(json: &str) -> Result<OrbitalSchema, ParseError> {
    let mut schema: OrbitalSchema = serde_json::from_str(json)?;
    orbital_core::stamp::ensure_stamped(&mut schema);
    Ok(schema)
}

/// Parse JSON with lenient options (allows trailing commas, comments)
pub fn parse_json_lenient(json: &str) -> Result<OrbitalSchema, ParseError> {
    // First try strict parsing
    if let Ok(schema) = parse_json(json) {
        return Ok(schema);
    }

    // Strip comments and trailing commas for lenient parsing
    let cleaned = strip_json_comments(json);
    parse_json(&cleaned)
}

/// Strip C-style comments from JSON
fn strip_json_comments(json: &str) -> String {
    let mut result = String::with_capacity(json.len());
    let mut chars = json.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        if c == '\\' && in_string {
            result.push(c);
            escape_next = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            result.push(c);
            continue;
        }

        if !in_string && c == '/' {
            if let Some(&next) = chars.peek() {
                if next == '/' {
                    // Line comment - skip until newline
                    chars.next();
                    while let Some(&c) = chars.peek() {
                        chars.next();
                        if c == '\n' {
                            result.push('\n');
                            break;
                        }
                    }
                    continue;
                } else if next == '*' {
                    // Block comment - skip until */
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '*' {
                            if let Some(&'/') = chars.peek() {
                                chars.next();
                                break;
                            }
                        }
                    }
                    continue;
                }
            }
        }

        result.push(c);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lolo_file_parses_through_the_registered_lowerer() {
        crate::loader::register_source_lowerer(orbital_test_support::lower_lolo_file);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("door.lolo");
        std::fs::write(
            &path,
            "app Door [version: \"1.0.0\"]\norbital DoorOrbital {\n  entity Door [runtime] {\n    id : string!\n  }\n  trait DoorLife -> Door [interaction] {\n    initial: closed\n    state closed {\n      INIT -> closed\n      OPEN -> open\n    }\n    state open {\n      CLOSE -> closed\n    }\n  }\n  page \"/door\" -> DoorLife\n}\n",
        )
        .unwrap();
        let schema = parse_file(&path).expect("a .lolo file parses");
        assert_eq!(schema.name, "Door");
        assert_eq!(schema.orbitals.len(), 1);
    }

    #[test]
    fn an_unknown_extension_is_still_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("door.txt");
        std::fs::write(&path, "{}").unwrap();
        assert!(matches!(
            parse_file(&path),
            Err(ParseError::UnsupportedFormat(ext)) if ext == "txt"
        ));
    }

    #[test]
    fn test_parse_minimal_schema() {
        let json = r#"{
            "name": "TestApp",
            "version": "1.0.0",
            "orbitals": []
        }"#;

        let schema = parse_json(json).unwrap();
        assert_eq!(schema.name, "TestApp");
        assert_eq!(schema.version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_parse_with_orbital() {
        let json = r#"{
            "name": "TestApp",
            "version": "1.0.0",
            "orbitals": [
                {
                    "name": "Task",
                    "entity": {
                        "name": "Task",
                        "fields": [
                            {
                                "name": "title",
                                "type": "string",
                                "required": true
                            }
                        ]
                    },
                    "traits": [
                        {
                            "name": "TaskLifecycle",
                            "stateMachine": {
                                "states": [
                                    { "name": "idle", "isInitial": true },
                                    { "name": "active" },
                                    { "name": "done" }
                                ],
                                "transitions": [
                                    {
                                        "from": "idle",
                                        "to": "active",
                                        "event": "START"
                                    }
                                ]
                            }
                        }
                    ],
                    "pages": []
                }
            ]
        }"#;

        let schema = parse_json(json).unwrap();
        assert_eq!(schema.orbitals.len(), 1);
        assert_eq!(schema.orbitals[0].name, "Task");
    }

    #[test]
    fn test_strip_comments() {
        let json_with_comments = r#"{
            // This is a line comment
            "name": "TestApp",
            /* This is a
               block comment */
            "version": "1.0.0",
            "orbitals": []
        }"#;

        let cleaned = strip_json_comments(json_with_comments);
        let schema = parse_json(&cleaned).unwrap();
        assert_eq!(schema.name, "TestApp");
    }

    #[test]
    fn test_parse_invalid_json() {
        let json = r#"{ invalid json }"#;
        let result = parse_json(json);
        assert!(result.is_err());
    }
}
