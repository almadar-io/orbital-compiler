//! Unified Error Types for Orbital Validation
//!
//! This module provides comprehensive error types that match the TypeScript
//! validation error codes from `builder/packages/shared/src/orbitals/validation/`.

use std::fmt;

/// Severity level for validation issues
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocking error - schema cannot be compiled
    Error,
    /// Non-blocking warning - schema can compile but may have issues
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => write!(f, "ERROR"),
            Severity::Warning => write!(f, "WARNING"),
        }
    }
}

/// Error codes matching TypeScript validation error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    // =========================================================================
    // Entity Errors (ORB_E_*)
    // =========================================================================
    /// Entity is missing a name
    EntityMissingName,
    /// Entity has no fields defined
    EntityNoFields,
    /// Field has an invalid type
    EntityInvalidFieldType,
    /// Duplicate field name in entity
    EntityDuplicateField,
    /// Field name doesn't match camelCase pattern
    EntityInvalidFieldName,
    /// Persistent entity missing collection property
    EntityMissingCollection,
    /// Relation target entity doesn't exist
    EntityInvalidRelation,
    /// Enum field has no values
    EntityEmptyEnumValues,
    /// A literal default on a semantic-domain field does not satisfy that domain
    /// (`email : email = "not-an-email"`).
    EntitySemanticValueInvalid,
    /// Phase F.3: an entity field declares `values: [...]` (literal-union)
    /// AND `default: <X>`, but X is not one of the declared values. Error.
    LiteralUnionDefaultMismatch,

    // =========================================================================
    // Trait Errors (ORB_T_*)
    // =========================================================================
    /// Trait reference not found
    TraitUndefined,
    /// Trait has invalid format
    TraitInvalidFormat,
    /// Trait category is not valid
    TraitInvalidCategory,
    /// State machine has no initial state
    TraitNoInitialState,
    /// State machine has multiple initial states
    TraitMultipleInitialStates,
    /// Duplicate state name in state machine
    TraitDuplicateState,
    /// Transition references non-existent state
    TraitInvalidTransition,
    /// Duplicate transition (same from/event without guard)
    TraitDuplicateTransition,
    /// Missing INIT transition on initial state
    TraitMissingInitTransition,
    /// A declared state is unreachable from the trait's initial state via any
    /// transition path. BFS from the initial state over `transitions` (skipping
    /// `from == "*"` wildcards and the boot `INIT` self-loop on the initial
    /// state) never reaches it — dead topology. Surfaces only as a generic
    /// "uncovered" in the verifier today; caught here at author time. Worked
    /// example: std-booking-system ScheduleDisplay's `displaying`/`refreshing`
    /// cluster.
    TraitStateUnreachable,
    /// INIT transition missing fetch effect for linked entity
    TraitInitMissingFetch,
    /// Interaction trait missing render_ui effects
    TraitMissingRenderUI,
    /// Deprecated ui property used
    TraitDeprecatedUI,
    /// Duplicate trait name across orbitals
    TraitDuplicateName,
    /// Event not defined but used in transition
    TraitUndefinedEvent,
    /// Config field typed `slot` has a default value not in `VALID_SLOTS`
    TraitInvalidSlotDefault,
    /// Config field typed `pattern` has a default value not in `PATTERN_TYPES`
    TraitInvalidPatternDefault,
    /// Config field typed `secret` has a non-string default — the value is a
    /// credential REFERENCE (env-var key name), never secret material (I-29)
    TraitSecretDefaultNotString,
    /// Config field typed `event` holds a value that is not a member of the
    /// trait's post-rename event set (SCAN-EVENT-KNOB-1). Definer-position
    /// knobs (A-UNIFY) skip this membership check — their value CREATES the
    /// event name — but still land here if the default isn't a plain string.
    TraitInvalidEventDefault,
    /// Config field's default value doesn't match its declared structural
    /// shape (array vs scalar, object vs string, missing required, etc.).
    /// Fires after the typed-config carriers (`items` / `properties` /
    /// `values`) make the declared shape concrete enough to check against.
    TraitConfigShapeMismatch,
    /// Config field uses `[object]` as its declared type. Banned because
    /// it lowers to an untyped `Array { inner: Json }` — the same shape
    /// the strict-lint floor catches as `Record<string, FieldValue |
    /// undefined>[]`. Authors should declare a named struct alias (e.g.
    /// `type FieldConfig = { name: string, label: string, ... }`) and
    /// use `[FieldConfig]` so the catalog + codegen carry real types.
    TraitConfigObjectArrayForbidden,
    /// A declaration uses a generic type — bare `json`, `object` (without
    /// declared `properties`), or `any` — in an entity field, config knob,
    /// type-alias member, or event payload field. Generic types are being
    /// eliminated from the language: renderable content belongs in `node`,
    /// structured data in inline `{ k: T }` shapes / named aliases /
    /// `Map K V`, and genuinely polymorphic atoms in declared type
    /// parameters. Hard error since 2026-09-01 (both behavior packages
    /// report zero occurrences).
    GenericTypeDeprecated,
    /// A `node`-typed entity field receives a value the validator cannot
    /// statically check (a computed expression rather than a pattern-tree
    /// literal). Non-blocking: computed renderable content is legitimate,
    /// but its callback wiring is unchecked — prefer literal fragments
    /// (optionally via `let` bindings, which ARE resolved and checked).
    NodeFieldUncheckedContent,
    /// A render-ui pattern literal is assigned to a field NOT typed `node`
    /// — renderable content hiding in a generic/structured field escapes
    /// every render-ui check. Type the field `node` (or `[node]`). Error
    /// severity.
    RenderableContentInGenericField,
    /// Config field's `@tier "..."` annotation uses a value outside the
    /// allowed set. Allowed values: `essential`, `customization`,
    /// `advanced`, `internal`. Defense-in-depth — the parser allowlists
    /// the four values today; this L2 rule catches future regressions
    /// and any malformed `.orb` IR that bypasses the parser.
    TraitConfigTierInvalid,
    /// A trait imported from an atom carries a `linkedEntity` rebind, but the
    /// atom's binding is not marked `@rebindable`. The binding is fixed and
    /// the consumer may not redirect it. See `@rebindable` in `.lolo`.
    TraitEntityNotRebindable,
    /// A `@rebindable` rebind points at an entity that lacks fields the trait
    /// reads (its inferred `entityContract.requires`). The target would break
    /// the behavior.
    RebindMissingEntityFields,
    /// A `.lolo` event alias's `(E : Entity)` type parameter — lowered to the
    /// `"$E"` payload sentinel by `orbital-lolo` (increment 3-I,
    /// `docs/Almadar_LOLO.md` ~line 315/1387) — survived the inline phase's
    /// substitution pass because the trait carrying the sentinel has no
    /// `linkedEntity` to monomorphize it against. Fix: bind the trait to an
    /// entity (or, once explicit application ships, apply the alias with a
    /// concrete argument).
    UnresolvedTypeParam,
    /// A call-site `events: { OLD: NEW }` rename names an OLD source key that
    /// does not exist in the imported trait's event set (its `emits[].event` ∪
    /// `state_machine.transitions[].event`). The rename silently no-ops:
    /// nothing is renamed and the author's intent is lost. Surfaced from the
    /// inline phase where the imported trait's pre-rename event set is still
    /// visible.
    EventRenameUnknownKey,
    /// A trait Reference's call-site `config` block supplies a key that
    /// matches no declared config knob on the referenced trait — the
    /// override silently no-ops (e.g. a stale `entity=` left over from a
    /// generator regression, or a knob the atom never declared).
    TraitConfigUnknownCallSiteKey,
    /// A trait Reference's call site omits a key for a knob the referenced
    /// trait declares `!` (required). Any value form (literal, `@config.X`
    /// forward, `@entity.*`, payload ref) satisfies the requirement — only
    /// a fully missing key fires. The atom's own page binding is exempt.
    TraitConfigMissingRequiredKey,

    // =========================================================================
    // Page Errors (ORB_P_*)
    // =========================================================================
    /// Page is missing a name
    PageMissingName,
    /// Page is missing a path
    PageMissingPath,
    /// Page path is invalid (must start with /)
    PageInvalidPath,
    /// Duplicate page path
    PageDuplicatePath,
    /// Page is missing traits array
    PageMissingTraits,
    /// Page traits array is empty
    PageEmptyTraits,
    /// Orbital has no pages defined
    OrbitalMissingPages,
    /// An `orbital X = Alias.orbitals.Y { config { … } } }` call-site
    /// override names a key the upstream orbital's declared `config {}`
    /// does not have (`Almadar_Orbital_Import.md` §4.5).
    OrbitalRefConfigUnknownKey,
    /// An `orbital X = Alias.orbitals.Y { omit { … } } }` call site drops a
    /// trait that a surviving trait still embeds via `@trait.<X>` — the
    /// embed token has nothing left to render.
    OrbitalRefOmitEmbeddedTrait,
    /// A declared `Orbital.config` or `OrbitalSchema.config` knob
    /// (`Almadar_Orbital_Import.md` §4.5) that no trait in the schema
    /// forwards (`config[*].default == "@config.<knob>"`) — dead: nothing
    /// publishes it, so no call site can ever reach it.
    OrbitalConfigDeadKnob,
    /// Static sections are forbidden (use render_ui)
    PageSectionsForbidden,
    /// Invalid trait reference on page
    PageInvalidTraitRef,
    /// Invalid view type
    PageInvalidViewType,

    // =========================================================================
    // Slot Errors (ORB_SLOT_*)
    // =========================================================================
    /// Main slot not covered by any trait
    SlotMainNotCovered,
    /// Multiple traits rendering to same slot (INIT state)
    SlotContention,
    /// Multiple traits rendering to same slot in a reachable post-INIT state
    /// (emitted as a warning; INIT-state case still errors via `SlotContention`)
    SlotContentionRuntime,
    /// Two or more traits mounted on one page repaint the same slot from inside
    /// a `ticks` block, with no guard proving either one dead. Tick bodies run
    /// every frame, so they paint together rather than alternating.
    SlotContentionTick,
    /// Invalid slot nesting (e.g., modal in modal)
    SlotInvalidNesting,
    /// Priority conflict between traits on same slot
    SlotPriorityConflict,
    /// Invalid slot name
    SlotInvalidName,
    /// HUD slot used in non-game context
    SlotHudNonGame,

    // =========================================================================
    // Render-UI Errors (ORB_RUI_*)
    // =========================================================================
    /// Invalid slot in render_ui effect
    RenderUIInvalidSlot,
    /// Invalid pattern type
    RenderUIInvalidPattern,
    /// Pattern missing type property
    RenderUIMissingPatternType,
    /// Invalid prop for pattern
    RenderUIInvalidProp,
    /// A non-drawable child composed under a canvas draw-host. A draw-host paints
    /// only neutral drawable descriptors (draw-sprite / draw-shape / draw-text /
    /// draw-*-layer); DOM chrome belongs in a `hud`/`overlay` slot.
    RenderUIChildNotDrawable,
    /// Missing required prop for pattern
    RenderUIMissingRequiredProp,
    /// Invalid itemAction configuration
    RenderUIInvalidItemAction,
    /// Unknown itemAction prop (e.g., hallucinated 'condition')
    RenderUIUnknownItemActionProp,
    /// Button/link missing action prop
    RenderUIMissingAction,
    /// Nested field path references invalid relation or field
    RenderUIInvalidFieldPath,
    /// Prop value type doesn't match expected type from registry
    RenderUIPropTypeMismatch,
    /// L2.1 keystone: render-ui prop bound to a `@payload.X` / `@entity.X`
    /// / `@config.X` / SExpression operator whose resolved type doesn't
    /// match any of the prop's declared `types` in the patterns registry.
    /// Distinct from `RenderUIPropTypeMismatch` (literal-only); this rule
    /// uses the F3 binding resolver to walk through bindings and
    /// SExpression operator return types (sharpened by F2's
    /// `returnSemantics` for input-dependent operators). Gap #6 in
    /// `docs/Almadar_Std_Verification.md`.
    RenderUIBindingTypeMismatch,
    /// L2.4: render-ui binds an array literal to a prop whose pattern
    /// declares per-element required fields (`items.properties.<field>.required`),
    /// and one or more elements omit a required field. Catches the
    /// landing-page/about-page/showcase class of bugs where the lolo
    /// `[StepCard]` type omits `id` but the consuming `step-flow-organism`
    /// pattern requires it — surfaced at codegen as TS2322 before this rule.
    /// Walks `OirValue::Array` element-by-element against
    /// `items.properties` from `patterns-registry.json`. Discovery context:
    /// `docs/Almadar_Std_Gaps.md` §13.
    RenderUIElementShapeMismatch,
    /// Entity name collides with a sibling trait's generated
    /// `<TraitName>State` or `<TraitName>Event` union type. The TS codegen
    /// emits both `import type { <EntityName> } from '@app/shared'` and
    /// `type <TraitName>State = '...'` / `type <TraitName>Event = '...'`
    /// in every server-handler / client-hook / view file — when the entity
    /// name equals one of those generated unions, the import collides with
    /// the local declaration and tsc raises TS2440 "Import declaration
    /// conflicts with local declaration".
    ///
    /// Defense-in-depth pair with the lolo lowering check
    /// `LowerError::EntityNameShadowsTraitUnion`: the lolo rule catches
    /// hand-authored `.lolo`, this validator rule catches `.orb` mutated
    /// directly by an agent or factory that bypassed the lolo parse path.
    /// Convention fix: rename the entity (e.g. `<TraitName>Data`).
    EntityNameShadowsTraitUnion,
    /// L2.2: render-ui prop with `kind: "event-ref"` or `kind: "callback"`
    /// bound to an event-name string whose declared payload schema in
    /// the trait's `emits {}` block doesn't match the component's
    /// documented bus emit shape (registry's `emitPayloadSchema` for
    /// event-ref props; `callbackArgs` for callback props). Gap #32 in
    /// `docs/Almadar_Std_Verification.md`.
    RenderUIEventBindingPayloadMismatch,
    /// L2.3: render-ui pattern carries a string literal that matches an
    /// event name no longer in the trait's event set after the inline
    /// phase applies an `events: { OLD: NEW }` override. Detects the
    /// stale literal that the compiler's events-rewrite pass should
    /// have rewritten — defense-in-depth for C1 (the compiler fix).
    /// Gap #30 in `docs/Almadar_Std_Verification.md`.
    RenderUIEventLiteralStale,

    // =========================================================================
    // Effect Errors (ORB_EFF_*)
    // =========================================================================
    /// call_service missing service name
    EffectCallServiceMissingService,
    /// call_service names a service in neither schema.services nor the
    /// services registry. Error severity.
    EffectCallServiceUnknownService,
    /// call_service missing action
    EffectCallServiceMissingAction,
    /// call_service missing onSuccess/onError handlers
    EffectCallServiceMissingHandlers,
    /// call_service params carry a key the action's registry contract does
    /// not declare (I-13 — the `recipient` vs `to` class)
    EffectCallServiceUnknownParam,
    /// call_service params omit a key the action's registry contract requires
    EffectCallServiceMissingParam,
    /// call_service literal param value contradicts the contract's param type
    EffectCallServiceParamType,
    /// set effect using invalid binding
    EffectSetInvalidBinding,
    /// set effect using @payload (should use @entity)
    EffectSetPayloadNotAllowed,
    /// fetch effect referencing invalid entity
    EffectFetchInvalidEntity,
    /// fetch effect include references non-existent or non-relation field
    EffectFetchInvalidInclude,
    /// Multiple render_ui effects targeting same slot
    EffectSlotConflict,
    /// Unknown effect type
    EffectUnknownType,
    /// Navigate effect missing required payload for target page
    EffectNavigateMissingPayload,
    /// Navigate effect references a path that doesn't resolve to any
    /// declared `pages[].path` across the resolved schema. Post-inline
    /// backstop for L1.2 — catches navigate calls injected by molecule-
    /// level `effects: { ... }` overrides AND navigate calls that
    /// targeted imported pages from `uses` refs that L1 couldn't see.
    EffectNavigateTargetUnreachable,
    /// Trait declares an event in its `emits[]` block but no transition
    /// fires it via `(emit X)` or operator emit-config success/failure.
    /// Usually a copy-paste leftover or a stale declaration after a
    /// rename; gap #2 defense in `docs/Almadar_Std_Verification.md`.
    EmitDeclaredButUnfired,
    /// `emits { @config.<knob> -> ... }` references a knob that isn't a
    /// DECLARED string-typed config field with a default (Option B contract).
    /// Left unresolved by the inline phase; surfaces here with the specific
    /// clause that failed.
    EmitConfigRefInvalid,
    /// render-ui has entity prop but no fetch effect for that entity in the same transition
    EffectMissingFetchForEntityPattern,
    /// `emit:` config carries a key the operator doesn't support
    /// (e.g. `on_message` on `fetch`, `failure` on `set`).
    EffectEmitKeyInvalid,
    /// Event name inside `emit:` isn't UPPER_SNAKE_CASE.
    EffectEmitInvalidName,
    /// Event in `emit:` isn't declared in the trait's `emits` array.
    EffectEmitUndeclaredEvent,
    /// Operator declares `requiresEmitSuccess: true` in canonical-operators.json
    /// but the effect call site is missing the trailing options object or its
    /// `emit.success` event key. Applies to fetch / persist / call-service /
    /// ref by default; any future operator can opt in via the same JSON flag.
    EffectMissingRequiredEmit,

    // =========================================================================
    // S-Expression Errors (SEXPR_*)
    // =========================================================================
    /// Unknown operator in S-expression
    SExprInvalidOperator,
    /// Wrong number of arguments for operator
    SExprArityMismatch,
    /// A `(fn …)` form in an argument position the operator does not take its
    /// lambda in — decidable wrong argument order, e.g. `(array/reduce arr (fn …) init)`
    SExprLambdaArgPosition,
    /// Effect operator used in guard context
    SExprEffectInGuard,
    /// Invalid binding reference
    SExprInvalidBinding,
    /// Invalid binding path syntax
    SExprInvalidBindingPath,
    /// Unknown entity in binding
    SExprUnknownEntity,
    /// Parse error in S-expression
    SExprParseError,

    // =========================================================================
    // Service Errors (ORB_SVC_*)
    // =========================================================================
    /// Duplicate service name
    ServiceDuplicateName,
    /// REST service missing baseUrl
    ServiceMissingBaseUrl,
    /// Invalid service URL
    ServiceInvalidUrl,
    /// Socket service missing events config
    ServiceMissingEvents,
    /// Reserved socket event name used
    ServiceReservedEvent,
    /// MCP service missing capabilities
    ServiceMissingCapabilities,
    /// MCP service missing serverPath
    ServiceMissingServerPath,
    /// Unknown service type
    ServiceUnknownType,
    /// Invalid integrator
    ServiceInvalidIntegrator,

    // =========================================================================
    // Cross-Orbital Errors (ORB_X_*)
    // =========================================================================
    /// Duplicate entity name across orbitals
    CrossOrbitalDuplicateEntity,
    /// Event listener without matching emitter
    CrossOrbitalOrphanListener,
    /// Event emitted but never listened to
    CrossOrbitalUnusedEmission,
    /// Circular dependency in event wiring
    CrossOrbitalCircularDependency,
    /// Event namespace collision
    CrossOrbitalEventCollision,
    /// Orbital missing name
    CrossOrbitalMissingName,
    /// Payload mismatch between emitter and listener
    CrossOrbitalPayloadMismatch,
    /// `listens` references a source trait or event that doesn't exist in
    /// the resolved IR. Defense-in-depth backstop for the L1 (`orbital-lolo`)
    /// `LowerError::ListenSourceUnresolved` check; surfaces the same defect
    /// after `use`-imports inline so atoms/molecules pulled into an orbital
    /// can't accidentally introduce dangling listens.
    CrossOrbitalListenSourceUnresolved,
    /// A `listens { Src.EVENT -> TRIGGER }` entry names an internal TRIGGER
    /// that no transition in the SAME trait reacts to. The bus would deliver
    /// the event but the state machine has no handler — the wire silently
    /// drops. Runs post-inline so the `events: { OLD: NEW }`-rename variant
    /// (where the rewrite renamed the transition event but the listen trigger
    /// drifted) is caught too. Defense-in-depth for the L1 (`orbital-lolo`)
    /// author-time check on hand-authored `.lolo`.
    ListenTriggerUnmatched,
    /// Composing trait's override map touches an event declared `scope: 'internal'`.
    /// Error severity. See COMP-7 in docs/Almadar_Agent_Behaviors_Plan.md.
    InternalEventExposed,
    /// A `type: 'relation'` field's `relation.entity` does not resolve to any
    /// entity declared in the same schema. Error severity; the entity-scope
    /// validator also emits `ORB_E_INVALID_RELATION` for the same condition.
    /// See COMP-9 in docs/Almadar_Agent_Behaviors_Plan.md.
    UnresolvedRelationTarget,
    /// A pattern prop tagged `kind: 'field-ref'` (scalar) or `itemKind: 'field-ref'`
    /// (array element) carries a value that does not resolve to any field on the
    /// linked entity. Error severity. See Phase 3.2/3.3 in
    /// docs/Almadar_Agent_Behaviors_Plan.md.
    UnresolvedPatternFieldRef,

    // =========================================================================
    // Binding Errors (ORB_BINDING_*)
    // =========================================================================
    /// Binding has invalid format
    BindingInvalidFormat,
    /// Unknown binding root
    BindingUnknownRoot,
    /// `@config.<key>` literal whose key no reachable config schema declares
    ConfigDanglingRef,
    /// Invalid binding path
    BindingInvalidPath,
    /// @state binding should not have path
    BindingStateNoPath,
    /// @now binding should not have path
    BindingNowNoPath,
    /// @payload not available in tick context
    BindingPayloadInTick,
    /// Unknown entity in @EntityName binding
    BindingUnknownEntity,
    /// Invalid field name in binding path
    BindingInvalidFieldName,
    /// Set target missing field path
    BindingSetTargetMissingPath,
    /// @payload.field references a field not declared in event payload schema
    BindingPayloadFieldUndeclared,
    /// @payload.field is declared but its type does not match the operator
    /// argument or comparison it appears in (e.g. string used where a number
    /// was expected). Warn-only in V2 Phase 0 rollout; escalates to error in
    /// the Phase 5 cleanup. Wired up in a follow-up pass over operator
    /// signatures; the class is declared here so the code path can reference
    /// it and emit warnings incrementally.
    BindingPayloadTypeIncompatible,
    /// `(set @entity.X ?payloadField)` writes a payload field whose declared
    /// type does not match the target entity field's declared type. The lolo
    /// crate catches this at author time for `.lolo`; this is the raw-`.orb`
    /// defense-in-depth backstop. Payload field types are sourced from the
    /// trait's `emits[].payload_schema` and `listens` payload metadata.
    SetValueTypeMismatch,
    /// `(set @entity.X literal)` assigns a string literal that is not a member
    /// of the target field's declared enum (`values: [...]`). The declared union
    /// was previously validated + emitted but never enforced — an out-of-set
    /// value passed silently. The lolo crate catches this at author time for
    /// `.lolo`; this is the raw-`.orb` defense-in-depth backstop.
    EnumValueNotMember,
    /// `(set @entity.X (op …))` assigns an operator/expression value whose
    /// inferred result type is incompatible with the target field's declared
    /// type (e.g. `(set @entity.count (str/concat …))` into a `number` field).
    /// The value's type is inferred *through* the S-expression via the canonical
    /// operator `return_semantics` lattice; an unknowable result (`Json`) never
    /// rejects (Gap G-LOLO-SET-OP-RETURN).
    SetReturnTypeMismatch,
    /// `(emit EVT {...})` literal payload is missing a field the event's
    /// declared payload marks as required. Error severity.
    EmitPayloadMissingRequired,
    /// `(emit EVT {...})` literal carries a field that is not declared on
    /// the event's payload schema. Error severity.
    EmitPayloadExtraField,
    /// `(emit EVT {...})` literal provides a value whose primitive type
    /// does not match the declared `PayloadField.field_type`. Error severity.
    EmitPayloadFieldTypeMismatch,
    /// An event/emit `payloadSchema` field name is not a valid TypeScript
    /// identifier (contains a dot, space, or leading digit). Codegen turns
    /// each field into a struct member, so `row.category` would emit broken
    /// TS. Validate-layer defense-in-depth for the lowering pass that
    /// flattens nested payload field names. Error severity.
    PayloadFieldNameInvalid,
    /// A render-ui pattern prop typed `EntityWith<K>` declares required entity
    /// fields (`requiredFields` in the registry), but the bound entity does not
    /// provide them and no remap/field-map supplies them. The component would
    /// render blank rows. Remap-aware (runs post-inline). Error severity.
    EntityPropContract,
    /// **RETIRED (2026-08-30) — superseded by `EmitPayloadNotProduced`, which
    /// checks the full shape rather than only the empty-payload case.** No
    /// longer raised anywhere; the variant is kept so external tooling matching
    /// on the wire string `ORB_EMIT_SUCCESS_PAYLOAD_RETURN_MISMATCH` does not
    /// break on an unknown code.
    ///
    /// `emit: { success: "X" }` config on a data operator declares success
    /// event X but X's declared payload does not match the operator's
    /// `effect.produces` at this call site. Warn-only.
    EmitSuccessPayloadReturnMismatch,
    /// **The payload invariant.** An event's declared payload type IS its
    /// access path: declaring `Survey` means the payload is the row (read
    /// `?name`); declaring `{ row : Survey }` means read `?row.name`. This
    /// fires when an event declares a field that the effect firing it does not
    /// actually emit — so every read of that field is `undefined` at runtime,
    /// even though `orb validate` and `orb verify` otherwise pass. Checked as
    /// width subtyping (`declared ⊆ produced`): declaring FEWER fields than are
    /// emitted is legal, declaring one that is never emitted is not. Replaces
    /// the narrower `EmitSuccessPayloadWrapped` (persist-only) and
    /// `EmitSuccessPayloadReturnMismatch` (empty-payload-only) checks.
    /// Error severity.
    EmitPayloadNotProduced,
    /// **RETIRED (2026-08-30) — superseded by `EmitPayloadNotProduced`.** This
    /// check only recognised a wrapper on `persist create`/`update` whose type
    /// named that exact entity; the general rule catches the same defect at any
    /// operator and in shapes this one was blind to (a wrapper naming a
    /// different entity, a field the operator never emits at all). Kept so
    /// external tooling matching `ORB_EMIT_SUCCESS_PAYLOAD_WRAPPED` still
    /// resolves.
    ///
    /// A `persist create`/`persist update` call's `emit: { success: "X" }`
    /// names an event X whose declared payload wraps the persisted entity
    /// under a single field (`type X = Event { row: EntityName }`), but
    /// `persist create`/`update` emits the persisted row directly (flat,
    /// unwrapped) on success in both @almadar/runtime and orbital-core —
    /// verified live 2026-08-30 (std-thread's `ThreadPostCreated`): a
    /// wrapped declaration validates and compiles clean but every
    /// `?row.<field>` read silently returns undefined at runtime. Warn-only.
    EmitSuccessPayloadWrapped,
    /// Phase F.2: two or more traits declare the same external event name in
    /// their `emits` arrays but disagree on the declared payload field list.
    /// Consumers that listen on the event will see one shape from one
    /// emitter and a different shape from another. Error severity.
    CrossEmitterDeclaredPayloadMismatch,

    /// @entity.field references a field not present on the linked entity
    BindingEntityFieldNotFound,
    /// @entity.field is referenced by render-ui / guard / effect but no
    /// transition in the same trait runs `(set @entity.field <value>)`. Per
    /// the explicit-binding rule, every `@entity.X` read must be backed by
    /// at least one explicit setter; otherwise the value is undefined and
    /// the component renders empty. Surfaces post-inline so atoms imported
    /// via `uses` get checked too. Defense-in-depth backstop for the L1
    /// `LowerError::EntityFieldNeverSet` check.
    BindingEntityFieldNeverSet,
    /// `(persist update|delete <Entity> @entity)` passes the WHOLE row, so the
    /// row key never appears as an `@entity.id` read and
    /// `BindingEntityFieldNeverSet` cannot see it. An unbound `id` resolves
    /// empty and the runtime refuses the write outright.
    BindingPersistRowIdNeverSet,
    /// `@trait` binding with no trait name — bare `@trait` is not resolvable.
    BindingTraitMissingName,
    /// `@trait.X` trait name doesn't match the `PascalCase` identifier pattern.
    BindingTraitInvalidFormat,
    /// `@trait.X` references a trait not declared in the current orbital.
    BindingTraitUnknown,
    /// A trait's render-ui references itself via `@trait.<self>`.
    BindingTraitSelfReference,
    /// Cycle detected in the `@trait.X` reference graph (e.g. A → B → A).
    BindingTraitCycle,
    /// `@trait.X` appears outside a `children:` array or a `node`-typed
    /// pattern prop — valid positions are where a subtree can be substituted.
    BindingTraitInvalidPosition,
    /// `@trait.X` (a render-only, client-side binding) appears outside a
    /// `render-ui` effect entirely — in a guard, a non-render effect
    /// (`fetch`/`set`/`persist`/`ref`/`call-service`), or a tick. Trait
    /// references are render-time composition primitives and have no meaning
    /// in data/effects/control flow.
    BindingTraitOutsideRenderUi,

    // =========================================================================
    // Entity Mutation / Tick Errors (ORB_ENTITY_*, ORB_TICK_*)
    // =========================================================================
    /// A non-`[shared]`, `Runtime`-persistence entity is mutated (via `set` or
    /// a tick write) by ≥2 distinct traits. Each mutator gets an isolated
    /// per-trait copy, so if one of the mutators' render-ui also reads the
    /// entity, it only ever sees its own trait's writes — never the others'.
    /// Error severity: add `[shared]` to the entity, or isolate the mutators.
    EntitySplitMutation,
    /// A `[shared]` entity has ≥2 distinct traits writing the SAME field.
    /// Coupled mechanics writing one field must be one ordered pipeline, not
    /// parallel writers — undefined last-write-wins ordering otherwise.
    SharedEntityFieldConflict,
    /// A `[shared]` entity is render-ui-read by more than one distinct trait.
    /// Exactly one render authority is allowed per shared entity, mirroring
    /// `SlotContention`'s single-owner rule.
    SharedEntityRenderAuthority,
    /// A trait runs a non-`Cron` tick (frame/duration interval) against a
    /// `Persistent` linked entity. Corpus-facing backstop mirroring the lolo
    /// L1 tick guards. Error severity.
    TickRequiresRuntime,
    /// A non-`Cron` tick effect calls a server-async operator (the canonical
    /// registry's `std-agent` category — `agent/generate`, `agent/invoke`,
    /// etc.). Repeated per-tick LLM/agent calls can pile up faster than they
    /// resolve. Error severity.
    TickAsyncEffect,

    // =========================================================================
    // Closed Circuit Errors (CIRCUIT_*)
    // =========================================================================
    /// Action emits event with no transition handler
    CircuitOrphanEvent,
    /// Same event emitted multiple times in same context
    CircuitDuplicateEvent,
    /// Interactive component missing itemActions
    CircuitDeadEndComponent,
    /// State has no incoming transitions
    CircuitUnreachableState,
    /// State has no outgoing transitions
    CircuitDeadEndState,
    /// Action-component mismatch
    CircuitActionComponentMismatch,
    /// Overlay slot (modal/drawer) missing exit transition
    CircuitNoOverlayExit,
    /// Modal-to-inline transition missing render-ui("main", ...) alongside render-ui("modal", null)
    CircuitModalExitIncomplete,
    /// Render-ui handler fires event with no matching transition from the rendering state
    CircuitDeadHandler,

    // =========================================================================
    // Icon Errors (ORB_UI_ICON_*)
    // =========================================================================
    /// Icon name has invalid format
    IconInvalidFormat,
    /// Icon name is empty
    IconEmpty,
    /// Icon may not exist in Lucide
    IconNotFound,

    // =========================================================================
    // Operator Literal Errors (ORB_OP_*)
    // =========================================================================
    /// A `time/*` operator was given a unit literal outside the vocabulary.
    /// Unchecked, it throws at evaluation time — and inside a read predicate
    /// that means rows are dropped fail-closed while the run still reports PASS
    /// (`S-CALENDAR-RANGE-FILTER-USES-UNSUPPORTED-TIME-UNITS`).
    OperatorInvalidTimeUnit,

    // =========================================================================
    // Query Singleton Errors (ORB_QUERY_*)
    // =========================================================================
    /// Query singleton missing search field
    QueryMissingSearchField,
    /// Query singleton missing recommended fields
    QueryMissingRecommendedFields,
    /// Pattern doesn't support query prop
    QueryUnsupportedPattern,
    /// Query references unknown singleton
    QueryUnknownSingleton,
    /// Query singleton is unused
    QueryUnusedSingleton,

    // =========================================================================
    // Trait Event Errors (ORB_T_EVT_*)
    // =========================================================================
    /// Event emitted but not declared in trait.emits
    TraitEventUndeclared,
    /// Event name not UPPER_SNAKE_CASE
    TraitEventInvalidName,
    /// Tick emits undeclared event
    TraitEventTickUndeclared,
    /// Listener scope mismatch
    TraitEventScopeMismatch,
    /// Duplicate event in trait.emits
    TraitEventDuplicate,
    /// External emit missing payload definition
    TraitEventExternalMissingPayload,

    // =========================================================================
    // Schema Errors (ORB_S_*)
    // =========================================================================
    /// Schema missing name
    SchemaMissingName,
    /// Schema has no orbitals
    SchemaNoOrbitals,
    /// Schema version is empty
    SchemaEmptyVersion,
    /// More than one differently-named `[identity]` entity in one composed
    /// program. `@user` is a single root resolving to a single viewer, so two
    /// declarations make every `@user.x` ambiguous. (Same-name duplicates are
    /// already rejected upstream by `validate_entity_uniqueness`.)
    SchemaIdentityNotUnique,
    /// A literal compared against an `@user.<field>` whose declared type is a
    /// closed vocabulary is not one of that vocabulary's members — a branch that
    /// can never be taken. The bug class that let std-healthcare's `"patient"`
    /// and std-lms's `"student"` ship green.
    SchemaRoleLiteralNotMember,
    /// `@user.<field>` names a field the declared `[identity]` entity does not
    /// have. Resolves to null at runtime, so the guard silently takes its
    /// negative branch.
    SchemaUnknownIdentityField,
    /// `@entity.<field>` (or `(object/get @entity field)`) inside an
    /// `@read`/`@create`/`@update`/`@delete` directive names a field the
    /// OWNING entity does not have.
    SchemaAccessPolicyUnknownEntityField,
    /// An access directive compares an `@entity.<field>` against `@user.id`,
    /// but `<field>` is not a relation to the `[identity]` entity. The
    /// comparison is then between a free-text scalar and a roster id — it
    /// type-checks, reads like ownership, and matches nothing, because the
    /// seeder only stamps viewer ids onto DECLARED relations. This is the
    /// `customerEmail` trap: a policy that silently denies everyone while
    /// looking correct.
    SchemaOwnerFieldNotIdentityTyped,
    /// An access directive reads a binding root other than `@entity`/`@user` —
    /// the directive has no call site, so `@payload`/`@config`/`@state` are
    /// not meaningful there.
    SchemaAccessPolicyInvalidBinding,
    /// An access directive reads `@user` in a program declaring no `[identity]`
    /// entity. Unlike a bare pre-existing `@user` read (which falls back to the
    /// implicit default shape), a NEW authorization construct must have a real
    /// roster — that is what makes personas switchable.
    SchemaAccessPolicyRequiresIdentity,
    /// Two entity declarations bind the same `persistent:` collection and
    /// declare DIFFERENT access directives. One table cannot honestly have two
    /// visibility rules, and the runtime's collection-level inheritance has no
    /// principled way to choose
    /// (`R-SHADOW-ENTITY-SHARES-COLLECTION-DIRECTIVES-DONT-APPLY`).
    SchemaCollectionPolicyConflict,
    /// Compose link-check: an orbital's `expects entity X` / `expects identity
    /// X` names a requirement no orbital in the program provides. The message
    /// names WHO ASKED — the consumer orbital — so the repair is attributable.
    SchemaExpectationUnsatisfied,
    /// Compose link-check: the named provider entity exists but lacks the
    /// expected kind (e.g. `expects identity Customer` where `Customer`
    /// carries no `[identity]` tag).
    SchemaExpectationProviderMismatch,
    /// Compose link-check: the provider entity declares no field with an
    /// expected shape name. Names consumer, provider, and field.
    SchemaExpectedFieldNotProvided,
    /// Compose link-check: the provider has the expected field but with an
    /// incompatible type (per orbital-core's `type_compat`; literal unions use
    /// subset semantics — every expected member must be a provider member).
    SchemaExpectedFieldTypeMismatch,
    /// Standalone AND compose self-consistency: the consumer uses a field it
    /// did not declare — an `@user.<field>` read outside its `expects
    /// identity` shape, or a `persist` payload field outside its `expects
    /// entity` shape. The consumer's declaration is its own source of truth.
    SchemaExpectationFieldUndeclared,
    /// A `uses A from "…" { config { … } }` call-site override names a key
    /// the imported behavior's app-level `config {}` (`OrbitalSchema.config`,
    /// `Almadar_Orbital_Import.md` §4.5) does not declare.
    UsesConfigUnknownKey,

    // =========================================================================
    // Identity Errors (ORB_ID_*) — Rabit V4 Phase 3 dual-carry id integrity
    // =========================================================================
    /// A populated dual-carry id field points at an id that is neither a node
    /// declared in the schema's arena (orbital/entity/trait/page/emit-event)
    /// nor a `schema.ledger` entry. The edge is dangling.
    OrbIdUnknownRef,
    /// Both a reference's name field and its id sibling are populated, but the
    /// ledger's `curName` for that id disagrees with the name (dual-carry drift).
    OrbIdNameMismatch,
    /// An id's kind prefix (`identity::id_kind_of`) disagrees with the reference
    /// position it occupies (e.g. an `ent_` id sitting in a trait `refId`).
    OrbIdKindMismatch,
    /// A ledger row whose id no node in the schema carries — a stale row left
    /// behind after its node was removed. Error severity.
    OrbIdLedgerOrphan,
    /// One event id is declared by two or more DIFFERENT traits in the same
    /// orbital (`stateMachine.events[].id` / `emits[].eventId`). Ids are the
    /// durable node keys of the identity graph; a shared id makes every
    /// id→name resolution (identity normalize, id-primary dispatch) ambiguous.
    /// Inline's event-id hygiene derives per-instance ids at expansion, so this
    /// fires only on an artifact produced outside that path.
    OrbIdDuplicateEventId,

    // =========================================================================
    // Generic Errors (ORB_GEN_*)
    // =========================================================================
    /// Generic invalid value
    InvalidValue,
    /// Generic invalid reference format
    InvalidReference,
    /// Theme reference names a theme not in the valid set (built-in
    /// `@almadar-ui/themes/` registry ∪ orbital-declared inline themes)
    UnknownTheme,
    /// Generic missing required field
    MissingField,
    /// Generic duplicate entity/item
    DuplicateEntity,
}

