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
    /// Relation target entity doesn't exist
    EntityInvalidRelation,
    /// State machine has no initial state
    TraitNoInitialState,
    /// Interaction trait missing render_ui effects
    TraitMissingRenderUI,
    /// A program trait is listed on no page
    TraitUnmounted,
    /// A trait-value literal (`<spec>.traits.<Trait>`) whose behavior or trait
    /// does not resolve against the installed behavior packages.
    TraitBehaviorRefUnresolved,
    /// A `behavior/ref` whose argument is computed at run time: a behavior from
    /// data must arrive as a `behavior` value, never re-parsed from text.
    TraitBehaviorRefNotLiteral,
    /// An `["unquote", …]` call outside a quasiquote body in an `.orb` input.
    TraitUnquoteOutsideQuasiquote,
    /// Config field typed `event` holds a value that is not a member of the
    /// trait's post-rename event set (SCAN-EVENT-KNOB-1). Definer-position
    /// knobs (A-UNIFY) skip this membership check — their value CREATES the
    /// event name — but still land here if the default isn't a plain string.
    TraitInvalidEventDefault,
    /// An event-typed config value addresses a trait the program does not have.
    TraitEventRefUnresolved,
    /// An event-typed config value addresses an event its trait does not declare as an external input.
    TraitEventRefNotExternalInput,
    /// Config field's default value doesn't match its declared structural
    /// shape (array vs scalar, object vs string, missing required, etc.).
    /// Fires after the typed-config carriers (`items` / `properties` /
    /// `values`) make the declared shape concrete enough to check against.
    TraitConfigShapeMismatch,
    /// A declaration uses a generic type — bare `json`, `object` (without
    /// declared `properties`), or `any` — in an entity field, config knob,
    /// type-alias member, or event payload field. Generic types are being
    /// eliminated from the language: renderable content belongs in `node`,
    /// structured data in inline `{ k: T }` shapes / named aliases /
    /// `Map K V`, and genuinely polymorphic atoms in declared type
    /// parameters. Hard error since 2026-09-01 (both behavior packages
    /// report zero occurrences).
    GenericTypeDeprecated,
    /// A type reference names no declared type, entity or primitive
    TypeUnknown,
    /// A type composition chain refers back to itself
    TypeCycle,
    /// Composed types do not share a kind (all Entity, all Trait, …)
    TypeKindMismatch,
    /// A field is declared in more than one composed entity
    TypeCompositionConflict,
    /// A type body does not match its `::` type annotation
    TypeAnnotationMismatch,
    /// A type alias body uses a type parameter the alias does not declare
    TypeUnknownParam,
    /// A type alias is applied with the wrong number of type arguments
    TypeWrongArgCount,
    /// An `emits` entry names a declared type that is not an Event type
    EventTypeWrongKind,
    /// Page is missing a path
    PageMissingPath,
    /// A page names a protocol that is no row of the protocol table
    PageUnknownProtocol,
    /// An `http` endpoint has no covering `net` listen grant
    PageEndpointGrantMissing,
    /// A protocol page's address does not type-check against the protocol's address record
    PageAddressInvalid,
    /// An `orbital X = Alias.orbitals.Y { roles { … } } }` target is not a
    /// member of the CONSUMER roster's vocabulary (`identity_entities` on the
    /// importing schema, post-inline).
    OrbitalRefRoleTargetNotMember,
    /// An `orbital X = Alias.orbitals.Y { roles { … } } }` key names no
    /// value in any UPSTREAM identity field's vocabulary (or the upstream
    /// alias declares no roster at all).
    OrbitalRefRoleUnknownUpstream,
    /// A kept trait/relation/page references an out-of-orbital entity that
    /// `obj.entities {}` never maps — the import splice has nothing to
    /// rebind it to.
    OrbitalRefEntityUnmapped,
    /// An `obj.entities {}` key names no entity in the upstream closure's
    /// out-of-orbital universe.
    OrbitalRefEntityUnknownKey,
    /// An entity an imported orbital expects of a sibling is provided by several sibling imports and no
    /// `entities {}` entry names which one it binds to.
    OrbitalRefEntityExpectAmbiguous,
    /// An `obj.entities {}` value names no entity the consumer schema
    /// declares.
    OrbitalRefEntityTargetUnknown,
    /// A `fields { OLD: NEW }` rename on an `orbital X = Alias.orbitals.Y
    /// { … }` reference left a structurally-recognized field-reference
    /// position (a config default's bare/list field-name value) still
    /// naming the pre-rename upstream field — the rename silently failed to
    /// reach it (G3, `docs/Almadar_Compiler_Gaps.md` §82, rung 2: what used
    /// to validate 0/0 with a dead reference now errors instead).
    OrbitalRefFieldRenameUnapplied,
    /// A page whose `main` a shell owns through a config channel
    /// (`contentTrait` …) also mounts a trait that paints a control into
    /// `main` which neither the owner nor anything it embeds references: the
    /// control stacks outside the shell's layout instead of inside it.
    PageControlOutsideShell,
    /// A composed trait's rendered control fires an event carrying a value
    /// that no other trait hears and whose own handling reads none of it: the
    /// value the user entered is discarded (a form submission nothing keeps).
    ControlValueDiscarded,

    // =========================================================================
    // Slot Errors (ORB_SLOT_*)
    // =========================================================================
    /// Main slot not covered by any trait
    SlotMainNotCovered,

    // =========================================================================
    // Render-UI Errors (ORB_RUI_*)
    // =========================================================================
    /// Invalid slot in render_ui effect
    RenderUIInvalidSlot,
    /// Invalid pattern type
    RenderUIInvalidPattern,
    /// Invalid prop for pattern
    RenderUIInvalidProp,
    /// A non-drawable child composed under a canvas draw-host. A draw-host paints
    /// only neutral drawable descriptors (draw-sprite / draw-shape / draw-text /
    /// draw-*-layer); DOM chrome belongs in a `hud`/`overlay` slot.
    RenderUIChildNotDrawable,
    /// Invalid itemAction configuration
    RenderUIInvalidItemAction,
    /// Unknown itemAction prop (e.g., hallucinated 'condition')
    RenderUIUnknownItemActionProp,
    /// Nested field path references invalid relation or field
    RenderUIInvalidFieldPath,
    /// A `form`-contract pattern offers an input for an `@intrinsic` entity
    /// field — intrinsic fields are never persisted, so the input is discarded.
    RenderUIFormFieldIntrinsic,
    /// L2.1 keystone: render-ui prop bound to a `@payload.X` / `@entity.X`
    /// / `@config.X` / SExpression operator whose resolved type doesn't
    /// match any of the prop's declared `types` in the patterns registry.
    /// Distinct from `RenderUIPropTypeMismatch` (literal-only); this rule
    /// uses the F3 binding resolver to walk through bindings and
    /// SExpression operator return types (sharpened by F2's
    /// `returnSemantics` for input-dependent operators). Gap #6 in
    /// `docs/Almadar_Std_Verification.md`.
    RenderUIBindingTypeMismatch,
    /// L2.2: render-ui prop with `kind: "event-ref"` or `kind: "callback"`
    /// bound to an event-name string whose declared payload schema in
    /// the trait's `emits {}` block doesn't match the component's
    /// documented bus emit shape (registry's `emitPayloadSchema` for
    /// event-ref props; `callbackArgs` for callback props). Gap #32 in
    /// `docs/Almadar_Std_Verification.md`.
    RenderUIEventBindingPayloadMismatch,
    /// call-service declares `emit.cancelled` but no `key` — nothing can
    /// cancel an unkeyed call
    CallServiceCancelledWithoutKey,
    /// call-service `key` is a non-string literal (number/bool/object/array)
    CallServiceKeyType,
    /// the app runs `cancel-call` but no `call-service` anywhere carries a `key`
    CancelCallNoKeyedCall,
    /// a literal `(as "k" CallKey)` in a program whose keyed calls are all keyed by literals, none of them `"k"`
    CallKeyUnused,
    /// `--final`: a `cancel-call` on a `CallKey` (or a list of them) in a finished program where no keyed call has a
    /// `CallKey`-typed `key:`, so nothing the program runs is named by a key made with `as`
    CancelCallKeyNeverCalled,
    /// a keyed call-service whose registry responseShape already has a `key` field
    CallServiceResultHasKey,

    /// an inbound handler (`listens { EVENT -> inbound "name" … }`) is reachable in a program whose app does not declare `name` in `inbound`
    InboundUndeclared,
    /// an `inbound` entry is not one lower-case hook name (letters, digits, `-`), or is listed twice
    InboundEntryInvalid,
    /// two traits handle the same inbound hook name
    InboundDuplicate,
    /// an inbound handler answers with an event its trait does not declare in `emits`
    InboundAnswerUndeclared,
    /// a trait other than an inbound handler emits the handler's answer event and the handler does not relay it: only the handler's own emit answers
    InboundAnswerForeign,
    /// an `os/` call needs something the calling orbital's grants do not cover
    OsGrantMissing,
    /// an `os/` call's target (path, command, host, port, variable, flags) is not a literal
    OsTargetNotStatic,
    /// a declared `os/` grant entry that no `os/` call of the orbitals holding it uses (warning)
    OsGrantUnused,
    /// a declared `os/` grant entry whose shape cannot match anything
    OsGrantEntryInvalid,
    /// a `redis` call whose origin is not `redis://` or `rediss://`, whose command is not a non-empty list of text,
    /// a `postgres` call whose origin is not `postgres://` or `postgresql://`, whose literal statement is not a single
    /// an event reads a page fact the `browser/page-facts` call did not request
    PageFactsUnrequestedRead,

    /// a `deployment` unit lists an orbital the program does not declare
    PlacementOrbitalUnknown,
    /// an orbital is listed in more than one `deployment` unit, or twice in one
    PlacementOrbitalDuplicate,
    /// a `deployment` is declared and an orbital is in none of its units
    PlacementOrbitalUnplaced,
    /// a `deployment` unit lists no orbital
    PlacementUnitEmpty,
    /// `random-bytes`, `credential-status` or `credential-forget` with the wrong number of arguments
    HostEffectArity,
    /// a key other than `emit`, or a non-object, as the options of a host effect
    HostEffectOptionUnknown,
    /// a literal `random-bytes` count that is not an integer from 1 to 1024
    RandomBytesCount,
    /// a literal request of a `credential-*` administration effect that is not an object of its declared parameters, or
    /// leaves one out
    CredentialRequestInvalid,
    /// `browser/page-facts` fields: an unknown word, an empty list or a duplicate
    PageFactsFieldInvalid,
    /// an event reads a page fact the `browser/page-facts` call did not request
    /// a `browser/prompt` or `browser/embed` where the server runs the transition: a `[background]` tick
    OnDeviceEffectOnServer,
    /// a literal `browser/prompt` response schema that is not a JSON object
    OnDeviceSchemaInvalid,
    /// a `browser/tab-*` request that is not an object of the op's closed keys, or whose literal values the kernel refuses
    TabRequestInvalid,
    /// set effect using @payload (should use @entity)
    EffectSetPayloadNotAllowed,
    /// fetch effect referencing invalid entity
    EffectFetchInvalidEntity,
    /// `persist batch` ops expression is not array-typed
    PersistBatchOpsNotArray,
    /// Multiple render_ui effects targeting same slot
    EffectSlotConflict,
    /// Unknown effect type
    EffectUnknownType,
    /// `notify` is sugar for `render-ui toast { type: alert, … }`, desugared
    /// at `.lolo` lowering (owner ruling 2026-09-11) — the `.orb` IR never
    /// carries a `notify` effect kind. A `notify` tuple reaching L2 means a
    /// hand-authored `.orb` or a registry not yet re-emitted.
    EffectNotifyRetired,
    /// Navigate effect missing required payload for target page
    EffectNavigateMissingPayload,
    /// Trait declares an event in its `emits[]` block but no transition
    /// fires it via `(emit X)` or operator emit-config success/failure.
    /// Usually a copy-paste leftover or a stale declaration after a
    /// rename; gap #2 defense in `docs/Almadar_Std_Verification.md`.
    EmitDeclaredButUnfired,
    /// `orb validate --final`: a rendered control fires an event no trait handles — not the
    /// rendering trait, an embedder of it, nor a `listens` route (G-ORB-187).
    EmitControlUnhandled,
    /// `emit:` config carries a key the operator doesn't support
    /// (e.g. `on_message` on `fetch`, `failure` on `set`).
    EffectEmitKeyInvalid,
    /// Event name inside `emit:` isn't UPPER_SNAKE_CASE.
    EffectEmitInvalidName,
    /// Event in `emit:` isn't declared in the trait's `emits` array.
    EffectEmitUndeclaredEvent,
    /// Operator declares `requiresEmitSuccess: true` in canonical-operators.json
    /// but the effect call site is missing the trailing options object or its
    /// `emit.success` event key. Applies to fetch / persist / call-service
    /// by default; any future operator can opt in via the same JSON flag.
    EffectMissingRequiredEmit,
    /// `deliver: ids` makes the success event's `data` a row-set reference; any use other than
    /// passing it whole as a `render-ui` prop value reads rows that were never delivered.
    FetchDeliverIdsRowsRead,
    /// `total: none` skips the count, so the success event's `totalCount` is never set.
    FetchTotalNoneRead,

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
    /// One transition's data effects touch both browser-stored and server-stored entities (ORB_X_MIXED_RESIDENCE)
    CrossMixedResidence,
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
    /// Unknown binding root
    BindingUnknownRoot,
    /// `@config.<key>` literal whose key no reachable config schema declares
    ConfigDanglingRef,
    /// A `field @entity` config knob names something that is not a field of the trait's bound entity
    ConfigFieldRefUnknown,
    /// A `field @entity T` config knob names a field whose type is not `T`
    ConfigFieldRefTypeMismatch,
    /// Invalid binding path
    BindingInvalidPath,
    /// @payload not available in tick context
    BindingPayloadInTick,
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
    /// A record literal written into an entity (a `persist` data literal, or a `set` into a relation field) names a
    /// key the entity does not declare.
    RowLiteralUnknownField,
    /// A record literal written into an entity carries a value whose type the declared field does not admit.
    RowLiteralFieldTypeMismatch,
    /// A `persist create` data literal omits a column the entity requires (no default, not framework-stamped).
    RowLiteralMissingRequired,
    /// `(set @entity.X (op …))` assigns an operator/expression value whose
    /// inferred result type is incompatible with the target field's declared
    /// type (e.g. `(set @entity.count (str/concat …))` into a `number` field).
    /// The value's type is inferred *through* the S-expression via the canonical
    /// operator `return_semantics` lattice; an unknowable result (`Json`) never
    /// rejects (Gap G-LOLO-SET-OP-RETURN).
    SetReturnTypeMismatch,
    /// An ordering comparison (`<`, `<=`, `>`, `>=`) whose operands infer to a
    /// number on one side and a string on the other. The JS runtime coerces,
    /// the compiled TypeScript does not typecheck, and Rust compares kinds —
    /// the paths disagree. Convert one side so both are the same kind.
    ComparisonOperandTypeMismatch,
    /// An operator argument whose static type contradicts the type the operator
    /// table declares for its position — `(str/upper @entity.count)` with
    /// `count: number`, `(+ "a" 1)`. Both evaluators raise the same typed
    /// mismatch at run time (D-12), so the program can never do what it says.
    OperatorArgTypeMismatch,
    /// A value handed to an operator has no determined type — an unresolved expression, an expression value, json,
    /// a loose union, a list whose items share no type — so it cannot be checked against the type the operator
    /// declares (or, at a type variable, what the variable stands for and the type of the result are unknown).
    /// Narrow it with `(as value T)` (checked, null on a misfit), or give it a real type (a declared payload or
    /// entity shape, a named union alias for a list of differently-shaped items).
    OperatorArgUndetermined,
    /// An ordering comparison (`<`, `<=`, `>`, `>=`) over an operand that can be missing or null — an optional
    /// entity field with no default, an optional payload field — that the program neither guards with a presence
    /// check (`(and (validate/required x) (< x 3))`, an `if`/`when` on presence) nor declares a default for.
    /// Both evaluators read a missing value as below any number; every push-down store fails the comparison, so
    /// the meaning is the program's to state.
    OrderingOptionalOperand,
    /// An operand that can be missing or null — an optional entity field with no default, an optional payload field —
    /// handed to a position the kernel raises on when it is null (`rejectsNull` in the operator table), with no presence
    /// check or default in front of it. The program says what a missing value means: a guard, a default, or a required field.
    OperatorNullOperand,
    /// `(set @entity.X @config.<knob>)` where the value is STILL the literal
    /// `@config.<knob>` token post-inline (no default resolved it, no
    /// call-site override supplied one) and `X` is a REQUIRED entity field.
    /// The emitter has nothing honest to assign — a required TS slot gets
    /// `undefined`. Give the knob a default or make the field optional.
    SetRequiredFieldFromOptionalKnob,
    /// `(emit EVT {...})` literal payload is missing a field the event's
    /// declared payload marks as required. Error severity.
    EmitPayloadMissingRequired,
    /// `(emit EVT {...})` literal carries a field that is not declared on
    /// the event's payload schema. Error severity.
    EmitPayloadExtraField,
    /// `(emit EVT {...})` literal provides a value whose primitive type
    /// does not match the declared `PayloadField.field_type`. Error severity.
    EmitPayloadFieldTypeMismatch,
    /// An `emit` repeats an earlier one of the same transition identically: the dispatch loop collapses the second.
    EmitRepeatsIdentically,
    /// An array field (entity or payload-schema) declares no element type.
    UntypedArrayField,
    /// A render-ui pattern prop typed `EntityWith<K>` declares required entity
    /// fields (`requiredFields` in the registry), but the bound entity does not
    /// provide them and no remap/field-map supplies them. The component would
    /// render blank rows. Remap-aware (runs post-inline). Error severity.
    EntityPropContract,
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
    /// Phase F.2: two or more traits declare the same external event name in
    /// their `emits` arrays but disagree on the declared payload field list.
    /// Consumers that listen on the event will see one shape from one
    /// emitter and a different shape from another. Error severity.
    CrossEmitterDeclaredPayloadMismatch,
    /// An effect (`emit`, `persist`, `set`, `notify`, …) inside the `fn` body of a collection operator: a lambda is
    /// evaluated for its value, an effect there never runs. Error severity.
    EffectInLambda,
    /// A `[shared]` entity is render-ui-read by more than one distinct trait.
    /// Exactly one render authority is allowed per shared entity, mirroring
    /// `SlotContention`'s single-owner rule.
    SharedEntityRenderAuthority,

    // =========================================================================
    // Closed Circuit Errors (CIRCUIT_*)
    // =========================================================================
    /// Action emits event with no transition handler
    CircuitOrphanEvent,
    /// Interactive component missing itemActions
    CircuitDeadEndComponent,
    /// State has no incoming transitions
    CircuitUnreachableState,
    /// State has no outgoing transitions
    CircuitDeadEndState,
    /// Action-component mismatch
    CircuitActionComponentMismatch,
    /// The trait that OWNS a page's content region (`page_content_owner`'s
    /// `channel`/`sole-writer`) has a user-triggered arm that repaints its
    /// own main-slot body with something offering no way on: no content-body
    /// pattern, no registry event-kind prop bound to a live (non-lifecycle)
    /// event or an unresolved binding, and no `@trait.X` embed. "User-
    /// triggered" means the event is reachable from a rendered affordance or
    /// a `listens` route, transitively, minus async-result and lifecycle
    /// events. Exempt: an arm running its own `fetch`/`persist` (its result
    /// arm repaints); a non-self-transition escaping to a state with another
    /// producible, non-lifecycle arm. Ported from the JS wiring lint's
    /// `viewer-stranded`, predicate revised 2026-09-12 per the F31 live
    /// probe (`play_scene` on `ui-minigolf-board`/`std-chem-titration`): the
    /// JS's labelled-control test and `@`/`?` sigil branch were both
    /// heuristics and are dropped — a registry event-kind prop is the only
    /// test. Error severity.
    CircuitViewerStranded,

    // =========================================================================
    // Icon Errors (ORB_UI_ICON_*)
    // =========================================================================
    /// Icon name has invalid format
    IconInvalidFormat,
    /// Icon name is empty
    IconEmpty,
    /// Event name not UPPER_SNAKE_CASE
    TraitEventInvalidName,
    /// A literal compared against an `@user.<field>` whose declared type is a
    /// closed vocabulary is not one of that vocabulary's members — a branch that
    /// can never be taken. The bug class that let std-healthcare's `"patient"`
    /// and std-lms's `"student"` ship green.
    SchemaRoleLiteralNotMember,
    /// `@user.<field>` names a field the declared `[identity]` entity does not
    /// have. Resolves to null at runtime, so the guard silently takes its
    /// negative branch.
    SchemaUnknownIdentityField,
    /// An access directive compares an `@entity.<field>` against `@user.id`,
    /// but `<field>` is not a relation to the `[identity]` entity. The
    /// comparison is then between a free-text scalar and a roster id — it
    /// type-checks, reads like ownership, and matches nothing, because the
    /// seeder only stamps viewer ids onto DECLARED relations. This is the
    /// `customerEmail` trap: a policy that silently denies everyone while
    /// looking correct.
    SchemaOwnerFieldNotIdentityTyped,
    /// A `data-theme` resolves to a literal theme the program does not declare (app or
    /// orbital `theme`, or the app's `themes`), so the compiled client ships no stylesheet for it (G-UI-106).
    ThemeUndeclared,
    /// Compose link-check: an orbital's `expects entity X` / `expects entity X [identity]`
    /// names a requirement no orbital in the program provides. The message
    /// names WHO ASKED — the consumer orbital — so the repair is attributable.
    SchemaExpectationUnsatisfied,
    /// Compose link-check: the provider has the expected field but with an
    /// incompatible type (per orbital-core's `type_compat`; literal unions use
    /// subset semantics — every expected member must be a provider member).
    SchemaExpectedFieldTypeMismatch,
    /// A spec proposition (`@invariant` on `expects entity`, `@holds` on an `expects trait` state) is broken: a
    /// write or transition of the program falsifies it on a counterexample the evaluator confirms.
    SchemaInvariantViolated,
    /// A spec proposition the prover could not decide (an expression outside the decidable fragment, or a write
    /// whose effect on the proposition is over-approximated). Never a pass.
    SchemaInvariantUnproved,
    /// A `uses A from "…" { config { … } }` call-site override names a key
    /// the imported behavior's app-level `config {}` (`OrbitalSchema.config`,
    /// `Almadar_Orbital_Import.md` §4.5) does not declare.
    UsesConfigUnknownKey,

    // =========================================================================
    // Generic Errors (ORB_GEN_*)
    // =========================================================================
    /// Generic invalid value
    InvalidValue,
    /// Generic invalid reference format
    InvalidReference,
    /// Generic missing required field
    MissingField,
    /// Generic duplicate entity/item
    DuplicateEntity,
}

