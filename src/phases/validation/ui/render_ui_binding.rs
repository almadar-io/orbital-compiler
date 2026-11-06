//! Render-UI Binding Resolver (F3 plan)
//!
//! Given a render-ui prop value (a primitive literal or a binding string
//! like `@payload.tabId`, `@entity.title`, `@config.pageSize`, `@trait.X`,
//! or an SExpression operator call), resolve it to a **type expression**
//! (one of the type names the pattern registry uses: `"string"`, `"number"`,
//! `"boolean"`, `"array"`, `"object"`, `"binding"` for unresolved, ...).
//!
//! Consumers compare the resolved type against the prop's declared types
//! from `EmbeddedPatternProp.types`. Mismatch → `ORB_RUI_BINDING_TYPE_MISMATCH`
//! (gap #6 in `docs/Almadar_Std_Verification.md`).
//!
//! ## What this module owns
//!
//! - The mechanical "where does this binding's type come from?" lookup.
//! - Operator return-type resolution via canonical-operators metadata,
//!   sharpened by F2's `return_semantics` for input-dependent operators.
//! - A small type lattice (string / number / boolean / array / object / null
//!   / unknown) that mirrors what `classify_prop_type` in render_ui.rs
//!   produces for literal values. The two sides need to agree because
//!   downstream comparisons are name-equality.
//!
//! ## What this module does NOT own
//!
//! - The validation rule itself (L2.1's `ORB_RUI_BINDING_TYPE_MISMATCH`).
//!   That lives next door in `render_ui.rs`, calls into this module to
//!   resolve a binding's source type, and emits a diagnostic on mismatch.
//! - The emit-payload conformance check (L2.2). That's a sibling rule
//!   that consults the `emit_payload_schema` / `callback_args` registry
//!   fields (F1 plumbing) — independent of this binding resolver.
//! - The cross-orbital trait clone resolution. F3 trusts the inline phase
//!   to have already substituted `uses` imports before validate runs.

use super::canonical_operators::get_canonical_operator;
use orbital_core_base::schema::{
    ConfigField, EntityDefinition, FieldDefinition, FieldType, PayloadField, SExpression,
    TraitDefinition,
};
use std::collections::HashMap;

/// A resolved binding type expressed in the same vocabulary
/// `classify_prop_type` produces for literal values, so registry comparisons
/// are pure name equality.
///
/// `Unknown` is the lattice top — emitted when the resolver can't determine
/// the type (alias chain didn't resolve, payload schema missing, operator
/// not in canonical registry, etc.). Validators should treat `Unknown` as
/// "skip this comparison" rather than failing — false positives here are
/// costly (every "I don't know" becomes a noisy error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingType {
    String,
    Number,
    Boolean,
    Array,
    Object,
    Null,
    /// Resolved primitive whose specific shape isn't represented in the
    /// pattern registry (e.g. `datetime` → still "string" for prop-compare
    /// purposes, but kept distinct here in case future rules want to treat
    /// it differently).
    Datetime,
    /// `@trait.Y` — a sentinel "rendered child trait" binding. Validator
    /// should skip type-compare; verify only that the referenced trait
    /// exists in the orbital. Not a real value type.
    TraitRef,
    /// Binding resolves to an entity reference (relation field). Treated
    /// as object for compare; the resolver records the entity name so
    /// future rules can recurse into the related entity's fields.
    Entity(String),
    /// Resolution failed or not yet implemented. Validator skips.
    Unknown,
    /// An unbound type variable: opaque, so only a prop that takes any value fits it.
    Var,
}

impl BindingType {
    /// Map to the registry's prop-type vocabulary so name-equality works
    /// against `EmbeddedPatternProp.types: ["string"]` etc.
    pub fn registry_type(&self) -> &'static str {
        match self {
            BindingType::String => "string",
            BindingType::Number => "number",
            BindingType::Boolean => "boolean",
            BindingType::Array => "array",
            BindingType::Object => "object",
            BindingType::Null => "null",
            BindingType::Datetime => "string", // datetime is a string subtype in TS / orb props
            BindingType::TraitRef => "trait",
            BindingType::Entity(_) => "object",
            BindingType::Unknown => "unknown",
            BindingType::Var => "var",
        }
    }
}

