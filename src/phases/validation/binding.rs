//! Binding Validation
//!
//! Validates binding references (@entity, @payload, @state, etc.)
//! Ported from `builder/packages/shared/src/orbitals/validation/binding-validator.ts`.

use super::canonical_operators::get_canonical_operator;
use super::entity_mutation::collect_entity_mutators;
use super::render_ui::is_render_ui_effect;
use super::{trait_path, ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core::schema::{
    EntityDefinition, Orbital, OrbitalSchema, PayloadField, SExpression, TraitDefinition,
    TraitReference,
};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Core binding roots
pub const CORE_BINDING_ROOTS: &[&str] = &[
    "entity",          // Current entity data
    "payload",         // Event payload
    "state",           // Current state name
    "now",             // Current timestamp
    "config",          // Trait configuration
    "trait",           // Trait context
    "user",            // User context
    "callsitePayload", // Call-site-captured event payload (resolved at the composing effect)
    "pages",           // Render-resolved schema sigil — the host orbital's pages (NavItem[])
    "currentTheme",    // Render-resolved schema sigil — the host orbital's data-theme key
];

/// Regex for valid binding format
static BINDING_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Support Unicode characters (e.g. Arabic, Cyrillic, etc.) in binding names
    // Structure: @root.field.nested or @root[index]
    Regex::new(r"^@[\p{L}_][\p{L}0-9_]*(\.[\p{L}0-9_]+|\[\d+\])*$").unwrap()
});

/// Regex for PascalCase entity names
static PASCAL_CASE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-zA-Z0-9]*$").unwrap());

/// Binding context for validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingContext {
    /// In a transition effect
    Effect,
    /// In a guard condition
    Guard,
    /// In a tick effect
    Tick,
    /// In a render_ui pattern prop
    RenderUI,
}

/// Validate all bindings in a schema
pub fn validate_all_bindings(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    // Collect trait names per orbital: `@trait.X` is orbital-scoped, so each
    // orbital gets its own known-trait set. Native inline traits and
    // reference-form traits both expose a local trait name via `trait_name()`
    // on the TraitReference enum.
    let orbital_trait_names: Vec<HashSet<String>> = schema
        .orbitals
        .iter()
        .map(|orbital| {
            orbital
                .traits
                .iter()
                .filter_map(|t| t.trait_name().map(|s| s.to_string()))
                .collect()
        })
        .collect();

    // Validate bindings in orbital inline traits
    // Note: Traits are now only defined inline within orbitals, not at schema level.
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let known_traits = &orbital_trait_names[i];

        // Group-aware setter context (explicit-binding rule, Task C): union
        // `(set @entity.X ...)` fields across every trait in this orbital
        // bound to the SAME `[shared]` entity, so a trait that only READS a
        // field another trait in the group SETS doesn't false-positive
        // `BindingEntityFieldNeverSet`. Only populated for `shared` entities —
        // absent keys fall back to the untouched per-trait behavior below.
        let shared_group_set_fields =
            collect_shared_group_set_fields(orbital, &ctx.entity_definitions);

        // Render-embed index (`@trait.<Child>` in transition render effects).
        // Drives BOTH binding fixes: a child trait rendered inside a parent's
        // transition reads/captures in that transition's scope, so credit its
        // `@entity.X` reads against the (transitively) embedding parents' same-
        // transition `(set)`s and resolve its `@callsitePayload` captures up the
        // embed chain to a real host event.
        let (embed_sites, embed_parents_of) = build_embed_index(orbital);
        let embed_set_credit = collect_embed_set_credit(orbital, &embed_sites, &embed_parents_of);

        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            if let TraitReference::Inline(trait_def) = trait_ref {
                let path = trait_path(i, &orbital.name, j, trait_ref);
                let result = validate_trait_bindings(
                    trait_def,
                    &path,
                    &ctx.entity_names,
                    &ctx.entity_definitions,
                    &shared_group_set_fields,
                    embed_set_credit.get(&trait_def.name),
                );
                ctx.merge(result);

                // `@trait.X` bindings (client-only, render-time) need their
                // own pass with orbital-scoped context: known traits in this
                // orbital + the current trait's own name (to catch self-ref).
                let trait_result = validate_trait_binding_usage(trait_def, &path, known_traits);
                ctx.merge(trait_result);
            }
        }

        // Detect cycles in the orbital's `@trait.X` reference graph after
        // all per-trait validation so the error is reported once per orbital
        // rather than once per cycle participant.
        let cycle_result =
            detect_trait_binding_cycles(&orbital.traits, &format!("orbitals[{}]", i));
        ctx.merge(cycle_result);

        // Call-site payload captures (`@callsitePayload.<field>`) are validated
        // against the COMPOSING event payload — resolved UP the `@trait.<X>`
        // embed chain to a real host transition, so a capture nested under a
        // hoisted render wrapper is checked against the wrapper's host event,
        // not the wrapper's own (payload-less) INIT.
        let capture_result = validate_callsite_payload_captures(
            orbital,
            &format!("orbitals[{}]", i),
            &embed_sites,
            &embed_parents_of,
        );
        ctx.merge(capture_result);
    }
}

/// Collect every `@trait.<Name>` reference (bare, no trailing path) from a
/// render tree, matching the `@trait.X` embedding grammar used everywhere else.
fn collect_embedded_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(name) = s.strip_prefix("@trait.") {
                if !name.is_empty() && !name.contains('.') {
                    out.insert(name.to_string());
                }
            }
        }
        SExpression::Array(items) => {
            for item in items {
                collect_embedded_trait_refs(item, out);
            }
        }
        SExpression::Object(map) => {
            for v in map.values() {
                collect_embedded_trait_refs(v, out);
            }
        }
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => {}
    }
}

/// One parent transition that embeds one-or-more child traits via a
/// `@trait.<Child>` node in its render (transition guard/effects). This is the
/// structural read-context edge both binding fixes key on: whatever renders a
/// child — a hand-authored `@trait.X` embed or a lowerer-hoisted inline JSX
/// child — puts the child inside THIS transition's scope, so the child's entity
/// reads and payload captures resolve against this transition's `(set)`s and
/// event payload. Provenance-free by design: the rule is about the embedding
/// relationship, not how the child trait came to exist.
struct EmbedSite {
    parent_name: String,
    parent_linked_entity: Option<String>,
    event: String,
    /// `@entity.<field>` fields written by a `(set …)` on this transition.
    set_fields: HashSet<String>,
    /// This transition's event payload schema (from the parent trait's
    /// `stateMachine.events`), if declared.
    payload: Option<Vec<PayloadField>>,
}