include!(concat!(env!("OUT_DIR"), "/error_code_docs.rs"));

impl ErrorCode {
    /// Get the string code matching TypeScript error codes
    pub fn as_str(&self) -> &'static str {
        match self {
            // Entity
            ErrorCode::EntityMissingName => "ORB_E_MISSING_NAME",
            ErrorCode::EntityInvalidRelation => "ORB_E_INVALID_RELATION",

            // Trait
            ErrorCode::TraitNoInitialState => "ORB_T_NO_INITIAL_STATE",
            ErrorCode::TraitMissingRenderUI => "ORB_T_MISSING_RENDER_UI",
            ErrorCode::TraitUnmounted => "ORB_T_TRAIT_UNMOUNTED",
            ErrorCode::TraitBehaviorRefUnresolved => "ORB_T_BEHAVIOR_REF_UNRESOLVED",
            ErrorCode::TraitBehaviorRefNotLiteral => "ORB_T_BEHAVIOR_REF_NOT_LITERAL",
            ErrorCode::TraitUnquoteOutsideQuasiquote => "ORB_T_UNQUOTE_OUTSIDE_QUASIQUOTE",
            ErrorCode::TraitInvalidEventDefault => "ORB_T_INVALID_EVENT_DEFAULT",
            ErrorCode::TraitEventRefUnresolved => "ORB_T_EVENT_REF_UNRESOLVED",
            ErrorCode::TraitEventRefNotExternalInput => "ORB_T_EVENT_REF_NOT_EXTERNAL_INPUT",
            ErrorCode::TraitConfigShapeMismatch => "ORB_T_CONFIG_SHAPE_MISMATCH",
            ErrorCode::GenericTypeDeprecated => "ORB_T_GENERIC_TYPE_DEPRECATED",
            ErrorCode::TypeUnknown => "ORB_T_TYPE_UNKNOWN",
            ErrorCode::TypeCycle => "ORB_T_TYPE_CYCLE",
            ErrorCode::TypeKindMismatch => "ORB_T_TYPE_KIND_MISMATCH",
            ErrorCode::TypeCompositionConflict => "ORB_T_TYPE_COMPOSITION_CONFLICT",
            ErrorCode::TypeAnnotationMismatch => "ORB_T_TYPE_ANNOTATION_MISMATCH",
            ErrorCode::TypeUnknownParam => "ORB_T_TYPE_UNKNOWN_PARAM",
            ErrorCode::TypeWrongArgCount => "ORB_T_TYPE_WRONG_ARG_COUNT",
            ErrorCode::EventTypeWrongKind => "ORB_T_EVENT_TYPE_WRONG_KIND",

            // Page
            ErrorCode::PageMissingPath => "ORB_P_MISSING_PATH",
            ErrorCode::PageUnknownProtocol => "ORB_P_UNKNOWN_PROTOCOL",
            ErrorCode::PageEndpointGrantMissing => "ORB_P_ENDPOINT_GRANT_MISSING",
            ErrorCode::PageAddressInvalid => "ORB_P_ADDRESS_INVALID",
            ErrorCode::OrbitalRefRoleTargetNotMember => "ORB_O_ROLE_TARGET_NOT_MEMBER",
            ErrorCode::OrbitalRefRoleUnknownUpstream => "ORB_O_ROLE_UNKNOWN_UPSTREAM",
            ErrorCode::OrbitalRefEntityUnmapped => "ORB_O_ENTITY_UNMAPPED",
            ErrorCode::OrbitalRefEntityUnknownKey => "ORB_O_ENTITY_UNKNOWN_KEY",
            ErrorCode::OrbitalRefEntityExpectAmbiguous => "ORB_O_ENTITY_EXPECT_AMBIGUOUS",
            ErrorCode::OrbitalRefEntityTargetUnknown => "ORB_O_ENTITY_TARGET_UNKNOWN",
            ErrorCode::OrbitalRefFieldRenameUnapplied => "ORB_O_FIELD_RENAME_UNAPPLIED",
            ErrorCode::PageControlOutsideShell => "ORB_PAGE_CONTROL_OUTSIDE_SHELL",
            ErrorCode::ControlValueDiscarded => "ORB_CONTROL_VALUE_DISCARDED",

            // Slot
            ErrorCode::SlotMainNotCovered => "ORB_SLOT_MAIN_NOT_COVERED",

            // Render-UI
            ErrorCode::RenderUIInvalidSlot => "ORB_RUI_INVALID_SLOT",
            ErrorCode::RenderUIInvalidPattern => "ORB_RUI_INVALID_PATTERN",
            ErrorCode::RenderUIInvalidProp => "ORB_RUI_INVALID_PROP",
            ErrorCode::RenderUIChildNotDrawable => "ORB_RUI_CHILD_NOT_DRAWABLE",
            ErrorCode::RenderUIInvalidItemAction => "ORB_RUI_INVALID_ITEM_ACTION",
            ErrorCode::RenderUIUnknownItemActionProp => "ORB_RUI_UNKNOWN_ITEM_ACTION_PROP",
            ErrorCode::RenderUIInvalidFieldPath => "ORB_RUI_INVALID_FIELD_PATH",
            ErrorCode::RenderUIFormFieldIntrinsic => "ORB_RUI_FORM_FIELD_INTRINSIC",
            ErrorCode::RenderUIBindingTypeMismatch => "ORB_RUI_BINDING_TYPE_MISMATCH",
            ErrorCode::RenderUIEventBindingPayloadMismatch => {
                "ORB_RUI_EVENT_BINDING_PAYLOAD_MISMATCH"
            }

            // Effect
            ErrorCode::CallServiceCancelledWithoutKey => "ORB_CALL_SERVICE_CANCELLED_WITHOUT_KEY",
            ErrorCode::CallServiceKeyType => "ORB_CALL_SERVICE_KEY_TYPE",
            ErrorCode::CancelCallNoKeyedCall => "ORB_CANCEL_CALL_NO_KEYED_CALL",
            ErrorCode::CallKeyUnused => "ORB_CALL_KEY_UNUSED",
            ErrorCode::CancelCallKeyNeverCalled => "ORB_CANCEL_CALL_KEY_NEVER_CALLED",
            ErrorCode::CallServiceResultHasKey => "ORB_CALL_SERVICE_RESULT_HAS_KEY",

            ErrorCode::InboundUndeclared => "ORB_INBOUND_UNDECLARED",
            ErrorCode::InboundEntryInvalid => "ORB_INBOUND_ENTRY_INVALID",
            ErrorCode::InboundDuplicate => "ORB_INBOUND_DUPLICATE",
            ErrorCode::InboundAnswerUndeclared => "ORB_INBOUND_ANSWER_UNDECLARED",
            ErrorCode::InboundAnswerForeign => "ORB_INBOUND_ANSWER_FOREIGN",
            ErrorCode::OsGrantMissing => "ORB_OS_GRANT_MISSING",
            ErrorCode::OsTargetNotStatic => "ORB_OS_TARGET_NOT_STATIC",
            ErrorCode::OsGrantUnused => "ORB_OS_GRANT_UNUSED",
            ErrorCode::OsGrantEntryInvalid => "ORB_OS_GRANT_ENTRY_INVALID",
            ErrorCode::PageFactsUnrequestedRead => "ORB_PAGE_FACTS_UNREQUESTED_READ",

            ErrorCode::PlacementOrbitalUnknown => "ORB_PLACEMENT_ORBITAL_UNKNOWN",
            ErrorCode::PlacementOrbitalDuplicate => "ORB_PLACEMENT_ORBITAL_DUPLICATE",
            ErrorCode::PlacementOrbitalUnplaced => "ORB_PLACEMENT_ORBITAL_UNPLACED",
            ErrorCode::PlacementUnitEmpty => "ORB_PLACEMENT_UNIT_EMPTY",
            ErrorCode::HostEffectArity => "ORB_HOST_EFFECT_ARITY",
            ErrorCode::HostEffectOptionUnknown => "ORB_HOST_EFFECT_OPTION_UNKNOWN",
            ErrorCode::CredentialRequestInvalid => "ORB_CREDENTIAL_REQUEST_INVALID",
            ErrorCode::RandomBytesCount => "ORB_RANDOM_BYTES_COUNT",
            ErrorCode::PageFactsFieldInvalid => "ORB_PAGE_FACTS_FIELD_INVALID",
            ErrorCode::OnDeviceEffectOnServer => "ORB_ON_DEVICE_EFFECT_ON_SERVER",
            ErrorCode::OnDeviceSchemaInvalid => "ORB_ON_DEVICE_SCHEMA_INVALID",
            ErrorCode::TabRequestInvalid => "ORB_TAB_REQUEST_INVALID",
            ErrorCode::EffectSetPayloadNotAllowed => "ORB_EFF_SET_PAYLOAD_NOT_ALLOWED",
            ErrorCode::EffectFetchInvalidEntity => "ORB_EFF_FETCH_INVALID_ENTITY",
            ErrorCode::PersistBatchOpsNotArray => "ORB_PERSIST_BATCH_OPS_NOT_ARRAY",
            ErrorCode::EffectSlotConflict => "ORB_EFF_SLOT_CONFLICT",
            ErrorCode::EffectUnknownType => "ORB_EFF_UNKNOWN_TYPE",
            ErrorCode::EffectNotifyRetired => "ORB_EFF_NOTIFY_RETIRED",
            ErrorCode::EffectNavigateMissingPayload => "ORB_EFF_NAVIGATE_MISSING_PAYLOAD",
            ErrorCode::EmitDeclaredButUnfired => "ORB_EMIT_DECLARED_BUT_UNFIRED",
            ErrorCode::EmitControlUnhandled => "ORB_EMIT_CONTROL_UNHANDLED",
            ErrorCode::EffectEmitKeyInvalid => "ORB_EFF_EMIT_KEY_INVALID",
            ErrorCode::EffectEmitInvalidName => "ORB_EFF_EMIT_INVALID_NAME",
            ErrorCode::EffectEmitUndeclaredEvent => "ORB_EFF_EMIT_UNDECLARED_EVENT",
            ErrorCode::EffectMissingRequiredEmit => "ORB_EFF_MISSING_REQUIRED_EMIT",
            ErrorCode::FetchDeliverIdsRowsRead => "ORB_FETCH_DELIVER_IDS_ROWS_READ",
            ErrorCode::FetchTotalNoneRead => "ORB_FETCH_TOTAL_NONE_READ",

            // S-Expression
            ErrorCode::SExprInvalidOperator => "SEXPR_INVALID_OPERATOR",
            ErrorCode::SExprArityMismatch => "SEXPR_ARITY_MISMATCH",
            ErrorCode::SExprLambdaArgPosition => "SEXPR_LAMBDA_ARG_POSITION",
            ErrorCode::SExprEffectInGuard => "SEXPR_EFFECT_IN_GUARD",

            // Cross-Orbital
            ErrorCode::CrossMixedResidence => "ORB_X_MIXED_RESIDENCE",
            ErrorCode::UnresolvedRelationTarget => "ORB_X_UNRESOLVED_RELATION_TARGET",
            ErrorCode::UnresolvedPatternFieldRef => "ORB_X_UNRESOLVED_PATTERN_FIELD_REF",

            // Binding
            ErrorCode::BindingUnknownRoot => "ORB_BINDING_UNKNOWN_ROOT",
            ErrorCode::ConfigDanglingRef => "ORB_CONFIG_DANGLING_REF",
            ErrorCode::ConfigFieldRefUnknown => "ORB_CONFIG_FIELD_REF_UNKNOWN",
            ErrorCode::ConfigFieldRefTypeMismatch => "ORB_CONFIG_FIELD_REF_TYPE_MISMATCH",
            ErrorCode::BindingInvalidPath => "ORB_BINDING_INVALID_PATH",
            ErrorCode::BindingPayloadInTick => "ORB_BINDING_PAYLOAD_IN_TICK",
            ErrorCode::SetValueTypeMismatch => "ORB_X_SET_VALUE_TYPE_MISMATCH",
            ErrorCode::EnumValueNotMember => "ORB_X_ENUM_VALUE_NOT_MEMBER",
            ErrorCode::RowLiteralUnknownField => "ORB_X_ROW_LITERAL_UNKNOWN_FIELD",
            ErrorCode::RowLiteralFieldTypeMismatch => "ORB_X_ROW_LITERAL_FIELD_TYPE_MISMATCH",
            ErrorCode::RowLiteralMissingRequired => "ORB_X_ROW_LITERAL_MISSING_REQUIRED",
            ErrorCode::SetReturnTypeMismatch => "ORB_X_SET_RETURN_TYPE_MISMATCH",
            ErrorCode::ComparisonOperandTypeMismatch => "ORB_X_COMPARISON_OPERAND_TYPE_MISMATCH",
            ErrorCode::OperatorArgTypeMismatch => "ORB_X_OPERATOR_ARG_TYPE_MISMATCH",
            ErrorCode::OperatorArgUndetermined => "ORB_X_OPERATOR_ARG_UNDETERMINED",
            ErrorCode::OrderingOptionalOperand => "ORB_X_ORDERING_OPTIONAL_OPERAND",
            ErrorCode::OperatorNullOperand => "ORB_X_OPERATOR_NULL_OPERAND",
            ErrorCode::SetRequiredFieldFromOptionalKnob => {
                "ORB_X_SET_REQUIRED_FIELD_FROM_OPTIONAL_KNOB"
            }
            ErrorCode::EmitPayloadMissingRequired => "ORB_EMIT_PAYLOAD_MISSING_REQUIRED",
            ErrorCode::EmitPayloadExtraField => "ORB_EMIT_PAYLOAD_EXTRA_FIELD",
            ErrorCode::EmitPayloadFieldTypeMismatch => "ORB_EMIT_PAYLOAD_FIELD_TYPE_MISMATCH",
            ErrorCode::EmitRepeatsIdentically => "ORB_EMIT_REPEATS_IDENTICALLY",
            ErrorCode::UntypedArrayField => "ORB_X_UNTYPED_ARRAY",
            ErrorCode::EntityPropContract => "ORB_X_ENTITY_PROP_CONTRACT",
            ErrorCode::EmitPayloadNotProduced => "ORB_EMIT_PAYLOAD_NOT_PRODUCED",
            ErrorCode::CrossEmitterDeclaredPayloadMismatch => {
                "ORB_X_CROSS_EMITTER_DECLARED_PAYLOAD_MISMATCH"
            }
            ErrorCode::EffectInLambda => "ORB_EFFECT_IN_LAMBDA",

            // Entity Mutation / Tick
            ErrorCode::SharedEntityRenderAuthority => "ORB_SHARED_ENTITY_RENDER_AUTHORITY",

            // Closed Circuit
            ErrorCode::CircuitOrphanEvent => "CIRCUIT_ORPHAN_EVENT",
            ErrorCode::CircuitDeadEndComponent => "CIRCUIT_DEAD_END_COMPONENT",
            ErrorCode::CircuitUnreachableState => "CIRCUIT_UNREACHABLE_STATE",
            ErrorCode::CircuitDeadEndState => "CIRCUIT_DEAD_END_STATE",
            ErrorCode::CircuitActionComponentMismatch => "CIRCUIT_ACTION_COMPONENT_MISMATCH",
            ErrorCode::CircuitViewerStranded => "ORB_CIRCUIT_VIEWER_STRANDED",

            // Icon
            ErrorCode::IconInvalidFormat => "ORB_UI_ICON_INVALID_FORMAT",
            ErrorCode::IconEmpty => "ORB_UI_ICON_EMPTY",

            // Query Singleton

            // Trait Event
            ErrorCode::TraitEventInvalidName => "ORB_T_EVT_INVALID_NAME",

            // Schema
            ErrorCode::SchemaRoleLiteralNotMember => "ORB_S_ROLE_LITERAL_NOT_MEMBER",
            ErrorCode::SchemaUnknownIdentityField => "ORB_S_UNKNOWN_IDENTITY_FIELD",
            ErrorCode::SchemaOwnerFieldNotIdentityTyped => "ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED",
            ErrorCode::ThemeUndeclared => "ORB_THEME_UNDECLARED",
            ErrorCode::SchemaExpectationUnsatisfied => "ORB_S_EXPECTATION_UNSATISFIED",
            ErrorCode::SchemaExpectedFieldTypeMismatch => "ORB_S_EXPECTED_FIELD_TYPE_MISMATCH",
            ErrorCode::SchemaInvariantViolated => "ORB_S_INVARIANT_VIOLATED",
            ErrorCode::SchemaInvariantUnproved => "ORB_S_INVARIANT_UNPROVED",
            ErrorCode::UsesConfigUnknownKey => "ORB_U_CONFIG_UNKNOWN_KEY",

            // Identity

            // Behavior Package

            // Generic
            ErrorCode::InvalidValue => "ORB_GEN_INVALID_VALUE",
            ErrorCode::InvalidReference => "ORB_GEN_INVALID_REFERENCE",
            ErrorCode::MissingField => "ORB_GEN_MISSING_FIELD",
            ErrorCode::DuplicateEntity => "ORB_GEN_DUPLICATE",
        }
    }

    /// Get the category for this error code
    pub fn category(&self) -> &'static str {
        match self {
            ErrorCode::EntityMissingName
            | ErrorCode::EntityInvalidRelation
            | ErrorCode::UntypedArrayField => "entity",

            // A generic type in a declaration position — a TYPE-system rejection
            // since the burndown reached zero (2026-09-01).
            ErrorCode::TraitUnmounted
            | ErrorCode::TraitNoInitialState
            | ErrorCode::TraitMissingRenderUI
            | ErrorCode::TraitBehaviorRefUnresolved
            | ErrorCode::TraitBehaviorRefNotLiteral
            | ErrorCode::TraitUnquoteOutsideQuasiquote
            | ErrorCode::TraitInvalidEventDefault
            | ErrorCode::TraitEventRefUnresolved
            | ErrorCode::TraitEventRefNotExternalInput
            | ErrorCode::TraitConfigShapeMismatch
            | ErrorCode::TypeUnknown
            | ErrorCode::TypeCycle
            | ErrorCode::TypeKindMismatch
            | ErrorCode::TypeCompositionConflict
            | ErrorCode::TypeAnnotationMismatch
            | ErrorCode::TypeUnknownParam
            | ErrorCode::TypeWrongArgCount
            | ErrorCode::EventTypeWrongKind
            | ErrorCode::GenericTypeDeprecated => "trait",

            ErrorCode::PageMissingPath
            | ErrorCode::PageUnknownProtocol
            | ErrorCode::PageEndpointGrantMissing
            | ErrorCode::PageAddressInvalid
            | ErrorCode::OrbitalRefRoleTargetNotMember
            | ErrorCode::OrbitalRefRoleUnknownUpstream
            | ErrorCode::OrbitalRefEntityUnmapped
            | ErrorCode::OrbitalRefEntityUnknownKey
            | ErrorCode::OrbitalRefEntityExpectAmbiguous
            | ErrorCode::OrbitalRefEntityTargetUnknown
            | ErrorCode::OrbitalRefFieldRenameUnapplied
            | ErrorCode::PageControlOutsideShell => "page",

            ErrorCode::SlotMainNotCovered => "slot",

            ErrorCode::RenderUIInvalidSlot
            | ErrorCode::RenderUIInvalidPattern
            | ErrorCode::RenderUIInvalidProp
            | ErrorCode::RenderUIChildNotDrawable
            | ErrorCode::RenderUIInvalidItemAction
            | ErrorCode::RenderUIUnknownItemActionProp
            | ErrorCode::RenderUIInvalidFieldPath
            | ErrorCode::RenderUIFormFieldIntrinsic
            | ErrorCode::RenderUIBindingTypeMismatch
            | ErrorCode::RenderUIEventBindingPayloadMismatch
            | ErrorCode::EntityPropContract => "render_ui",

            ErrorCode::CallServiceCancelledWithoutKey
            | ErrorCode::CallServiceKeyType
            | ErrorCode::CancelCallNoKeyedCall
            | ErrorCode::CallKeyUnused
            | ErrorCode::CancelCallKeyNeverCalled
            | ErrorCode::CallServiceResultHasKey

            | ErrorCode::InboundUndeclared
            | ErrorCode::InboundEntryInvalid
            | ErrorCode::InboundDuplicate
            | ErrorCode::InboundAnswerUndeclared
            | ErrorCode::InboundAnswerForeign
            | ErrorCode::OsGrantMissing
            | ErrorCode::OsTargetNotStatic
            | ErrorCode::OsGrantUnused
            | ErrorCode::OsGrantEntryInvalid
            | ErrorCode::PageFactsUnrequestedRead

            | ErrorCode::PlacementOrbitalUnknown
            | ErrorCode::PlacementOrbitalDuplicate
            | ErrorCode::PlacementOrbitalUnplaced
            | ErrorCode::PlacementUnitEmpty
            | ErrorCode::HostEffectArity
            | ErrorCode::HostEffectOptionUnknown
            | ErrorCode::CredentialRequestInvalid
            | ErrorCode::RandomBytesCount
            | ErrorCode::PageFactsFieldInvalid
            | ErrorCode::OnDeviceEffectOnServer
            | ErrorCode::OnDeviceSchemaInvalid
            | ErrorCode::TabRequestInvalid
            | ErrorCode::EffectNotifyRetired
            | ErrorCode::EffectSetPayloadNotAllowed
            | ErrorCode::EffectFetchInvalidEntity
            | ErrorCode::PersistBatchOpsNotArray
            | ErrorCode::EffectSlotConflict
            | ErrorCode::EffectUnknownType
            | ErrorCode::EffectNavigateMissingPayload
            | ErrorCode::EffectEmitKeyInvalid
            | ErrorCode::EffectEmitInvalidName
            | ErrorCode::EffectEmitUndeclaredEvent
            | ErrorCode::EffectMissingRequiredEmit
            | ErrorCode::FetchDeliverIdsRowsRead
            | ErrorCode::FetchTotalNoneRead
            | ErrorCode::SetValueTypeMismatch
            | ErrorCode::EnumValueNotMember
            | ErrorCode::RowLiteralUnknownField
            | ErrorCode::RowLiteralFieldTypeMismatch
            | ErrorCode::RowLiteralMissingRequired
            | ErrorCode::SetReturnTypeMismatch
            | ErrorCode::ComparisonOperandTypeMismatch
            | ErrorCode::OperatorArgTypeMismatch
            | ErrorCode::OperatorArgUndetermined
            | ErrorCode::OrderingOptionalOperand
            | ErrorCode::OperatorNullOperand
            | ErrorCode::SetRequiredFieldFromOptionalKnob
            | ErrorCode::EmitDeclaredButUnfired
            | ErrorCode::EmitControlUnhandled => "effect",

            ErrorCode::SExprInvalidOperator
            | ErrorCode::SExprArityMismatch
            | ErrorCode::SExprLambdaArgPosition
            | ErrorCode::SExprEffectInGuard => "sexpr",

            ErrorCode::CrossMixedResidence
            | ErrorCode::UnresolvedRelationTarget
            | ErrorCode::UnresolvedPatternFieldRef
            | ErrorCode::CrossEmitterDeclaredPayloadMismatch => "cross",

            ErrorCode::BindingUnknownRoot
            | ErrorCode::ConfigDanglingRef
            | ErrorCode::ConfigFieldRefUnknown
            | ErrorCode::ConfigFieldRefTypeMismatch
            | ErrorCode::BindingInvalidPath
            | ErrorCode::BindingPayloadInTick
            | ErrorCode::EmitPayloadMissingRequired
            | ErrorCode::EmitPayloadExtraField
            | ErrorCode::EmitPayloadFieldTypeMismatch
            | ErrorCode::EmitRepeatsIdentically
            | ErrorCode::EmitPayloadNotProduced
            | ErrorCode::EffectInLambda => "binding",

            ErrorCode::SharedEntityRenderAuthority => "entity_mutation",

            ErrorCode::CircuitOrphanEvent
            | ErrorCode::CircuitDeadEndComponent
            | ErrorCode::CircuitUnreachableState
            | ErrorCode::CircuitDeadEndState
            | ErrorCode::CircuitActionComponentMismatch
            | ErrorCode::CircuitViewerStranded
            | ErrorCode::ControlValueDiscarded => "circuit",

            ErrorCode::IconInvalidFormat
            | ErrorCode::IconEmpty => "icon",

            ErrorCode::TraitEventInvalidName => "trait_event",

            ErrorCode::SchemaRoleLiteralNotMember
            | ErrorCode::SchemaUnknownIdentityField
            | ErrorCode::SchemaOwnerFieldNotIdentityTyped
            | ErrorCode::SchemaExpectationUnsatisfied
            | ErrorCode::SchemaExpectedFieldTypeMismatch
            | ErrorCode::SchemaInvariantViolated
            | ErrorCode::SchemaInvariantUnproved
            | ErrorCode::UsesConfigUnknownKey => "schema",

            ErrorCode::InvalidValue
            | ErrorCode::InvalidReference
            | ErrorCode::MissingField
            | ErrorCode::DuplicateEntity => "generic",

            ErrorCode::ThemeUndeclared => "schema",
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

/// The one `ORB_X_UNTYPED_ARRAY` diagnostic, shared by the entity-field and
/// payload-field checks so the wording can't drift between them.
pub fn untyped_array_field(path: &str, field_name: &str) -> ValidationError {
    ValidationError::error(
        ErrorCode::UntypedArrayField,
        path,
        format!(
            "Field '{}' is typed `array` but its element type is undeclared.",
            field_name
        ),
    )
    .with_suggestion(
        "Declare the element type as `[T]` (e.g. `[string]`) instead of the bare `array`.",
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

    #[test]
    fn every_error_code_carries_its_doc_comment_as_description() {
        assert!(!ErrorCode::ALL.is_empty());
        for code in ErrorCode::ALL {
            assert!(
                !code.description().trim().is_empty(),
                "{} has no doc comment",
                code.as_str()
            );
        }
    }

    #[test]
    fn a_code_s_description_is_its_doc_comment() {
        assert_eq!(
            ErrorCode::CircuitDeadEndState.description(),
            "State has no outgoing transitions"
        );
        assert_ne!(
            ErrorCode::CircuitUnreachableState.description(),
            ErrorCode::CircuitDeadEndState.description()
        );
    }
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
    fn every_lolo_type_error_code_is_a_known_code() {
        let known = |s: &str| ErrorCode::ALL.iter().any(|c| c.as_str() == s);
        for code in [
            "ORB_T_TYPE_UNKNOWN",
            "ORB_T_TYPE_CYCLE",
            "ORB_T_TYPE_KIND_MISMATCH",
            "ORB_T_TYPE_COMPOSITION_CONFLICT",
            "ORB_T_TYPE_ANNOTATION_MISMATCH",
            "ORB_T_TYPE_UNKNOWN_PARAM",
            "ORB_T_TYPE_WRONG_ARG_COUNT",
            "ORB_T_EVENT_TYPE_WRONG_KIND",
            "ORB_T_GENERIC_TYPE_DEPRECATED",
        ] {
            assert!(known(code), "{code} is not in ErrorCode::ALL");
        }
        assert!(!known("ORB_T_TYPE_NOT_A_REAL_CODE"));
        assert!(!known("ELOLO_T_PAYLOAD_SIBLING"));
        assert_eq!(ErrorCode::TypeUnknown.category(), "trait");
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

    /// Variants a std lint atom also owns that Rust still references outside . Each retirement
    /// removes its entry; the test fails if a new double owner appears or a listed one is already retired.
    const STILL_DOUBLE_OWNED: &[&str] = &[
        "CallKeyUnused",
        "CallServiceCancelledWithoutKey",
        "CallServiceKeyType",
        "CircuitOrphanEvent",
        "ConfigDanglingRef",
        "CrossEmitterDeclaredPayloadMismatch",
        "EffectFetchInvalidEntity",
        "EffectSetPayloadNotAllowed",
        "EnumValueNotMember",
        "GenericTypeDeprecated",
        "HttpAuthInvalid",
        "RenderUIInvalidSlot",
        "SExprInvalidOperator",
        "SchemaExpectationUnsatisfied",
        "SchemaExpectedFieldTypeMismatch",
        "SchemaOwnerFieldNotIdentityTyped",
        "TabRequestInvalid",
        "TraitInvalidEventDefault",
    ];

    fn double_owned(
        rust_refs: &std::collections::BTreeSet<String>,
        std_codes: &std::collections::BTreeSet<String>,
    ) -> Vec<String> {
        ErrorCode::ALL
            .iter()
            .filter(|c| std_codes.contains(c.as_str()))
            .map(|c| format!("{c:?}"))
            .filter(|name| rust_refs.contains(name))
            .collect()
    }

    fn rust_refs_outside_tests(dir: &std::path::Path, out: &mut std::collections::BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_refs_outside_tests(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && !path.ends_with("phases/validation/error.rs")
            {
                let text = std::fs::read_to_string(&path).unwrap();
                let live = text.split("#[cfg(test)]").next().unwrap();
                for part in live.split("ErrorCode::").skip(1) {
                    let name: String = part
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .collect();
                    out.insert(name);
                }
            }
        }
    }

    fn std_atom_codes(dir: &std::path::Path) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("std-orb-lint-") && name.ends_with(".lolo") {
                let text = std::fs::read_to_string(&path).unwrap();
                for part in text.split("code: \"").skip(1) {
                    let code: String = part
                        .chars()
                        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                        .collect();
                    out.insert(code);
                }
            }
        }
        out
    }

    #[test]
    fn a_code_owned_by_a_std_lint_atom_has_no_rust_emitter() {
        let atoms = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .map(|a| a.join("packages/almadar-std/behaviors/lolo/cli/atoms"))
            .find(|p| p.is_dir());
        let Some(atoms) = atoms else {
            eprintln!("std atoms not beside this checkout: double-owner check skipped");
            return;
        };
        let mut refs = std::collections::BTreeSet::new();
        rust_refs_outside_tests(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut refs);
        let found = double_owned(&refs, &std_atom_codes(&atoms));
        let listed: Vec<String> = STILL_DOUBLE_OWNED.iter().map(|s| s.to_string()).collect();
        let new: Vec<_> = found.iter().filter(|f| !listed.contains(f)).collect();
        let retired: Vec<_> = listed.iter().filter(|l| !found.contains(l)).collect();
        assert!(new.is_empty(), "new double owners (Rust emits what a std lint atom owns): {new:?}");
        assert!(retired.is_empty(), "retired: drop from STILL_DOUBLE_OWNED: {retired:?}");
    }

    #[test]
    fn control_double_owner_needs_both_a_rust_reference_and_a_std_code() {
        let std_codes: std::collections::BTreeSet<String> =
            ["ORB_UI_ICON_EMPTY".to_string()].into();
        let both: std::collections::BTreeSet<String> = ["IconEmpty".to_string()].into();
        let rust_only: std::collections::BTreeSet<String> = ["EntityNoFields".to_string()].into();
        assert_eq!(double_owned(&both, &std_codes), vec!["IconEmpty".to_string()]);
        assert!(double_owned(&rust_only, &std_codes).is_empty());
        assert!(double_owned(&both, &std::collections::BTreeSet::new()).is_empty());
    }
}