impl ErrorCode {
    /// Get the string code matching TypeScript error codes
    pub fn as_str(&self) -> &'static str {
        match self {
            // Entity
            ErrorCode::EntityMissingName => "ORB_E_MISSING_NAME",
            ErrorCode::EntityNoFields => "ORB_E_NO_FIELDS",
            ErrorCode::EntityInvalidFieldType => "ORB_E_INVALID_FIELD_TYPE",
            ErrorCode::EntityDuplicateField => "ORB_E_DUPLICATE_FIELD",
            ErrorCode::EntityInvalidFieldName => "ORB_E_INVALID_FIELD_NAME",
            ErrorCode::EntityMissingCollection => "ORB_E_MISSING_COLLECTION",
            ErrorCode::EntityInvalidRelation => "ORB_E_INVALID_RELATION",
            ErrorCode::EntityEmptyEnumValues => "ORB_E_EMPTY_ENUM_VALUES",
            ErrorCode::EntitySemanticValueInvalid => "ORB_E_SEMANTIC_VALUE_INVALID",
            ErrorCode::LiteralUnionDefaultMismatch => "ORB_E_LITERAL_UNION_DEFAULT_MISMATCH",

            // Trait
            ErrorCode::TraitUndefined => "ORB_T_UNDEFINED_TRAIT",
            ErrorCode::TraitInvalidFormat => "ORB_T_INVALID_FORMAT",
            ErrorCode::TraitInvalidCategory => "ORB_T_INVALID_CATEGORY",
            ErrorCode::TraitNoInitialState => "ORB_T_NO_INITIAL_STATE",
            ErrorCode::TraitMultipleInitialStates => "ORB_T_MULTIPLE_INITIAL_STATES",
            ErrorCode::TraitDuplicateState => "ORB_T_DUPLICATE_STATE",
            ErrorCode::TraitInvalidTransition => "ORB_T_INVALID_TRANSITION",
            ErrorCode::TraitDuplicateTransition => "ORB_T_DUPLICATE_TRANSITION",
            ErrorCode::TraitMissingInitTransition => "ORB_T_MISSING_INIT_TRANSITION",
            ErrorCode::TraitStateUnreachable => "ORB_T_STATE_UNREACHABLE",
            ErrorCode::TraitInitMissingFetch => "ORB_T_INIT_MISSING_FETCH",
            ErrorCode::TraitMissingRenderUI => "ORB_T_MISSING_RENDER_UI",
            ErrorCode::TraitDeprecatedUI => "ORB_T_DEPRECATED_UI",
            ErrorCode::TraitDuplicateName => "ORB_T_DUPLICATE_NAME",
            ErrorCode::TraitUndefinedEvent => "ORB_T_UNDEFINED_EVENT",
            ErrorCode::TraitInvalidSlotDefault => "ORB_T_INVALID_SLOT_DEFAULT",
            ErrorCode::TraitInvalidPatternDefault => "ORB_T_INVALID_PATTERN_DEFAULT",
            ErrorCode::TraitSecretDefaultNotString => "ORB_T_SECRET_DEFAULT_NOT_STRING",
            ErrorCode::TraitInvalidEventDefault => "ORB_T_INVALID_EVENT_DEFAULT",
            ErrorCode::TraitConfigShapeMismatch => "ORB_T_CONFIG_SHAPE_MISMATCH",
            ErrorCode::TraitConfigObjectArrayForbidden => "ORB_T_CONFIG_OBJECT_ARRAY_FORBIDDEN",
            ErrorCode::GenericTypeDeprecated => "ORB_T_GENERIC_TYPE_DEPRECATED",
            ErrorCode::NodeFieldUncheckedContent => "ORB_RUI_NODE_FIELD_UNCHECKED_CONTENT",
            ErrorCode::RenderableContentInGenericField => "ORB_RUI_RENDERABLE_IN_GENERIC_FIELD",
            ErrorCode::TraitConfigTierInvalid => "ORB_T_CONFIG_TIER_INVALID",
            ErrorCode::TraitEntityNotRebindable => "ORB_T_ENTITY_NOT_REBINDABLE",
            ErrorCode::RebindMissingEntityFields => "ORB_T_REBIND_MISSING_FIELDS",
            ErrorCode::UnresolvedTypeParam => "ORB_T_UNRESOLVED_TYPE_PARAM",
            ErrorCode::EventRenameUnknownKey => "ORB_T_EVENT_RENAME_UNKNOWN_KEY",
            ErrorCode::TraitConfigUnknownCallSiteKey => "ORB_T_CONFIG_UNKNOWN_CALL_SITE_KEY",
            ErrorCode::TraitConfigMissingRequiredKey => "ORB_T_CONFIG_MISSING_REQUIRED_KEY",

            // Page
            ErrorCode::PageMissingName => "ORB_P_MISSING_NAME",
            ErrorCode::PageMissingPath => "ORB_P_MISSING_PATH",
            ErrorCode::PageInvalidPath => "ORB_P_INVALID_PATH",
            ErrorCode::PageDuplicatePath => "ORB_P_DUPLICATE_PATH",
            ErrorCode::PageMissingTraits => "ORB_P_MISSING_TRAITS",
            ErrorCode::PageEmptyTraits => "ORB_P_EMPTY_TRAITS",
            ErrorCode::OrbitalMissingPages => "ORB_O_MISSING_PAGES",
            ErrorCode::OrbitalRefConfigUnknownKey => "ORB_O_CONFIG_UNKNOWN_KEY",
            ErrorCode::OrbitalRefOmitEmbeddedTrait => "ORB_O_OMIT_EMBEDDED_TRAIT",
            ErrorCode::OrbitalConfigDeadKnob => "ORB_O_CONFIG_DEAD_KNOB",
            ErrorCode::PageSectionsForbidden => "ORB_P_SECTIONS_FORBIDDEN",
            ErrorCode::PageInvalidTraitRef => "ORB_P_INVALID_TRAIT_REF",
            ErrorCode::PageInvalidViewType => "ORB_P_INVALID_VIEW_TYPE",

            // Slot
            ErrorCode::SlotMainNotCovered => "ORB_SLOT_MAIN_NOT_COVERED",
            ErrorCode::SlotContention => "ORB_SLOT_CONTENTION",
            ErrorCode::SlotContentionRuntime => "ORB_SLOT_CONTENTION_RUNTIME",
            ErrorCode::SlotContentionTick => "ORB_SLOT_CONTENTION_TICK",
            ErrorCode::SlotInvalidNesting => "ORB_SLOT_INVALID_NESTING",
            ErrorCode::SlotPriorityConflict => "ORB_SLOT_PRIORITY_CONFLICT",
            ErrorCode::SlotInvalidName => "ORB_SLOT_INVALID_NAME",
            ErrorCode::SlotHudNonGame => "ORB_SLOT_HUD_NON_GAME",

            // Render-UI
            ErrorCode::RenderUIInvalidSlot => "ORB_RUI_INVALID_SLOT",
            ErrorCode::RenderUIInvalidPattern => "ORB_RUI_INVALID_PATTERN",
            ErrorCode::RenderUIMissingPatternType => "ORB_RUI_MISSING_PATTERN_TYPE",
            ErrorCode::RenderUIInvalidProp => "ORB_RUI_INVALID_PROP",
            ErrorCode::RenderUIChildNotDrawable => "ORB_RUI_CHILD_NOT_DRAWABLE",
            ErrorCode::RenderUIMissingRequiredProp => "ORB_RUI_MISSING_REQUIRED_PROP",
            ErrorCode::RenderUIInvalidItemAction => "ORB_RUI_INVALID_ITEM_ACTION",
            ErrorCode::RenderUIUnknownItemActionProp => "ORB_RUI_UNKNOWN_ITEM_ACTION_PROP",
            ErrorCode::RenderUIMissingAction => "ORB_RUI_MISSING_ACTION",
            ErrorCode::RenderUIInvalidFieldPath => "ORB_RUI_INVALID_FIELD_PATH",
            ErrorCode::RenderUIPropTypeMismatch => "ORB_RUI_PROP_TYPE_MISMATCH",
            ErrorCode::RenderUIBindingTypeMismatch => "ORB_RUI_BINDING_TYPE_MISMATCH",
            ErrorCode::RenderUIElementShapeMismatch => "ORB_RUI_ELEMENT_SHAPE_MISMATCH",
            ErrorCode::EntityNameShadowsTraitUnion => "ORB_T_ENTITY_NAME_SHADOWS_TRAIT_UNION",
            ErrorCode::RenderUIEventBindingPayloadMismatch => {
                "ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH"
            }
            ErrorCode::RenderUIEventLiteralStale => "ORB_X_RENDER_UI_EVENT_LITERAL_STALE",

            // Effect
            ErrorCode::EffectCallServiceMissingService => "ORB_EFF_CALL_SERVICE_MISSING_SERVICE",
            ErrorCode::EffectCallServiceUnknownService => "ORB_EFF_CALL_SERVICE_UNKNOWN_SERVICE",
            ErrorCode::EffectCallServiceMissingAction => "ORB_EFF_CALL_SERVICE_MISSING_ACTION",
            ErrorCode::EffectCallServiceMissingHandlers => "ORB_EFF_CALL_SERVICE_MISSING_HANDLERS",
            ErrorCode::EffectCallServiceUnknownParam => "ORB_EFF_CALL_SERVICE_UNKNOWN_PARAM",
            ErrorCode::EffectCallServiceMissingParam => "ORB_EFF_CALL_SERVICE_MISSING_PARAM",
            ErrorCode::EffectCallServiceParamType => "ORB_EFF_CALL_SERVICE_PARAM_TYPE",
            ErrorCode::EffectSetInvalidBinding => "ORB_EFF_SET_INVALID_BINDING",
            ErrorCode::EffectSetPayloadNotAllowed => "ORB_EFF_SET_PAYLOAD_NOT_ALLOWED",
            ErrorCode::EffectFetchInvalidEntity => "ORB_EFF_FETCH_INVALID_ENTITY",
            ErrorCode::EffectFetchInvalidInclude => "ORB_EFF_FETCH_INVALID_INCLUDE",
            ErrorCode::EffectSlotConflict => "ORB_EFF_SLOT_CONFLICT",
            ErrorCode::EffectUnknownType => "ORB_EFF_UNKNOWN_TYPE",
            ErrorCode::EffectNavigateMissingPayload => "ORB_EFF_NAVIGATE_MISSING_PAYLOAD",
            ErrorCode::EffectNavigateTargetUnreachable => "ORB_EFF_NAVIGATE_TARGET_UNREACHABLE",
            ErrorCode::EmitDeclaredButUnfired => "ORB_EMIT_DECLARED_BUT_UNFIRED",
            ErrorCode::EmitConfigRefInvalid => "ORB_EMIT_CONFIG_REF_INVALID",
            ErrorCode::EffectMissingFetchForEntityPattern => "ORB_EFF_MISSING_FETCH_FOR_ENTITY",
            ErrorCode::EffectEmitKeyInvalid => "ORB_EFF_EMIT_KEY_INVALID",
            ErrorCode::EffectEmitInvalidName => "ORB_EFF_EMIT_INVALID_NAME",
            ErrorCode::EffectEmitUndeclaredEvent => "ORB_EFF_EMIT_UNDECLARED_EVENT",
            ErrorCode::EffectMissingRequiredEmit => "ORB_EFF_MISSING_REQUIRED_EMIT",

            // S-Expression
            ErrorCode::SExprInvalidOperator => "SEXPR_INVALID_OPERATOR",
            ErrorCode::SExprArityMismatch => "SEXPR_ARITY_MISMATCH",
            ErrorCode::SExprLambdaArgPosition => "SEXPR_LAMBDA_ARG_POSITION",
            ErrorCode::SExprEffectInGuard => "SEXPR_EFFECT_IN_GUARD",
            ErrorCode::SExprInvalidBinding => "SEXPR_INVALID_BINDING",
            ErrorCode::SExprInvalidBindingPath => "SEXPR_INVALID_BINDING_PATH",
            ErrorCode::SExprUnknownEntity => "SEXPR_UNKNOWN_ENTITY",
            ErrorCode::SExprParseError => "SEXPR_PARSE_ERROR",

            // Service
            ErrorCode::ServiceDuplicateName => "ORB_SVC_DUPLICATE_NAME",
            ErrorCode::ServiceMissingBaseUrl => "ORB_SVC_MISSING_BASE_URL",
            ErrorCode::ServiceInvalidUrl => "ORB_SVC_INVALID_URL",
            ErrorCode::ServiceMissingEvents => "ORB_SVC_MISSING_EVENTS",
            ErrorCode::ServiceReservedEvent => "ORB_SVC_RESERVED_EVENT",
            ErrorCode::ServiceMissingCapabilities => "ORB_SVC_MISSING_CAPABILITIES",
            ErrorCode::ServiceMissingServerPath => "ORB_SVC_MISSING_SERVER_PATH",
            ErrorCode::ServiceUnknownType => "ORB_SVC_UNKNOWN_TYPE",
            ErrorCode::ServiceInvalidIntegrator => "ORB_SVC_INVALID_INTEGRATOR",

            // Cross-Orbital
            ErrorCode::CrossOrbitalDuplicateEntity => "ORB_X_DUPLICATE_ENTITY",
            ErrorCode::CrossOrbitalOrphanListener => "ORB_X_ORPHAN_LISTENER",
            ErrorCode::CrossOrbitalUnusedEmission => "ORB_X_UNUSED_EMISSION",
            ErrorCode::CrossOrbitalCircularDependency => "ORB_X_CIRCULAR_DEPENDENCY",
            ErrorCode::CrossOrbitalEventCollision => "ORB_X_EVENT_COLLISION",
            ErrorCode::CrossOrbitalMissingName => "ORB_X_MISSING_ORBITAL_NAME",
            ErrorCode::CrossOrbitalPayloadMismatch => "ORB_X_PAYLOAD_MISMATCH",
            ErrorCode::CrossOrbitalListenSourceUnresolved => "ORB_X_LISTEN_SOURCE_UNRESOLVED",
            ErrorCode::ListenTriggerUnmatched => "ORB_X_LISTEN_TRIGGER_UNMATCHED",
            ErrorCode::InternalEventExposed => "ORB_X_INTERNAL_EVENT_EXPOSED",
            ErrorCode::UnresolvedRelationTarget => "ORB_X_UNRESOLVED_RELATION_TARGET",
            ErrorCode::UnresolvedPatternFieldRef => "ORB_X_UNRESOLVED_PATTERN_FIELD_REF",

            // Binding
            ErrorCode::BindingInvalidFormat => "ORB_BINDING_INVALID_FORMAT",
            ErrorCode::BindingUnknownRoot => "ORB_BINDING_UNKNOWN_ROOT",
            ErrorCode::ConfigDanglingRef => "ORB_CONFIG_DANGLING_REF",
            ErrorCode::BindingInvalidPath => "ORB_BINDING_INVALID_PATH",
            ErrorCode::BindingStateNoPath => "ORB_BINDING_STATE_NO_PATH",
            ErrorCode::BindingNowNoPath => "ORB_BINDING_NOW_NO_PATH",
            ErrorCode::BindingPayloadInTick => "ORB_BINDING_PAYLOAD_IN_TICK",
            ErrorCode::BindingUnknownEntity => "ORB_BINDING_UNKNOWN_ENTITY",
            ErrorCode::BindingInvalidFieldName => "ORB_BINDING_INVALID_FIELD_NAME",
            ErrorCode::BindingSetTargetMissingPath => "ORB_BINDING_SET_TARGET_MISSING_PATH",
            ErrorCode::BindingPayloadFieldUndeclared => "ORB_BINDING_PAYLOAD_FIELD_UNDECLARED",
            ErrorCode::BindingPayloadTypeIncompatible => "ORB_BINDING_PAYLOAD_TYPE_INCOMPATIBLE",
            ErrorCode::SetValueTypeMismatch => "ORB_X_SET_VALUE_TYPE_MISMATCH",
            ErrorCode::EnumValueNotMember => "ORB_X_ENUM_VALUE_NOT_MEMBER",
            ErrorCode::SetReturnTypeMismatch => "ORB_X_SET_RETURN_TYPE_MISMATCH",
            ErrorCode::EmitPayloadMissingRequired => "ORB_EMIT_PAYLOAD_MISSING_REQUIRED",
            ErrorCode::EmitPayloadExtraField => "ORB_EMIT_PAYLOAD_EXTRA_FIELD",
            ErrorCode::EmitPayloadFieldTypeMismatch => "ORB_EMIT_PAYLOAD_FIELD_TYPE_MISMATCH",
            ErrorCode::PayloadFieldNameInvalid => "ORB_X_PAYLOAD_FIELD_NAME",
            ErrorCode::EntityPropContract => "ORB_X_ENTITY_PROP_CONTRACT",
            ErrorCode::EmitSuccessPayloadReturnMismatch => {
                "ORB_EMIT_SUCCESS_PAYLOAD_RETURN_MISMATCH"
            }
            ErrorCode::EmitSuccessPayloadWrapped => "ORB_EMIT_SUCCESS_PAYLOAD_WRAPPED",
            ErrorCode::EmitPayloadNotProduced => "ORB_EMIT_PAYLOAD_NOT_PRODUCED",
            ErrorCode::CrossEmitterDeclaredPayloadMismatch => {
                "ORB_X_CROSS_EMITTER_DECLARED_PAYLOAD_MISMATCH"
            }
            ErrorCode::BindingEntityFieldNotFound => "ORB_BINDING_ENTITY_FIELD_NOT_FOUND",
            ErrorCode::BindingEntityFieldNeverSet => "ORB_BINDING_ENTITY_FIELD_NEVER_SET",
            ErrorCode::BindingPersistRowIdNeverSet => "ORB_BINDING_PERSIST_ROW_ID_NEVER_SET",
            ErrorCode::BindingTraitMissingName => "ORB_BINDING_TRAIT_MISSING_NAME",
            ErrorCode::BindingTraitInvalidFormat => "ORB_BINDING_TRAIT_INVALID_FORMAT",
            ErrorCode::BindingTraitUnknown => "ORB_BINDING_TRAIT_UNKNOWN",
            ErrorCode::BindingTraitSelfReference => "ORB_BINDING_TRAIT_SELF_REFERENCE",
            ErrorCode::BindingTraitCycle => "ORB_BINDING_TRAIT_CYCLE",
            ErrorCode::BindingTraitInvalidPosition => "ORB_BINDING_TRAIT_INVALID_POSITION",
            ErrorCode::BindingTraitOutsideRenderUi => "ORB_BINDING_TRAIT_OUTSIDE_RENDER_UI",

            // Entity Mutation / Tick
            ErrorCode::EntitySplitMutation => "ORB_ENTITY_SPLIT_MUTATION",
            ErrorCode::SharedEntityFieldConflict => "ORB_SHARED_ENTITY_FIELD_CONFLICT",
            ErrorCode::SharedEntityRenderAuthority => "ORB_SHARED_ENTITY_RENDER_AUTHORITY",
            ErrorCode::TickRequiresRuntime => "ORB_TICK_REQUIRES_RUNTIME",
            ErrorCode::TickAsyncEffect => "ORB_TICK_ASYNC_EFFECT",

            // Closed Circuit
            ErrorCode::CircuitOrphanEvent => "CIRCUIT_ORPHAN_EVENT",
            ErrorCode::CircuitDuplicateEvent => "CIRCUIT_DUPLICATE_EVENT",
            ErrorCode::CircuitDeadEndComponent => "CIRCUIT_DEAD_END_COMPONENT",
            ErrorCode::CircuitUnreachableState => "CIRCUIT_UNREACHABLE_STATE",
            ErrorCode::CircuitDeadEndState => "CIRCUIT_DEAD_END_STATE",
            ErrorCode::CircuitActionComponentMismatch => "CIRCUIT_ACTION_COMPONENT_MISMATCH",
            ErrorCode::CircuitNoOverlayExit => "CIRCUIT_NO_OVERLAY_EXIT",
            ErrorCode::CircuitModalExitIncomplete => "CIRCUIT_MODAL_EXIT_INCOMPLETE",
            ErrorCode::CircuitDeadHandler => "CIRCUIT_DEAD_HANDLER",

            // Icon
            ErrorCode::IconInvalidFormat => "ORB_UI_ICON_INVALID_FORMAT",
            ErrorCode::IconEmpty => "ORB_UI_ICON_EMPTY",
            ErrorCode::IconNotFound => "ORB_UI_ICON_NOT_FOUND",
            ErrorCode::OperatorInvalidTimeUnit => "ORB_OP_INVALID_TIME_UNIT",

            // Query Singleton
            ErrorCode::QueryMissingSearchField => "ORB_QUERY_MISSING_SEARCH",
            ErrorCode::QueryMissingRecommendedFields => "ORB_QUERY_MISSING_RECOMMENDED",
            ErrorCode::QueryUnsupportedPattern => "ORB_QUERY_UNSUPPORTED_PATTERN",
            ErrorCode::QueryUnknownSingleton => "ORB_QUERY_UNKNOWN_SINGLETON",
            ErrorCode::QueryUnusedSingleton => "ORB_QUERY_UNUSED_SINGLETON",

            // Trait Event
            ErrorCode::TraitEventUndeclared => "ORB_T_EVT_UNDECLARED",
            ErrorCode::TraitEventInvalidName => "ORB_T_EVT_INVALID_NAME",
            ErrorCode::TraitEventTickUndeclared => "ORB_T_EVT_TICK_UNDECLARED",
            ErrorCode::TraitEventScopeMismatch => "ORB_T_EVT_SCOPE_MISMATCH",
            ErrorCode::TraitEventDuplicate => "ORB_T_EVT_DUPLICATE",
            ErrorCode::TraitEventExternalMissingPayload => "ORB_T_EVT_EXTERNAL_MISSING_PAYLOAD",

            // Schema
            ErrorCode::SchemaMissingName => "ORB_S_MISSING_NAME",
            ErrorCode::SchemaNoOrbitals => "ORB_S_NO_ORBITALS",
            ErrorCode::SchemaEmptyVersion => "ORB_S_EMPTY_VERSION",
            ErrorCode::SchemaIdentityNotUnique => "ORB_S_IDENTITY_NOT_UNIQUE",
            ErrorCode::SchemaRoleLiteralNotMember => "ORB_S_ROLE_LITERAL_NOT_MEMBER",
            ErrorCode::SchemaUnknownIdentityField => "ORB_S_UNKNOWN_IDENTITY_FIELD",
            ErrorCode::SchemaAccessPolicyUnknownEntityField => {
                "ORB_S_ACCESS_POLICY_UNKNOWN_ENTITY_FIELD"
            }
            ErrorCode::SchemaOwnerFieldNotIdentityTyped => "ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED",
            ErrorCode::SchemaAccessPolicyInvalidBinding => "ORB_S_ACCESS_POLICY_INVALID_BINDING",
            ErrorCode::SchemaAccessPolicyRequiresIdentity => {
                "ORB_S_ACCESS_POLICY_REQUIRES_IDENTITY"
            }
            ErrorCode::SchemaCollectionPolicyConflict => "ORB_S_COLLECTION_POLICY_CONFLICT",
            ErrorCode::SchemaExpectationUnsatisfied => "ORB_S_EXPECTATION_UNSATISFIED",
            ErrorCode::SchemaExpectationProviderMismatch => "ORB_S_EXPECTATION_PROVIDER_MISMATCH",
            ErrorCode::SchemaExpectedFieldNotProvided => "ORB_S_EXPECTED_FIELD_NOT_PROVIDED",
            ErrorCode::SchemaExpectedFieldTypeMismatch => "ORB_S_EXPECTED_FIELD_TYPE_MISMATCH",
            ErrorCode::SchemaExpectationFieldUndeclared => "ORB_S_EXPECTATION_FIELD_UNDECLARED",
            ErrorCode::UsesConfigUnknownKey => "ORB_U_CONFIG_UNKNOWN_KEY",

            // Identity
            ErrorCode::OrbIdUnknownRef => "ORB_ID_UNKNOWN_REF",
            ErrorCode::OrbIdNameMismatch => "ORB_ID_NAME_MISMATCH",
            ErrorCode::OrbIdKindMismatch => "ORB_ID_KIND_MISMATCH",
            ErrorCode::OrbIdLedgerOrphan => "ORB_ID_LEDGER_ORPHAN",
            ErrorCode::OrbIdDuplicateEventId => "ORB_ID_DUPLICATE_EVENT_ID",

            // Generic
            ErrorCode::InvalidValue => "ORB_GEN_INVALID_VALUE",
            ErrorCode::InvalidReference => "ORB_GEN_INVALID_REFERENCE",
            ErrorCode::UnknownTheme => "ORB_X_UNKNOWN_THEME",
            ErrorCode::MissingField => "ORB_GEN_MISSING_FIELD",
            ErrorCode::DuplicateEntity => "ORB_GEN_DUPLICATE",
        }
    }

    /// Get the category for this error code
    pub fn category(&self) -> &'static str {
        match self {
            ErrorCode::EntityMissingName
            | ErrorCode::EntityNoFields
            | ErrorCode::EntityInvalidFieldType
            | ErrorCode::EntityDuplicateField
            | ErrorCode::EntityInvalidFieldName
            | ErrorCode::EntityMissingCollection
            | ErrorCode::EntityInvalidRelation
            | ErrorCode::EntityEmptyEnumValues
            | ErrorCode::EntitySemanticValueInvalid
            | ErrorCode::EntityNameShadowsTraitUnion
            | ErrorCode::LiteralUnionDefaultMismatch => "entity",

            ErrorCode::TraitUndefined
            | ErrorCode::TraitInvalidFormat
            | ErrorCode::TraitInvalidCategory
            | ErrorCode::TraitNoInitialState
            | ErrorCode::TraitMultipleInitialStates
            | ErrorCode::TraitDuplicateState
            | ErrorCode::TraitInvalidTransition
            | ErrorCode::TraitDuplicateTransition
            | ErrorCode::TraitMissingInitTransition
            | ErrorCode::TraitStateUnreachable
            | ErrorCode::TraitInitMissingFetch
            | ErrorCode::TraitMissingRenderUI
            | ErrorCode::TraitDeprecatedUI
            | ErrorCode::TraitDuplicateName
            | ErrorCode::TraitUndefinedEvent
            | ErrorCode::TraitInvalidSlotDefault
            | ErrorCode::TraitInvalidPatternDefault
            | ErrorCode::TraitSecretDefaultNotString
            | ErrorCode::TraitInvalidEventDefault
            | ErrorCode::TraitConfigShapeMismatch
            | ErrorCode::TraitConfigObjectArrayForbidden
            | ErrorCode::NodeFieldUncheckedContent
            | ErrorCode::RenderableContentInGenericField
            | ErrorCode::TraitConfigTierInvalid
            | ErrorCode::TraitEntityNotRebindable
            | ErrorCode::RebindMissingEntityFields
            | ErrorCode::EventRenameUnknownKey
            | ErrorCode::TraitConfigUnknownCallSiteKey
            | ErrorCode::TraitConfigMissingRequiredKey
            | ErrorCode::UnresolvedTypeParam
            // A generic type in a declaration position — a TYPE-system rejection
            // since the burndown reached zero (2026-09-01).
            | ErrorCode::GenericTypeDeprecated => "trait",

            ErrorCode::PageMissingName
            | ErrorCode::PageMissingPath
            | ErrorCode::PageInvalidPath
            | ErrorCode::PageDuplicatePath
            | ErrorCode::PageMissingTraits
            | ErrorCode::PageEmptyTraits
            | ErrorCode::OrbitalMissingPages
            | ErrorCode::OrbitalRefConfigUnknownKey
            | ErrorCode::OrbitalRefOmitEmbeddedTrait
            | ErrorCode::OrbitalConfigDeadKnob
            | ErrorCode::PageSectionsForbidden
            | ErrorCode::PageInvalidTraitRef
            | ErrorCode::PageInvalidViewType => "page",

            ErrorCode::SlotMainNotCovered
            | ErrorCode::SlotContention
            | ErrorCode::SlotContentionRuntime
            | ErrorCode::SlotContentionTick
            | ErrorCode::SlotInvalidNesting
            | ErrorCode::SlotPriorityConflict
            | ErrorCode::SlotInvalidName
            | ErrorCode::SlotHudNonGame => "slot",

            ErrorCode::RenderUIInvalidSlot
            | ErrorCode::RenderUIInvalidPattern
            | ErrorCode::RenderUIMissingPatternType
            | ErrorCode::RenderUIInvalidProp
            | ErrorCode::RenderUIChildNotDrawable
            | ErrorCode::RenderUIMissingRequiredProp
            | ErrorCode::RenderUIInvalidItemAction
            | ErrorCode::RenderUIUnknownItemActionProp
            | ErrorCode::RenderUIMissingAction
            | ErrorCode::RenderUIInvalidFieldPath
            | ErrorCode::RenderUIPropTypeMismatch
            | ErrorCode::RenderUIBindingTypeMismatch
            | ErrorCode::RenderUIElementShapeMismatch
            | ErrorCode::RenderUIEventBindingPayloadMismatch
            | ErrorCode::EntityPropContract
            | ErrorCode::RenderUIEventLiteralStale => "render_ui",

            ErrorCode::EffectCallServiceMissingService
            | ErrorCode::EffectCallServiceUnknownService
            | ErrorCode::EffectCallServiceMissingAction
            | ErrorCode::EffectCallServiceMissingHandlers
            | ErrorCode::EffectCallServiceUnknownParam
            | ErrorCode::EffectCallServiceMissingParam
            | ErrorCode::EffectCallServiceParamType
            | ErrorCode::EffectSetInvalidBinding
            | ErrorCode::EffectSetPayloadNotAllowed
            | ErrorCode::EffectFetchInvalidEntity
            | ErrorCode::EffectFetchInvalidInclude
            | ErrorCode::EffectSlotConflict
            | ErrorCode::EffectUnknownType
            | ErrorCode::EffectNavigateMissingPayload
            | ErrorCode::EffectNavigateTargetUnreachable
            | ErrorCode::EffectMissingFetchForEntityPattern
            | ErrorCode::EffectEmitKeyInvalid
            | ErrorCode::EffectEmitInvalidName
            | ErrorCode::EffectEmitUndeclaredEvent
            | ErrorCode::EffectMissingRequiredEmit
            | ErrorCode::SetValueTypeMismatch
            | ErrorCode::EnumValueNotMember
            | ErrorCode::SetReturnTypeMismatch
            | ErrorCode::EmitDeclaredButUnfired
            | ErrorCode::EmitConfigRefInvalid => "effect",

            ErrorCode::SExprInvalidOperator
            | ErrorCode::SExprArityMismatch
            | ErrorCode::SExprLambdaArgPosition
            | ErrorCode::SExprEffectInGuard
            | ErrorCode::SExprInvalidBinding
            | ErrorCode::SExprInvalidBindingPath
            | ErrorCode::SExprUnknownEntity
            | ErrorCode::SExprParseError => "sexpr",

            ErrorCode::ServiceDuplicateName
            | ErrorCode::ServiceMissingBaseUrl
            | ErrorCode::ServiceInvalidUrl
            | ErrorCode::ServiceMissingEvents
            | ErrorCode::ServiceReservedEvent
            | ErrorCode::ServiceMissingCapabilities
            | ErrorCode::ServiceMissingServerPath
            | ErrorCode::ServiceUnknownType
            | ErrorCode::ServiceInvalidIntegrator => "service",

            ErrorCode::CrossOrbitalDuplicateEntity
            | ErrorCode::CrossOrbitalOrphanListener
            | ErrorCode::CrossOrbitalUnusedEmission
            | ErrorCode::CrossOrbitalCircularDependency
            | ErrorCode::CrossOrbitalEventCollision
            | ErrorCode::CrossOrbitalMissingName
            | ErrorCode::CrossOrbitalPayloadMismatch
            | ErrorCode::CrossOrbitalListenSourceUnresolved
            | ErrorCode::ListenTriggerUnmatched
            | ErrorCode::InternalEventExposed
            | ErrorCode::UnresolvedRelationTarget
            | ErrorCode::UnresolvedPatternFieldRef
            | ErrorCode::CrossEmitterDeclaredPayloadMismatch => "cross",

            ErrorCode::BindingInvalidFormat
            | ErrorCode::BindingUnknownRoot
            | ErrorCode::ConfigDanglingRef
            | ErrorCode::BindingInvalidPath
            | ErrorCode::BindingStateNoPath
            | ErrorCode::BindingNowNoPath
            | ErrorCode::BindingPayloadInTick
            | ErrorCode::BindingUnknownEntity
            | ErrorCode::BindingInvalidFieldName
            | ErrorCode::BindingSetTargetMissingPath
            | ErrorCode::BindingPayloadFieldUndeclared
            | ErrorCode::BindingPayloadTypeIncompatible
            | ErrorCode::EmitPayloadMissingRequired
            | ErrorCode::EmitPayloadExtraField
            | ErrorCode::EmitPayloadFieldTypeMismatch
            | ErrorCode::PayloadFieldNameInvalid
            | ErrorCode::EmitSuccessPayloadReturnMismatch
            | ErrorCode::EmitSuccessPayloadWrapped
            | ErrorCode::EmitPayloadNotProduced
            | ErrorCode::BindingEntityFieldNotFound
            | ErrorCode::BindingEntityFieldNeverSet
            | ErrorCode::BindingPersistRowIdNeverSet
            | ErrorCode::BindingTraitMissingName
            | ErrorCode::BindingTraitInvalidFormat
            | ErrorCode::BindingTraitUnknown
            | ErrorCode::BindingTraitSelfReference
            | ErrorCode::BindingTraitCycle
            | ErrorCode::BindingTraitInvalidPosition
            | ErrorCode::BindingTraitOutsideRenderUi => "binding",

            ErrorCode::EntitySplitMutation
            | ErrorCode::SharedEntityFieldConflict
            | ErrorCode::SharedEntityRenderAuthority
            | ErrorCode::TickRequiresRuntime
            | ErrorCode::TickAsyncEffect => "entity_mutation",

            ErrorCode::CircuitOrphanEvent
            | ErrorCode::CircuitDuplicateEvent
            | ErrorCode::CircuitDeadEndComponent
            | ErrorCode::CircuitUnreachableState
            | ErrorCode::CircuitDeadEndState
            | ErrorCode::CircuitActionComponentMismatch
            | ErrorCode::CircuitNoOverlayExit
            | ErrorCode::CircuitModalExitIncomplete
            | ErrorCode::CircuitDeadHandler => "circuit",

            ErrorCode::IconInvalidFormat | ErrorCode::IconEmpty | ErrorCode::IconNotFound => "icon",

            ErrorCode::OperatorInvalidTimeUnit => "operator",

            ErrorCode::QueryMissingSearchField
            | ErrorCode::QueryMissingRecommendedFields
            | ErrorCode::QueryUnsupportedPattern
            | ErrorCode::QueryUnknownSingleton
            | ErrorCode::QueryUnusedSingleton => "query",

            ErrorCode::TraitEventUndeclared
            | ErrorCode::TraitEventInvalidName
            | ErrorCode::TraitEventTickUndeclared
            | ErrorCode::TraitEventScopeMismatch
            | ErrorCode::TraitEventDuplicate
            | ErrorCode::TraitEventExternalMissingPayload => "trait_event",

            ErrorCode::SchemaMissingName
            | ErrorCode::SchemaNoOrbitals
            | ErrorCode::SchemaEmptyVersion
            | ErrorCode::SchemaIdentityNotUnique
            | ErrorCode::SchemaRoleLiteralNotMember
            | ErrorCode::SchemaUnknownIdentityField
            | ErrorCode::SchemaAccessPolicyUnknownEntityField
            | ErrorCode::SchemaOwnerFieldNotIdentityTyped
            | ErrorCode::SchemaAccessPolicyInvalidBinding
            | ErrorCode::SchemaAccessPolicyRequiresIdentity
            | ErrorCode::SchemaCollectionPolicyConflict
            | ErrorCode::SchemaExpectationUnsatisfied
            | ErrorCode::SchemaExpectationProviderMismatch
            | ErrorCode::SchemaExpectedFieldNotProvided
            | ErrorCode::SchemaExpectedFieldTypeMismatch
            | ErrorCode::SchemaExpectationFieldUndeclared
            | ErrorCode::UsesConfigUnknownKey => "schema",

            ErrorCode::OrbIdUnknownRef
            | ErrorCode::OrbIdNameMismatch
            | ErrorCode::OrbIdKindMismatch
            | ErrorCode::OrbIdLedgerOrphan
            | ErrorCode::OrbIdDuplicateEventId => "identity",

            ErrorCode::InvalidValue
            | ErrorCode::InvalidReference
            | ErrorCode::UnknownTheme
            | ErrorCode::MissingField
            | ErrorCode::DuplicateEntity => "generic",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// S16a — the PRIMARY error an author hits when they write the implicit
/// collection shorthand in a `children:` position: an operator name lands where
/// a UI pattern type was expected, and `Unknown pattern type: 'array/map'`
/// names neither the cause nor the cure. Source-tagged: the caller resolves the
/// name through the embedded operator registry first, so this only fires for a
/// REAL operator, never for a genuine pattern typo.
pub fn collection_operator_as_pattern(code: ErrorCode, path: &str, op: &str) -> ValidationError {
    ValidationError::error(
        code,
        path,
        format!(
            "'{}' is an operator, not a UI pattern type — a collection callback in a \
             `children:` position must use the explicit lambda form.",
            op
        ),
    )
    .with_suggestion(format!(
        "Write `({} <collection> (fn (item) <node>))`. The implicit shorthand \
         `({} <collection> {{ … }})` is only recognised outside `children:`; here the \
         body is not treated as a lambda, so @item never comes into scope. <node> may be \
         a raw `type:` object or a JSX element.",
        op, op
    ))
}

/// S16 — the one diagnostic for an out-of-scope `@item` / `@index`, shared by
/// both emitters of the class (`binding.rs`'s unknown-root arm and
/// `sigil_context.rs`'s context-rules arm) so the two can never drift.
///
/// `item` / `index` are bound ONLY by the collection-callback arm of each
/// walker, so reaching either call site proves the binding escaped its
/// callback. The generic "allowed sigils" advice is actively wrong here: no
/// other sigil names the element being iterated, so it sends the author down a
/// path with no fix at the end.
///
/// ⚠️ The remedy is the EXPLICIT lambda, not "avoid JSX". Verified 2026-08-04
/// with two probes identical but for the map shape: `(array/map @entity.rows
/// (fn (item) <Alias.traits.X content={@item.title} />))` validates 0/0, while
/// the implicit-`@item` shorthand `(array/map @entity.rows { … @item … })` in a
/// `children:` position fails — only the pinned 3-tuple `["array/map", <coll>,
/// ["fn", <param>, <node>]]` is recognised there (`SExpression::
/// as_mapped_children`). With no recognised lambda, hoisted JSX has nothing for
/// `splice_lambda_trait_refs` to splice back into, which is what strands the
/// binding. JSX inside a lambda is fine and is used throughout the shipped
/// corpus.
///
/// Severity is unchanged at both sites: `binding.rs` used `ValidationError::
/// warning`, which is the documented shim that already produces a blocking
/// error, so this replaces the wording only.
pub fn out_of_scope_collection_local(code: ErrorCode, path: &str, root: &str) -> ValidationError {
    ValidationError::error(
        code,
        path,
        format!(
            "Binding @{} is not in scope here — @item / @index are bound only inside a \
             collection callback (array/map, array/filter, …).",
            root
        ),
    )
    .with_suggestion(
        "Under `children:` the collection callback must use the EXPLICIT lambda form — \
         `(array/map <collection> (fn (item) <node>))`. The implicit shorthand \
         `(array/map <collection> { … })` is not recognised there, so its body is not \
         treated as a lambda and @item never comes into scope. Both a raw `type:` object \
         and a JSX element are valid as <node>.",
    )
}

/// A validation error with full context
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Error code
    pub code: ErrorCode,
    /// Human-readable error message
    pub message: String,
    /// JSON path to the error location
    pub path: String,
    /// Severity (Error or Warning)
    pub severity: Severity,
    /// Suggestion for fixing the error
    pub suggestion: Option<String>,
    /// Valid values for enum-like fields
    pub valid_values: Option<Vec<String>>,
    /// Expected schema shape
    pub expected_shape: Option<String>,
    /// Additional context for LLM fix guidance
    pub fix_guidance: Option<String>,
}

