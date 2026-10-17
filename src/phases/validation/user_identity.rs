//! Validators for the `[identity]` entity — the declared shape of `@user`.
//!
//! Named `user_identity` rather than `identity` because `identity`/`id_integrity`
//! already mean the Rabit V4 dual-carry NODE ids in this crate.
//!
//! A program with no `[identity]` entity is validated against the **implicit
//! default identity shape** — what `normalizeUserContext` produces (id, uid,
//! email, name) plus an unconstrained `role : string`, minus `permissions`
//! (`Almadar_LOLO_Identity.md` §11 "Also settled", lands with I.3). The corpus
//! reads `@user` on exactly those fields, and the std library's six `@user`
//! atoms read only `id`/`role`, so the default keeps every existing program
//! compiling while making a typo'd field (which nothing can ever carry) an
//! error. `role` carries no declared members in the default, so no literal
//! comparison can be rejected without a declaration — the membership win stays
//! exactly proportional to declaring.
//!
//! The walk is over the whole serialized trait rather than over guards alone.
//! std-healthcare's 13 dead `"patient"` comparisons live in **config knob
//! defaults** (`navItems : [NavItem] = (if ["=", @user.role, "patient"] …)`),
//! not in transitions, so a guard-only walker would report zero problems on the
//! very organism that motivated the rule.

use super::{ErrorCode, ValidationContext, ValidationError, ValidationResult};
use orbital_core_base::schema::{EntityDefinition, EntityRef, Orbital, OrbitalSchema, TraitReference};
use orbital_core_base::type_compat::enum_value_is_member;
use serde_json::Value;

/// Where the identity shape a `@user` check resolves against came from. An
/// `expects entity X [identity] { shape }` is the consumer's own declared source of
/// truth — reads outside it fail with `ORB_S_EXPECTATION_FIELD_UNDECLARED`
/// rather than the unknown-identity-field code.
#[derive(Clone, Copy)]
pub(super) enum IdentitySource<'a> {
    /// A real `[identity]` entity declared in the schema.
    Declared,
    /// No declaration anywhere — the implicit default shape.
    Default,
    /// The orbital's own `expects entity [identity]` declaration (shape present).
    Expectation {
        orbital: &'a str,
        entity: Option<&'a str>,
    },
}

/// `EQUALITY_OPS` / `vocabulary` / `identity_field` / `user_field_of` moved to
/// `orbital-core/src/runtime/entity_access.rs` (C1-C convergence) so
/// `orbital-core::runtime::role_satisfying_policy` — the Rust twin of JS
/// `roleSatisfyingPolicy`, which `orbital-verify` calls per persist step —
/// shares the SAME `@user.<f>` shape readers `check_comparison` below uses,
/// rather than a second copy. Re-exported under their original names/paths so
/// every existing caller in this crate (notably `validation::entity_access`,
/// which imports `identity_field`/`user_field_of` via
/// `super::user_identity::{...}`) is unaffected; `check_comparison`'s
/// behavior is byte-identical to before this move.
pub(crate) use orbital_core_base::entity_vocabulary::{identity_field, user_field_of, vocabulary, EQUALITY_OPS};

/// Every `[identity]`-tagged entity in `orbitals`, primaries first, each
/// paired with whether it is an orbital's PRIMARY entity (`true`) or an
/// auxiliary copy pulled in by an import (`false`). The schema-wide and
/// orbital-slice callers (`identity_entities_tagged` / Stage B's
/// `identity_entities_of`) both apply the SAME primary-shadows-import rule
/// over whatever orbital slice they're given — this is the one walker.
fn identity_entities_tagged_of(orbitals: &[Orbital]) -> Vec<(&EntityDefinition, bool)> {
    let mut out = Vec::new();
    for orbital in orbitals {
        if let EntityRef::Inline(def) = &orbital.entity {
            if def.identity {
                out.push((def, true));
            }
        }
    }
    for orbital in orbitals {
        for er in orbital.auxiliary_entities.iter().flatten() {
            if let EntityRef::Inline(def) = er {
                if def.identity {
                    out.push((def, false));
                }
            }
        }
    }
    out
}