/// Build the `@trait.<Child>` render-embed index for one orbital's resolved
/// (inline) traits. Returns the flat site list plus `child name -> site
/// indices` (a child can be embedded by several parent transitions). Only
/// transition guards/effects are scanned — that is where rendering actually
/// happens, and the inline phase substitutes a hoisted wrapper's
/// `@config.children` (holding the nested `@trait.<Grandchild>` refs) into its
/// render body, so a Host→Stack→Typography chain surfaces as two transition
/// embed edges here.
fn build_embed_index(orbital: &Orbital) -> (Vec<EmbedSite>, HashMap<String, Vec<usize>>) {
    let mut sites: Vec<EmbedSite> = Vec::new();
    let mut parents_of: HashMap<String, Vec<usize>> = HashMap::new();
    for tr in &orbital.traits {
        let TraitReference::Inline(def) = tr else {
            continue;
        };
        let event_payloads: HashMap<&str, &[PayloadField]> = def
            .state_machine
            .events
            .as_ref()
            .map(|events| {
                events
                    .iter()
                    .filter_map(|e| {
                        e.payload_schema
                            .as_ref()
                            .map(|p| (e.key.as_str(), p.as_slice()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        for transition in &def.state_machine.transitions {
            let mut embedded = HashSet::new();
            if let Some(guard) = transition.guard.as_ref() {
                collect_embedded_trait_refs(guard, &mut embedded);
            }
            if let Some(effects) = transition.effects.as_ref() {
                for effect in effects {
                    collect_embedded_trait_refs(effect, &mut embedded);
                }
            }
            if embedded.is_empty() {
                continue;
            }
            let mut referenced: HashMap<String, String> = HashMap::new();
            let mut set_fields: HashSet<String> = HashSet::new();
            if let Some(guard) = transition.guard.as_ref() {
                guard.collect_entity_field_refs(
                    &mut referenced,
                    &mut set_fields,
                    &transition.event,
                );
            }
            if let Some(effects) = transition.effects.as_ref() {
                for effect in effects {
                    effect.collect_entity_field_refs(
                        &mut referenced,
                        &mut set_fields,
                        &transition.event,
                    );
                }
            }
            let idx = sites.len();
            sites.push(EmbedSite {
                parent_name: def.name.clone(),
                parent_linked_entity: def.linked_entity.clone(),
                event: transition.event.clone(),
                set_fields,
                payload: event_payloads
                    .get(transition.event.as_str())
                    .map(|p| p.to_vec()),
            });
            for child in embedded {
                parents_of.entry(child).or_default().push(idx);
            }
        }
    }
    (sites, parents_of)
}

/// Every embed site that transitively renders `child` — the child's direct
/// parents, their parents, and so on up the `@trait.<X>` chain. Walking UP the
/// chain is what lets a capture / read reach the REAL host transition (e.g.
/// `Host.CampaignLoadFailed`) through intermediate render wrappers (e.g. a
/// hoisted `Stack` whose own `INIT` self-loop is not a meaningful scope).
/// Cycle-guarded by trait name.
fn transitive_ancestor_sites(
    child: &str,
    parents_of: &HashMap<String, Vec<usize>>,
    sites: &[EmbedSite],
) -> Vec<usize> {
    let mut result: Vec<usize> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = vec![child.to_string()];
    while let Some(name) = stack.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        if let Some(idxs) = parents_of.get(&name) {
            for &i in idxs {
                result.push(i);
                stack.push(sites[i].parent_name.clone());
            }
        }
    }
    result
}

/// Validate one captured payload field path against a composing event's payload
/// schema. Mirrors `validate_payload_field_references` but keys off a plain
/// dotted path (the `@callsitePayload.<path>` payload) resolved in the composing
/// event's scope, so the error is identical to a direct `@payload.<field>` on
/// that event.
fn validate_captured_payload_path(
    result: &mut ValidationResult,
    field_path: &str,
    payload_fields: Option<&[PayloadField]>,
    event_name: &str,
    path: &str,
) {
    let segments: Vec<&str> = field_path
        .split('.')
        .map(|seg| seg.split('[').next().unwrap_or(seg))
        .filter(|seg| !seg.is_empty())
        .collect();
    if segments.is_empty() {
        return;
    }
    let field_name = segments[0];
    match payload_fields {
        Some(fields) => {
            if let Some(unresolved) = resolve_payload_path(fields, &segments) {
                let valid_at_level: Vec<String> =
                    unresolved.siblings.iter().map(|f| f.name.clone()).collect();
                let suggestion = payload_sigil_hybrid_suggestion(&segments, Some(fields))
                    .unwrap_or_else(|| {
                        format!(
                            "Add {{ \"name\": \"{}\", \"type\": \"...\" }} to the event's payload array",
                            unresolved.segment
                        )
                    });
                result.add_error(
                    ValidationError::error(
                        ErrorCode::BindingPayloadFieldUndeclared,
                        path,
                        format!(
                            "Payload field '{}' is not declared in event '{}' payload schema",
                            unresolved.segment, event_name
                        ),
                    )
                    .with_suggestion(suggestion)
                    .with_valid_values(valid_at_level),
                );
            }
        }
        None => {
            let suggestion = payload_sigil_hybrid_suggestion(&segments, None).unwrap_or_else(|| {
                format!(
                    "Add a payload schema to the '{}' event definition, or bind a field the event declares",
                    event_name
                )
            });
            result.add_error(
                ValidationError::error(
                    ErrorCode::BindingPayloadFieldUndeclared,
                    path,
                    format!(
                        "Event '{}' uses @payload.{} (.lolo spelling: `?{}` or `@payload.{}`) but has no payload schema defined",
                        event_name, field_name, field_name, field_name
                    ),
                )
                .with_suggestion(suggestion),
            );
        }
    }
}

/// Split a `@callsitePayload.<path>` field path into index-stripped segments.
fn payload_path_segments(field_path: &str) -> Vec<&str> {
    field_path
        .split('.')
        .map(|seg| seg.split('[').next().unwrap_or(seg))
        .filter(|seg| !seg.is_empty())
        .collect()
}

/// Validate every call-site payload capture in an orbital against a COMPOSING
/// event payload found by resolving UP the `@trait.<Child>` embed chain. A
/// capture lives on a child trait's declared config-field default as
/// `@callsitePayload.<field>` (the lolo lowerer tagged it there; the inline
/// phase keeps it out of the child body). The composing scope is the transition
/// that renders the capturing child — directly, or through one or more hoisted
/// render wrappers (e.g. a `Stack` whose own `INIT` self-loop carries no
/// payload). The capture is satisfied when ANY transitively-embedding parent
/// transition's event declares the field (warning-suppression posture, matching
/// the `[shared]` group rule); only when NO embedding parent's event declares it
/// is an error raised — reported against the nearest parent that had SOME
/// payload schema, else a "no payload schema" diagnostic.
fn validate_callsite_payload_captures(
    orbital: &Orbital,
    orbital_path: &str,
    embed_sites: &[EmbedSite],
    embed_parents_of: &HashMap<String, Vec<usize>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // child trait name -> its captured payload field paths
    let mut captures_by_trait: HashMap<&str, Vec<String>> = HashMap::new();
    for tr in &orbital.traits {
        if let TraitReference::Inline(def) = tr {
            let mut paths = Vec::new();
            if let Some(cfg) = def.config.as_ref() {
                for field in cfg.values() {
                    if let Some(default) = field.default.as_ref() {
                        orbital_core::schema::collect_callsite_payload_paths(default, &mut paths);
                    }
                }
            }
            if !paths.is_empty() {
                captures_by_trait.insert(def.name.as_str(), paths);
            }
        }
    }
    if captures_by_trait.is_empty() {
        return result;
    }

    for (child, paths) in &captures_by_trait {
        let ancestors = transitive_ancestor_sites(child, embed_parents_of, embed_sites);
        // A capture only has a determinate composing scope when the child is
        // rendered somewhere. Orphans (embedded nowhere) can't be resolved — the
        // original pass silently skipped them, so keep that.
        if ancestors.is_empty() {
            continue;
        }
        for field_path in paths.iter() {
            let segments = payload_path_segments(field_path);
            if segments.is_empty() {
                continue;
            }
            let satisfied = ancestors.iter().any(|&i| {
                matches!(&embed_sites[i].payload, Some(fields)
                    if resolve_payload_path(fields, &segments).is_none())
            });
            if satisfied {
                continue;
            }
            // Unsatisfied everywhere up the chain — raise ONE error. Prefer a
            // parent that DID declare a payload (precise "field not declared in
            // event X" with valid values); fall back to a "no payload schema"
            // diagnostic against the nearest parent.
            let with_schema = ancestors
                .iter()
                .map(|&i| &embed_sites[i])
                .find(|s| s.payload.is_some());
            let target = with_schema.unwrap_or_else(|| &embed_sites[ancestors[0]]);
            let path = format!(
                "{} (embeds {}: @callsitePayload.{})",
                orbital_path, child, field_path
            );
            validate_captured_payload_path(
                &mut result,
                field_path,
                target.payload.as_deref(),
                &target.event,
                &path,
            );
        }
    }

    result
}

/// Per-trait credited `(set @entity.<field>)` fields sourced from the trait's
/// (transitively) embedding parents — the entity fields a hoisted / embedded
/// child may READ without a local setter because a parent that renders it set
/// them in the same render context. Keyed on the render-embed relationship, so
/// a hand-authored `@trait.X` embed earns the same credit as a lowerer-hoisted
/// inline child. A parent's sets only credit a child bound to the SAME entity.
fn collect_embed_set_credit(
    orbital: &Orbital,
    embed_sites: &[EmbedSite],
    embed_parents_of: &HashMap<String, Vec<usize>>,
) -> HashMap<String, HashSet<String>> {
    let mut credit: HashMap<String, HashSet<String>> = HashMap::new();
    for tr in &orbital.traits {
        let TraitReference::Inline(def) = tr else {
            continue;
        };
        let ancestors = transitive_ancestor_sites(&def.name, embed_parents_of, embed_sites);
        if ancestors.is_empty() {
            continue;
        }
        let mut fields: HashSet<String> = HashSet::new();
        for &i in &ancestors {
            let site = &embed_sites[i];
            if site.parent_linked_entity == def.linked_entity {
                fields.extend(site.set_fields.iter().cloned());
            }
        }
        if !fields.is_empty() {
            credit.insert(def.name.clone(), fields);
        }
    }
    credit
}

/// Validate all bindings in a trait
pub fn validate_trait_bindings(
    trait_def: &TraitDefinition,
    path: &str,
    known_entities: &HashSet<String>,
    entity_definitions: &HashMap<String, EntityDefinition>,
    shared_group_set_fields: &HashMap<String, HashSet<String>>,
    embed_set_credit: Option<&HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Resolve the trait's linked entity for @entity.field validation
    let linked_entity_def = trait_def
        .linked_entity
        .as_ref()
        .and_then(|name| entity_definitions.get(name));

    // Build event -> payload fields map
    let event_payloads: HashMap<&str, &[PayloadField]> = trait_def
        .state_machine
        .events
        .as_ref()
        .map(|events| {
            events
                .iter()
                .filter_map(|e| {
                    e.payload_schema
                        .as_ref()
                        .map(|p| (e.key.as_str(), p.as_slice()))
                })
                .collect()
        })
        .unwrap_or_default();

    // Validate bindings in transitions
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);

        // Get payload fields for this transition's event
        let payload_fields = event_payloads.get(transition.event.as_str()).copied();

        // Validate guard bindings
        if let Some(guard) = &transition.guard {
            result.merge(validate_expr_bindings(
                guard,
                &format!("{}.guard", trans_path),
                BindingContext::Guard,
                known_entities,
            ));
            // Validate payload field references in guards
            result.merge(validate_payload_field_references(
                guard,
                &format!("{}.guard", trans_path),
                payload_fields,
                &transition.event,
            ));
            // Validate entity field references in guards
            result.merge(validate_entity_field_references(
                guard,
                &format!("{}.guard", trans_path),
                linked_entity_def,
                entity_definitions,
            ));
        }

        // Validate effect bindings
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let effect_path = format!("{}.effects[{}]", trans_path, j);
                result.merge(validate_expr_bindings(
                    effect,
                    &effect_path,
                    BindingContext::Effect,
                    known_entities,
                ));
                // Validate payload field references in effects
                result.merge(validate_payload_field_references(
                    effect,
                    &effect_path,
                    payload_fields,
                    &transition.event,
                ));
                // Validate entity field references in effects
                result.merge(validate_entity_field_references(
                    effect,
                    &effect_path,
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
    }

    // Validate bindings in ticks
    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            for (j, effect) in tick.effects.iter().enumerate() {
                let tick_path = format!("{}.ticks[{}].effects[{}]", path, i, j);
                result.merge(validate_expr_bindings(
                    effect,
                    &tick_path,
                    BindingContext::Tick,
                    known_entities,
                ));
                // Validate entity field references in tick effects
                result.merge(validate_entity_field_references(
                    effect,
                    &tick_path,
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
    }

    // Explicit-binding rule: every @entity.<field> read must be backed by
    // an explicit (set @entity.<field> ...) somewhere in the trait. Skipped
    // when the trait has no linkedEntity. `@intrinsic` fields on the resolved
    // linked entity are exempt (runtime-managed widget state) — keyed on the
    // resolved entity so the exemption survives a `linkedEntity` rebind.
    let intrinsic_fields: HashSet<String> = linked_entity_def
        .map(|e| {
            e.fields
                .iter()
                .filter(|f| f.intrinsic == Some(true))
                .filter_map(|f| f.name.clone())
                .collect()
        })
        .unwrap_or_default();
    result.merge(validate_entity_fields_have_setters(
        trait_def,
        path,
        &intrinsic_fields,
        shared_group_set_fields,
        embed_set_credit,
    ));

    result
}

/// Union `(set @entity.X ...)` fields across every trait in `orbital` bound
/// to the same `[shared]` entity. Non-shared entities are omitted entirely —
/// `validate_entity_fields_have_setters` falls back to per-trait behavior via
/// `unwrap_or(false)` when a key is absent, so this function only needs to
/// carry the `[shared]` case. Reuses `entity_mutation::collect_entity_mutators`
/// (itself built on `collect_entity_field_refs`) rather than re-walking the
/// trait's SExpression tree a second time.
fn collect_shared_group_set_fields(
    orbital: &Orbital,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> HashMap<String, HashSet<String>> {
    let inline_traits: Vec<&TraitDefinition> = orbital
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(def) => Some(def),
            TraitReference::Reference { .. } => None,
        })
        .collect();

    collect_entity_mutators(inline_traits)
        .into_iter()
        .filter(|(entity_name, _)| {
            entity_definitions
                .get(entity_name)
                .map(|e| e.shared)
                .unwrap_or(false)
        })
        .map(|(entity_name, field_map)| (entity_name, field_map.into_keys().collect()))
        .collect()
}

/// Validate bindings in an expression
fn validate_expr_bindings(
    expr: &SExpression,
    path: &str,
    context: BindingContext,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    validate_expr_bindings_with_locals(expr, path, context, known_entities, &HashSet::new())
}

/// Validate bindings in an expression, with lambda-local variable tracking.
/// `lambda_locals` contains parameter names bound by enclosing lambdas (e.g., "item", "f", "o").
/// Bindings like `@item.field` are valid when "item" is in `lambda_locals`.
fn validate_expr_bindings_with_locals(
    expr: &SExpression,
    path: &str,
    context: BindingContext,
    known_entities: &HashSet<String>,
    lambda_locals: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::String(s) if s.starts_with('@') => {
            // Check if this binding root is a lambda-local variable
            let root = s[1..].split('.').next().unwrap_or("");
            let root = root.split('[').next().unwrap_or(root);
            if lambda_locals.contains(root) {
                // Valid lambda-bound variable — skip standard binding validation
            } else {
                result.merge(validate_binding(s, path, context, known_entities));
            }
        }
        SExpression::Array(arr) => {
            // Detect lambda expressions: ["lambda", [params...], body]
            if arr.len() == 3 {
                if let SExpression::String(op) = &arr[0] {
                    if op == "lambda" || op == "fn" {
                        if let SExpression::Array(params) = &arr[1] {
                            // Extract lambda parameter names and add to locals
                            // Format: ["fn", ["param1", "param2"], body]
                            let mut extended_locals = lambda_locals.clone();
                            for param in params {
                                if let SExpression::String(name) = param {
                                    extended_locals.insert(name.clone());
                                }
                            }
                            // Validate the lambda body with extended locals
                            return validate_expr_bindings_with_locals(
                                &arr[2],
                                &format!("{}[2]", path),
                                context,
                                known_entities,
                                &extended_locals,
                            );
                        } else if let SExpression::String(param_name) = &arr[1] {
                            // Single string parameter shorthand
                            // Format: ["fn", "param", body]
                            let mut extended_locals = lambda_locals.clone();
                            extended_locals.insert(param_name.clone());
                            return validate_expr_bindings_with_locals(
                                &arr[2],
                                &format!("{}[2]", path),
                                context,
                                known_entities,
                                &extended_locals,
                            );
                        }
                    }
                    // Detect `let`: ["let", [[name, valueExpr], ...], body].
                    // SEQUENTIAL `let`: each binding's value-expr sees the
                    // locals bound by EARLIER bindings in the same `let` (and
                    // the outer scope); the body sees them all. This matches
                    // BOTH runtimes — the Rust evaluator's `LetOp` and the JS
                    // `EffectExecutor` `let` both fold each value against an
                    // accumulating child scope. (The old parallel model —
                    // value-exprs in the outer scope only — diverged from
                    // runtime and falsely rejected `(let ((a …) (b @a)))`.)
                    if op == "let" {
                        let mut extended_locals = lambda_locals.clone();
                        match &arr[1] {
                            SExpression::Array(bindings) => {
                                for (bi, binding) in bindings.iter().enumerate() {
                                    if let SExpression::Array(pair) = binding {
                                        // Validate the value against locals from
                                        // PRIOR bindings only (name inserted
                                        // after), so a self-reference still errors.
                                        if pair.len() >= 2 {
                                            result.merge(validate_expr_bindings_with_locals(
                                                &pair[1],
                                                &format!("{}[1][{}][1]", path, bi),
                                                context,
                                                known_entities,
                                                &extended_locals,
                                            ));
                                        }
                                        if let Some(SExpression::String(name)) = pair.first() {
                                            extended_locals.insert(name.clone());
                                        }
                                    }
                                }
                            }
                            SExpression::Object(bindings) => {
                                // Object-form bindings have no defined order, so
                                // credit every name before validating any value
                                // (permissive — never falsely rejects). lolo only
                                // ever lowers `let` to the ordered array form above.
                                for name in bindings.keys() {
                                    extended_locals.insert(name.clone());
                                }
                                for (name, value_expr) in bindings {
                                    result.merge(validate_expr_bindings_with_locals(
                                        value_expr,
                                        &format!("{}[1].{}", path, name),
                                        context,
                                        known_entities,
                                        &extended_locals,
                                    ));
                                }
                            }
                            _ => {}
                        }
                        result.merge(validate_expr_bindings_with_locals(
                            &arr[2],
                            &format!("{}[2]", path),
                            context,
                            known_entities,
                            &extended_locals,
                        ));
                        return result;
                    }
                }
            }
            // Detect collection operators (map, filter, find, etc.) that introduce
            // implicit @item/@index lambda scope on their body argument (args[2+]).
            // Uses the canonical operator registry to identify collection/array operators
            // that accept a callback (arity >= 2).
            if arr.len() >= 3 {
                if let SExpression::String(op) = &arr[0] {
                    let is_collection_op = is_lambda_scope_operator(op);
                    if is_collection_op {
                        // Validate the source (args[1]) without implicit locals
                        result.merge(validate_expr_bindings_with_locals(
                            &arr[1],
                            &format!("{}[1]", path),
                            context,
                            known_entities,
                            lambda_locals,
                        ));
                        // Validate the body (args[2+]) with @item/@index in scope
                        let mut extended_locals = lambda_locals.clone();
                        extended_locals.insert("item".to_string());
                        extended_locals.insert("index".to_string());
                        for (i, item) in arr[2..].iter().enumerate() {
                            result.merge(validate_expr_bindings_with_locals(
                                item,
                                &format!("{}[{}]", path, i + 2),
                                context,
                                known_entities,
                                &extended_locals,
                            ));
                        }
                        // Validate the operator name itself (skip, it's not a binding)
                        return result;
                    }
                }
            }
            // Normal array — recurse into all elements
            for (i, item) in arr.iter().enumerate() {
                result.merge(validate_expr_bindings_with_locals(
                    item,
                    &format!("{}[{}]", path, i),
                    context,
                    known_entities,
                    lambda_locals,
                ));
            }
        }
        SExpression::Object(obj) => {
            for (key, value) in obj {
                result.merge(validate_expr_bindings_with_locals(
                    value,
                    &format!("{}.{}", path, key),
                    context,
                    known_entities,
                    lambda_locals,
                ));
            }
        }
        _ => {}
    }

    result
}

/// Validate a single binding
pub fn validate_binding(
    binding: &str,
    path: &str,
    context: BindingContext,
    known_entities: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Check binding format
    if !BINDING_REGEX.is_match(binding) {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingInvalidFormat,
                path,
                format!("Invalid binding format: '{}'", binding),
            )
            .with_suggestion("Use @root.field.nested format"),
        );
        return result;
    }

    // Parse binding parts
    let parts: Vec<&str> = binding[1..].split('.').collect();
    if parts.is_empty() {
        return result;
    }

    let root = parts[0];
    // Handle array index in first part (e.g., @items[0])
    let root = root.split('[').next().unwrap_or(root);

    // Validate binding root
    if CORE_BINDING_ROOTS.contains(&root) {
        // Core binding
        match root {
            "payload" if context == BindingContext::Tick => {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::BindingPayloadInTick,
                        path,
                        "@payload is not available in tick context",
                    )
                    .with_suggestion(
                        "Tick effects don't have event payloads - use @entity instead",
                    ),
                );
            }
            "state" if parts.len() > 1 => {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::BindingStateNoPath,
                        path,
                        "@state should not have a path - it returns the state name directly",
                    )
                    .with_suggestion("Use @state without a path"),
                );
            }
            "now" if parts.len() > 1 => {
                result.add_error(
                    ValidationError::warning(
                        ErrorCode::BindingNowNoPath,
                        path,
                        "@now should not have a path - it returns the current timestamp directly",
                    )
                    .with_suggestion("Use @now without a path"),
                );
            }
            _ => {}
        }
    } else if PASCAL_CASE_REGEX.is_match(root) {
        // Entity binding (e.g., @Task, @User)
        if !known_entities.is_empty() && !known_entities.contains(root) {
            result.add_error(
                ValidationError::warning(
                    ErrorCode::BindingUnknownEntity,
                    path,
                    format!("Entity '{}' in binding may not exist", root),
                )
                .with_suggestion("Ensure the entity is defined in the schema")
                .with_valid_values(known_entities.iter().cloned().collect()),
            );
        }
    } else if root == "item" || root == "index" {
        // S16 — same class as `sigil_context.rs`'s arm, same one diagnostic:
        // callers strip lambda-locals before reaching here (see
        // `validate_expr_bindings_with_locals`), so `item`/`index` at this
        // point escaped its collection callback.
        result.add_error(super::error::out_of_scope_collection_local(
            ErrorCode::BindingUnknownRoot,
            path,
            root,
        ));
    } else {
        result.add_error(
            ValidationError::warning(
                ErrorCode::BindingUnknownRoot,
                path,
                format!("Unknown binding root: '@{}'", root),
            )
            .with_suggestion("Use @entity, @payload, @state, @now, @config, or @EntityName"),
        );
    }

    result
}