impl ValidationError {
    /// Create a new error
    pub fn error(code: ErrorCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: path.into(),
            severity: Severity::Error,
            suggestion: None,
            valid_values: None,
            expected_shape: None,
            fix_guidance: None,
        }
    }

    // `warning()` — the legacy alias of `error()` — was removed 2026-09-02:
    // a constructor named "warning" that produced a blocking error misnamed
    // severity at every call site. `phase_f_warning()` — the only genuinely
    // non-blocking constructor — was removed 2026-09-02 too: every site that
    // used it was audited and flipped to `error()`. `Severity::Warning` and
    // `ValidationResult::warnings` remain (JSON output shape, `is_warning()`),
    // but nothing in this codebase constructs a `Warning`-severity finding
    // any more.

    /// Add a suggestion
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Add valid values
    pub fn with_valid_values(mut self, values: Vec<String>) -> Self {
        self.valid_values = Some(values);
        self
    }

    /// Add expected shape
    pub fn with_expected_shape(mut self, shape: impl Into<String>) -> Self {
        self.expected_shape = Some(shape.into());
        self
    }

    /// Add fix guidance
    pub fn with_fix_guidance(mut self, guidance: impl Into<String>) -> Self {
        self.fix_guidance = Some(guidance.into());
        self
    }

    /// Check if this is an error (not a warning)
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Check if this is a warning
    pub fn is_warning(&self) -> bool {
        self.severity == Severity::Warning
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.code, self.path, self.message)?;
        if let Some(suggestion) = &self.suggestion {
            write!(f, " ({})", suggestion)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// Result of validation containing all errors and warnings
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    /// All validation errors (blocking)
    pub errors: Vec<ValidationError>,
    /// All validation warnings (non-blocking)
    pub warnings: Vec<ValidationError>,
}