fn identity_entities_tagged(schema: &OrbitalSchema) -> Vec<(&EntityDefinition, bool)> {
    identity_entities_tagged_of(&schema.orbitals)
}

/// The `[identity]` entities that decide what `@user` resolves to, computed
/// over an arbitrary orbital slice — same primary-shadows-import rule
/// `identity_entities` applies schema-wide (see its doc comment for the
/// rationale). Stage B's orbital-import splice calls this over one alias's
/// `AliasEntry.orbitals` to find the UPSTREAM roster a `roles {}` remap
/// resolves against, independent of the consumer schema's own roster.
pub(crate) fn identity_entities_of(orbitals: &[Orbital]) -> Vec<&EntityDefinition> {
    let tagged = identity_entities_tagged_of(orbitals);
    let has_primary = tagged.iter().any(|(_, primary)| *primary);
    tagged
        .into_iter()
        .filter(|(_, primary)| *primary || !has_primary)
        .map(|(def, _)| def)
        .collect()
}

/// The `[identity]` entities that decide what `@user` resolves to.
///
/// A behavior declares its own roster so it runs standalone — that entity is
/// its DEFAULT viewer. Composing the behavior never imports the orbital that
/// owns it, but a trait bound to one of its siblings drags the roster along as
/// an auxiliary copy (Gap #22), tag and all, which would read as a second
/// roster. So a PRIMARY roster shadows every imported copy: the composing app
/// declares who the viewer is, and the copies yield.
///
/// With no primary roster the copies still count — a thin app that wraps one
/// behavior and declares no roster of its own inherits that behavior's, and
/// two imported rosters with no host roster still collide, because nothing in
/// the program says which one `@user` is.
///
/// Shared with `entity_access.rs`, which needs the same answer for the four
/// access directives.
pub(crate) fn identity_entities(schema: &OrbitalSchema) -> Vec<&EntityDefinition> {
    identity_entities_of(&schema.orbitals)
}

