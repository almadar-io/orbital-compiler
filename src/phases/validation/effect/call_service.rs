//! call_service effect validation.

use super::super::{ErrorCode, ValidationError, ValidationResult};
use super::{extract_emit_object_ref, validate_emit_config, DeclaredEmit};
use orbital_core::schema::SExpression;
use std::collections::HashSet;

/// Validate call_service effect (array form)
pub(crate) fn validate_call_service(
    arr: &[SExpression],
    path: &str,
    known_services: &HashSet<String>,
    service_actions: &std::collections::HashMap<String, HashSet<String>>,
    declared_emits: Option<&[DeclaredEmit]>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Expected: ["call-service", "serviceName", "action", { onSuccess: [...], onError: [...] }]
    if arr.len() < 3 {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectCallServiceMissingService,
                path,
                "call_service requires service name and action",
            )
            .with_suggestion(
                "Use: [\"call-service\", \"serviceName\", \"action\", { onSuccess: [...] }]",
            ),
        );
        return result;
    }

    // Check service name
    if let SExpression::String(service_name) = &arr[1] {
        if !known_services.is_empty() && !known_services.contains(service_name) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::EffectCallServiceMissingService,
                    path,
                    format!("Service '{}' is not defined", service_name),
                )
                .with_suggestion("Define the service in schema.services")
                .with_valid_values(known_services.iter().cloned().collect()),
            );
        } else if known_services.is_empty()
            && !service_actions.is_empty()
            && !service_actions.contains_key(service_name)
        {
            // No inline schema.services (every .lolo lowers to services: None),
            // so gate against the services registry. Staged as a WARNING until
            // lolo grows a service-declaration form.
            result.add_error(
                ValidationError::phase_f_warning(
                    ErrorCode::EffectCallServiceUnknownService,
                    path,
                    format!(
                        "Service '{}' is not a registered integrator or declared service",
                        service_name
                    ),
                )
                .with_suggestion(
                    "Register the integrator (almadar-sync services) or add it to DECLARED_SERVICES, or check the name for a typo",
                )
                .with_valid_values(service_actions.keys().cloned().collect()),
            );
        }

        // Validate action against services registry (if loaded). A declared-only
        // service carries an EMPTY action set: name known, actions unchecked.
        if let Some(SExpression::String(action)) = arr.get(2) {
            if let Some(valid_actions) = service_actions.get(service_name) {
                if !valid_actions.is_empty() && !valid_actions.contains(action) {
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::EffectCallServiceMissingAction,
                            path,
                            format!(
                                "Action '{}' is not a known action on service '{}'",
                                action, service_name
                            ),
                        )
                        .with_suggestion("Check the service contract for valid action names")
                        .with_valid_values(valid_actions.iter().cloned().collect()),
                    );
                }
            }
        }
    } else {
        result.add_error(ValidationError::error(
            ErrorCode::EffectCallServiceMissingService,
            path,
            "Service name must be a string",
        ));
    }

    // Check action
    if let Some(SExpression::String(_action)) = arr.get(2) {
        // Action is present and is a string - valid
    } else if arr.len() >= 3 {
        result.add_error(ValidationError::error(
            ErrorCode::EffectCallServiceMissingAction,
            path,
            "Action must be a string",
        ));
    }

    // Optional trailing options carrying `emit:` (call-service is async one-shot).
    if arr.len() > 3 {
        if let SExpression::Object(options) = &arr[arr.len() - 1] {
            if let Some(emit_obj) = extract_emit_object_ref(options) {
                result.merge(validate_emit_config(
                    emit_obj,
                    &["success", "failure"],
                    declared_emits,
                    path,
                ));
            }
        }
    }

    result
}

/// Validate call_service effect (object form)
pub(crate) fn validate_call_service_object(
    obj: &std::collections::HashMap<String, SExpression>,
    path: &str,
    known_services: &HashSet<String>,
    service_actions: &std::collections::HashMap<String, HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Get the call_service value
    let call_service = obj.get("call_service").or_else(|| obj.get("call-service"));

    if let Some(SExpression::Object(cs_obj)) = call_service {
        // Check service
        if let Some(SExpression::String(service)) = cs_obj.get("service") {
            if !known_services.is_empty() && !known_services.contains(service) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::EffectCallServiceMissingService,
                        path,
                        format!("Service '{}' is not defined", service),
                    )
                    .with_valid_values(known_services.iter().cloned().collect()),
                );
            } else if known_services.is_empty()
                && !service_actions.is_empty()
                && !service_actions.contains_key(service)
            {
                // Same registry gate as the array form; staged as a WARNING.
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::EffectCallServiceUnknownService,
                        path,
                        format!(
                            "Service '{}' is not a registered integrator or declared service",
                            service
                        ),
                    )
                    .with_suggestion(
                        "Register the integrator (almadar-sync services) or add it to DECLARED_SERVICES, or check the name for a typo",
                    )
                    .with_valid_values(service_actions.keys().cloned().collect()),
                );
            }

            // Validate action against services registry (if loaded). A declared-only
            // service carries an EMPTY action set: name known, actions unchecked.
            if let Some(SExpression::String(action)) = cs_obj.get("action") {
                if let Some(valid_actions) = service_actions.get(service) {
                    if !valid_actions.is_empty() && !valid_actions.contains(action) {
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::EffectCallServiceMissingAction,
                                path,
                                format!(
                                    "Action '{}' is not a known action on service '{}'",
                                    action, service
                                ),
                            )
                            .with_suggestion("Check the service contract for valid action names")
                            .with_valid_values(valid_actions.iter().cloned().collect()),
                        );
                    }
                }
            }
        } else {
            result.add_error(ValidationError::error(
                ErrorCode::EffectCallServiceMissingService,
                path,
                "call_service requires 'service' property",
            ));
        }

        // Check action
        if !cs_obj.contains_key("action") {
            result.add_error(ValidationError::error(
                ErrorCode::EffectCallServiceMissingAction,
                path,
                "call_service requires 'action' property",
            ));
        }
    }

    result
}