/// Validate a set effect target binding
pub fn validate_set_target(binding: &str, path: &str) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !binding.starts_with('@') {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingInvalidFormat,
                path,
                format!(
                    "set target must be a binding starting with @: '{}'",
                    binding
                ),
            )
            .with_suggestion("Use @entity.field or @EntityName.field"),
        );
        return result;
    }

    // Parse binding
    let parts: Vec<&str> = binding[1..].split('.').collect();
    if parts.is_empty() {
        return result;
    }

    let root = parts[0];

    // Cannot set @payload (read-only)
    if root == "payload" {
        result.add_error(
            ValidationError::error(
                ErrorCode::EffectSetPayloadNotAllowed,
                path,
                "@payload is read-only and cannot be a set target",
            )
            .with_suggestion("Use @entity to update entity state"),
        );
    }

    // Cannot set @state or @now directly
    if root == "state" || root == "now" {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingInvalidPath,
                path,
                format!("@{} cannot be a set target", root),
            )
            .with_suggestion("Use @entity.field to update entity data"),
        );
    }

    // Must have a field path (can't set root directly)
    if parts.len() == 1 && (root == "entity" || root == "config") {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingSetTargetMissingPath,
                path,
                format!("set target @{} requires a field path", root),
            )
            .with_suggestion(format!("Use @{}.fieldName", root)),
        );
    }

    result
}

/// The first path segment that fails to resolve while walking a `@payload`
/// binding through nested `properties`, plus the field set at that level (for
/// the `valid_values` suggestion).
struct UnresolvedPayloadSegment<'a> {
    segment: &'a str,
    siblings: &'a [PayloadField],
}

/// `?payload.<field>` lexes as ONE dotted payload sigil and lowers to
/// `@payload.payload.<field>` — an author reaching for the (illegal) hybrid
/// form, not a field literally named "payload". Detected when the path's head
/// is `payload`, deeper segments follow, and the schema declares no field
/// actually named `payload`; returns the corrective suggestion.
fn payload_sigil_hybrid_suggestion(
    segments: &[&str],
    payload_fields: Option<&[PayloadField]>,
) -> Option<String> {
    if segments.len() < 2 || segments[0] != "payload" {
        return None;
    }
    if let Some(fields) = payload_fields {
        if fields.iter().any(|f| f.name == "payload") {
            return None;
        }
    }
    let rest = segments[1..].join(".");
    Some(format!(
        "the `?` sigil already denotes the event payload — `?payload.{rest}` lowers to `@payload.payload.{rest}`; write `?{rest}` or `@payload.{rest}` instead"
    ))
}

/// Walk `segments` (already `@payload.`-stripped + index-stripped) through the
/// payload schema, descending into each matched field's `properties`. Returns
/// `None` when every segment resolves, or the first segment that doesn't (with
/// the sibling set at that level). Stops descending once a segment matches a
/// leaf (no `properties`) — deeper segments on a leaf are runtime object access
/// the schema doesn't enumerate, not a declaration error.
fn resolve_payload_path<'a>(
    fields: &'a [PayloadField],
    segments: &'a [&'a str],
) -> Option<UnresolvedPayloadSegment<'a>> {
    let mut level = fields;
    for seg in segments {
        match level.iter().find(|f| f.name == *seg) {
            None => {
                return Some(UnresolvedPayloadSegment {
                    segment: seg,
                    siblings: level,
                })
            }
            Some(field) => match field.properties.as_deref() {
                Some(props) => level = props,
                None => return None,
            },
        }
    }
    None
}