/// Every name tagged `[identity]` anywhere, shadowed copies included.
///
/// Owner-column typing (`ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED`) asks whether a
/// column compared to `@user.id` relates to *a* roster, not to *the* one. A
/// shadowed copy is still a roster — `Timesheet.employeeId : Employee` is an
/// owner column whether or not `Employee` won the `@user` binding — so this
/// keeps the copies, where `identity_entities` drops them.
pub(super) fn identity_entity_names(schema: &OrbitalSchema) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for (def, _) in identity_entities_tagged(schema) {
        let name = def.name.as_str();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

// `vocabulary` / `identity_field` / `user_field_of` — see the `pub(crate) use`
// re-export above.

pub fn validate_user_identity(schema: &OrbitalSchema, ctx: &mut ValidationContext) {
    let mut result = ValidationResult::new();
    let declared = identity_entities(schema);

    // Two DIFFERENTLY-NAMED identity entities. Same-name duplicates across
    // orbitals are already a hard error upstream (`validate_entity_uniqueness`).
    let mut names: Vec<&str> = declared.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    if names.len() > 1 {
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaIdentityNotUnique,
                "orbitals",
                format!(
                    "{} entities are tagged [identity]: {}. `@user` resolves to a single viewer, \
                     so more than one declaration makes every `@user.x` ambiguous.",
                    names.len(),
                    names.join(", ")
                ),
            )
            .with_suggestion(
                "Keep exactly one [identity] entity and reference it from the other orbitals \
                 (`ownerId : <IdentityEntity>`).",
            ),
        );
    }

    // Undeclared identity ⇒ `@user` still carries the implicit DEFAULT shape —
    // what `normalizeUserContext` produces (id, uid, email, name) plus an
    // unconstrained `role : string`, minus `permissions`
    // (`Almadar_LOLO_Identity.md` §11 "Also settled"). `role` has no declared
    // members, so no literal comparison can be rejected; only a field outside
    // the shape (a typo nothing can ever carry) is an error.
    //
    // Per-orbital resolution: a declared `[identity]` entity wins schema-wide.
    // Otherwise an orbital's own `expects entity [identity] … { shape }` is its declared
    // source of truth (self-consistency: reads outside the declared shape fail
    // against the orbital's OWN declaration). A shape-less `expects entity [identity]`
    // leaves the default shape in place — existence-only, as unchecked as today.
    let default_identity = default_identity_shape();
    let declared_identity = declared.first().copied();

    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let expectation_entity;
        let (identity, source) = match declared_identity {
            Some(identity) => (identity, IdentitySource::Declared),
            None => match super::expectations::identity_expectation(orbital) {
                Some(exp) if exp.shape.is_some() => {
                    expectation_entity = super::expectations::shape_as_identity_entity(
                        exp.name.unwrap_or("@user"),
                        exp.shape.unwrap_or(&[]),
                    );
                    (
                        &expectation_entity,
                        IdentitySource::Expectation {
                            orbital: orbital.name.as_str(),
                            entity: exp.name,
                        },
                    )
                }
                _ => (&default_identity, IdentitySource::Default),
            },
        };
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            // An `expects entity [identity]` declaration covers the orbital's AUTHORED
            // surface — its own inline traits and call-site config. Bodies
            // inlined from `uses[]` imports carry `source_behavior`; their
            // internal `@user` reads are the imported behavior's own contract
            // and invisible to derivation, so checking them against the
            // derived declaration manufactures false mismatches
            // (G-EXPECT-DERIVE-IMPORTS: std-browse's scopeField filter reads
            // `@user.id` in every BrowseList consumer → spurious
            // FIELD_UNDECLARED → demote on a params-{} factory build).
            if matches!(source, IdentitySource::Expectation { .. })
                && trait_def.source_behavior.is_some()
            {
                continue;
            }
            let path = super::trait_path(i, &orbital.name, j, trait_ref);
            let Ok(value) = serde_json::to_value(trait_def) else {
                continue;
            };
            walk(&value, &path, identity, source, &mut result);
            check_config_role_lists(trait_def, &path, identity, &mut result);
        }

        // Self-consistency: when a REAL roster won the type environment above,
        // the authored `expects entity [identity] { shape }` was never compared to
        // anything — reads resolve against the roster, which knows more than the
        // declaration, so an under-declaration passes. It only becomes the type
        // environment once the orbital is SLICED, which is where it then fails.
        //
        // `expects` is a LOWER BOUND: omitting fields the provider happens to
        // have is legal and is the point. Declaring FEWER than the orbital
        // actually reads is not — a provider satisfying the declaration exactly
        // would not carry the field. So check `derived ⊆ authored` here too,
        // against the DECLARATION, with the same authored-surface rule as above.
        //
        // This is what let 219 blocks declaring `{ role : string }` while reading
        // `@user.email`/`@user.name` ship and stay valid until materialization.
        if matches!(source, IdentitySource::Declared) {
            if let Some(exp) = super::expectations::identity_expectation(orbital) {
                if let Some(shape) = exp.shape {
                    let declared = super::expectations::shape_as_identity_entity(
                        exp.name.unwrap_or("@user"),
                        shape,
                    );
                    let declared_source = IdentitySource::Expectation {
                        orbital: orbital.name.as_str(),
                        entity: exp.name,
                    };
                    for (j, trait_ref) in orbital.traits.iter().enumerate() {
                        let TraitReference::Inline(trait_def) = trait_ref else {
                            continue;
                        };
                        if trait_def.source_behavior.is_some() {
                            continue;
                        }
                        let path = super::trait_path(i, &orbital.name, j, trait_ref);
                        let Ok(value) = serde_json::to_value(trait_def) else {
                            continue;
                        };
                        walk(&value, &path, &declared, declared_source, &mut result);
                    }
                }
            }
        }
    }

    if let Some(identity) = declared_identity {
        check_page_nav_roles(schema, identity, &mut result);
    }

    ctx.merge(result);
}