/// Trait-level context the resolver needs to look up bindings. Caller
/// constructs this once per trait (or per transition for `@payload`
/// scope) and passes it through.
pub struct BindingContext<'a> {
    /// The trait whose render-ui pattern is being validated. Owns the
    /// `linked_entity` reference for `@entity.X` lookups and the `config`
    /// schema for `@config.X` lookups.
    pub trait_def: &'a TraitDefinition,
    /// Schema-level entity registry; resolves `linked_entity` to its
    /// `FieldDefinition` list for `@entity.X` field-type queries.
    pub entity_definitions: &'a HashMap<String, EntityDefinition>,
    /// The triggering event's declared payload schema, if known. Set by
    /// the caller from the transition's `event` key — usually by looking
    /// up `state_machine.events[]` or the trait's `listens[]` block. None
    /// when the resolver is invoked from a context where the triggering
    /// event isn't determinable (e.g. INIT transitions, which emit no
    /// dispatch payload).
    pub event_payload: Option<&'a [PayloadField]>,
    /// Names of traits in the same orbital, for `@trait.Y` reference
    /// existence checks. The resolver only verifies the trait is named
    /// here; structural type-compare is skipped (rendered child traits
    /// produce arbitrary slot trees).
    pub sibling_trait_names: &'a [String],
}

/// Resolve a render-ui prop value to its source type.
///
/// `@`-prefixed strings are treated as bindings; everything else is a
/// literal whose primitive type comes from the SExpression node kind.
/// SExpression-form operator calls (arrays whose first element is a
/// known operator name) resolve via the canonical operators metadata.
pub fn resolve_binding_type(value: &SExpression, ctx: &BindingContext) -> BindingType {
    match value {
        SExpression::String(s) if s.starts_with('@') => resolve_binding_path(s, ctx),
        SExpression::String(_) => BindingType::String,
        SExpression::Number(_) => BindingType::Number,
        SExpression::Bool(_) => BindingType::Boolean,
        SExpression::Array(arr) => resolve_array_value(arr, ctx),
        SExpression::Object(_) => BindingType::Object,
        SExpression::Null => BindingType::Null,
    }
}

/// Resolve an `@`-prefixed binding path (`@payload.X`, `@entity.X`,
/// `@config.X`, `@trait.Y`) against the binding context.
fn resolve_binding_path(path: &str, ctx: &BindingContext) -> BindingType {
    // Strip the `@` and split on `.`. First segment is the scope.
    let stripped = &path[1..];
    let mut parts = stripped.splitn(2, '.');
    let scope = parts.next().unwrap_or("");
    let rest = parts.next();

    match scope {
        "payload" => match rest {
            Some(field) => resolve_payload_field(field, ctx),
            // Bare `@payload` is the whole payload object.
            None => BindingType::Object,
        },
        "entity" => match rest {
            Some(field) => resolve_entity_field(field, ctx),
            // Bare `@entity` is the whole linked entity row (object).
            // Some atoms intentionally bind the full entity collection
            // (e.g. `entity: "@entity"` on data-list) — array is more
            // accurate but the schema doesn't distinguish here yet, so
            // return Object as the safe lower bound.
            None => BindingType::Object,
        },
        "config" => match rest {
            Some(field) => resolve_config_field(field, ctx),
            None => BindingType::Object,
        },
        "user" => match rest {
            Some(field) => resolve_user_field(field, ctx),
            // Bare `@user` is the whole identity row (object).
            None => BindingType::Object,
        },
        "trait" => match rest {
            Some(name) => {
                if ctx.sibling_trait_names.iter().any(|n| n == name) {
                    BindingType::TraitRef
                } else {
                    // Trait not found — leave as Unknown so the binding-
                    // type rule emits a "skip" rather than a false-typed
                    // diagnostic. A separate rule (`ORB_X_TRAIT_REF_NOT_FOUND`,
                    // not in this module's scope) catches the missing-
                    // trait case with the right error code.
                    BindingType::Unknown
                }
            }
            None => BindingType::Unknown,
        },
        _ => BindingType::Unknown,
    }
}

/// Element kind of an array-typed `@`-binding, when the declaration pins it:
/// a config knob's `items` (or `[string]`/`[number]`/`[object]` type tag), an
/// entity array field's `items`, or a payload field's `[T]` tag. `Unknown`
/// whenever the element kind is not declared, so callers defer.
pub fn resolve_array_element_type(value: &SExpression, ctx: &BindingContext) -> BindingType {
    let SExpression::String(s) = value else {
        return BindingType::Unknown;
    };
    let Some(path) = s.strip_prefix('@') else {
        return BindingType::Unknown;
    };
    let mut parts = path.splitn(2, '.');
    let (Some(scope), Some(field)) = (parts.next(), parts.next()) else {
        return BindingType::Unknown;
    };
    // Only a direct field reference: a deeper path is a different value.
    if field.contains('.') {
        return BindingType::Unknown;
    }
    match scope {
        "config" => {
            let Some(cf) = ctx.trait_def.config.as_ref().and_then(|c| c.get(field)) else {
                return BindingType::Unknown;
            };
            if let Some(items) = &cf.items {
                return field_definition_to_binding(items, ctx.entity_definitions);
            }
            array_tag_element(&cf.field_type)
        }
        "entity" => {
            let Some(entity) = ctx
                .trait_def
                .linked_entity
                .as_ref()
                .and_then(|n| ctx.entity_definitions.get(n))
            else {
                return BindingType::Unknown;
            };
            match entity
                .fields
                .iter()
                .find(|f| f.name.as_deref() == Some(field) && f.field_type == FieldType::Array)
                .and_then(|f| f.items.as_deref())
            {
                Some(items) => field_definition_to_binding(items, ctx.entity_definitions),
                None => BindingType::Unknown,
            }
        }
        "payload" => {
            let Some(declared) = ctx
                .event_payload
                .and_then(|p| p.iter().find(|d| d.name == field))
            else {
                return BindingType::Unknown;
            };
            array_tag_element(&declared.field_type)
        }
        _ => BindingType::Unknown,
    }
}