impl ValidationResult {
    /// Create a new empty result
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an error
    pub fn add_error(&mut self, error: ValidationError) {
        if error.is_warning() {
            self.warnings.push(error);
        } else {
            self.errors.push(error);
        }
    }

    /// Add multiple errors
    pub fn add_errors(&mut self, errors: impl IntoIterator<Item = ValidationError>) {
        for error in errors {
            self.add_error(error);
        }
    }

    /// Merge another result into this one
    pub fn merge(&mut self, other: ValidationResult) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }

    /// Check if validation passed (no errors)
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Check if there are any warnings
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Get total error count
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// Get total warning count
    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    /// Get all issues (errors + warnings)
    pub fn all_issues(&self) -> impl Iterator<Item = &ValidationError> {
        self.errors.iter().chain(self.warnings.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_str() {
        assert_eq!(ErrorCode::EntityMissingName.as_str(), "ORB_E_MISSING_NAME");
        assert_eq!(
            ErrorCode::TraitNoInitialState.as_str(),
            "ORB_T_NO_INITIAL_STATE"
        );
        assert_eq!(ErrorCode::PageMissingPath.as_str(), "ORB_P_MISSING_PATH");
    }

    #[test]
    fn test_error_code_category() {
        assert_eq!(ErrorCode::EntityMissingName.category(), "entity");
        assert_eq!(ErrorCode::TraitNoInitialState.category(), "trait");
        assert_eq!(ErrorCode::PageMissingPath.category(), "page");
        assert_eq!(ErrorCode::SlotMainNotCovered.category(), "slot");
    }

    #[test]
    fn test_validation_error_creation() {
        let error = ValidationError::error(
            ErrorCode::EntityMissingName,
            "orbitals[0].entity.name",
            "Entity name is required",
        )
        .with_suggestion("Add a name for your entity");

        assert!(error.is_error());
        assert!(!error.is_warning());
        assert_eq!(error.code, ErrorCode::EntityMissingName);
        assert!(error.suggestion.is_some());
    }

    #[test]
    fn test_validation_result() {
        let mut result = ValidationResult::new();
        assert!(result.is_valid());

        result.add_error(ValidationError::error(
            ErrorCode::EntityMissingName,
            "test",
            "missing",
        ));
        assert!(!result.is_valid());
        assert_eq!(result.error_count(), 1);

        // `ValidationError::warning` is intentionally a shim that produces
        // an Error-severity diagnostic (see its docstring) — the validator
        // has zero non-error output by design. So routing it through
        // `add_error` puts it in the `errors` bucket, not `warnings`.
        result.add_error(ValidationError::error(
            ErrorCode::TraitMissingRenderUI,
            "test",
            "warning",
        ));
        assert_eq!(result.warning_count(), 0);
        assert_eq!(result.error_count(), 2);
    }
}