/// A page's nav entry roles (`@roles`) are matched against the viewer's role, so each
/// must be a declared value of some role field of the [identity] entity.
fn check_page_nav_roles(
    schema: &OrbitalSchema,
    identity: &EntityDefinition,
    result: &mut ValidationResult,
) {
    let role_fields: Vec<(&str, &Vec<String>)> = identity
        .fields
        .iter()
        .filter_map(|f| Some((f.name.as_deref()?, vocabulary(f)?)))
        .collect();
    if role_fields.is_empty() {
        return;
    }
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        for (k, page_ref) in orbital.pages.iter().flatten().enumerate() {
            let orbital_core_base::schema::PageRef::Inline(page) = page_ref else {
                continue;
            };
            for role in page.roles.iter().flatten() {
                if role_fields
                    .iter()
                    .any(|(_, vocab)| enum_value_is_member(role, vocab))
                {
                    continue;
                }
                let (field_name, vocab) = role_fields[0];
                result.add_error(
                    ValidationError::error(
                        ErrorCode::SchemaRoleLiteralNotMember,
                        format!("orbitals[{i}].pages[{k}].roles").as_str(),
                        format!(
                            "Page \"{}\" lists role \"{}\" for its nav entry, but no role field \
                             of the [identity] entity `{}` declares it, so no viewer ever sees the entry.",
                            page.path, role, identity.name
                        ),
                    )
                    .with_valid_values(vocab.to_vec())
                    .with_suggestion(format!(
                        "Use one of: {}, or add \"{}\" to `{}.{}`.",
                        vocab.join(", "),
                        role,
                        identity.name,
                        field_name
                    )),
                );
            }
        }
    }
}

/// Every literal of a config-held role list (`allowedRoles`, nav-item / action
/// `roles`, nested `children` roles) must be a declared member of the role field
/// the trait tests it against. Sites come from the same detector the
/// orbital-import remap uses (`inline::rewrite::role_list_sites`), so a list
/// that is never compared to a role source is never checked.
fn check_config_role_lists(
    trait_def: &orbital_core_base::schema::TraitDefinition,
    path: &str,
    identity: &EntityDefinition,
    result: &mut ValidationResult,
) {
    use crate::phases::inline::rewrite::{role_list_literals, role_list_sites};
    use std::collections::HashMap;
    let mut role_fields: HashMap<String, Vec<String>> = HashMap::new();
    for field in &identity.fields {
        if let (Some(name), Some(values)) = (field.name.as_ref(), vocabulary(field)) {
            role_fields.insert(name.clone(), values.clone());
        }
    }
    if role_fields.is_empty() {
        return;
    }
    let Some(cfg) = trait_def.config.as_ref() else {
        return;
    };
    for site in role_list_sites(trait_def, &role_fields) {
        let Some(default) = cfg.get(&site.knob).and_then(|f| f.default.as_ref()) else {
            continue;
        };
        let Some(vocab) = role_fields.get(&site.field) else {
            continue;
        };
        for literal in role_list_literals(default, &site) {
            if enum_value_is_member(&literal, vocab) {
                continue;
            }
            result.add_error(role_literal_error(
                &literal,
                &site.field,
                identity,
                vocab,
                &format!("{}.config.{}", path, site.knob),
            ));
        }
    }
}

fn role_literal_error(
    literal: &str,
    field_name: &str,
    identity: &EntityDefinition,
    vocab: &[String],
    path: &str,
) -> ValidationError {
    ValidationError::error(
        ErrorCode::SchemaRoleLiteralNotMember,
        path,
        format!(
            "\"{}\" is not a declared value of `@user.{}` on the [identity] entity `{}`. \
             This comparison can never be true — the branch is dead.",
            literal, field_name, identity.name
        ),
    )
    .with_valid_values(vocab.to_vec())
    .with_suggestion(format!(
        "Use one of: {}, or add \"{}\" to `{}.{}`.",
        vocab.join(", "),
        literal,
        identity.name,
        field_name
    ))
}