/// Validate that @payload.field references are declared in the event's payload schema
fn validate_payload_field_references(
    expr: &SExpression,
    path: &str,
    payload_fields: Option<&[PayloadField]>,
    event_name: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::String(s) if s.starts_with("@payload.") => {
            // Walk the full dotted path through nested `properties`, so a
            // nested binding `@payload.row.category` resolves against the
            // `row` object field's `category` property (entity-ref payload
            // fields now lower to ONE nested object field, not dotted-flat
            // members).
            let binding_path = &s[9..]; // Skip "@payload."
            let segments: Vec<&str> = binding_path
                .split('.')
                .map(|seg| seg.split('[').next().unwrap_or(seg))
                .filter(|seg| !seg.is_empty())
                .collect();

            if segments.is_empty() {
                return result;
            }
            let field_name = segments[0];

            match payload_fields {
                Some(fields) => {
                    if let Some(unresolved) = resolve_payload_path(fields, &segments) {
                        let valid_at_level: Vec<String> =
                            unresolved.siblings.iter().map(|f| f.name.clone()).collect();
                        let suggestion = payload_sigil_hybrid_suggestion(&segments, Some(fields))
                            .unwrap_or_else(|| {
                                format!(
                                    "Add {{ \"name\": \"{}\", \"type\": \"...\" }} to the event's payload array",
                                    unresolved.segment
                                )
                            });
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::BindingPayloadFieldUndeclared,
                                path,
                                format!(
                                    "Payload field '{}' is not declared in event '{}' payload schema",
                                    unresolved.segment, event_name
                                ),
                            )
                            .with_suggestion(suggestion)
                            .with_valid_values(valid_at_level),
                        );
                    }
                }
                None => {
                    // No payload schema defined for this event. Lifecycle
                    // events structurally carry no payload — telling the
                    // author to "add a payload schema" to INIT sends a model
                    // in circles (observed: Run 19); the fix is to not read
                    // @payload there at all.
                    let is_lifecycle = matches!(event_name, "INIT" | "LOAD" | "$MOUNT");
                    let suggestion = if is_lifecycle {
                        format!(
                            "'{event_name}' is a lifecycle event and carries NO payload — @payload.{field_name} \
                             (written `?{field_name}` in .lolo) can never resolve here. Read entity/config \
                             state instead, or move this logic to the transition for the event that \
                             actually delivers '{field_name}'."
                        )
                    } else {
                        payload_sigil_hybrid_suggestion(&segments, None).unwrap_or_else(|| {
                            format!(
                                "declare the field in the event's `emits` payload block: `{} -> external {{ {} : <type> }}` — an event read via @payload.{} must declare that field",
                                event_name, field_name, field_name
                            )
                        })
                    };
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::BindingPayloadFieldUndeclared,
                            path,
                            format!(
                                "Event '{}' uses @payload.{} (.lolo spelling: `?{}` or `@payload.{}`) but has no payload schema defined",
                                event_name, field_name, field_name, field_name
                            ),
                        )
                        .with_suggestion(suggestion),
                    );
                }
            }
        }
        SExpression::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                result.merge(validate_payload_field_references(
                    item,
                    &format!("{}[{}]", path, i),
                    payload_fields,
                    event_name,
                ));
            }
        }
        SExpression::Object(obj) => {
            for (key, value) in obj {
                result.merge(validate_payload_field_references(
                    value,
                    &format!("{}.{}", path, key),
                    payload_fields,
                    event_name,
                ));
            }
        }
        _ => {}
    }

    result
}

/// Validate that @entity.field references exist on the linked entity definition.
/// Also validates @EntityName.field references against the named entity.
fn validate_entity_field_references(
    expr: &SExpression,
    path: &str,
    linked_entity_def: Option<&EntityDefinition>,
    entity_definitions: &HashMap<String, EntityDefinition>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    match expr {
        SExpression::String(s) if s.starts_with("@entity.") => {
            // Extract the first field name from @entity.fieldName or @entity.fieldName.nested
            let binding_path = &s[8..]; // Skip "@entity."
            let field_name = binding_path.split('.').next().unwrap_or("");
            // Strip array index (e.g., "items[0]" -> "items")
            let field_name = field_name.split('[').next().unwrap_or(field_name);

            if field_name.is_empty() {
                return result;
            }

            // Skip common meta-fields that exist at runtime but not in schema
            if field_name == "id" || field_name == "createdAt" || field_name == "updatedAt" {
                return result;
            }

            if let Some(entity_def) = linked_entity_def {
                let field_exists = entity_def
                    .fields
                    .iter()
                    .any(|f| f.name.as_ref().map(|n| n == field_name).unwrap_or(false));

                if !field_exists {
                    let available_fields: Vec<String> = entity_def
                        .fields
                        .iter()
                        .filter_map(|f| f.name.clone())
                        .collect();
                    result.add_error(
                        ValidationError::error(
                            ErrorCode::BindingEntityFieldNotFound,
                            path,
                            format!(
                                "@entity.{} — field '{}' not found on entity '{}'",
                                field_name, field_name, entity_def.name
                            ),
                        )
                        .with_suggestion(format!(
                            "Add field '{}' to entity '{}' or fix the binding path",
                            field_name, entity_def.name
                        ))
                        .with_valid_values(available_fields),
                    );
                }
            }
        }
        SExpression::String(s) if s.starts_with('@') => {
            // Check @EntityName.field bindings
            let without_at = &s[1..];
            let parts: Vec<&str> = without_at.split('.').collect();
            if parts.len() >= 2 {
                let root = parts[0].split('[').next().unwrap_or(parts[0]);
                if PASCAL_CASE_REGEX.is_match(root) {
                    if let Some(entity_def) = entity_definitions.get(root) {
                        let field_name = parts[1].split('[').next().unwrap_or(parts[1]);
                        // Skip common meta-fields
                        if field_name != "id"
                            && field_name != "createdAt"
                            && field_name != "updatedAt"
                        {
                            let field_exists = entity_def
                                .fields
                                .iter()
                                .any(|f| f.name.as_ref().map(|n| n == field_name).unwrap_or(false));
                            if !field_exists {
                                let available_fields: Vec<String> = entity_def
                                    .fields
                                    .iter()
                                    .filter_map(|f| f.name.clone())
                                    .collect();
                                result.add_error(
                                    ValidationError::error(
                                        ErrorCode::BindingEntityFieldNotFound,
                                        path,
                                        format!(
                                            "@{}.{} — field '{}' not found on entity '{}'",
                                            root, field_name, field_name, root
                                        ),
                                    )
                                    .with_suggestion(format!(
                                        "Add field '{}' to entity '{}' or fix the binding path",
                                        field_name, root
                                    ))
                                    .with_valid_values(available_fields),
                                );
                            }
                        }
                    }
                }
            }
        }
        SExpression::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                result.merge(validate_entity_field_references(
                    item,
                    &format!("{}[{}]", path, i),
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
        SExpression::Object(obj) => {
            for (key, value) in obj {
                result.merge(validate_entity_field_references(
                    value,
                    &format!("{}.{}", path, key),
                    linked_entity_def,
                    entity_definitions,
                ));
            }
        }
        _ => {}
    }

    result
}

/// Validate that every `@entity.<field>` referenced in a trait's
/// transitions/guards/effects/ticks has at least one explicit
/// `(set @entity.<field> <value>)` somewhere in the trait.
///
/// Per the explicit-binding rule, `@entity.X` is populated EXCLUSIVELY by
/// an explicit `(set @entity.X Y)` effect. If the schema author binds
/// `@entity.X` in render-ui / a guard / an effect but no transition runs
/// `(set)` for that field, the value is undefined at runtime and the
/// component renders empty. This validator surfaces that defect at parse
/// time so authors see a clear error instead of debugging an empty UI.
///
/// `@intrinsic` fields are exempt: they are runtime-managed widget state the
/// atom computes itself (e.g. a calendar's `selectedRange`), never authored
/// data, so a read with no `(set)` is correct, not a defect. The exemption is
/// keyed on the resolved linked entity's `intrinsic` flag, so it holds through a
/// `linkedEntity` rebind (the intrinsic field rides along via `extends`) — not
/// only for a directly-declared inline entity.
///
/// Skipped when the trait has no `linkedEntity` (no entity to bind to).
///
/// `shared_group_set_fields` makes the check group-aware for `[shared]`
/// entities: a keyed entry is the UNION of `(set @entity.X ...)` fields
/// across every trait in the orbital bound to that entity (see
/// `collect_shared_group_set_fields`). A field read by this trait is
/// satisfied if EITHER this trait sets it OR any trait in the shared group
/// does. Non-shared entities have no entry in the map, so the OR'd condition
/// is always `false` and behavior is byte-identical to the old per-trait-only
/// check.
fn validate_entity_fields_have_setters(
    trait_def: &TraitDefinition,
    path: &str,
    intrinsic_fields: &HashSet<String>,
    shared_group_set_fields: &HashMap<String, HashSet<String>>,
    embed_set_credit: Option<&HashSet<String>>,
) -> ValidationResult {
    let mut result = ValidationResult::new();
    if trait_def.linked_entity.is_none() {
        return result;
    }
    let trait_linked_entity = trait_def
        .linked_entity
        .clone()
        .unwrap_or_else(|| "?".to_string());

    let mut referenced_fields: HashMap<String, String> = HashMap::new();
    let mut set_fields: HashSet<String> = HashSet::new();

    for transition in trait_def.state_machine.transitions.iter() {
        let event = transition.event.as_str();
        if let Some(guard) = &transition.guard {
            guard.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, event);
        }
        if let Some(effects) = &transition.effects {
            for effect in effects {
                effect.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, event);
            }
        }
    }
    if let Some(ticks) = &trait_def.ticks {
        for tick in ticks {
            for effect in &tick.effects {
                effect.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, "TICK");
            }
        }
    }

    let group_set_fields = trait_def
        .linked_entity
        .as_ref()
        .and_then(|linked| shared_group_set_fields.get(linked));

    // `(persist update|delete X @entity)` hands the WHOLE row to the effect, so
    // the row key is never a `@entity.id` read and the loop below cannot see
    // it. Credit it as a read on the transition that writes.
    if !set_fields.contains("id") {
        if let Some(event) = persist_row_write_event(trait_def) {
            let group_credits_id = group_set_fields
                .map(|group| group.contains("id"))
                .unwrap_or(false);
            let embed_credits_id = embed_set_credit
                .map(|credit| credit.contains("id"))
                .unwrap_or(false);
            if !group_credits_id && !embed_credits_id {
                result.add_error(
                    ValidationError::phase_f_warning(
                        ErrorCode::BindingPersistRowIdNeverSet,
                        path,
                        format!(
                            "trait {:?} runs (persist update|delete {} @entity) on event {:?} \
                             but no transition runs (set @entity.id <value>); the whole row is \
                             passed, so the write lands on the row keyed by the empty string",
                            trait_def.name, trait_linked_entity, event
                        ),
                    )
                    .with_suggestion(format!(
                        "Bind the row key before the write — (set @entity.id ?row.id) on the \
                         transition that selects the {} row, or ?data.id on the fetch success \
                         that loaded it",
                        trait_linked_entity
                    )),
                );
            }
        }
    }

    for (field, first_event) in referenced_fields.iter() {
        if intrinsic_fields.contains(field) {
            continue;
        }
        let satisfied_by_group = group_set_fields
            .map(|group| group.contains(field))
            .unwrap_or(false);
        // A field a rendering parent set in the same render context (reached via
        // the `@trait.<Child>` embed chain, same entity) satisfies the read.
        let satisfied_by_embed = embed_set_credit
            .map(|credit| credit.contains(field))
            .unwrap_or(false);
        if !set_fields.contains(field) && !satisfied_by_group && !satisfied_by_embed {
            // Staged as a warning during the rollout so existing atoms in
            // `@almadar/std` keep loading. Graduates to an error after the
            // L4 std audit completes (every render-ui that binds `@entity.X`
            // gets a corresponding `(set @entity.X <value>)` in INIT or a
            // success-listener transition). The L1 lolo
            // `LowerError::EntityFieldNeverSet` is already a hard error for
            // new authoring.
            result.add_error(
                ValidationError::phase_f_warning(
                    ErrorCode::BindingEntityFieldNeverSet,
                    path,
                    format!(
                        "trait {:?} reads @entity.{} (first seen on event {:?}) but no \
                         transition runs (set @entity.{} <value>); per the explicit-binding \
                         rule, every @entity.X must be set explicitly or it stays undefined",
                        trait_def.name, field, first_event, field
                    ),
                )
                .with_suggestion(format!(
                    "Add (set @entity.{} <default-or-payload>) in the trait's INIT \
                     transition or in a transition that runs before the read on entity {:?}",
                    field, trait_linked_entity
                )),
            );
        }
    }

    result
}

/// The first event whose effects run `(persist update|delete <Entity> @entity)`
/// — a row-keyed write that carries the id implicitly. `create` is excluded:
/// the store mints the key.
fn persist_row_write_event(trait_def: &TraitDefinition) -> Option<String> {
    for transition in trait_def.state_machine.transitions.iter() {
        let Some(effects) = transition.effects.as_ref() else {
            continue;
        };
        for effect in effects {
            if expr_persists_whole_row(effect) {
                return Some(transition.event.clone());
            }
        }
    }
    None
}