/// Element kind of a `[T]` type tag with a primitive or `object` element.
fn array_tag_element(tag: &str) -> BindingType {
    match tag {
        "[string]" => BindingType::String,
        "[number]" => BindingType::Number,
        "[boolean]" => BindingType::Boolean,
        "[object]" => BindingType::Object,
        _ => BindingType::Unknown,
    }
}

/// Look up `field` in the triggering event's declared payload schema.
fn resolve_payload_field(field: &str, ctx: &BindingContext) -> BindingType {
    let Some(payload) = ctx.event_payload else {
        return BindingType::Unknown;
    };
    // The path may go deeper than the payload's flat field list (e.g.
    // `@payload.row.title` when the payload declares `row: Article` and
    // the validator should recurse into the entity). For F3's first
    // landing we resolve only the top-level segment; nested payloads
    // (object-typed payload fields) come back as Object and the rule
    // falls back to "skip" for the deeper compare.
    let top = field.split('.').next().unwrap_or(field);
    for declared in payload.iter() {
        if declared.name == top {
            return field_type_str_to_binding(&declared.field_type);
        }
    }
    BindingType::Unknown
}

/// The type a `@entity.X` or `@payload.X` binding was declared with, by name: an entity field's whole-type
/// projection (a nullable sealed field lowers to a union carrying its source type there) or a payload field's type.
pub fn declared_source_type(value: &SExpression, ctx: &BindingContext) -> Option<String> {
    let SExpression::String(path) = value else {
        return None;
    };
    if let Some(field) = path.strip_prefix("@entity.") {
        let linked = ctx.trait_def.linked_entity.as_ref()?;
        let entity = ctx.entity_definitions.get(linked)?;
        let f = entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(field))?;
        return f
            .projected_from
            .as_ref()
            .filter(|p| p.field.is_none())
            .map(|p| p.type_name.clone());
    }
    let field = path.strip_prefix("@payload.")?;
    let declared = ctx.event_payload?.iter().find(|d| d.name == field)?;
    Some(
        declared
            .field_type
            .trim_end_matches('!')
            .trim_end_matches('?')
            .to_string(),
    )
}

/// Look up `field` on the trait's linked entity. Descends nested paths
/// (`focusedRow.authorName`) through object `properties` and relation targets
/// so a binding like `@entity.focusedRow.authorName` resolves to the leaf type
/// (`string`) instead of capping at the container field's `object`.
fn resolve_entity_field(field: &str, ctx: &BindingContext) -> BindingType {
    let Some(linked) = &ctx.trait_def.linked_entity else {
        return BindingType::Unknown;
    };
    let Some(entity) = ctx.entity_definitions.get(linked) else {
        return BindingType::Unknown;
    };
    let mut parts = field.split('.');
    let top = parts.next().unwrap_or(field);
    let rest: Vec<&str> = parts.collect();
    for f in entity.fields.iter() {
        if f.name.as_deref() == Some(top) {
            return descend_field_path(f, &rest, ctx);
        }
    }
    BindingType::Unknown
}

/// Resolve the binding type of `field` after walking the remaining dotted
/// segments `rest` from its definition. Walks object `properties` (struct
/// shapes lowered by the lolo lowerer) and relation targets. Falls back to
/// the container's own type when a segment can't be resolved (e.g. the
/// sub-shape is unknown), preserving the prior "object lower bound" behavior.
fn descend_field_path(f: &FieldDefinition, rest: &[&str], ctx: &BindingContext) -> BindingType {
    let Some((next, tail)) = rest.split_first() else {
        return field_definition_to_binding(f, ctx.entity_definitions);
    };
    match f.field_type {
        FieldType::Object => {
            if let Some(props) = &f.properties {
                if let Some(child) = props.get(*next) {
                    return descend_field_path(child, tail, ctx);
                }
            }
            BindingType::Object
        }
        FieldType::Relation => {
            if let Some(rel) = &f.relation {
                if let Some(target) = ctx.entity_definitions.get(&rel.entity) {
                    for tf in target.fields.iter() {
                        if tf.name.as_deref() == Some(*next) {
                            return descend_field_path(tf, tail, ctx);
                        }
                    }
                }
            }
            BindingType::Object
        }
        _ => field_definition_to_binding(f, ctx.entity_definitions),
    }
}

