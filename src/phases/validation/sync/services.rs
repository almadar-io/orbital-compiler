//! Service Definitions - Synced from TypeScript
//!
//! Source: builder/packages/shared/src/orbitals/types/service.ts
//!
//! Last synced: 2025-01-25
//! To regenerate: npm run sync:rust-validation

use std::collections::HashSet;
use std::sync::LazyLock;

/// Valid service types
pub static SERVICE_TYPES: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ["rest", "socket", "mcp"].into_iter().collect());

/// Valid REST auth types
pub static REST_AUTH_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["api-key", "bearer", "basic", "oauth2"]
        .into_iter()
        .collect()
});

/// Reserved socket event names (cannot be used as custom events)
pub static RESERVED_SOCKET_EVENTS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Internal system events
        "INIT",
        "ERROR",
        "CONNECT",
        "DISCONNECT",
        "RECONNECT",
        "HEARTBEAT",
        // Socket.io reserved events
        "connection",
        "disconnect",
        "error",
        "connect",
        "reconnect",
        "reconnect_attempt",
        "reconnect_error",
        "reconnect_failed",
        "ping",
        "pong",
    ]
    .into_iter()
    .collect()
});

/// MCP capability types
pub static MCP_CAPABILITIES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["tools", "prompts", "resources", "sampling", "logging"]
        .into_iter()
        .collect()
});

/// Check if a service type is valid
pub fn is_valid_service_type(service_type: &str) -> bool {
    SERVICE_TYPES.contains(service_type)
}

/// Check if a REST auth type is valid
pub fn is_valid_rest_auth_type(auth_type: &str) -> bool {
    REST_AUTH_TYPES.contains(auth_type)
}

/// Check if a socket event name is reserved
pub fn is_reserved_socket_event(event: &str) -> bool {
    RESERVED_SOCKET_EVENTS.contains(event)
}

/// Check if an MCP capability is valid
pub fn is_valid_mcp_capability(capability: &str) -> bool {
    MCP_CAPABILITIES.contains(capability)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_types() {
        assert!(is_valid_service_type("rest"));
        assert!(is_valid_service_type("socket"));
        assert!(is_valid_service_type("mcp"));
        assert!(!is_valid_service_type("graphql")); // Not supported yet
    }

    #[test]
    fn test_rest_auth_types() {
        assert!(is_valid_rest_auth_type("api-key"));
        assert!(is_valid_rest_auth_type("bearer"));
        assert!(is_valid_rest_auth_type("oauth2"));
        assert!(!is_valid_rest_auth_type("custom"));
    }

    #[test]
    fn test_reserved_socket_events() {
        assert!(is_reserved_socket_event("INIT"));
        assert!(is_reserved_socket_event("connect"));
        assert!(is_reserved_socket_event("disconnect"));
        assert!(!is_reserved_socket_event("MESSAGE"));
        assert!(!is_reserved_socket_event("CUSTOM_EVENT"));
    }

    #[test]
    fn test_mcp_capabilities() {
        assert!(is_valid_mcp_capability("tools"));
        assert!(is_valid_mcp_capability("prompts"));
        assert!(!is_valid_mcp_capability("unknown"));
    }
}
