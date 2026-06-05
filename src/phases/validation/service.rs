//! Service Validation
//!
//! Validates service definitions at the orbital level: REST, Socket, MCP.
//! Services can be inline definitions or references to imported services.

use super::error::{ErrorCode, ValidationError, ValidationResult};
use super::ValidationContext;
use orbital_core::schema::{OrbitalSchema, ServiceDefinition, ServiceRef};
use regex::Regex;
use std::sync::LazyLock;

static SERVICE_REF_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*\.services\.[a-zA-Z][a-zA-Z0-9]*$").unwrap());

/// Validate all services in a schema
pub fn validate_all_services(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if let Some(services) = &orbital.services {
            for (j, service_ref) in services.iter().enumerate() {
                let path = format!("orbitals[{}].services[{}]", i, j);
                let result = validate_service_ref(service_ref, &path);
                ctx.merge(result);

                // Track service names for duplicate detection
                if let ServiceRef::Inline(service_def) = service_ref {
                    let name = service_def.name();
                    if ctx.service_names.contains(name) {
                        ctx.add_error(ValidationError::error(
                            ErrorCode::DuplicateEntity,
                            &path,
                            format!("Duplicate service name '{}'", name),
                        ));
                    } else {
                        ctx.service_names.insert(name.to_string());
                    }
                }
            }
        }
    }
}

/// Validate a service reference (inline or reference string)
pub fn validate_service_ref(service_ref: &ServiceRef, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    match service_ref {
        ServiceRef::Inline(service_def) => {
            let service_result = validate_service_definition(service_def, path);
            result.merge(service_result);
        }
        ServiceRef::Reference(ref_str) => {
            // Validate reference format: "Alias.services.ServiceName"
            if !is_valid_service_reference(ref_str) {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidReference,
                    path,
                    format!(
                        "Invalid service reference '{}'. Must be in format 'Alias.services.ServiceName'",
                        ref_str
                    ),
                ));
            }
        }
        ServiceRef::Object(obj) => {
            // Validate the inner reference field. Override fields are
            // free-form (any of base_url / url / server_path / headers /
            // description) and only get applied at inline time, so there's
            // nothing else to validate here. Mirrors how PageRef::Object
            // is treated by `validate_page_ref` (the path override is not
            // validated against any schema).
            if !is_valid_service_reference(&obj.reference) {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidReference,
                    path,
                    format!(
                        "Invalid service reference '{}'. Must be in format 'Alias.services.ServiceName'",
                        obj.reference
                    ),
                ));
            }
        }
    }

    result
}

/// Validate an inline service definition
pub fn validate_service_definition(service: &ServiceDefinition, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    match service {
        ServiceDefinition::Rest(rest) => {
            // Validate name
            if rest.name.is_empty() {
                result.add_error(ValidationError::error(
                    ErrorCode::EntityMissingName,
                    format!("{}.name", path),
                    "Service name is required",
                ));
            }

            // Validate base URL
            if rest.base_url.is_empty() {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidValue,
                    format!("{}.baseUrl", path),
                    "REST service requires a baseUrl",
                ));
            } else if !is_valid_url(&rest.base_url) {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidValue,
                    format!("{}.baseUrl", path),
                    format!("Invalid URL: '{}'", rest.base_url),
                ));
            }

            // Validate auth config
            if let Some(auth) = &rest.auth {
                // For api-key auth, keyName should be provided
                if auth.auth_type == orbital_core::schema::RestAuthType::ApiKey
                    && auth.key_name.is_none() {
                        result.add_error(ValidationError::warning(
                            ErrorCode::MissingField,
                            format!("{}.auth.keyName", path),
                            "api-key auth should specify keyName",
                        ));
                    }

                // secretEnv should follow env var naming convention
                if let Some(secret_env) = &auth.secret_env {
                    if !is_valid_env_var_name(secret_env) {
                        result.add_error(ValidationError::warning(
                            ErrorCode::InvalidValue,
                            format!("{}.auth.secretEnv", path),
                            format!("secretEnv '{}' should be UPPER_SNAKE_CASE", secret_env),
                        ));
                    }
                }
            }

            // Validate timeout
            if let Some(timeout) = rest.timeout {
                if timeout == 0 {
                    result.add_error(ValidationError::warning(
                        ErrorCode::InvalidValue,
                        format!("{}.timeout", path),
                        "Timeout of 0 may cause immediate failures",
                    ));
                }
            }
        }
        ServiceDefinition::Socket(socket) => {
            // Validate name
            if socket.name.is_empty() {
                result.add_error(ValidationError::error(
                    ErrorCode::EntityMissingName,
                    format!("{}.name", path),
                    "Service name is required",
                ));
            }

            // Validate WebSocket URL
            if socket.url.is_empty() {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidValue,
                    format!("{}.url", path),
                    "Socket service requires a url",
                ));
            } else if !socket.url.starts_with("ws://") && !socket.url.starts_with("wss://") {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidValue,
                    format!("{}.url", path),
                    format!(
                        "Socket URL must start with ws:// or wss://, got: '{}'",
                        socket.url
                    ),
                ));
            }

            // Validate events
            if socket.events.inbound.is_empty() && socket.events.outbound.is_empty() {
                result.add_error(ValidationError::warning(
                    ErrorCode::MissingField,
                    format!("{}.events", path),
                    "Socket service has no inbound or outbound events defined",
                ));
            }
        }
        ServiceDefinition::Mcp(mcp) => {
            // Validate name
            if mcp.name.is_empty() {
                result.add_error(ValidationError::error(
                    ErrorCode::EntityMissingName,
                    format!("{}.name", path),
                    "Service name is required",
                ));
            }

            // Validate server path
            if mcp.server_path.is_empty() {
                result.add_error(ValidationError::error(
                    ErrorCode::InvalidValue,
                    format!("{}.serverPath", path),
                    "MCP service requires a serverPath",
                ));
            }

            // Validate capabilities
            if mcp.capabilities.is_empty() {
                result.add_error(ValidationError::warning(
                    ErrorCode::MissingField,
                    format!("{}.capabilities", path),
                    "MCP service has no capabilities defined",
                ));
            }
        }
    }

    result
}