/// Look up `field` on the program's identity entity — the `[identity]`-tagged
/// entity plus the viewer if one is declared, else the viewer alone (see
/// `user_identity::with_viewer`).
/// Mirrors `user_identity.rs`'s field-EXISTENCE walk but resolves a TYPE for
/// prop-compare purposes instead of emitting an unknown-field diagnostic —
/// that check stays owned by `user_identity.rs`; this one only reuses its
/// shape source. Nested paths (`@user.profile.avatarUrl`) descend through
/// `descend_field_path`, same as `@entity`.
///
/// `ctx.entity_definitions` is the schema-wide registry (every orbital's
/// PRIMARY + auxiliary entities), so the declared `[identity]` entity is
/// found there when one exists — no schema/orbital plumbing needed here.
/// This does not see a narrower per-orbital `expects entity [identity] { shape }`
/// (that requires the orbital, which `BindingContext` doesn't carry); a
/// field outside the wider fallback shape just resolves `Unknown` (skip),
/// never a false mismatch.
fn resolve_user_field(field: &str, ctx: &BindingContext) -> BindingType {
    // Traits inlined from `uses[]` (`source_behavior.is_some()`) read
    // `@user` against the IMPORTED behavior's own contract, not the
    // consumer's — checking them against the consumer's identity shape
    // manufactures false mismatches (G-EXPECT-DERIVE-IMPORTS, see the same
    // exemption in `user_identity.rs`). Skip.
    if ctx.trait_def.source_behavior.is_some() {
        return BindingType::Unknown;
    }
    let identity = match ctx.entity_definitions.values().find(|e| e.identity) {
        Some(e) => super::user_identity::with_viewer(e),
        None => super::user_identity::default_identity_shape(),
    };
    let mut parts = field.split('.');
    let top = parts.next().unwrap_or(field);
    let rest: Vec<&str> = parts.collect();
    for f in identity.fields.iter() {
        if f.name.as_deref() == Some(top) {
            return descend_field_path(f, &rest, ctx);
        }
    }
    BindingType::Unknown
}

/// Look up `field` on the trait's `config` block.
fn resolve_config_field(field: &str, ctx: &BindingContext) -> BindingType {
    let Some(config) = &ctx.trait_def.config else {
        return BindingType::Unknown;
    };
    let top = field.split('.').next().unwrap_or(field);
    let Some(cf) = config.get(top) else {
        return BindingType::Unknown;
    };
    config_field_to_binding(cf)
}

/// Convert the IR's `FieldType` enum to the resolver's lattice.
fn field_type_to_binding(ft: &FieldType) -> BindingType {
    match ft {
        FieldType::String => BindingType::String,
        FieldType::Number | FieldType::Int | FieldType::Sized(_) => BindingType::Number,
        FieldType::Boolean => BindingType::Boolean,
        FieldType::Date | FieldType::Datetime | FieldType::Timestamp | FieldType::Instant => {
            BindingType::Datetime
        }
        // Semantic domains bind as strings — a render slot treats them as text.
        FieldType::Email
        | FieldType::Url
        | FieldType::Phone
        | FieldType::Uuid
        | FieldType::Image => BindingType::String,
        // Money binds as a number; File binds as an object (the canonical
        // {name, url, mimeType, sizeBytes} struct value).
        FieldType::Money => BindingType::Number,
        FieldType::File => BindingType::Object,
        FieldType::Bytes => BindingType::String,
        FieldType::CallKey => BindingType::String,
        FieldType::Array | FieldType::Tuple => BindingType::Array,
        FieldType::Object => BindingType::Object,
        FieldType::Enum => BindingType::String,
        FieldType::Relation => BindingType::Object,
        // `trait`-typed config values are `@trait.X` strings at the binding
        // level — the codegen / runtime substitute them with embedded UI later.
        FieldType::Trait => BindingType::String,
        // An orbital value is data (an import body), never rendered.
        FieldType::Orbital => BindingType::Object,
        FieldType::Behavior => BindingType::Object,
        // `slot` and `pattern` resolve to strings at the binding level; so
        // does `event` (Phase B struct-field carrier) — an event name is a
        // string at the render-binding lattice, same as the scalar
        // config-knob `event` type.
        FieldType::Slot
        | FieldType::Pattern
        | FieldType::Event
        | FieldType::EventAddress
        | FieldType::Field => BindingType::String,
        // Renderable content binds like an object (a pattern tree) — and the
        // prop-compat rule already accepts anything where `node` is expected.
        FieldType::Node => BindingType::Object,
        FieldType::SExpr => BindingType::Unknown,
        // Every variant is an object, so a union binds like one.
        FieldType::Union => BindingType::Object,
        FieldType::TypeVar(_) => BindingType::Var,
    }
}