/// The type of `@user` for the trait named `trait_name`: the program's `[identity]` roster, else the declaring
/// orbital's `expects entity [identity] { … }` shape, else the implicit default shape.
pub(crate) fn identity_type(schema: &OrbitalSchema, trait_name: &str) -> orbital_core_base::ir::OirType {
    let orbital = schema.orbitals.iter().find(|o| {
        o.traits
            .iter()
            .any(|t| matches!(t, orbital_core_base::schema::TraitReference::Inline(def) if def.name == trait_name))
    });
    let expected = orbital
        .and_then(super::expectations::identity_expectation)
        .and_then(|e| e.shape.map(<[_]>::to_vec));
    let roster = identity_entities(schema).into_iter().next().map(|def| def.fields.clone());
    let fields = roster
        .or(expected)
        .unwrap_or_else(|| default_identity_shape().fields);
    orbital_core_base::ir::OirType::Struct {
        fields: fields
            .iter()
            .filter_map(|f| {
                let name = f.name.clone()?;
                Some(orbital_core_base::ir::OirField::new(name, orbital_core_base::ir::types::field_shape(f)))
            })
            .collect(),
    }
}

/// The implicit `@user` shape of a program with no `[identity]` entity.
pub(super) fn default_identity_shape() -> EntityDefinition {
    serde_json::from_value(serde_json::json!({
        "name": "@user",
        "fields": [
            { "name": "id", "type": "string" },
            { "name": "uid", "type": "string" },
            { "name": "email", "type": "string" },
            { "name": "name", "type": "string" },
            { "name": "role", "type": "string" }
        ]
    }))
    .expect("default identity shape must deserialize")
}

fn walk(
    value: &Value,
    path: &str,
    identity: &EntityDefinition,
    source: IdentitySource,
    result: &mut ValidationResult,
) {
    match value {
        Value::String(s) => check_field_exists(s, path, identity, source, result),
        Value::Array(items) => {
            check_comparison(items, path, identity, result);
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{}[{}]", path, i), identity, source, result);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                // `sourceEntityDefinition` is PROVENANCE — the atom's original
                // entity, kept for tooling. It is not an entity of the composed
                // program, so its access policies govern nothing here and their
                // `@user.role` literals belong to the ATOM's role vocabulary,
                // not the host's.
                //
                // Walking it reports the organism's own roles as dead branches
                // against the consumer's roster: composing one `std-time-tracking`
                // trait into an app whose roster is `Person` produced 17
                // `ORB_S_ROLE_LITERAL_NOT_MEMBER` errors for `"approver"` /
                // `"employee"` — literals of a `Timesheet`/`Employee` entity that
                // is nowhere in the app. The consumer had already overridden the
                // binding (`-> PfTimesheet`), which is the language's own answer.
                //
                // Nothing is lost: when a trait is NOT rebound, the atom's entity
                // IS materialised — as the orbital's primary or an auxiliary —
                // and those same policies are validated there, on a definition
                // that really is in the program.
                if k == "sourceEntityDefinition" {
                    continue;
                }
                walk(v, &format!("{}.{}", path, k), identity, source, result);
            }
        }
        _ => {}
    }
}

