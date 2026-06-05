//! Service reference inlining and call-site overrides.

use super::context::{parse_reference, InlineContext, ParsedReference};
use super::InlineError;
use orbital_core::schema::{ServiceDefinition, ServiceRef, ServiceRefObject};

/// Apply override fields from a `ServiceRefObject` to a resolved
/// `ServiceDefinition`. Mirrors `apply_overrides_to_trait` (Phase 3.5.G.A.1)
/// for services. The overrides are variant-aware: REST-only fields
/// (`base_url`, `headers`) only apply to `ServiceDefinition::Rest`,
/// `url` only to `Socket`, `server_path` only to `Mcp`. `description`
/// applies to all three. Overrides that don't match the resolved variant
/// are silently ignored — this matches how `PageRefObject::path` only
/// applies when the resolved page is inline.
fn apply_overrides_to_service(service_def: &mut ServiceDefinition, obj: &ServiceRefObject) {
    match service_def {
        ServiceDefinition::Rest(rest) => {
            if let Some(description) = &obj.description {
                rest.description = Some(description.clone());
            }
            if let Some(base_url) = &obj.base_url {
                rest.base_url = base_url.clone();
            }
            if let Some(headers) = &obj.headers {
                rest.headers = Some(headers.clone());
            }
        }
        ServiceDefinition::Socket(socket) => {
            if let Some(description) = &obj.description {
                socket.description = Some(description.clone());
            }
            if let Some(url) = &obj.url {
                socket.url = url.clone();
            }
        }
        ServiceDefinition::Mcp(mcp) => {
            if let Some(description) = &obj.description {
                mcp.description = Some(description.clone());
            }
            if let Some(server_path) = &obj.server_path {
                mcp.server_path = server_path.clone();
            }
        }
    }
}

/// Inline service references
pub(crate) fn inline_services(
    services: &Option<Vec<ServiceRef>>,
    ctx: &InlineContext,
) -> Result<Option<Vec<ServiceRef>>, InlineError> {
    match services {
        None => Ok(None),
        Some(service_refs) => {
            let mut result = Vec::new();

            for service_ref in service_refs {
                match service_ref {
                    ServiceRef::Inline(_) => {
                        result.push(service_ref.clone());
                    }
                    ServiceRef::Object(obj) => {
                        // Resolve the reference to its inline definition,
                        // apply override fields, and push a single Inline.
                        // Mirrors the trait-side path in `inline_traits`
                        // (Phase 3.5.G.A.1) and the page-side path in
                        // `inline_pages` (PageRef::Object branch above).
                        let parsed = parse_reference(&obj.reference)?;
                        match parsed {
                            ParsedReference::Service { alias, name } => {
                                let inlined = ctx.get_service(&alias, &name)?;
                                match inlined {
                                    ServiceRef::Inline(mut service_def) => {
                                        apply_overrides_to_service(&mut service_def, obj);
                                        result.push(ServiceRef::Inline(service_def));
                                    }
                                    other => {
                                        // Defensive: `ctx.get_service` is
                                        // expected to return Inline. If it
                                        // returns a Reference/Object (because
                                        // the upstream orbital had not been
                                        // inlined yet), pass it through
                                        // unchanged so a later pass can
                                        // resolve it.
                                        result.push(other);
                                    }
                                }
                            }
                            _ => return Err(InlineError::InvalidReference(obj.reference.clone())),
                        }
                    }
                    ServiceRef::Reference(ref_str) => {
                        let parsed = parse_reference(ref_str)?;
                        match parsed {
                            ParsedReference::Service { alias, name } => {
                                let inlined = ctx.get_service(&alias, &name)?;
                                result.push(inlined);
                            }
                            _ => return Err(InlineError::InvalidReference(ref_str.clone())),
                        }
                    }
                }
            }

            Ok(Some(result))
        }
    }
}