fn expr_persists_whole_row(expr: &SExpression) -> bool {
    let SExpression::Array(items) = expr else {
        return false;
    };
    let is_row_write = matches!(items.first(), Some(SExpression::String(op)) if op == "persist")
        && matches!(items.get(1), Some(SExpression::String(action)) if action == "update" || action == "delete")
        && matches!(items.get(3), Some(SExpression::String(data)) if data == "@entity");
    is_row_write || items.iter().any(expr_persists_whole_row)
}

/// Check if an operator introduces an implicit @item/@index lambda scope.
/// Uses the embedded operator registry (Phase 3 of Lolo v3): collection and
/// std-array operators with arity >= 2 (i.e., they accept a callback/predicate argument).
/// Also handles unnamespaced aliases (e.g., "map" checks "array/map" too).
fn is_lambda_scope_operator(op: &str) -> bool {
    // Replaced canonical_operators lookup with embedded::get_operator() in Phase 3 of Lolo v3 plan.
    if let Some(meta) = crate::embedded::get_operator(op) {
        return (meta.category == "collection" || meta.category == "std-array")
            && meta.min_arity >= 2;
    }
    // Legacy fallback for operators not yet in operators.json
    if let Some(meta) = get_canonical_operator(op) {
        return (meta.category == "collection" || meta.category == "std-array")
            && meta.min_arity >= 2;
    }
    // Check with array/ prefix for unnamespaced names (e.g., "map" -> "array/map")
    if !op.contains('/') {
        let namespaced = format!("array/{}", op);
        if let Some(meta) = crate::embedded::get_operator(&namespaced) {
            return meta.min_arity >= 2;
        }
        if let Some(meta) = get_canonical_operator(&namespaced) {
            return meta.min_arity >= 2;
        }
    }
    false
}

// ============================================================================
// `@trait.X` binding validation (client-only render-time binding)
// ============================================================================

/// Run a dedicated pass over a trait's expressions looking for `@trait.X`
/// bindings and validating each against the orbital-scoped trait registry.
///
/// The binding is strictly single-segment — `@trait.X`. Any additional
/// segment (historically a `.slot` suffix) is rejected as invalid format.
///
/// Checks:
/// - `@trait` bare (no name) → [`ErrorCode::BindingTraitMissingName`].
/// - `@trait.foo` (non-PascalCase) → [`ErrorCode::BindingTraitInvalidFormat`].
/// - `@trait.Foo.anything` (extra segment) → [`ErrorCode::BindingTraitInvalidFormat`].
/// - `@trait.Foo` where `Foo` is not in `known_traits` →
///   [`ErrorCode::BindingTraitUnknown`].
/// - `@trait.<self>` (current trait references itself) →
///   [`ErrorCode::BindingTraitSelfReference`].
/// - `@trait.X` outside a `render-ui` effect (in a guard, a non-render
///   effect, or a tick) → [`ErrorCode::BindingTraitOutsideRenderUi`].
///   `@trait.X` is a render-only, client-side composition primitive with no
///   meaning in data/effects/control flow.
pub fn validate_trait_binding_usage(
    trait_def: &TraitDefinition,
    path: &str,
    known_traits: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Walk transitions (guards + effects) and ticks, collecting every
    // `@trait.*` binding string. `@trait.X` is a render-only binding: it's
    // permitted only inside a `render-ui` effect's pattern tree. In a guard,
    // a non-render effect, or a tick it is meaningless and rejected here.
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        let trans_path = format!("{}.stateMachine.transitions[{}]", path, i);
        if let Some(guard) = &transition.guard {
            collect_and_validate_trait_bindings(
                guard,
                &format!("{}.guard", trans_path),
                &trait_def.name,
                known_traits,
                &mut result,
                /* allow_trait_refs */ false,
            );
        }
        if let Some(effects) = &transition.effects {
            for (j, effect) in effects.iter().enumerate() {
                let is_render = is_render_ui_effect(effect);
                collect_and_validate_trait_bindings(
                    effect,
                    &format!("{}.effects[{}]", trans_path, j),
                    &trait_def.name,
                    known_traits,
                    &mut result,
                    is_render,
                );
            }
        }
    }

    if let Some(ticks) = &trait_def.ticks {
        for (i, tick) in ticks.iter().enumerate() {
            if let Some(guard) = &tick.guard {
                collect_and_validate_trait_bindings(
                    guard,
                    &format!("{}.ticks[{}].guard", path, i),
                    &trait_def.name,
                    known_traits,
                    &mut result,
                    /* allow_trait_refs */ false,
                );
            }
            for (j, effect) in tick.effects.iter().enumerate() {
                collect_and_validate_trait_bindings(
                    effect,
                    &format!("{}.ticks[{}].effects[{}]", path, i, j),
                    &trait_def.name,
                    known_traits,
                    &mut result,
                    /* allow_trait_refs */ false,
                );
            }
        }
    }

    result
}

/// Walk an SExpression tree and validate every `@trait.*` string binding.
/// Non-binding strings and non-`@trait` bindings are skipped (they're
/// handled by the general `validate_binding` pass).
///
/// `allow_trait_refs` controls the *position* check: when `false`, any
/// `@trait.X` is rejected as appearing outside a `render-ui` effect (in a
/// guard, a non-render effect, or a tick). The caller sets it `true` only for
/// `render-ui` effects, where the finer-grained intra-pattern position check
/// (`validate_trait_binding_positions`) takes over.
fn collect_and_validate_trait_bindings(
    expr: &SExpression,
    path: &str,
    current_trait_name: &str,
    known_traits: &HashSet<String>,
    result: &mut ValidationResult,
    allow_trait_refs: bool,
) {
    match expr {
        SExpression::String(s) if s.starts_with("@trait") && s.len() >= 6 => {
            if !allow_trait_refs {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::BindingTraitOutsideRenderUi,
                        path,
                        format!(
                            "@trait binding '{}' appears outside a render-ui effect; \
                             @trait references are render-only composition primitives \
                             and may only be used inside a (render-ui ...) pattern tree",
                            s
                        ),
                    )
                    .with_suggestion(
                        "Move the @trait reference into a render-ui children array, \
                         or replace it with the intended data/payload binding",
                    ),
                );
                return;
            }
            // Accept the literal `@trait` or `@trait.X[.slot]`; other strings
            // starting with `@trait...` that don't match the binding regex
            // fall through to the format error below.
            result.merge(validate_trait_binding(
                s,
                path,
                current_trait_name,
                known_traits,
            ));
        }
        SExpression::Array(arr) => {
            // A `render-ui` effect that opens anywhere in the tree — at the
            // top level OR nested inside a conditional/case branch (e.g. a
            // browse atom's `browseLook` switch over multiple body trees) —
            // is a valid `@trait` context: its pattern tree composes trait
            // references. The same is true for `render-ui`-typed config
            // field defaults, which the inline phase substitutes into such
            // branches. So once we descend into a `render-ui` array, allow
            // `@trait` refs for the whole subtree regardless of the
            // inherited flag.
            let child_allow = allow_trait_refs || is_render_ui_effect(expr);
            for (i, item) in arr.iter().enumerate() {
                collect_and_validate_trait_bindings(
                    item,
                    &format!("{}[{}]", path, i),
                    current_trait_name,
                    known_traits,
                    result,
                    child_allow,
                );
            }
        }
        SExpression::Object(obj) => {
            for (k, v) in obj {
                collect_and_validate_trait_bindings(
                    v,
                    &format!("{}.{}", path, k),
                    current_trait_name,
                    known_traits,
                    result,
                    allow_trait_refs,
                );
            }
        }
        _ => {}
    }
}

/// Validate a single `@trait.X[.slot]` binding string.
fn validate_trait_binding(
    binding: &str,
    path: &str,
    current_trait_name: &str,
    known_traits: &HashSet<String>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !BINDING_REGEX.is_match(binding) {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingTraitInvalidFormat,
                path,
                format!("Invalid @trait binding format: '{}'", binding),
            )
            .with_suggestion("Use @trait.<PascalCaseTraitName>[.<slot>]"),
        );
        return result;
    }

    // strip leading `@` then split on `.`
    let parts: Vec<&str> = binding[1..].split('.').collect();

    // parts[0] must be "trait" — validator is only called for those.
    if parts.len() == 1 {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingTraitMissingName,
                path,
                "@trait binding missing trait name",
            )
            .with_suggestion("Use @trait.<TraitName> to reference another trait's current frame"),
        );
        return result;
    }

    let trait_name = parts[1];

    if !PASCAL_CASE_REGEX.is_match(trait_name) {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingTraitInvalidFormat,
                path,
                format!(
                    "@trait binding trait name '{}' is not PascalCase",
                    trait_name
                ),
            )
            .with_suggestion("Trait names must be PascalCase (e.g., @trait.ItemSearch)"),
        );
        return result;
    }

    if trait_name == current_trait_name {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitSelfReference,
            path,
            format!("Trait '{}' references itself via @trait", trait_name),
        )
        .with_suggestion(
            "A trait's render-ui cannot embed its own frame; restructure to avoid self-reference",
        ));
        return result;
    }

    if !known_traits.is_empty() && !known_traits.contains(trait_name) {
        result.add_error(
            ValidationError::error(
                ErrorCode::BindingTraitUnknown,
                path,
                format!(
                    "@trait.{} references a trait not declared in this orbital",
                    trait_name
                ),
            )
            .with_suggestion("Declare the trait in the orbital, or check spelling")
            .with_valid_values(known_traits.iter().cloned().collect()),
        );
        return result;
    }

    // The binding is single-segment — `@trait.<Name>`. Any additional
    // segment (historically a `.slot` suffix) is rejected here. The
    // decision to drop slot-specific routing is documented in
    // `docs/Almadar_Std_Gaps.md` §3.8 — cross-slot pulling would force
    // compiled apps to extract per-slot pattern trees, lift hooks into
    // a shared context, and rewrite event dispatch, for a niche case.
    if parts.len() > 2 {
        result.add_error(ValidationError::error(
            ErrorCode::BindingTraitInvalidFormat,
            path,
            format!(
                "@trait binding has too many segments: '{}' (expected @trait.<Name>)",
                binding
            ),
        ));
    }

    result
}

// ============================================================================
// Cycle detection in the `@trait.X` reference graph
// ============================================================================

/// Detect cycles among `@trait.X` references within a single orbital. Each
/// trait's render-ui effects are scanned for outgoing references; a DFS
/// cycle detector reports the first cycle found. Scope is per-orbital —
/// cross-orbital references are not currently supported by the binding.
pub fn detect_trait_binding_cycles(
    traits: &[TraitReference],
    orbital_path: &str,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Build adjacency list: trait name -> set of trait names referenced via
    // `@trait.X` inside that trait's effects.
    let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();
    let mut trait_indices: HashMap<String, usize> = HashMap::new();
    for (i, t) in traits.iter().enumerate() {
        if let TraitReference::Inline(def) = t {
            adjacency.insert(def.name.clone(), HashSet::new());
            trait_indices.insert(def.name.clone(), i);
        }
    }

    for t in traits.iter() {
        if let TraitReference::Inline(def) = t {
            let mut refs: HashSet<String> = HashSet::new();
            for transition in &def.state_machine.transitions {
                if let Some(effects) = &transition.effects {
                    for effect in effects {
                        collect_trait_refs(effect, &mut refs);
                    }
                }
            }
            if let Some(ticks) = &def.ticks {
                for tick in ticks {
                    for effect in &tick.effects {
                        collect_trait_refs(effect, &mut refs);
                    }
                }
            }
            if let Some(entry) = adjacency.get_mut(&def.name) {
                *entry = refs;
            }
        }
    }

    // DFS cycle detection. White=unvisited, Gray=on-stack, Black=finished.
    #[derive(PartialEq, Copy, Clone)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let mut color: HashMap<&str, Color> = adjacency
        .keys()
        .map(|k| (k.as_str(), Color::White))
        .collect();
    let mut parent: HashMap<&str, &str> = HashMap::new();

    let nodes: Vec<&str> = adjacency.keys().map(|k| k.as_str()).collect();

    for start in nodes {
        if color.get(start).copied() != Some(Color::White) {
            continue;
        }
        let mut stack: Vec<(&str, std::collections::hash_set::Iter<String>)> =
            vec![(start, adjacency[start].iter())];
        color.insert(start, Color::Gray);

        while let Some((node, mut children_iter)) = stack.pop() {
            if let Some(child) = children_iter.next() {
                // push the iterator back before descending so we can resume
                stack.push((node, children_iter));
                let child_str = child.as_str();
                match color.get(child_str).copied() {
                    Some(Color::White) => {
                        parent.insert(child_str, node);
                        color.insert(child_str, Color::Gray);
                        if let Some(edges) = adjacency.get(child_str) {
                            stack.push((child_str, edges.iter()));
                        }
                    }
                    Some(Color::Gray) => {
                        // Cycle found — reconstruct path child → ... → child.
                        let mut cycle_path: Vec<&str> = vec![child_str];
                        let mut cur = node;
                        while cur != child_str {
                            cycle_path.push(cur);
                            match parent.get(cur).copied() {
                                Some(p) => cur = p,
                                None => break,
                            }
                        }
                        cycle_path.push(child_str);
                        cycle_path.reverse();
                        let idx = trait_indices.get(child_str).copied().unwrap_or(0);
                        result.add_error(
                            ValidationError::error(
                                ErrorCode::BindingTraitCycle,
                                format!("{}.traits[{} ({})]", orbital_path, idx, child_str),
                                format!(
                                    "@trait reference cycle detected: {}",
                                    cycle_path.join(" -> ")
                                ),
                            )
                            .with_suggestion(
                                "Break the cycle by removing one of the @trait.X references",
                            ),
                        );
                        return result;
                    }
                    _ => {}
                }
            } else {
                color.insert(node, Color::Black);
            }
        }
    }

    result
}