/// `@user.<field>` must name a declared field of the identity entity — or, when
/// no `[identity]` entity is declared, a field of the orbital's `expects
/// identity` shape (its own declaration) or the implicit default shape.
fn check_field_exists(
    s: &str,
    path: &str,
    identity: &EntityDefinition,
    source: IdentitySource,
    result: &mut ValidationResult,
) {
    let Some(field) = user_field_of(s) else {
        return;
    };
    if identity_field(identity, field).is_some() {
        return;
    }
    let mut declared: Vec<&str> = identity
        .fields
        .iter()
        .filter_map(|f| f.name.as_deref())
        .collect();
    declared.sort_unstable();
    if let IdentitySource::Expectation { orbital, entity } = source {
        let expectation = match entity {
            Some(name) => format!("`expects entity {} [identity]`", name),
            None => "`expects entity [identity]`".to_string(),
        };
        result.add_error(
            ValidationError::error(
                ErrorCode::SchemaExpectationFieldUndeclared,
                path,
                format!(
                    "`{}` reads `@user.{}`, but its {} declaration only declares: {}. The \
                     declaration is this orbital's own source of truth for the `@user` fields \
                     it reads.",
                    orbital,
                    field,
                    expectation,
                    declared.join(", ")
                ),
            )
            .with_suggestion(format!(
                "Add `{}` to the expects shape, or read one of: {}",
                field,
                declared.join(", ")
            )),
        );
        return;
    }
    let declared_in_source = matches!(source, IdentitySource::Declared);
    let (message, suggestion) = if declared_in_source {
        (
            format!(
                "`@user.{}` is not a field of the [identity] entity `{}`. It resolves to null, \
                 so every branch reading it silently takes its negative arm.",
                field, identity.name
            ),
            format!(
                "Declare `{}` on entity {}, or read one of: {}",
                field,
                identity.name,
                declared.join(", ")
            ),
        )
    } else {
        (
            format!(
                "`@user.{}` is not part of the default identity shape ({}). No [identity] \
                 entity is declared, so nothing can carry this field — it resolves to null and \
                 every branch reading it silently takes its negative arm.",
                field,
                declared.join(", ")
            ),
            format!(
                "Declare an [identity] entity carrying `{}`, or read one of: {}",
                field,
                declared.join(", ")
            ),
        )
    };
    result.add_error(
        ValidationError::error(ErrorCode::SchemaUnknownIdentityField, path, message)
            .with_suggestion(suggestion),
    );
}

/// Is a `Value::Array` an `array/includes` haystack of LITERAL role members,
/// and if so, which elements are the members?
///
/// Mirrors `inline::rewrite::try_rewrite_role_includes` shape-for-shape (the
/// two cite each other): a haystack is literal only when every element is a
/// string literal — either a `["list", …]` array-construction call (the wrap
/// `mark_literal_list_in_place` applies to any config-array-override value
/// that would otherwise collide with operator dispatch; both `.lolo`
/// spellings `(a b)` and `[a, b]` lower through it) whose TAIL is all-string
/// (the head is the `list` marker itself, not a member), or a bare array
/// whose every element (including position 0) is a string literal — the
/// shape `rewrite_role_literals`'s multi-target expansion produces
/// (`["array/includes", [targets…], "@user.<f>"]`). Any other shape — e.g. a
/// nested call like `["object/get", ["array/nth", …], "allowedRoles"]`, a
/// per-step dynamic role list — is not a literal haystack: `None`.
fn literal_haystack_members(items: &[Value]) -> Option<Vec<&Value>> {
    let is_list_call = matches!(items.first(), Some(Value::String(s)) if s == "list");
    let members = if is_list_call { &items[1..] } else { items };
    members
        .iter()
        .all(|v| matches!(v, Value::String(_)))
        .then(|| members.iter().collect())
}