/// Convert a `FieldDefinition` to a `BindingType`, handling the relation
/// case by recording the target entity name when available.
fn field_definition_to_binding(
    f: &FieldDefinition,
    _entity_definitions: &HashMap<String, EntityDefinition>,
) -> BindingType {
    if let FieldType::Relation = f.field_type {
        if let Some(rel) = &f.relation {
            return BindingType::Entity(rel.entity.clone());
        }
    }
    field_type_to_binding(&f.field_type)
}

/// Convert a `ConfigField`'s string-coded type to the resolver's lattice.
/// Config fields use a free-form `field_type: String` in IR (e.g.
/// `"string"`, `"number"`, `"object"`, `"\"a\" | \"b\""`); we map the
/// common cases and fall back to `Unknown` for unions / unrecognised forms.
fn config_field_to_binding(cf: &ConfigField) -> BindingType {
    match cf.field_type.as_str() {
        "string" => BindingType::String,
        "number" => BindingType::Number,
        "boolean" => BindingType::Boolean,
        "array" | "[object]" | "[string]" | "[number]" => BindingType::Array,
        // A named-element array (`[FieldSpec]`) is still an array; its element
        // kind is read separately from the knob's `items`.
        t if t.len() > 2 && t.starts_with('[') && t.ends_with(']') => BindingType::Array,
        "object" => BindingType::Object,
        "datetime" | "date" | "timestamp" | "instant" => BindingType::Datetime,
        // Literal-union types like `"underline" | "default" | "pills"`
        // resolve to string (every member is a string literal). Cheap
        // heuristic: if the type contains a `"`, assume string-union.
        s if s.contains('"') => BindingType::String,
        s if s.len() > 1 && s.starts_with('$') => BindingType::Var,
        _ => BindingType::Unknown,
    }
}

/// Map the IR's payload-field `field_type` string to the resolver's
/// lattice. Mirrors what trait `payloadSchema` entries serialize as.
fn field_type_str_to_binding(s: &str) -> BindingType {
    match s {
        "string" | "string!" => BindingType::String,
        "number" | "number!" => BindingType::Number,
        "boolean" | "boolean!" => BindingType::Boolean,
        "array" | "[object]" | "[string]" | "[number]" => BindingType::Array,
        "object" | "object!" => BindingType::Object,
        "datetime" | "date" | "timestamp" | "instant" | "instant!" => BindingType::Datetime,
        "null" => BindingType::Null,
        s if s.len() > 1 && s.starts_with('$') => BindingType::Var,
        // Typed entity references (e.g. `Article`, `Article!`) — recorded
        // as Entity(name) so future rules can recurse into the entity's
        // fields. Strip optional `!` suffix.
        s => {
            let trimmed = s.trim_end_matches('!');
            if orbital_core_base::type_compat::short_type_name(trimmed)
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            {
                BindingType::Entity(trimmed.to_string())
            } else {
                BindingType::Unknown
            }
        }
    }
}

/// Resolve an SExpression array value. Two cases matter:
///
/// 1. `["op-name", arg, arg]` — an operator call. Look up the operator's
///    `return_type` (and `return_semantics` for input-dependent kinds).
/// 2. Anything else (inline array literal `["a", "b"]`) — pure data,
///    type Array.
fn resolve_array_value(arr: &[SExpression], ctx: &BindingContext) -> BindingType {
    let Some(first) = arr.first() else {
        return BindingType::Array;
    };
    let SExpression::String(op_name) = first else {
        return BindingType::Array;
    };
    let Some(meta) = get_canonical_operator(op_name) else {
        return BindingType::Array;
    };

    // Input-dependent return type: derive from the args, not the static
    // `return_type`. Currently we only handle the cases where the answer
    // is determinable without descending into per-arg evaluation; the
    // rest fall back to the static `return_type`. Rich derivation
    // (lambda-result, branch-union union widening) lands when the L2.5
    // arithmetic-context rule needs it.
    if let Some(semantics) = meta.return_semantics {
        match semantics {
            "branch-union" => {
                // `(if cond X Y)` — return type is union of arg<1> and
                // arg<2>. If both branches resolve to the same type,
                // that's the result; else Unknown (we don't model
                // unions yet).
                if arr.len() < 4 {
                    return BindingType::Unknown;
                }
                let then_t = resolve_binding_type(&arr[2], ctx);
                let else_t = resolve_binding_type(&arr[3], ctx);
                if then_t == else_t {
                    then_t
                } else {
                    BindingType::Unknown
                }
            }
            "element-of-arg<0>" | "element-of-arg<0>?" => {
                // Element type of arg<0>'s array. Not derivable without
                // an array-of-T type model; v1 returns Unknown so the
                // L2 rule skips. Future: when arg<0> is `@entity.X`
                // with `array` field type whose `items` carries a typed
                // FieldDefinition, recurse into that.
                BindingType::Unknown
            }
            "lambda-result" => {
                // `array/map (xs (fn x body))` returns Array<typeof body>.
                // Without lambda body inference, return Array as the
                // best lower bound.
                BindingType::Array
            }
            "identity-of-arg<0>" => {
                // `array/filter` / `array/reject` preserve input type.
                if arr.len() >= 2 {
                    resolve_binding_type(&arr[1], ctx)
                } else {
                    BindingType::Array
                }
            }
            _ => declared_return_to_binding(meta, arr, ctx),
        }
    } else {
        declared_return_to_binding(meta, arr, ctx)
    }
}

