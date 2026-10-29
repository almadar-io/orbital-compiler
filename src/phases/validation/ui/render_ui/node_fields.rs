//! Entity fields a render-ui node references, and the prop-type vocabulary of literal values.

use orbital_core_base::schema::SExpression;
use std::collections::HashSet;

/// True when an `@`-binding refers to the entity as a whole — the linked
/// entity record (`@entity`) or its loaded collection (`@payload`,
/// `@payload.data`, `@data`) — rather than a sub-field. Only whole-entity
/// bindings let a prop's `requiredFields` be checked against the linked
/// entity; a sub-field binding (`@entity.items`, `@config.team`) binds an
/// element collection whose type is opaque here.
fn is_whole_entity_binding(binding: &str) -> bool {
    let mut segs = binding.trim_start_matches('@').split('.');
    match segs.next() {
        // The loaded entity collection.
        Some("payload") | Some("data") => true,
        // The entity record itself — only with no sub-path.
        Some("entity") => segs.next().is_none(),
        _ => false,
    }
}

/// Resolve which entity name a render-ui node's `entity` binding targets —
/// a literal `entity: "Name"`, a WHOLE-entity binding (`@entity`,
/// `@payload[.data]`, `@data`, resolving to the trait's `linked_entity`), or
/// `None` for a SUB-FIELD/opaque binding (`@entity.items`, `@config.team`)
/// or an absent `linked_entity`. Factored out of
/// `validate_entity_prop_contract_inner` so `phases::inline`'s pre-merge
/// reference collection (`required_entity_fields_for_node`) resolves a
/// node's bound entity exactly the same way — never two definitions of
/// "what this node is bound to".
fn resolve_node_bound_entity<'a>(
    obj: &'a orbital_core_base::schema::SObject,
    linked_entity: Option<&'a str>,
) -> Option<&'a str> {
    match obj.get("entity") {
        Some(SExpression::String(s)) if !s.starts_with('@') => Some(s.as_str()),
        Some(SExpression::String(s)) => {
            if is_whole_entity_binding(s) {
                linked_entity
            } else {
                None
            }
        }
        None => linked_entity,
        // Any non-string literal value (array/object) — element type opaque.
        Some(_) => None,
    }
}

/// Entity field NAMES a single render-ui node's pattern-registry
/// `requiredFields` contract references on `entity_name` — every prop whose
/// registry schema declares `requiredFields`, is actually bound at this
/// node, and whose binding resolves (`resolve_node_bound_entity`) to
/// `entity_name`. A render-ui pattern's entity-prop contract is a reference
/// the composed trait makes on an entity WITHOUT spelling `@entity.<field>`
/// literally, so `phases::inline::collect_trait_referenced_entity_fields`
/// calls this for every render-ui node in a trait's body and unions the
/// result into "referenced" — the merge and `validate_entity_prop_contract`
/// must never disagree about what a pattern requires. Registry lookup
/// (`crate::embedded::get_pattern`) and bound-entity resolution are the SAME
/// code the validator uses, not reimplemented; field-map/remap provision
/// (`is_field_provided`'s columns/fields checks) is a SATISFACTION concern
/// for the validator and plays no part in what counts as referenced here —
/// the pattern's requiredFields names themselves are the reference, exactly
/// as the validator checks them.
pub(crate) fn required_entity_fields_for_node(
    pattern_type: &str,
    obj: &orbital_core_base::schema::SObject,
    entity_name: &str,
    linked_entity: Option<&str>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(embedded_pattern) = crate::embedded::get_pattern(pattern_type) else {
        return out;
    };
    if resolve_node_bound_entity(obj, linked_entity) != Some(entity_name) {
        return out;
    }
    for (prop_name, prop_schema) in embedded_pattern.props_schema.iter() {
        let Some(required_fields) = prop_schema.required_fields.as_ref() else {
            continue;
        };
        if required_fields.is_empty() || !obj.contains_key(prop_name) {
            continue;
        }
        out.extend(required_fields.iter().cloned());
    }
    out
}

/// Classify an S-expression value into a simple type string for prop type validation.
pub(crate) fn classify_prop_type(value: &SExpression) -> &'static str {
    match value {
        SExpression::String(s) if s.starts_with('@') => "binding",
        SExpression::String(_) => "string",
        SExpression::Number(_) => "number",
        SExpression::Bool(_) => "boolean",
        // An operator-call SExpr (`(not x)`, `(== a b)`, `(* a b)`, `(array/map ...)`)
        // is a computed expression, not a literal array — its type is the operator's
        // return type, resolved by `resolve_binding_type`. `literal_value_type` returns
        // None for it (the canonical get_operator-backed "computed vs literal" test) and
        // Some("array") for a genuine literal array; classify the computed case as
        // "binding" so the static literal-type check defers to the binding resolver
        // instead of mis-typing every operator call as "array".
        SExpression::Array(_) => match orbital_core_eval::literal_type::literal_value_type(value) {
            Some(t) => t,
            None => "binding",
        },
        SExpression::Object(_) => "object",
        SExpression::Null => "null",
    }
}