/// A literal compared against a closed-vocabulary `@user.<field>` must be one of
/// that field's declared members.
pub(super) fn check_comparison(
    items: &[Value],
    path: &str,
    identity: &EntityDefinition,
    result: &mut ValidationResult,
) {
    let Some(Value::String(op)) = items.first() else {
        return;
    };
    let is_equality = EQUALITY_OPS.contains(&op.as_str());
    let is_includes = op == "array/includes";
    if !is_equality && !is_includes {
        return;
    }
    let args = &items[1..];
    if args.len() < 2 {
        return;
    }

    // `array/includes` is directional, but the sigil can sit on EITHER side:
    // `(array/includes @user.roles "ghost")` checks a literal against a
    // multi-valued field's own vocabulary (field is the haystack, position
    // 0); the Stage-B orbital-import role rewrite
    // (`inline/rewrite.rs::rewrite_role_literals`, which this validator
    // mirrors shape-for-shape) instead produces
    // `(array/includes ["owner","project_manager"] @user.role)` — a literal
    // array haystack with the sigil as the needle, position 1. Locate the
    // sigil by content, not position, so both shapes validate.
    let (sigil_side, literal_side): (&[Value], &[Value]) = if is_includes {
        let first_is_sigil = matches!(
            args.first(),
            Some(Value::String(s)) if user_field_of(s).is_some()
        );
        if first_is_sigil {
            (&args[..1], &args[1..2])
        } else {
            (&args[1..2], &args[..1])
        }
    } else {
        (args, args)
    };

    let Some((field_name, field)) = sigil_side.iter().find_map(|arg| {
        let Value::String(s) = arg else { return None };
        let name = user_field_of(s)?;
        identity_field(identity, name).map(|f| (name, f))
    }) else {
        return;
    };
    let Some(vocab) = vocabulary(field) else {
        return;
    };

    // The literal side is either a single scalar (equality, or an
    // `array/includes` whose haystack is the sigil's own multi-valued field)
    // or, for the role-rewrite shape, a literal array whose EVERY member
    // must be checked individually — but only when the haystack IS a
    // literal array. Two legitimate call shapes also serialize as a JSON
    // array here: a per-step dynamic role list (`(object/get (array/nth
    // @config.steps i) "allowedRoles")`, std-step-flow's role guard) and the
    // `["list", …]` array-construction wrap every knob-array override lowers
    // through (`mark_literal_list_in_place`, both `.lolo` spellings `(a b)`
    // and `[a, b]`). `literal_haystack_members` tells them apart; `None`
    // means a dynamic haystack — skip the membership check (no finding).
    let literals: Vec<&Value> = if is_includes {
        match literal_side.first() {
            Some(Value::Array(items)) => match literal_haystack_members(items) {
                Some(members) => members,
                None => return,
            },
            Some(other) => vec![other],
            None => Vec::new(),
        }
    } else {
        literal_side.iter().collect()
    };

    for arg in literals {
        let Value::String(literal) = arg else {
            continue;
        };
        // Skip the sigil operand itself and any other binding.
        if literal.starts_with('@') {
            continue;
        }
        if enum_value_is_member(literal, vocab) {
            continue;
        }
        result.add_error(role_literal_error(
            literal, field_name, identity, vocab, path,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orbital_with_identity(name: &str, entity_name: &str) -> Orbital {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "entity": {
                "name": entity_name,
                "identity": true,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": []
        }))
        .expect("orbital fixture must deserialize")
    }

    fn orbital_no_identity(name: &str, entity_name: &str) -> Orbital {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "entity": {
                "name": entity_name,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            },
            "traits": []
        }))
        .expect("orbital fixture must deserialize")
    }

    /// Stage B (orbital-import splice) calls `identity_entities_of` over one
    /// alias's own orbitals — this is the same primary-shadows-import rule
    /// `identity_entities(schema)` applies schema-wide, just parameterized
    /// over an arbitrary orbital slice instead of `&schema.orbitals`.
    #[test]
    fn primary_shadows_auxiliary_import_over_an_arbitrary_orbital_slice() {
        let primary = orbital_with_identity("Main", "Person");
        let mut imported = orbital_no_identity("Imported", "Widget");
        imported.auxiliary_entities = Some(vec![orbital_core_base::schema::EntityRef::Inline(
            serde_json::from_value(serde_json::json!({
                "name": "ImportedOwner",
                "identity": true,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            }))
            .unwrap(),
        )]);
        let orbitals = [primary, imported];
        let names: Vec<&str> = identity_entities_of(&orbitals)
            .into_iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Person"],
            "the primary roster must shadow the imported aux copy"
        );
    }

    /// With no primary roster, the imported copies still count.
    #[test]
    fn no_primary_roster_falls_back_to_imported_copies() {
        let mut imported = orbital_no_identity("Imported", "Widget");
        imported.auxiliary_entities = Some(vec![orbital_core_base::schema::EntityRef::Inline(
            serde_json::from_value(serde_json::json!({
                "name": "ImportedOwner",
                "identity": true,
                "persistence": "persistent",
                "fields": [ { "name": "id", "type": "string", "required": true } ]
            }))
            .unwrap(),
        )]);
        let orbitals = [imported];
        let names: Vec<&str> = identity_entities_of(&orbitals)
            .into_iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["ImportedOwner"]);
    }

    /// `identity_entities(schema)` is now a thin wrapper — same result as
    /// calling `identity_entities_of(&schema.orbitals)` directly.
    #[test]
    fn identity_entities_delegates_to_identity_entities_of() {
        let schema: OrbitalSchema = serde_json::from_value(serde_json::json!({
            "name": "t",
            "version": "1.0.0",
            "orbitals": [
                serde_json::to_value(orbital_with_identity("Main", "Person")).unwrap()
            ]
        }))
        .expect("schema fixture must deserialize");
        let names: Vec<&str> = identity_entities(&schema)
            .into_iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["Person"]);
    }

    /// `[identity]` entity carrying a closed-vocabulary `role` field
    /// (`manager`/`cto`) for `check_comparison`'s `array/includes` haystack
    /// shape tests.
    fn identity_with_role_vocab() -> EntityDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "Person",
            "identity": true,
            "persistence": "persistent",
            "fields": [
                { "name": "id", "type": "string", "required": true },
                {
                    "name": "role",
                    "type": "enum",
                    "values": ["manager", "cto"]
                }
            ]
        }))
        .expect("identity fixture must deserialize")
    }

    fn run_comparison(items_json: serde_json::Value) -> ValidationResult {
        let items = match items_json {
            Value::Array(a) => a,
            other => panic!("fixture must be a JSON array: {other:?}"),
        };
        let mut result = ValidationResult::new();
        check_comparison(&items, "path", &identity_with_role_vocab(), &mut result);
        result
    }

    /// A dynamic per-step haystack (std-step-flow's role guard idiom) is not
    /// a literal role list — no membership check, no finding. This is the
    /// exact shape that produced the `object/get`/`allowedRoles` false
    /// positive on 8 corpus organisms.
    #[test]
    fn array_includes_dynamic_object_get_haystack_is_not_checked() {
        let result = run_comparison(serde_json::json!([
            "array/includes",
            [
                "object/get",
                ["array/nth", "@config.steps", 0],
                "allowedRoles"
            ],
            "@user.role"
        ]));
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// A `["list", …]` array-construction call is a literal haystack whose
    /// TAIL is the member set — `"list"` itself is never checked against the
    /// vocabulary (the false positive on cicd-pipeline/cms/inventory), and
    /// an out-of-vocabulary tail member still gets caught.
    #[test]
    fn array_includes_list_call_haystack_checks_tail_only() {
        let ok = run_comparison(serde_json::json!([
            "array/includes",
            ["list", "cto"],
            "@user.role"
        ]));
        assert!(ok.errors.is_empty(), "{:?}", ok.errors);

        let bad = run_comparison(serde_json::json!([
            "array/includes",
            ["list", "ceo"],
            "@user.role"
        ]));
        assert_eq!(bad.errors.len(), 1);
        assert!(bad.errors[0].message.contains("\"ceo\""));
        assert!(!bad.errors[0].message.contains("\"list\""));
    }

    /// A bare literal array (no `list` wrap) — the shape
    /// `rewrite_role_literals`'s multi-target expansion produces
    /// (`["array/includes", [targets…], "@user.<f>"]`) — checks every
    /// element.
    #[test]
    fn array_includes_bare_literal_array_haystack_checks_every_element() {
        let ok = run_comparison(serde_json::json!([
            "array/includes",
            ["manager", "cto"],
            "@user.role"
        ]));
        assert!(ok.errors.is_empty(), "{:?}", ok.errors);

        let bad = run_comparison(serde_json::json!([
            "array/includes",
            ["manager", "ceo"],
            "@user.role"
        ]));
        assert_eq!(bad.errors.len(), 1);
        assert!(bad.errors[0].message.contains("\"ceo\""));
    }
}