/// The result of an operator call under the lattice, read off its declared signature: a primitive result is
/// itself, a list/record result is `Array`/`Object`, and a result that is a type variable is the type its
/// arguments of that variable share (`(or @payload.n 10)` is a `Number`), else `Unknown`.
fn declared_return_to_binding(
    meta: &orbital_core_base::language::canonical_operators::OperatorMeta,
    arr: &[SExpression],
    ctx: &BindingContext,
) -> BindingType {
    use orbital_core_base::language::registry::{
        get_operator, OperatorTypeRef, OperatorTypeRefKind,
    };
    match meta.returns {
        OperatorTypeRef::Primitive(name) => static_return_to_binding(name),
        OperatorTypeRef::Tagged(OperatorTypeRefKind::Var { name, .. }) => {
            let Some(op) = get_operator(meta.name) else {
                return BindingType::Unknown;
            };
            let signature = orbital_core_base::signature::Signature::of(op);
            let mut shared: Option<BindingType> = None;
            for (index, arg) in arr.iter().skip(1).enumerate() {
                let bare = matches!(
                    signature.formal(index),
                    Some(OperatorTypeRef::Tagged(OperatorTypeRefKind::Var { name: n, .. })) if n == name
                );
                if !bare {
                    continue;
                }
                let ty = resolve_binding_type(arg, ctx);
                match &shared {
                    None => shared = Some(ty),
                    Some(prior) if *prior == ty => {}
                    Some(_) => return BindingType::Unknown,
                }
            }
            shared.unwrap_or(BindingType::Unknown)
        }
        other => static_return_to_binding(other.kind_name()),
    }
}