/// Recursively collect trait names from every `@trait.X` string appearing
/// anywhere in an SExpression tree.
fn collect_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) if s.starts_with("@trait.") => {
            if let Some(rest) = s.strip_prefix("@trait.") {
                if let Some(name) = rest.split('.').next() {
                    if !name.is_empty() {
                        out.insert(name.to_string());
                    }
                }
            }
        }
        SExpression::Array(arr) => {
            for item in arr {
                collect_trait_refs(item, out);
            }
        }
        SExpression::Object(obj) => {
            for v in obj.values() {
                collect_trait_refs(v, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_bindings() {
        let known_entities: HashSet<String> =
            ["Task", "User"].iter().map(|s| s.to_string()).collect();

        let result = validate_binding(
            "@entity.title",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result.is_valid());

        let result = validate_binding(
            "@payload.data",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result.is_valid());

        let result = validate_binding(
            "@Task.title",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result.is_valid());
    }

    #[test]
    fn test_payload_in_tick() {
        let result = validate_binding(
            "@payload.data",
            "test",
            BindingContext::Tick,
            &HashSet::new(),
        );
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadInTick));
    }

    #[test]
    fn test_state_with_path() {
        let result = validate_binding(
            "@state.name",
            "test",
            BindingContext::Effect,
            &HashSet::new(),
        );
        // Warning, not error
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingStateNoPath));
    }

    #[test]
    fn test_unknown_entity() {
        let known_entities: HashSet<String> = ["Task"].iter().map(|s| s.to_string()).collect();
        let result = validate_binding(
            "@NonExistent.field",
            "test",
            BindingContext::Effect,
            &known_entities,
        );
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingUnknownEntity));
    }

    #[test]
    fn test_set_target_payload() {
        let result = validate_set_target("@payload.field", "test");
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::EffectSetPayloadNotAllowed));
    }

    #[test]
    fn test_set_target_missing_path() {
        let result = validate_set_target("@entity", "test");
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingSetTargetMissingPath));
    }

    #[test]
    fn test_payload_field_undeclared_no_schema() {
        let expr = SExpression::String("@payload.rawData".to_string());
        let result = validate_payload_field_references(&expr, "test", None, "READ");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared));
    }

    #[test]
    fn test_payload_field_undeclared_missing_field() {
        let expr = SExpression::String("@payload.unknownField".to_string());
        let fields = vec![PayloadField {
            name: "rawData".to_string(),
            field_type: "array".to_string(),
            required: Some(true),
            description: None,
            properties: None,
        }];
        let result = validate_payload_field_references(&expr, "test", Some(&fields), "READ");
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared));
    }

    #[test]
    fn test_payload_field_declared() {
        let expr = SExpression::String("@payload.rawData".to_string());
        let fields = vec![PayloadField {
            name: "rawData".to_string(),
            field_type: "array".to_string(),
            required: Some(true),
            description: None,
            properties: None,
        }];
        let result = validate_payload_field_references(&expr, "test", Some(&fields), "READ");
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_payload_sigil_hybrid_gets_corrective_suggestion() {
        // `?payload.date` lowers to `@payload.payload.date` — the suggestion
        // must name the sigil mistake, not "add a field named payload".
        let expr = SExpression::String("@payload.payload.date".to_string());
        let fields = vec![PayloadField {
            name: "date".to_string(),
            field_type: "string".to_string(),
            required: Some(true),
            description: None,
            properties: None,
        }];
        let result = validate_payload_field_references(&expr, "test", Some(&fields), "SLOT_CLICK");
        let err = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared)
            .expect("hybrid path must still error");
        let suggestion = err.suggestion.as_deref().unwrap_or("");
        assert!(suggestion.contains("`?date`"), "got suggestion: {suggestion}");
        assert!(suggestion.contains("`@payload.date`"), "got suggestion: {suggestion}");
    }

    #[test]
    fn test_payload_field_literally_named_payload_keeps_default_suggestion() {
        // A schema that DOES declare a field named `payload` is not the hybrid
        // mistake — deeper misses under it keep the generic suggestion.
        let expr = SExpression::String("@payload.payload.missing".to_string());
        let fields = vec![PayloadField {
            name: "payload".to_string(),
            field_type: "object".to_string(),
            required: Some(true),
            description: None,
            properties: Some(vec![PayloadField {
                name: "present".to_string(),
                field_type: "string".to_string(),
                required: Some(true),
                description: None,
                properties: None,
            }]),
        }];
        let result = validate_payload_field_references(&expr, "test", Some(&fields), "READ");
        let err = result
            .errors
            .iter()
            .find(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared)
            .expect("missing nested field must error");
        let suggestion = err.suggestion.as_deref().unwrap_or("");
        assert!(
            !suggestion.contains("sigil"),
            "legit `payload`-named field must not get the hybrid hint: {suggestion}"
        );
    }

    #[test]
    fn test_lambda_bound_item_is_valid() {
        let known_entities: HashSet<String> = HashSet::new();
        // ["lambda", ["item"], ["=", "@item.status", "pending"]]
        let expr = SExpression::Array(vec![
            SExpression::String("lambda".to_string()),
            SExpression::Array(vec![SExpression::String("item".to_string())]),
            SExpression::Array(vec![
                SExpression::String("=".to_string()),
                SExpression::String("@item.status".to_string()),
                SExpression::String("pending".to_string()),
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        // @item inside lambda should NOT produce BindingUnknownRoot
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Lambda-bound @item should not trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn fc5_item_in_scope_inside_array_map_children() {
        let known_entities: HashSet<String> = HashSet::new();
        // ["array/map", "@entity.items", ["fn", "item", { type: badge, label: "@item.name" }]]
        let node = SExpression::Object(
            [
                ("type".to_string(), SExpression::String("badge".to_string())),
                (
                    "label".to_string(),
                    SExpression::String("@item.name".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let expr = SExpression::Array(vec![
            SExpression::String("array/map".to_string()),
            SExpression::String("@entity.items".to_string()),
            SExpression::Array(vec![
                SExpression::String("fn".to_string()),
                SExpression::String("item".to_string()),
                node,
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "@item inside an array/map lambda body must be in scope. Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn fc5_item_out_of_scope_outside_array_map() {
        let known_entities: HashSet<String> = HashSet::new();
        // A bare `@item.name` with no enclosing map/lambda → unknown root.
        let expr = SExpression::Object(
            [
                ("type".to_string(), SExpression::String("badge".to_string())),
                (
                    "label".to_string(),
                    SExpression::String("@item.name".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "@item outside any lambda must be out of scope (unknown root). Errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_fn_string_param_lambda_is_valid() {
        let known_entities: HashSet<String> = HashSet::new();
        // ["fn", "h", ["=", "@h.owner", "player"]]
        // String param shorthand (not wrapped in array)
        let expr = SExpression::Array(vec![
            SExpression::String("fn".to_string()),
            SExpression::String("h".to_string()),
            SExpression::Array(vec![
                SExpression::String("=".to_string()),
                SExpression::String("@h.owner".to_string()),
                SExpression::String("player".to_string()),
            ]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Lambda-bound @h with string param should not trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn test_entity_field_not_found() {
        use orbital_core::schema::{FieldDefinition, FieldType};

        let entity_def = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "Task".to_string(),
            fields: vec![
                FieldDefinition {
                    name: Some("title".to_string()),
                    field_type: FieldType::String,
                    required: None,
                    primary_key: None,
                    unique: None,
                    relation: None,
                    items: None,
                    values: None,
                    default: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                },
                FieldDefinition {
                    name: Some("status".to_string()),
                    field_type: FieldType::String,
                    required: None,
                    primary_key: None,
                    unique: None,
                    relation: None,
                    items: None,
                    values: None,
                    default: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                },
            ],
            collection: None,
            persistence: None,
            instances: None,
            id: None,
        };

        let entity_defs = HashMap::new();

        // @entity.title should pass (field exists)
        let expr = SExpression::String("@entity.title".to_string());
        let result =
            validate_entity_field_references(&expr, "test", Some(&entity_def), &entity_defs);
        assert!(
            result.errors.is_empty(),
            "Valid field should not produce warnings: {:?}",
            result.errors
        );

        // @entity.nonExistent should error
        let expr = SExpression::String("@entity.nonExistent".to_string());
        let result =
            validate_entity_field_references(&expr, "test", Some(&entity_def), &entity_defs);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNotFound),
            "Missing field should produce BindingEntityFieldNotFound error"
        );
    }

    #[test]
    fn test_entity_field_id_skipped() {
        use orbital_core::schema::{FieldDefinition, FieldType};

        let entity_def = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "Task".to_string(),
            fields: vec![FieldDefinition {
                name: Some("title".to_string()),
                field_type: FieldType::String,
                required: None,
                primary_key: None,
                unique: None,
                relation: None,
                items: None,
                values: None,
                default: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            }],
            collection: None,
            persistence: None,
            instances: None,
            id: None,
        };

        let entity_defs = HashMap::new();

        // @entity.id should pass (meta-field, always present at runtime)
        let expr = SExpression::String("@entity.id".to_string());
        let result =
            validate_entity_field_references(&expr, "test", Some(&entity_def), &entity_defs);
        assert!(result.errors.is_empty(), "id should be skipped");
    }

    #[test]
    fn test_named_entity_field_validation() {
        use orbital_core::schema::{FieldDefinition, FieldType};

        let entity_def = EntityDefinition {
            shared: false,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: "User".to_string(),
            fields: vec![FieldDefinition {
                name: Some("email".to_string()),
                field_type: FieldType::String,
                required: None,
                primary_key: None,
                unique: None,
                relation: None,
                items: None,
                values: None,
                default: None,
                properties: None,
                intrinsic: None,
                description: None,
                synonyms: None,
            }],
            collection: None,
            persistence: None,
            instances: None,
            id: None,
        };

        let mut entity_defs = HashMap::new();
        entity_defs.insert("User".to_string(), entity_def);

        // @User.email should pass
        let expr = SExpression::String("@User.email".to_string());
        let result = validate_entity_field_references(&expr, "test", None, &entity_defs);
        assert!(result.errors.is_empty());

        // @User.phone should error
        let expr = SExpression::String("@User.phone".to_string());
        let result = validate_entity_field_references(&expr, "test", None, &entity_defs);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ErrorCode::BindingEntityFieldNotFound),);
    }

    #[test]
    fn test_non_lambda_item_is_invalid() {
        let known_entities: HashSet<String> = HashSet::new();
        // @item.status outside a lambda should still warn
        let expr = SExpression::String("@item.status".to_string());
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "Non-lambda @item should trigger BindingUnknownRoot"
        );
    }

    #[test]
    fn test_sequential_let_binding_sees_prior_siblings() {
        let known_entities: HashSet<String> = HashSet::new();
        // (let ((a @entity.n) (b @a)) (set @entity.n @b))
        // `@a` in b's value and `@b` in the body must resolve to the earlier
        // bindings — sequential `let`, matching both runtimes.
        let expr = arr(vec![
            s("let"),
            arr(vec![
                arr(vec![s("a"), s("@entity.n")]),
                arr(vec![s("b"), s("@a")]),
            ]),
            arr(vec![s("set"), s("@entity.n"), s("@b")]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "sequential let: a later binding must see earlier siblings; got {:?}",
            result.errors
        );
    }

    #[test]
    fn test_let_self_reference_still_errors() {
        let known_entities: HashSet<String> = HashSet::new();
        // (let ((a @a)) (set @entity.n @a)) — `a` references itself before it is
        // bound; sequential scope must still reject it.
        let expr = arr(vec![
            s("let"),
            arr(vec![arr(vec![s("a"), s("@a")])]),
            arr(vec![s("set"), s("@entity.n"), s("@a")]),
        ]);
        let result = validate_expr_bindings(&expr, "test", BindingContext::Effect, &known_entities);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingUnknownRoot),
            "self-referencing let binding @a must still error"
        );
    }

    // ==========================================================================
    // Explicit-binding rule tests (BindingEntityFieldNeverSet)
    // ==========================================================================

    /// Build a minimal trait fixture for the entity-fields-have-setters tests.
    /// Each transition is `(idle -> idle on EVENT, [effects])`. Linked entity
    /// is required for the validator to run.
    fn build_trait_fixture(
        name: &str,
        linked_entity: Option<&str>,
        transitions: Vec<(&str, Vec<SExpression>)>,
    ) -> orbital_core::schema::TraitDefinition {
        use orbital_core::schema::{
            StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
        };
        let trans = transitions
            .into_iter()
            .map(|(event, effects)| TransitionDefinition {
                event_id: None,
                from: StringOrArray::Single("idle".to_string()),
                to: "idle".to_string(),
                event: event.to_string(),
                guard: None,
                effects: if effects.is_empty() {
                    None
                } else {
                    Some(effects)
                },
            })
            .collect();
        orbital_core::schema::TraitDefinition {
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: vec![],
            description: None,
            linked_entity: linked_entity.map(|s| s.to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "idle".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                    time_bound_ms: None,
                }],
                events: None,
                transitions: trans,
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    fn s(v: &str) -> SExpression {
        SExpression::String(v.to_string())
    }
    fn arr(items: Vec<SExpression>) -> SExpression {
        SExpression::Array(items)
    }

    #[test]
    fn test_entity_field_never_set_render_ui_no_setter() {
        // (render-ui main { currentPage: "@entity.currentPage" })
        // No `(set @entity.currentPage ...)` anywhere — must fail.
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "BadPagination",
            Some("PagedItem"),
            vec![("INIT", vec![render_ui])],
        );
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNeverSet),
            "Reading @entity.currentPage with no (set) must produce BindingEntityFieldNeverSet — got {:?}",
            result.errors,
        );
    }

    /// POSITIVE CONTROL: `(persist update X @entity)` hands over the whole row,
    /// so `id` is never an `@entity.id` read and the sibling never-set check is
    /// structurally blind to it. Without this the write lands on row "".
    #[test]
    fn test_persist_update_whole_row_without_id_setter_warns() {
        let persist = arr(vec![s("persist"), s("update"), s("Ticket"), s("@entity")]);
        let trait_def =
            build_trait_fixture("SaveTicket", Some("Ticket"), vec![("SAVE", vec![persist])]);
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingPersistRowIdNeverSet),
            "(persist update Ticket @entity) with no (set @entity.id) must warn — got {:?}",
            result.warnings,
        );
    }

    /// NEGATIVE CONTROL: the row key IS bound, so there is nothing to report.
    #[test]
    fn test_persist_update_with_id_setter_is_silent() {
        let set_id = arr(vec![s("set"), s("@entity.id"), s("?row.id")]);
        let persist = arr(vec![s("persist"), s("update"), s("Ticket"), s("@entity")]);
        let trait_def = build_trait_fixture(
            "SaveTicket",
            Some("Ticket"),
            vec![("SELECT", vec![set_id]), ("SAVE", vec![persist])],
        );
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingPersistRowIdNeverSet),
            "A bound row key must not warn — got {:?}",
            result.warnings,
        );
    }

    /// `create` mints its own key — it must never be reported.
    #[test]
    fn test_persist_create_whole_row_is_not_a_row_keyed_write() {
        let persist = arr(vec![s("persist"), s("create"), s("Ticket"), s("@entity")]);
        let trait_def =
            build_trait_fixture("NewTicket", Some("Ticket"), vec![("SAVE", vec![persist])]);
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingPersistRowIdNeverSet),
            "(persist create …) must not warn — got {:?}",
            result.warnings,
        );
    }

    #[test]
    fn test_intrinsic_entity_field_read_with_no_setter_is_exempt() {
        // (render-ui main { range: "@entity.selectedRange" }) with no setter —
        // would normally warn, but `selectedRange` is intrinsic (runtime-managed
        // widget state), so it must be exempt. Mirrors a calendar atom rebound
        // onto a domain entity that carries `selectedRange` as an intrinsic field.
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("range".to_string(), s("@entity.selectedRange"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "CalendarView",
            Some("Appointment"),
            vec![("INIT", vec![render_ui])],
        );
        let intrinsic: HashSet<String> = ["selectedRange".to_string()].into_iter().collect();
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &intrinsic,
            &HashMap::new(),
            None,
        );
        assert!(
            result.warnings.is_empty() && result.errors.is_empty(),
            "Intrinsic @entity.selectedRange with no (set) must be exempt — got {:?}",
            result.warnings,
        );
    }

    #[test]
    fn test_entity_field_set_explicitly_passes() {
        // INIT runs (set @entity.currentPage 1) then render-ui binds it. Pass.
        let set_effect = arr(vec![
            s("set"),
            s("@entity.currentPage"),
            SExpression::Number(1.into()),
        ]);
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "GoodPagination",
            Some("PagedItem"),
            vec![("INIT", vec![set_effect, render_ui])],
        );
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result.errors.is_empty(),
            "Field with explicit (set) must pass — errors: {:?}",
            result.errors,
        );
    }

    #[test]
    fn test_entity_field_set_in_different_transition_passes() {
        // INIT (set @entity.currentPage 1); render-ui happens in PAGE transition.
        // The setter doesn't have to be in the same transition as the read.
        let set_effect = arr(vec![
            s("set"),
            s("@entity.currentPage"),
            SExpression::Number(1.into()),
        ]);
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "CrossTransitionPagination",
            Some("PagedItem"),
            vec![("INIT", vec![set_effect]), ("PAGE", vec![render_ui])],
        );
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result.errors.is_empty(),
            "Setter in different transition still satisfies the rule — errors: {:?}",
            result.errors,
        );
    }

    #[test]
    fn test_entity_field_id_meta_skipped() {
        // @entity.id is a runtime-meta field. No (set @entity.id) needed.
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("entityId".to_string(), s("@entity.id"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "MetaIdReader",
            Some("Item"),
            vec![("INIT", vec![render_ui])],
        );
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result.errors.is_empty(),
            "@entity.id is a runtime-meta field; no setter required",
        );
    }

    #[test]
    fn test_entity_field_skipped_when_no_linked_entity() {
        // Trait without linkedEntity — the rule is skipped (the @entity.* reads
        // are someone else's problem; could be a render-only trait).
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("anything".to_string(), s("@entity.anything"));
                m
            }),
        ]);
        let trait_def =
            build_trait_fixture("LinkedEntityless", None, vec![("INIT", vec![render_ui])]);
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result.errors.is_empty(),
            "Trait without linkedEntity should skip the rule",
        );
    }

    #[test]
    fn test_entity_field_set_via_payload_passes() {
        // (set @entity.currentPage @payload.page) — the value source is a
        // payload reference, not a literal, but the (set) call still counts
        // as the explicit setter.
        let set_effect = arr(vec![s("set"), s("@entity.currentPage"), s("@payload.page")]);
        let render_ui = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("currentPage".to_string(), s("@entity.currentPage"));
                m
            }),
        ]);
        let trait_def = build_trait_fixture(
            "PayloadDrivenPagination",
            Some("PagedItem"),
            vec![("PAGE", vec![set_effect, render_ui])],
        );
        let result = validate_entity_fields_have_setters(
            &trait_def,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result.errors.is_empty(),
            "(set @entity.X @payload.X) is a valid setter — errors: {:?}",
            result.errors,
        );
    }

    #[test]
    fn test_entity_field_only_in_guard_no_setter_fails() {
        // Guard reads @entity.locked but no transition (set)s it. Must fail.
        let guard_only_trait = {
            use orbital_core::schema::{
                StateDefinition, StateMachine, StringOrArray, TransitionDefinition,
            };
            orbital_core::schema::TraitDefinition {
                name: "GuardOnlyReader".to_string(),
                entity_rebindable: None,
                entity_contract: None,
                category: None,
                capabilities: vec![],
                description: None,
                linked_entity: Some("Item".to_string()),
                emits: None,
                listens: None,
                state_machine: StateMachine {
                    states: vec![StateDefinition {
                        name: "idle".to_string(),
                        is_initial: Some(true),
                        is_terminal: None,
                        description: None,
                        time_bound_ms: None,
                    }],
                    events: None,
                    transitions: vec![TransitionDefinition {
                        event_id: None,
                        from: StringOrArray::Single("idle".to_string()),
                        to: "idle".to_string(),
                        event: "TRY".to_string(),
                        guard: Some(s("@entity.locked")),
                        effects: None,
                    }],
                },
                ticks: None,
                config: None,
                data_entities: None,
                scope: None,
                source_behavior: None,
                source_entity_definition: None,
                id: None,
                linked_entity_id: None,
                entity_ref_ids: None,
                trait_embed_ids: None,
            }
        };
        let result = validate_entity_fields_have_setters(
            &guard_only_trait,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            None,
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNeverSet),
            "Guard-only @entity.X read with no setter must fail — errors: {:?}",
            result.errors,
        );
    }

    // ==========================================================================
    // Group-aware setter tests (Task C — `[shared]` entities union set_fields
    // across every trait bound to them via `collect_shared_group_set_fields`).
    // ==========================================================================

    fn shared_entity_def(name: &str) -> EntityDefinition {
        EntityDefinition {
            shared: true,
            identity: false,
            read_policy: None,
            create_policy: None,
            update_policy: None,
            delete_policy: None,
            access_waivers: None,
            name: name.to_string(),
            collection: None,
            persistence: None,
            instances: None,
            fields: vec![],
            id: None,
        }
    }

    fn orbital_with_traits(traits: Vec<TraitDefinition>) -> Orbital {
        Orbital {
            name: "TestOrbital".to_string(),
            uses: None,
            expects: None,
            theme: None,
            services: None,
            entity: orbital_core::schema::EntityRef::Inline(shared_entity_def("Unused")),
            auxiliary_entities: None,
            traits: traits.into_iter().map(TraitReference::Inline).collect(),
            pages: None,
            emits: None,
            listens: None,
            exposes: None,
            domain_context: None,
            design: None,
            id: None,
        }
    }

    #[test]
    fn callsite_payload_capture_validates_against_composing_event() {
        use orbital_core::schema::{ConfigField, EventDefinition};

        // Composer trait renders `@trait.Child` on event `Failed` (payload {error}).
        let render = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = HashMap::new();
                m.insert("type".to_string(), s("stack"));
                m.insert("children".to_string(), arr(vec![s("@trait.Child")]));
                m
            }),
        ]);
        let mut composer =
            build_trait_fixture("Composer", Some("E"), vec![("Failed", vec![render])]);
        let ev: EventDefinition = serde_json::from_value(serde_json::json!({
            "key": "Failed",
            "payloadSchema": [{ "name": "error", "type": "string" }]
        }))
        .unwrap();
        composer.state_machine.events = Some(vec![ev]);

        // Child trait carries a call-site payload capture on its config default.
        let make_child = |field: &str| {
            let mut child = build_trait_fixture("Child", Some("E"), vec![]);
            let cfg: HashMap<String, ConfigField> = serde_json::from_value(serde_json::json!({
                "content": { "type": "string", "default": format!("@callsitePayload.{field}") }
            }))
            .unwrap();
            child.config = Some(cfg);
            child
        };

        // Undeclared field on the composing event → error (same code as a direct
        // `@payload.<field>` on that event).
        let orbital = orbital_with_traits(vec![composer.clone(), make_child("bogus")]);
        let (sites, parents_of) = build_embed_index(&orbital);
        let result =
            validate_callsite_payload_captures(&orbital, "orbitals[0]", &sites, &parents_of);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared
                    && e.message.contains("bogus")),
            "undeclared captured field must error against the composing event: {:?}",
            result.errors
        );

        // Field declared on the composing event → no error.
        let orbital_ok = orbital_with_traits(vec![composer, make_child("error")]);
        let (sites_ok, parents_of_ok) = build_embed_index(&orbital_ok);
        let result_ok = validate_callsite_payload_captures(
            &orbital_ok,
            "orbitals[0]",
            &sites_ok,
            &parents_of_ok,
        );
        assert!(
            result_ok.errors.is_empty(),
            "a captured field declared on the composing event must pass: {:?}",
            result_ok.errors
        );
    }

    /// One `{ children: [<refs>] }` render-ui effect. Mirrors the post-inline
    /// shape of a wrapper whose `@config.children` was substituted into its body.
    fn render_children(refs: Vec<&str>) -> SExpression {
        arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = HashMap::new();
                m.insert(
                    "children".to_string(),
                    arr(refs.into_iter().map(s).collect()),
                );
                m
            }),
        ])
    }

    fn callsite_content(field: &str) -> HashMap<String, orbital_core::schema::ConfigField> {
        serde_json::from_value(serde_json::json!({
            "content": { "type": "string", "default": format!("@callsitePayload.{field}") }
        }))
        .unwrap()
    }

    // FIX 2 (C-CALLSITEPAYLOAD-EMBED-HOST): a `@callsitePayload` capture nested
    // under a hoisted render wrapper resolves UP the `@trait` embed chain to the
    // real host event — not the wrapper's own payload-less INIT self-loop.
    #[test]
    fn callsite_payload_capture_resolves_through_nested_render_wrapper() {
        use orbital_core::schema::EventDefinition;
        // Host.Failed (payload {error}) renders @trait.Wrapper; Wrapper.INIT (no
        // payload) renders @trait.Child; Child captures @callsitePayload.error.
        let mut host = build_trait_fixture(
            "Host",
            Some("Campaign"),
            vec![("Failed", vec![render_children(vec!["@trait.Wrapper"])])],
        );
        let ev: EventDefinition = serde_json::from_value(serde_json::json!({
            "key": "Failed", "payloadSchema": [{ "name": "error", "type": "string" }]
        }))
        .unwrap();
        host.state_machine.events = Some(vec![ev]);
        let wrapper = build_trait_fixture(
            "Wrapper",
            Some("Campaign"),
            vec![("INIT", vec![render_children(vec!["@trait.Child"])])],
        );
        let make_child = |field: &str| {
            let mut c = build_trait_fixture("Child", Some("Campaign"), vec![]);
            c.config = Some(callsite_content(field));
            c
        };

        // `error` is declared on the host event reached up the chain → clean.
        let orbital = orbital_with_traits(vec![host.clone(), wrapper.clone(), make_child("error")]);
        let (sites, parents_of) = build_embed_index(&orbital);
        let result =
            validate_callsite_payload_captures(&orbital, "orbitals[0]", &sites, &parents_of);
        assert!(
            result.errors.is_empty(),
            "nested capture must resolve to the host event, not the wrapper INIT: {:?}",
            result.errors
        );

        // `bogus` is declared on NO event up the chain → genuine error survives.
        let orbital_bad = orbital_with_traits(vec![host, wrapper, make_child("bogus")]);
        let (sites_b, parents_b) = build_embed_index(&orbital_bad);
        let result_bad =
            validate_callsite_payload_captures(&orbital_bad, "orbitals[0]", &sites_b, &parents_b);
        assert!(
            result_bad
                .errors
                .iter()
                .any(|e| e.code == ErrorCode::BindingPayloadFieldUndeclared
                    && e.message.contains("bogus")),
            "a field declared on no composing event up the chain must still error: {:?}",
            result_bad.errors
        );
    }

    // FIX 1 (ORB_BINDING_ENTITY_FIELD_NEVER_SET): a child rendered by a parent
    // via `@trait.X` may READ a non-shared `@entity.<field>` the PARENT set in
    // the same render context, without a local setter.
    #[test]
    fn embedded_child_entity_read_credited_to_parent_setter() {
        // Parent.Loaded sets @entity.title (Course, non-shared) AND renders
        // @trait.Child; Child reads @entity.title with no local setter.
        let set_title = arr(vec![s("set"), s("@entity.title"), s("t")]);
        let parent = build_trait_fixture(
            "Parent",
            Some("Course"),
            vec![(
                "Loaded",
                vec![set_title, render_children(vec!["@trait.Child"])],
            )],
        );
        let child_read = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = HashMap::new();
                m.insert("text".to_string(), s("@entity.title"));
                m
            }),
        ]);
        let child = build_trait_fixture("Child", Some("Course"), vec![("INIT", vec![child_read])]);

        let orbital = orbital_with_traits(vec![parent.clone(), child.clone()]);
        let (sites, parents_of) = build_embed_index(&orbital);
        let credit = collect_embed_set_credit(&orbital, &sites, &parents_of);
        let result = validate_entity_fields_have_setters(
            &child,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            credit.get("Child"),
        );
        assert!(
            result.warnings.is_empty() && result.errors.is_empty(),
            "child's @entity.title read must be credited to the embedding parent's setter: {:?}",
            result.warnings
        );

        // Negative: parent does NOT set title → child still warns (no credit).
        let parent_noset = build_trait_fixture(
            "Parent",
            Some("Course"),
            vec![("Loaded", vec![render_children(vec!["@trait.Child"])])],
        );
        let orbital2 = orbital_with_traits(vec![parent_noset, child.clone()]);
        let (sites2, parents2) = build_embed_index(&orbital2);
        let credit2 = collect_embed_set_credit(&orbital2, &sites2, &parents2);
        let result2 = validate_entity_fields_have_setters(
            &child,
            "test",
            &HashSet::new(),
            &HashMap::new(),
            credit2.get("Child"),
        );
        assert!(
            result2
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNeverSet),
            "with no parent setter the never-set warning must remain: {:?}",
            result2.warnings
        );
    }

    #[test]
    fn group_aware_setter_shared_entity_pass() {
        // Trait A only READS @entity.sharedX; Trait B SETS it. Entity is
        // [shared] — the group-aware check must NOT warn for Trait A.
        let read_effect = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("val".to_string(), s("@entity.sharedX"));
                m
            }),
        ]);
        let trait_a = build_trait_fixture(
            "ReaderTrait",
            Some("Pool"),
            vec![("INIT", vec![read_effect])],
        );
        let set_effect = arr(vec![
            s("set"),
            s("@entity.sharedX"),
            SExpression::Number(1.into()),
        ]);
        let trait_b = build_trait_fixture(
            "WriterTrait",
            Some("Pool"),
            vec![("INIT", vec![set_effect])],
        );

        let orbital = orbital_with_traits(vec![trait_a.clone(), trait_b]);
        let mut entity_defs = HashMap::new();
        entity_defs.insert("Pool".to_string(), shared_entity_def("Pool"));

        let group = collect_shared_group_set_fields(&orbital, &entity_defs);
        let result =
            validate_entity_fields_have_setters(&trait_a, "test", &HashSet::new(), &group, None);
        assert!(
            result.warnings.is_empty() && result.errors.is_empty(),
            "Trait A's read of a [shared]-group field set by Trait B must be satisfied: {:?}",
            result.warnings
        );
    }

    #[test]
    fn group_aware_setter_shared_entity_never_set_errors() {
        // Neither trait in the shared group ever sets @entity.neverSet —
        // must still warn.
        let read_effect = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("val".to_string(), s("@entity.neverSet"));
                m
            }),
        ]);
        let trait_a = build_trait_fixture(
            "ReaderTrait",
            Some("Pool"),
            vec![("INIT", vec![read_effect])],
        );
        let other_set = arr(vec![
            s("set"),
            s("@entity.somethingElse"),
            SExpression::Number(1.into()),
        ]);
        let trait_b =
            build_trait_fixture("WriterTrait", Some("Pool"), vec![("INIT", vec![other_set])]);

        let orbital = orbital_with_traits(vec![trait_a.clone(), trait_b]);
        let mut entity_defs = HashMap::new();
        entity_defs.insert("Pool".to_string(), shared_entity_def("Pool"));

        let group = collect_shared_group_set_fields(&orbital, &entity_defs);
        let result =
            validate_entity_fields_have_setters(&trait_a, "test", &HashSet::new(), &group, None);
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNeverSet),
            "Neither trait in the shared group sets neverSet — must still warn: {:?}",
            result.warnings
        );
    }

    #[test]
    fn group_aware_setter_non_shared_entity_unaffected() {
        // Non-shared entity: Trait B's setter must NOT satisfy Trait A's read
        // (byte-identical to the pre-Task-C per-trait-only behavior).
        let read_effect = arr(vec![
            s("render-ui"),
            s("main"),
            SExpression::Object({
                let mut m = std::collections::HashMap::new();
                m.insert("val".to_string(), s("@entity.x"));
                m
            }),
        ]);
        let trait_a = build_trait_fixture(
            "ReaderTrait",
            Some("Solo"),
            vec![("INIT", vec![read_effect])],
        );
        let set_effect = arr(vec![
            s("set"),
            s("@entity.x"),
            SExpression::Number(1.into()),
        ]);
        let trait_b = build_trait_fixture(
            "WriterTrait",
            Some("Solo"),
            vec![("INIT", vec![set_effect])],
        );

        let orbital = orbital_with_traits(vec![trait_a.clone(), trait_b]);
        let mut entity_defs = HashMap::new();
        entity_defs.insert(
            "Solo".to_string(),
            EntityDefinition {
                shared: false,
                identity: false,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Solo".to_string(),
                collection: None,
                persistence: None,
                instances: None,
                fields: vec![],
                id: None,
            },
        );

        let group = collect_shared_group_set_fields(&orbital, &entity_defs);
        assert!(
            group.is_empty(),
            "Non-shared entities must not appear in the group map: {:?}",
            group
        );
        let result =
            validate_entity_fields_have_setters(&trait_a, "test", &HashSet::new(), &group, None);
        assert!(
            result
                .warnings
                .iter()
                .any(|e| e.code == ErrorCode::BindingEntityFieldNeverSet),
            "Non-shared entity must keep per-trait-only behavior — Trait B's setter must not satisfy Trait A: {:?}",
            result.warnings
        );
    }
}