/// Check if a service reference is valid format: "Alias.services.ServiceName"
fn is_valid_service_reference(ref_str: &str) -> bool {
    SERVICE_REF_REGEX.is_match(ref_str)
}

/// Check if a URL is valid (basic check)
fn is_valid_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("${")
}

/// Check if an environment variable name follows UPPER_SNAKE_CASE convention
fn is_valid_env_var_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    // Must start with letter or underscore
    let first_char = name.chars().next().unwrap();
    if !first_char.is_ascii_uppercase() && first_char != '_' {
        return false;
    }

    // Rest must be uppercase letters, digits, or underscores
    name.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core::schema::{RestAuthConfig, RestAuthType, RestServiceDefInner};

    #[test]
    fn test_valid_service_reference() {
        assert!(is_valid_service_reference("Weather.services.openweather"));
        assert!(is_valid_service_reference("API.services.myService"));
        assert!(!is_valid_service_reference("weather.services.openweather")); // must start with uppercase
        assert!(!is_valid_service_reference("Weather.service.openweather")); // must be .services
        assert!(!is_valid_service_reference("Weather.openweather")); // missing .services
    }

    #[test]
    fn test_valid_url() {
        assert!(is_valid_url("https://api.example.com"));
        assert!(is_valid_url("http://localhost:3000"));
        assert!(is_valid_url("${API_BASE_URL}")); // env var interpolation
        assert!(!is_valid_url("api.example.com"));
        assert!(!is_valid_url("ftp://example.com"));
    }

    #[test]
    fn test_valid_env_var_name() {
        assert!(is_valid_env_var_name("API_KEY"));
        assert!(is_valid_env_var_name("OPENWEATHER_API_KEY"));
        assert!(is_valid_env_var_name("_PRIVATE_KEY"));
        assert!(!is_valid_env_var_name("api_key")); // lowercase
        assert!(!is_valid_env_var_name("apiKey")); // camelCase
        assert!(!is_valid_env_var_name("")); // empty
    }

    #[test]
    fn test_validate_rest_service() {
        let service = ServiceDefinition::Rest(RestServiceDefInner {
            name: "openweather".to_string(),
            description: None,
            base_url: "https://api.openweathermap.org/data/2.5".to_string(),
            headers: None,
            auth: Some(RestAuthConfig {
                auth_type: RestAuthType::ApiKey,
                key_name: Some("appid".to_string()),
                location: None,
                secret_env: Some("OPENWEATHER_API_KEY".to_string()),
            }),
            timeout: None,
        });

        let result = validate_service_definition(&service, "test");
        assert!(result.is_valid());
    }

    #[test]
    fn test_validate_rest_service_missing_name() {
        let service = ServiceDefinition::Rest(RestServiceDefInner {
            name: "".to_string(),
            description: None,
            base_url: "https://api.example.com".to_string(),
            headers: None,
            auth: None,
            timeout: None,
        });

        let result = validate_service_definition(&service, "test");
        assert!(!result.is_valid());
    }

    #[test]
    fn test_validate_rest_service_invalid_url() {
        let service = ServiceDefinition::Rest(RestServiceDefInner {
            name: "test".to_string(),
            description: None,
            base_url: "not-a-url".to_string(),
            headers: None,
            auth: None,
            timeout: None,
        });

        let result = validate_service_definition(&service, "test");
        assert!(!result.is_valid());
    }
}