/// Map an operator's static `return_type` string to the resolver's
/// lattice. Mirrors `field_type_str_to_binding` but covers the operator
/// vocabulary (which adds `"void"` and `"any"`).
fn static_return_to_binding(s: &str) -> BindingType {
    match s {
        "string" => BindingType::String,
        "number" => BindingType::Number,
        "boolean" => BindingType::Boolean,
        "array" => BindingType::Array,
        "object" => BindingType::Object,
        "null" => BindingType::Null,
        _ => BindingType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbital_core_base::schema::{StateMachine, StringOrArray, TransitionDefinition};

    fn empty_trait(name: &str, linked: Option<&str>) -> TraitDefinition {
        TraitDefinition {
            dispatch: None,
            effect_row: None,
            type_params: None,
            type_args: None,
            name: name.to_string(),
            entity_rebindable: None,
            entity_contract: None,
            description: None,
            linked_entity: linked.map(|s| s.to_string()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![],
                events: None,
                transitions: vec![],
            }
            .into(),
            ticks: None,
            config: None,
            data_entities: None,
            source_declaration: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
        }
    }

    #[test]
    fn resolves_literal_primitives() {
        let t = empty_trait("T", None);
        let entities: HashMap<String, EntityDefinition> = HashMap::new();
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: None,
            sibling_trait_names: &[],
        };
        assert_eq!(
            resolve_binding_type(&SExpression::String(("hello".to_string()).into()), &ctx),
            BindingType::String
        );
        assert_eq!(
            resolve_binding_type(&SExpression::Number(42.0), &ctx),
            BindingType::Number
        );
        assert_eq!(
            resolve_binding_type(&SExpression::Bool(true), &ctx),
            BindingType::Boolean
        );
    }

    #[test]
    fn resolves_payload_field_to_declared_type() {
        let t = empty_trait("T", None);
        let entities: HashMap<String, EntityDefinition> = HashMap::new();
        let payload = vec![PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "tabId".to_string(),
            field_type: "string".to_string(),
            required: Some(true),
            description: None,
            properties: None,
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
            relation: None,
        }];
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: Some(&payload),
            sibling_trait_names: &[],
        };
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@payload.tabId".to_string()).into()),
                &ctx
            ),
            BindingType::String
        );
        // Unknown field falls through to Unknown
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@payload.notDeclared".to_string()).into()),
                &ctx
            ),
            BindingType::Unknown
        );
    }

    #[test]
    fn resolves_entity_field_to_declared_type() {
        let t = empty_trait("T", Some("Article"));
        let mut entities: HashMap<String, EntityDefinition> = HashMap::new();
        entities.insert(
            "Article".to_string(),
            EntityDefinition {
                timestamps: None,
                soft_delete: None,
                description: None,
                visual_prompt: None,
                asset_ref: None,
                store: None,
                shared: false,
                identity: false,
                browser: false,
                locale_instances: None,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Article".to_string(),
                collection: None,
                persistence: None,
                instances: None,
                fields: vec![FieldDefinition {
                    nullable: None,
                    additional_properties: None,
                    mock: None,
                    merged_from: None,
                    name: Some("title".to_string()),
                    field_type: FieldType::String,
                    required: Some(true),
                    primary_key: None,
                    unique: None,
                    default: None,
                    values: None,
                    relation: None,
                    items: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                    projected_from: None,
                    record: None,
                    derived: None,
                }],
                id: None,
                invariants: None,
                indexes: None,
            },
        );
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: None,
            sibling_trait_names: &[],
        };
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@entity.title".to_string()).into()),
                &ctx
            ),
            BindingType::String
        );
    }

    #[test]
    fn resolves_user_field_against_implicit_default_shape() {
        // No `[identity]` entity declared anywhere in `entity_definitions` —
        // `@user` is the viewer alone (`user_identity::default_identity_shape()`).
        let t = empty_trait("T", None);
        let entities: HashMap<String, EntityDefinition> = HashMap::new();
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: None,
            sibling_trait_names: &[],
        };
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@user.name".to_string()).into()),
                &ctx
            ),
            BindingType::String
        );
        // Field outside the default shape → Unknown, not a false mismatch.
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@user.notReal".to_string()).into()),
                &ctx
            ),
            BindingType::Unknown
        );
    }

    #[test]
    fn resolves_user_field_against_declared_identity_entity() {
        // A real `[identity]` entity in `entity_definitions` adds its fields to the viewer's.
        let t = empty_trait("T", None);
        let mut entities: HashMap<String, EntityDefinition> = HashMap::new();
        entities.insert(
            "Member".to_string(),
            EntityDefinition {
                timestamps: None,
                soft_delete: None,
                description: None,
                visual_prompt: None,
                asset_ref: None,
                store: None,
                shared: false,
                identity: true,
                browser: false,
                locale_instances: None,
                read_policy: None,
                create_policy: None,
                update_policy: None,
                delete_policy: None,
                access_waivers: None,
                name: "Member".to_string(),
                collection: None,
                persistence: None,
                instances: None,
                fields: vec![FieldDefinition {
                    nullable: None,
                    additional_properties: None,
                    mock: None,
                    merged_from: None,
                    name: Some("loyaltyPoints".to_string()),
                    field_type: FieldType::Number,
                    required: Some(true),
                    primary_key: None,
                    unique: None,
                    default: None,
                    values: None,
                    relation: None,
                    items: None,
                    properties: None,
                    intrinsic: None,
                    description: None,
                    synonyms: None,
                    projected_from: None,
                    record: None,
                    derived: None,
                }],
                id: None,
                invariants: None,
                indexes: None,
            },
        );
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: None,
            sibling_trait_names: &[],
        };
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@user.loyaltyPoints".to_string()).into()),
                &ctx
            ),
            BindingType::Number
        );
        // `@user` is the viewer plus the roster row: `role` comes from the
        // viewer; a field on neither resolves Unknown (skip).
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@user.role".to_string()).into()),
                &ctx
            ),
            BindingType::String
        );
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@user.notReal".to_string()).into()),
                &ctx
            ),
            BindingType::Unknown
        );
    }

    #[test]
    fn skips_user_field_for_trait_inlined_from_uses() {
        // A trait carrying `source_behavior` was inlined from a `uses[]`
        // import; its internal `@user` reads are the IMPORTED behavior's
        // own contract, not the consumer's (G-EXPECT-DERIVE-IMPORTS) — the
        // resolver must skip rather than type-check against the consumer's
        // identity shape, even though `name` exists on the default shape.
        let mut t = empty_trait("T", None);
        t.source_behavior = Some(orbital_core_base::schema::SourceBehaviorMetadata {
            behavior: "std-browse".to_string(),
            alias: "Browse".to_string(),
            original_name: "BrowseList".to_string(),
        });
        let entities: HashMap<String, EntityDefinition> = HashMap::new();
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: None,
            sibling_trait_names: &[],
        };
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@user.name".to_string()).into()),
                &ctx
            ),
            BindingType::Unknown
        );
    }

    #[test]
    fn resolves_trait_ref_when_sibling_exists() {
        let t = empty_trait("T", None);
        let entities: HashMap<String, EntityDefinition> = HashMap::new();
        let siblings = vec!["CmsHubTabs".to_string()];
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: None,
            sibling_trait_names: &siblings,
        };
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@trait.CmsHubTabs".to_string()).into()),
                &ctx
            ),
            BindingType::TraitRef
        );
        // Missing sibling → Unknown (caller's other rule catches the
        // missing-trait error with a more specific code).
        assert_eq!(
            resolve_binding_type(
                &SExpression::String(("@trait.MissingOne".to_string()).into()),
                &ctx
            ),
            BindingType::Unknown
        );
    }

    #[test]
    fn resolves_default_operator_coalesce() {
        let t = empty_trait("T", None);
        let entities: HashMap<String, EntityDefinition> = HashMap::new();
        let payload = vec![PayloadField {
            nullable: None,
            additional_properties: None,
            values: None,
            name: "totalCount".to_string(),
            field_type: "number".to_string(),
            required: None,
            description: None,
            properties: None,
            type_when: None,
            entity: None,
            projected_from: None,
            items: None,
            relation: None,
        }];
        let ctx = BindingContext {
            trait_def: &t,
            entity_definitions: &entities,
            event_payload: Some(&payload),
            sibling_trait_names: &[],
        };
        // `(default @payload.totalCount 10)` — both args resolve to Number,
        // so `default`'s signature (`T, T -> T`) returns Number. `or` is
        // boolean-only: it is no coalesce.
        let expr = SExpression::Array(
            vec![
                SExpression::String(("default".to_string()).into()),
                SExpression::String(("@payload.totalCount".to_string()).into()),
                SExpression::Number(10.0),
            ]
            .into(),
        );
        assert_eq!(resolve_binding_type(&expr, &ctx), BindingType::Number);
    }

    fn config_field_of(field_type: &str) -> ConfigField {
        serde_json::from_value(serde_json::json!({ "type": field_type }))
            .expect("config field deserializes")
    }

    #[test]
    fn every_point_in_time_binds_as_text_and_a_duration_stays_a_number() {
        for ft in [
            FieldType::Date,
            FieldType::Datetime,
            FieldType::Timestamp,
            FieldType::Instant,
        ] {
            assert_eq!(field_type_to_binding(&ft), BindingType::Datetime, "{ft:?}");
        }
        for tag in ["date", "datetime", "timestamp", "instant", "instant!"] {
            assert_eq!(
                field_type_str_to_binding(tag),
                BindingType::Datetime,
                "{tag}"
            );
        }
        for tag in ["date", "datetime", "timestamp", "instant"] {
            let cf = config_field_of(tag);
            assert_eq!(config_field_to_binding(&cf), BindingType::Datetime, "{tag}");
        }
        assert_eq!(
            field_type_to_binding(&FieldType::Number),
            BindingType::Number,
            "control"
        );
        assert_eq!(
            field_type_str_to_binding("number"),
            BindingType::Number,
            "control"
        );
    }

    #[test]
    fn registry_type_round_trips_to_classify_prop_type_vocab() {
        // The registry's prop-type vocabulary must match what
        // classify_prop_type in render_ui.rs produces; otherwise
        // downstream comparison is name-mismatched. Sanity-check the
        // mapping for the common cases.
        assert_eq!(BindingType::String.registry_type(), "string");
        assert_eq!(BindingType::Number.registry_type(), "number");
        assert_eq!(BindingType::Boolean.registry_type(), "boolean");
        assert_eq!(BindingType::Array.registry_type(), "array");
        assert_eq!(BindingType::Object.registry_type(), "object");
        assert_eq!(BindingType::Datetime.registry_type(), "string");
        assert_eq!(field_type_str_to_binding("$t"), BindingType::Var);
        assert_eq!(
            field_type_str_to_binding("$"),
            BindingType::Unknown,
            "control: no name"
        );
    }

    // Intentionally referenced to silence unused-import warnings for
    // shapes the future L2.1 rule will need (transitions to look up
    // event payload schema by event key).
    #[allow(dead_code)]
    fn _shape_anchor() -> (StringOrArray, TransitionDefinition) {
        unimplemented!()
    }
}
