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
    /// `local` on a non-persistent entity (ORB_E_LOCAL_REQUIRES_PERSISTENT)
    EntityLocalRequiresPersistent,
    /// `mock` seeding on an entity that is not `local` (ORB_E_MOCK_SEED_NOT_LOCAL)
    EntityMockSeedNotLocal,
    /// Both `mock` seeding and written-out `instances` (ORB_E_SEED_SOURCE_CONFLICT)
    EntitySeedSourceConflict,
    /// `[identity]` entity declared `local` (ORB_E_IDENTITY_LOCAL)
    EntityIdentityLocal,
    /// A written-out row with an unknown field, a missing required field, or (local) no id (ORB_E_INSTANCE_INVALID)
    EntityInstanceInvalid,
    /// `instances <lang>` on an entity whose rows are not browser-stored (ORB_E_LOCALE_INSTANCES_NOT_LOCAL)
    EntityLocaleInstancesNotLocal,
    /// `instances <lang>` for a locale the app does not declare, or for its default locale (ORB_E_LOCALE_INSTANCES_UNKNOWN_LOCALE)
    EntityLocaleInstancesUnknownLocale,
    /// Locale rows written without the default `instances`, or missing a declared locale (ORB_E_LOCALE_INSTANCES_INCOMPLETE)
    EntityLocaleInstancesIncomplete,
    /// Relation target entity doesn't exist
    EntityInvalidRelation,
    /// Enum field has no values
    EntityEmptyEnumValues,
    /// A literal default on a semantic-domain field does not satisfy that domain
    /// (`email : email = "not-an-email"`).
    EntitySemanticValueInvalid,
    /// An enum field's `@mock` names a candidate that is not one of its values.
    EntityMockNotInEnum,
    /// Phase F.3: an entity field declares `values: [...]` (literal-union)
    /// AND `default: <X>`, but X is not one of the declared values. Error.
    LiteralUnionDefaultMismatch,
    /// An entity field's `default` is structurally incompatible with its
    /// declared type — a relation field defaulting to a whole row (`seedRow
    /// : Note = {}`) instead of an id/absent, a number/boolean default on a
    /// string/enum field, etc. Rung-2 prevention: `check_field_def_shape`
    /// (`config_type.rs`) owns the one shape-vs-type check, reused here for
    /// entity defaults. Skips the string-vs-enum-membership case, which
    /// `LiteralUnionDefaultMismatch` already reports.
    EntityDefaultTypeMismatch,

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
    /// An event declared, emitted or transitioned on has an empty name
    TraitEmptyEventName,
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
    /// A trait-value literal (`<spec>.traits.<Trait>`) whose behavior or trait
    /// does not resolve against the installed behavior packages.
    TraitBehaviorRefUnresolved,
    /// A `behavior/ref` whose argument is computed at run time: a behavior from
    /// data must arrive as a `behavior` value, never re-parsed from text.
    TraitBehaviorRefNotLiteral,
    /// An `["unquote", …]` call outside a quasiquote body in an `.orb` input.
    TraitUnquoteOutsideQuasiquote,
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
    /// An event-typed config value addresses a trait the program does not have.
    TraitEventRefUnresolved,
    /// An event-typed config value addresses an event its trait does not declare as an external input.
    TraitEventRefNotExternalInput,
    /// A declared external input (`listens { EVENT -> external { … } }`) has no
    /// transition from the trait's initial state: inputs are complete commands.
    TraitExternalInputNotResting,
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
    /// A `linkedEntity` rebind names something that is not any orbital's
    /// primary or auxiliary entity anywhere in the schema — the §52
    /// rebind-merge (`phases::inline::entity::merge_into_named_entity`) has
    /// nowhere to land the imported trait's fields. Retired 2026-09-19: this
    /// used to fall back to the caller's primary entity silently; it is now
    /// a hard error instead of a wrong-entity merge.
    RebindTargetUnresolved,
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
    /// A `local` trait's transitions contain a server-bound effect
    /// (`fetch`/`persist`/`call-service`/`ref`/`deref`/`swap`/`watch`/
    /// `atomic`/`fetch-stream`), or a guard reading `@user.*`. `local`
    /// declares client-only dispatch (`OirTrait::dispatch_mode`) — a trait
    /// that actually touches the server or reads viewer identity is
    /// mis-tagged; its transitions would never await the round trip they
    /// depend on. Wave 2. Error severity.
    TraitLocalTouchesServer,
    /// A tick that runs in the browser (its trait is not server-awaited; the tick is not
    /// `[background]`) carries a server effect — a server-owned data effect, or a
    /// `runsOn: server` operator. Nothing replays a tick on the server, so the effect would be
    /// dropped. Server work goes through an emitted event a server-side trait handles.
    TraitClientTickServerEffect,
    /// `local` on a trait whose linked entity is neither `[shared]` nor
    /// `[runtime]`. `local` is only legal on an entity multiple traits
    /// co-mutate client-side (`[shared]`) or one that is never persisted at
    /// all (`[runtime]`) — on an ordinary persistent entity it would silently
    /// hide server-gated writes behind client-only dispatch. Wave 2. Error
    /// severity.
    TraitLocalOnNonSharedPersistent,
    /// A transition guard reads `@user.*` while the trait's
    /// `OirTrait::dispatch_mode()` is `RuntimeOptimistic` or
    /// `HybridClientOnly` — evaluated client-side only. An identity check
    /// that never reaches the server is not an authorization boundary: the
    /// client can construct any `@user` value it likes. Wave 2. Warning
    /// severity (the guard may still be a legitimate UX precondition; it is
    /// never a security boundary).
    AuthGuardClientOnly,

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
    /// `indexing: index` without `access: public`, a title, a description or an app `origin`
    PageIndexRequiresPublic,
    /// `indexing: index` on a `:param` route (concrete URL inputs are not declarable yet)
    PageIndexParamRoute,
    /// `access: authenticated` page in an app that declares no `signIn` route
    PageAuthNoSignIn,
    /// The app's `signIn` route is no page, or is itself `access: authenticated`
    PageSignInUnknownRoute,
    /// A page is declared on the host's own sign-in route (`/login`)
    PageHostReservedRoute,
    /// One locale holds two pages of the same `translationOf` group
    PageTranslationDuplicateLocale,
    /// Page is missing traits array
    PageMissingTraits,
    /// Page traits array is empty
    PageEmptyTraits,
    /// Orbital has no pages defined
    OrbitalMissingPages,
    /// Two orbitals of one program share a name
    OrbitalDuplicateName,
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
    /// An `obj.entities {}` value names no entity the consumer schema
    /// declares.
    OrbitalRefEntityTargetUnknown,
    /// An `obj.pages {}` / `obj.mounts {}` key names no page path the
    /// imported orbital declares.
    OrbitalRefPageUnknownPath,
    /// An `orbital X = Alias.orbitals.Y { extend { … } } }` field name is
    /// already declared on the upstream primary entity (after `fields {}`
    /// renames), or equals a `fields {}` rename TARGET (Stage C).
    OrbitalRefExtendFieldCollision,
    /// An `extend { … }` field no kept trait reads/sets/renders and no
    /// orbital-level config knob value forwards (Stage C, warning — rung 2).
    OrbitalRefExtendFieldUnread,
    /// A `fields { OLD: NEW }` rename on an `orbital X = Alias.orbitals.Y
    /// { … }` reference left a structurally-recognized field-reference
    /// position (a config default's bare/list field-name value) still
    /// naming the pre-rename upstream field — the rename silently failed to
    /// reach it (G3, `docs/Almadar_Compiler_Gaps.md` §82, rung 2: what used
    /// to validate 0/0 with a dead reference now errors instead).
    OrbitalRefFieldRenameUnapplied,
    /// A `mock { … }` key names no field of the imported primary entity.
    OrbitalRefMockUnknownField,
    /// A `retype { … }` field the upstream primary entity does not declare (after
    /// `fields {}` renames).
    OrbitalRefRetypeUnknownField,
    /// A `retype { … }` field also added by `extend {}`, or retyped twice.
    OrbitalRefRetypeFieldConflict,
    /// An `entity [persistent: …]` override on an upstream entity whose persistence
    /// mode does not allow overrides (the factory-runtime rule).
    OrbitalRefEntityPersistenceLocked,
    /// A `traits { T { … } }` key naming no trait of the upstream orbital.
    OrbitalRefTraitsUnknownTrait,
    /// A `traits { T { … } }` key naming a trait `omit {}` / `only {}` dropped.
    OrbitalRefTraitsOmittedTrait,
    /// Static sections are forbidden (use render_ui)
    PageSectionsForbidden,
    /// Invalid trait reference on page
    PageInvalidTraitRef,
    /// Invalid view type
    PageInvalidViewType,
    /// A declared inline page is not the target of any `navigate` effect,
    /// any literal `href` value, and is not the root page or a
    /// parameterized page — nothing in the schema can ever route a viewer
    /// to it. Ported from the JS wiring lint's `page-absent-from-nav`,
    /// generalised: no navItems-array opt-in gate, and scalar `href` values
    /// count (not only object-array entries).
    PageUnreachable,
    /// A page's content owner (`page_content_owner`'s `channel`/`ambiguous`)
    /// leaves a SECOND page-declared, inline, content-main-writing trait
    /// unclaimed: not an owner itself, not embedded by any client-bound
    /// trait, and not kin to the owner through either the config-channel or
    /// embed-containment relation (`reduce_to_owners({owner, this}).len() ==
    /// 1` would mean related, not a rival). Two content bodies stacking into
    /// one page's `main`. Ported from the JS wiring lint's
    /// `unclaimed-main-writer` (warning there; error here per the owner's Q3
    /// ruling).
    PageSecondContentBody,
    /// A page whose `main` a shell owns through a config channel
    /// (`contentTrait` …) also mounts a trait that paints a control into
    /// `main` which neither the owner nor anything it embeds references: the
    /// control stacks outside the shell's layout instead of inside it.
    PageControlOutsideShell,
    /// A composed trait (`sourceBehavior` set) has a state no event the
    /// program can produce leads to: every transition into it waits on an
    /// event nothing routes in, no control or effect fires, and the host is
    /// not declared to send (`external`). An atom's own inputs are its offer;
    /// a composition that never wires one leaves that part of it dead.
    ComposedStateUnreachable,
    /// A composed trait (`sourceBehavior` set) has a live transition, from a
    /// reachable state, on an event the program cannot produce under its
    /// resolved config: no listens route, control or effect fires it, it is
    /// not a lifecycle event and not declared `external` (G-CROSS-144).
    ComposedTransitionUnreachable,
    /// A composed trait's rendered control fires an event carrying a value
    /// that no other trait hears and whose own handling reads none of it: the
    /// value the user entered is discarded (a form submission nothing keeps).
    ControlValueDiscarded,
    /// A page whose content owner is ambiguous: two or more page-declared
    /// `main` writers, none composing the others, no shell channel naming
    /// one. Renders stacked alone, but any shell that designates one
    /// `contentTrait` turns the rest into second bodies (G-ORB-128).
    PageAmbiguousContentOwner,

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
    /// A pattern that declares `@accessibleName` props has none of them, no
    /// `aria-label`/`aria-labelledby`, and is not `aria-hidden`
    A11yUnnamed,
    /// Nested field path references invalid relation or field
    RenderUIInvalidFieldPath,
    /// A `form`-contract pattern offers an input for an `@intrinsic` entity
    /// field — intrinsic fields are never persisted, so the input is discarded.
    RenderUIFormFieldIntrinsic,
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
    /// call-service declares `emit.cancelled` but no `key` — nothing can
    /// cancel an unkeyed call
    CallServiceCancelledWithoutKey,
    /// call-service `key` is a non-string literal (number/bool/object/array)
    CallServiceKeyType,
    /// the app runs `cancel-call` but no `call-service` anywhere carries a `key`
    CancelCallNoKeyedCall,
    /// a keyed call-service whose registry responseShape already has a `key` field
    CallServiceResultHasKey,
    /// an `http` origin, `tokenUrl` origin, or `url` knob default is not in `egress`
    HttpEgressUndeclared,
    /// an `egress` entry is not `scheme://host[:port]`
    HttpEgressEntryInvalid,
    /// an inbound handler (`listens { EVENT -> inbound "name" … }`) is reachable in a program whose app does not declare `name` in `inbound`
    InboundUndeclared,
    /// an `inbound` entry is not one lower-case hook name (letters, digits, `-`), or is listed twice
    InboundEntryInvalid,
    /// two traits handle the same inbound hook name
    InboundDuplicate,
    /// an inbound handler answers with an event its trait does not declare in `emits`
    InboundAnswerUndeclared,
    /// an inbound `verifies` header is not a lower-case header name, or its credential is not a credential name
    InboundVerifyInvalid,
    /// an `os/` call needs something the calling orbital's grants do not cover
    OsGrantMissing,
    /// an `os/` call's target (path, command, host, port, variable, flags) is not a literal
    OsTargetNotStatic,
    /// a declared `os/` grant entry that no `os/` call of the orbitals holding it uses (warning)
    OsGrantUnused,
    /// a declared `os/` grant entry whose shape cannot match anything
    OsGrantEntryInvalid,
    /// a `deployment` unit lists an orbital the program does not declare
    PlacementOrbitalUnknown,
    /// an orbital is listed in more than one `deployment` unit, or twice in one
    PlacementOrbitalDuplicate,
    /// a `deployment` is declared and an orbital is in none of its units
    PlacementOrbitalUnplaced,
    /// a `deployment` unit lists no orbital
    PlacementUnitEmpty,
    /// an `http` origin is neither a string literal nor a `url`-typed `@config` knob
    HttpOriginNotStatic,
    /// an `http` method is not one of GET POST PUT PATCH DELETE
    HttpMethodUnknown,
    /// an `http` request/auth/paginate object carries a key outside its closed set
    HttpOptionUnknown,
    /// `http` `auth.scheme` is not in the closed scheme set
    HttpAuthSchemeUnknown,
    /// An HTTP auth, grant-cache, presign, or webpush configuration violates its contract.
    HttpAuthInvalid,
    /// A token-exchange `rotate` is not a `{ field, credential }` rotation of the body's `refresh_token` slot.
    HttpTokenRotateInvalid,
    /// An HTTP request/auth header name is not a token, or its value is not a single-line string.
    HttpHeaderInvalid,
    /// an `http` with `auth` sits where its trait runs on the client (a `local` trait, a browser-run tick)
    HttpAuthOnClient,
    /// an `http` request has more than one body kind, or a body on GET
    HttpBodyConflict,
    /// `random-bytes`, `credential-status` or `credential-forget` with the wrong number of arguments
    HostEffectArity,
    /// a key other than `emit`, or a non-object, as the options of a host effect
    HostEffectOptionUnknown,
    /// a literal `random-bytes` count that is not an integer from 1 to 1024
    RandomBytesCount,
    /// an OAuth client object with an unknown or missing key, both or neither of `issuer`/`endpoints`, or a bad word
    OAuthClientInvalid,
    /// an `oauth-client` `op` outside the closed set, or an input the op does not take or requires
    OAuthOpUnknown,
    /// `oauth-endpoint` without an `oauth-client` auth, or a target word or method that does not serve the op
    OAuthEndpointMismatch,
    /// an `oauth-client` request that carries anything but `auth`
    OAuthRequestConflict,
    /// a provider word outside `google | github | auth0`
    OAuthProviderUnknown,
    /// an unknown key, or a missing `redirectUri`, in `oauth-authorize` options
    OAuthOptionUnknown,
    /// `http` `paginate` without `max`
    HttpPaginateMaxMissing,
    /// `http` `stream` without `emit.onMessage`
    HttpStreamWithoutOnMessage,
    /// `browser/page-facts` fields: an unknown word, an empty list or a duplicate
    PageFactsFieldInvalid,
    /// an event reads a page fact the `browser/page-facts` call did not request
    PageFactsUnrequestedRead,
    /// a `browser/beacon` request or option key outside its closed set
    BeaconOptionUnknown,
    /// a `browser/beacon` request without `text`
    BeaconTextRequired,
    /// a literal `browser/beacon` URL that is not an absolute http(s) URL
    BeaconUrlNotAbsolute,
    /// a `browser/prompt` or `browser/embed` where the server runs the transition: a `[background]` tick
    OnDeviceEffectOnServer,
    /// a literal `browser/prompt` response schema that is not a JSON object
    OnDeviceSchemaInvalid,
    /// set effect using invalid binding
    EffectSetInvalidBinding,
    /// set effect using @payload (should use @entity)
    EffectSetPayloadNotAllowed,
    /// fetch effect referencing invalid entity
    EffectFetchInvalidEntity,
    /// fetch effect include references non-existent or non-relation field
    EffectFetchInvalidInclude,
    /// `persist batch` ops expression is not array-typed
    PersistBatchOpsNotArray,
    /// `persist batch` op tuple with an unknown action or the wrong arity
    PersistBatchBadOp,
    /// `persist batch` ops name both browser-stored and server-stored entities
    PersistBatchMixedPlacement,
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
    /// `orb validate --final`: a rendered control fires an event no trait handles — not the
    /// rendering trait, an embedder of it, nor a `listens` route (G-ORB-187).
    EmitControlUnhandled,
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
    /// `program/eval` carries no `emit.failure` event: the validator's errors
    /// come back through the failure arm, so without one they are dropped.
    EffectProgramEvalFailureMissing,
    /// Operator declares `requiresEmitSuccess: true` in canonical-operators.json
    /// but the effect call site is missing the trailing options object or its
    /// `emit.success` event key. Applies to fetch / persist / call-service /
    /// ref by default; any future operator can opt in via the same JSON flag.
    EffectMissingRequiredEmit,
    /// A success event only `(fetch X { id: … })` produces declares `data` as a list; a
    /// by-id fetch delivers one row (G-ORB-185).
    EffectFetchByIdPayloadList,
    /// A `fetch` option outside its closed enum (`deliver: rows | ids`, `total: exact | none`).
    EffectFetchInvalidOption,
    /// `deliver: ids` makes the success event's `data` a row-set reference; any use other than
    /// passing it whole as a `render-ui` prop value reads rows that were never delivered.
    FetchDeliverIdsRowsRead,
    /// `deliver: ids` with `include`: included relations are row content.
    FetchDeliverIdsEntityRefs,
    /// `deliver: ids` with no `emit.success`: the reference is only ever passed whole to a `render-ui`
    /// prop, and nothing can reference a result that no event carries.
    FetchDeliverIdsNoSuccess,
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
    /// One transition's data effects touch both browser-stored and server-stored entities (ORB_X_MIXED_RESIDENCE)
    CrossMixedResidence,
    /// Event listener without matching emitter
    CrossOrbitalOrphanListener,
    /// Event emitted but never listened to
    CrossOrbitalUnusedEmission,
    /// A trait emits an event (an `(emit …)` effect or an operator emit-config
    /// result) that a SIBLING trait in the same orbital has an arm for, but no
    /// declared `listens` route delivers it from that emitter. Events reach
    /// another trait only through declared routes, so the intent terminates in
    /// nothing (std-pong's PADDLE_MOVE, 2026-09-24).
    EmitUnroutedToHandler,
    /// A trait writing a client-held `[runtime]` frame from client ticks (or
    /// bound to a `[shared]` one another trait ticks) is not `local`: its
    /// dispatches post to a stateful host whose stale copy folds back.
    ClientTickNotLocal,
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
    /// A `listens { S.E -> TRIGGER }` route whose source trait S DOES
    /// declare event E in its `emits[]` contract, but nothing in the schema
    /// currently produces E live — no effect emit, no rendered `action:`/
    /// event prop, no structured `itemActions` entry, and no trait embedded
    /// (directly or transitively) inside S produces it either. The contract
    /// permits it; a config override (an itemActions/browseItemActions
    /// narrowed at this call site, say) removed the only producer, so the
    /// route is a live wire with nothing behind it — fires whatever the
    /// emit's declared `scope` is (`external` included: a cross-orbital
    /// broadcast still needs a live broadcaster). Ported from the JS wiring
    /// lint's `listener-affordance-removed-by-config`.
    CrossOrbitalListenSourceNeverProduces,
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
    /// A `fetch`'s `filter` compares a LITERAL `(object/get @entity <field>)`
    /// against a value, but no `persist create`/`update` anywhere in the
    /// schema (own trait, sibling trait, or a `persistent:`-collection-mate
    /// entity) ever supplies that field — the filter can never match a real
    /// row. Cross-orbital because the persistor routinely lives in a sibling
    /// orbital (the modal + persistor idiom). `[identity]`-roster entities
    /// and `@intrinsic` fields are exempt; a declared default only exempts
    /// the field when it folds, under the trait's resolved config, to the
    /// SAME literal the filter's comparand folds to — an undecidable
    /// (payload/entity/user-bound) comparand stays silent rather than
    /// guessing. Ported from `filter-field-never-written`. Error severity.
    FilterFieldNeverWritten,
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
    /// A `listens { Src.EVENT -> TRIGGER }` route's TRIGGER declares a
    /// required leaf payload field the route never produces: no `with { }`
    /// mapping supplies it, and (pass-through, no mapping) the source
    /// trait's declared `emits { EVENT { ... } }` contract never declares it
    /// at the same dotted path either. The consumer's transition reads
    /// `?payload.<field>` as `undefined` at runtime.
    ListenRouteStarvesRequiredField,
    /// A host trait's `listens { Child.EV -> EV }` route where `Child` is a trait the host
    /// embeds (directly or transitively), `EV` is fired by a control inside `Child`'s subtree,
    /// and the host already has a transition on `EV`. The control's emit bubbles to every scope
    /// on the embed chain, so the host already hears it directly; the route is a second delivery
    /// of the same physical event.
    ListenRedundantEmbedRoute,

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
    /// `@event`/`@prevEvents`/`@prevStates`/`@fromState`/`@toState` in a tick, which has no delivery
    BindingDispatchRootInTick,
    /// `@fromState`/`@toState`/`@prevStates`/`@prevEvents` written with a path
    BindingRootNoPath,
    /// `@event.<path>` that does not follow the `{ event, payload, source }` record
    BindingEventInvalidPath,
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
    /// A value handed to an operator's type variable has no determined type — an unresolved expression, an
    /// expression value, json, a list whose items share no type — so what the variable stands for, and the type of
    /// the call's result, is unknown. There are no generic types: give the value a real type (a declared
    /// payload or entity shape, a typed `let`, a named union alias for a list of differently-shaped items).
    OperatorArgUndetermined,
    /// An ordering comparison (`<`, `<=`, `>`, `>=`) over an operand that can be missing or null — an optional
    /// entity field with no default, an optional payload field — that the program neither guards with a presence
    /// check (`(and (validate/required x) (< x 3))`, an `if`/`when` on presence) nor declares a default for.
    /// Both evaluators read a missing value as below any number; every push-down store fails the comparison, so
    /// the meaning is the program's to state.
    OrderingOptionalOperand,
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
    /// An event/emit `payloadSchema` field name is not a valid TypeScript
    /// identifier (contains a dot, space, or leading digit). Codegen turns
    /// each field into a struct member, so `row.category` would emit broken
    /// TS. Validate-layer defense-in-depth for the lowering pass that
    /// flattens nested payload field names. Error severity.
    PayloadFieldNameInvalid,
    /// An array field (entity or payload-schema) declares no element type.
    UntypedArrayField,
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
    /// Two traits of one program carry the same trait id; listens match sources by id, so each hears the other's emits.
    CrossDuplicateTraitId,

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
    /// A transition writes `@entity.F` (or its whole row) and renders an
    /// embedded trait (`@trait.X`, incl. a hoisted inline `<A.traits.T/>`)
    /// whose call-site value reads that field of the same PERSISTENT entity.
    /// The embed resolves `@entity` against its own frame at its own render
    /// (its mount), so it shows the value from before this transition. Read
    /// the value the transition used instead (a call-site capture, `?row.F`),
    /// or route the host event to the embed's INIT.
    BindingEmbedReadsArmWrite,
    /// `persist create E { ... }` with an explicit data literal whose key set
    /// lacks a field that `E.@create` compares (e.g. `@create sender ==
    /// @user.id` and the literal has no `sender`). The access policy will
    /// never be satisfied by a row this transition itself creates — every
    /// future `@update`/`@delete`/`@read` gated on that field locks the
    /// creator out of their own row. A bare `persist create @entity` (whole
    /// frame) is not a hit. Wave 2. Error severity.
    PersistCreateMissingPolicyField,
    /// `persist create E { id: "literal", … }`: the row names a string-literal id, so the
    /// second run creates a row that already exists and fails with `duplicate-id`. A seed
    /// or singleton re-save is an authored `persist upsert` (create-or-replace by id).
    /// Error severity.
    PersistCreateLiteralId,
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
    /// A render-ui payload embeds `@config.<knob>` (as a child or inside a
    /// `children` entry) where `<knob>` is a `trait`-typed config field whose
    /// resolved value is `none` (unbound), and the render-ui is not guarded
    /// by an `(if @config.<knob> ...)` wrapping it in the same effect list.
    /// An unbound trait knob's embed renders nothing — the closed-circuit
    /// rule that no wire may terminate in nothing (owner ruling 2026-09-13:
    /// a `trait`-typed config knob may be `none`).
    RenderUnboundTraitKnobEmbedded,
    /// A render-ui payload embeds an `orbital`-typed field: an orbital value
    /// is data (an import body) and never renders — only trait values embed.
    RenderOrbitalValueEmbedded,
    /// A bridged embed delivers an event nothing in the program sends.
    EmbedBridgeListensUnsent,
    /// A bridged embed re-emits an event nothing in the program handles.
    EmbedBridgeEmitsUnheard,

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
    /// Post-inline: an entity field carrying `mergedFrom` (added by the
    /// rebind-merge auto-merge, `merge_imported_entity_fields`) that no
    /// trait of the consuming orbital reads, writes, fetch-includes, or
    /// persists — R-ENTITY-FIELD-UNION-ACROSS-SAME-NAMED-ENTITY. The merge
    /// already narrows to referenced fields at compose time; this catches
    /// drift when a later edit removes the trait/effect that used to
    /// justify the merge. Error severity.
    EntityMergedFieldUnreferenced,
    /// A trait runs a non-`Cron` tick (frame/duration interval) against a
    /// `Persistent` linked entity. Corpus-facing backstop mirroring the lolo
    /// L1 tick guards. Error severity.
    TickRequiresRuntime,
    /// A non-`Cron` tick effect calls a server-async operator (the canonical
    /// registry's `std-agent` category — `agent/generate`, `agent/invoke`,
    /// etc.). Repeated per-tick LLM/agent calls can pile up faster than they
    /// resolve. Error severity.
    TickAsyncEffect,
    /// An explicit `persist create`/`persist update` data LITERAL (an
    /// `OirExpr::Object`, never a bare whole-frame `@entity`) names an
    /// `@intrinsic` field of the target entity. Intrinsic fields are
    /// trait-owned view-state/machinery (`OirEntity::frame_fields`'s ONLY
    /// allow-list) — never a persisted column — so writing one into the
    /// database is a category error the author almost certainly meant as a
    /// domain field. Wave 2. Error severity.
    EntityIntrinsicFieldPersisted,
    /// An `@intrinsic` field is used as an AUTHORIZATION input: it appears
    /// in an access policy (`@create`/`@read`/`@update`/`@delete`), or in a
    /// transition guard that also reads `@user.*`/role/identity AND the
    /// transition performs `persist`/`call-service`. Intrinsic fields are
    /// trait-computed view-state the runtime never seeds or persists — using
    /// one to gate a mutation means the gate is trivially bypassable (the
    /// caller controls the "intrinsic" value directly). A plain precondition
    /// guard with no identity comparison is not a hit. Wave 2. Error
    /// severity.
    GuardOnFrameFieldAuthorizes,
    /// `(set @entity.<f> ...)` writes a non-`@intrinsic` column of a
    /// `[persistent]` entity in a transition with no row binding (no
    /// `id`/`?row`-bearing event payload, no `entityId` in scope) AND the
    /// owning trait never runs `persist create`/`persist update` on that
    /// entity anywhere — so the write can never land: it is not framed to
    /// an identified row, and nothing ever persists it. A draft form that
    /// sets columns across several transitions before a final `persist
    /// create` is legitimate (the write eventually lands); this only fires
    /// when the trait has no landing transition at all. Wave 2. Error
    /// severity.
    SetColumnWithoutRow,

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
    /// A rendered affordance's event-bearing prop (`action`, `itemActions[].event`,
    /// `submitEvent`, `onChange`, …) targets a lifecycle event
    /// (`orbital_core::events::BUS_EXCLUDED_EVENTS`). The bus never
    /// re-delivers `INIT`/`LOAD`/`$MOUNT`, so the control is dead on click.
    RenderActionLifecycleEvent,
    /// A transition's effects (`fetch`/`persist`/`call-service`) declare an
    /// `emit.failure` event, but no arm at the destination state (`from ===
    /// to || from === '*'`) handles it — the async result contract's failure
    /// half is a live wire terminating in nothing. Ported from the JS wiring
    /// lint's `failure-arm-missing` (client-bound traits only). Error severity.
    CircuitFailureArmMissing,
    /// The failure-handling arm found for [`CircuitFailureArmMissing`]'s
    /// contract exists but paints nothing (no `render-ui <slot> <non-null>`
    /// anywhere in its effects, including through `if`/`do`) — the failure is
    /// caught but never shown to the user. Ported from `failure-arm-renders-
    /// nothing`. Error severity.
    CircuitFailureArmRendersNothing,
    /// A transition renders a non-self-overlaying pattern into an overlay
    /// slot (`modal`/`drawer`/`overlay`), but the destination state has no
    /// `CLOSE`/`CANCEL` arm — the generic portal shell's own dismiss control
    /// (X / Escape / overlay-click) fires an event nothing handles. Ported
    /// from `modal-shell-close-deaf`. Error severity.
    CircuitModalShellCloseDeaf,
    /// A transition renders a SELF-overlaying pattern (`slideOver: true`
    /// anywhere in the payload tree) into an overlay slot — the generic
    /// portal shell wraps it too, producing two stacked overlay chromes.
    /// Ported from `self-overlay-inside-modal-shell`. Error severity.
    CircuitSelfOverlayInShell,
    /// A transition's own effects list contains a top-level `(emit <event>)`
    /// naming a lifecycle event (`orbital_core::events::BUS_EXCLUDED_EVENTS`)
    /// — the bus never re-delivers it, so this half of the closed circuit is
    /// written but can never run. Sibling production site to
    /// [`RenderActionLifecycleEvent`](ErrorCode::RenderActionLifecycleEvent)
    /// (an explicit effect instead of a rendered affordance prop). Ported
    /// from the JS wiring lint's `dead-lifecycle-emit`. Error severity.
    CircuitLifecycleEmitUndelivered,
    /// A transition arm re-emits its OWN trigger event (`X --X--> Y` whose
    /// effects contain `(emit X ...)`), the `to` state has another arm on X,
    /// and the re-emitted payload does not derive from `@payload` — the
    /// runtime path re-enters an arm on the same event with unchanged payload
    /// and loops forever (G-RUNTIME-020). Payloads derived from `@payload`
    /// (sentinel fan-out counters) make progress per hop and are not flagged;
    /// neither is a self-emit whose target state has no same-event arm (a
    /// dead-end broadcast). Error severity.
    CircuitSelfEmitLoop,
    /// An inline trait owns a non-empty `stateMachine.transitions` but is
    /// absent from `client_bound_traits(orbital)` — no page declares it and
    /// no page's `@trait.X` embed closure reaches it, so the client never
    /// mounts a state machine for it and it can never handle an event in the
    /// shipped app. Ported from the JS wiring lint's
    /// `client-unbound-state-machine`. Error severity.
    CircuitClientUnboundStateMachine,
    /// A page trait that initializes on `INIT` has a reachable state with no `INIT` arm: trait
    /// state survives unmount, so revisiting the page from that state does nothing (G-ORB-186).
    CircuitInitUncoveredState,
    /// A transition moves `from -> to` (not a self-transition) with NO
    /// effects at all, out of a `from` state some OTHER arm already painted
    /// into a main slot (`orbital_core::slots::MAIN_SLOTS`) — the machine
    /// leaves that state but the screen keeps its old paint standing, so
    /// every affordance routed here is dead. Client-bound gated (an unbound
    /// trait's arms never run in the shipped client at all). Ported from the
    /// JS wiring lint's `dead-bodiless-action`. Error severity.
    CircuitDeadBodilessAction,
    /// A state-CHANGING transition (`from != to`) whose `fetch`/`persist`
    /// effects declare async-result events (`collect_async_result_events`:
    /// EVERY value of the `emit` map, recursing through `if`/`do`) that the
    /// destination state's own arms (`from === to-state || from === "*"`)
    /// don't handle — the requester departs before it can hear its own
    /// operation's result, so the cascade lands on a machine deaf to it and
    /// wedges. NOT client-bound gated (the JS predicate has none here,
    /// unlike its `failure-arm-missing` sibling). Ported from the JS wiring
    /// lint's `async-result-deaf-target` (warning there; error here per the
    /// 0/0 ruling). Error severity.
    CircuitAsyncResultDeafTarget,
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
    /// An entity declares ≥1 owner column (a declared relation to the
    /// `[identity]` entity, via `owner_fields_from_schema` — never by field
    /// name) but no `read_policy` and no stated `access_waivers.read`
    /// reason. An undeclared policy is ALLOW-ALL, not deny-all, so every
    /// viewer reads every row. L3 port of JS `unscoped-owned-entity`.
    EntityOwnerColumnUnscoped,
    /// A trait's `config.groupByField` + `config.columns[].key` bucket rows
    /// of the linked entity by strict key equality, but the field's declared
    /// enum allows a value no column key names — those rows match no column
    /// and silently disappear. L3 port of JS `groupby-enum-column-gap`.
    RenderGroupbyEnumColumnGap,
    /// A trait embedded via `@trait.X` by ≥2 referrers that itself declares
    /// an `@config.<key>` forward. One declaration serves one embedder (its
    /// forwards chain to the first referrer only), so the second embedder's
    /// call-site config never takes effect. L3 port of JS
    /// `embedded-sibling-single-referrer`.
    EmbedConfigForwardMultiReferrer,
    /// A trait pins `config.theme` to a non-empty literal that isn't a
    /// binding (`@…`) and doesn't match the schema's app-level `theme`
    /// declaration. `""` and bindings are unpinned — an orbital riding the
    /// ambient default is silent. L3 port of the app-level branch of JS
    /// `app-theme-divergent`; the dominant-vote branch is a name/plurality
    /// heuristic and is retired, not ported.
    ThemePinContradictsApp,
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
    /// L2 twin of `orbital-lolo`'s `LowerError::SharedEntityWithoutPersistenceMode`
    /// (`ORB_S_SHARED_WITHOUT_PERSISTENCE_MODE`): `EntityDefinition.shared &&
    /// persistence.is_none()` on the input schema. Nothing about a `[shared]`
    /// entity may be inferred — it must say `persistent: <collection>` or
    /// `runtime` explicitly. Catches a `.orb` mutated directly (factory
    /// edits, agent rewrites) that bypasses the `.lolo` parse path. Runs
    /// before any persistence defaulting. Wave 2. Error severity.
    SchemaSharedWithoutPersistenceMode,

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
    // Behavior Package Errors (ORB_BEHAVIOR_*)
    // =========================================================================
    /// `<prefix>/<name>` names a prefix no installed behavior package declares.
    BehaviorPackageNotInstalled,
    /// The prefix resolved to an installed package, but it has no behavior by
    /// that name (or `std/behaviors/<name>` matched neither the std source nor
    /// any other installed package).
    BehaviorNotFound,
    /// `std/behaviors/<name>` matched the same behavior name in two or more
    /// non-std installed packages.
    BehaviorAmbiguous,
    /// Two different installed packages declare the same `orb.prefix`.
    BehaviorPrefixConflict,
    /// A behavior name is duplicated within or across installed package sources.
    BehaviorNameDuplicate,
    /// An installed behavior package's registry cannot be read or indexed.
    BehaviorPackageInvalid,

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

include!(concat!(env!("OUT_DIR"), "/error_code_docs.rs"));

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
            ErrorCode::EntityLocalRequiresPersistent => "ORB_E_LOCAL_REQUIRES_PERSISTENT",
            ErrorCode::EntityMockSeedNotLocal => "ORB_E_MOCK_SEED_NOT_LOCAL",
            ErrorCode::EntitySeedSourceConflict => "ORB_E_SEED_SOURCE_CONFLICT",
            ErrorCode::EntityIdentityLocal => "ORB_E_IDENTITY_LOCAL",
            ErrorCode::EntityInstanceInvalid => "ORB_E_INSTANCE_INVALID",
            ErrorCode::EntityLocaleInstancesNotLocal => "ORB_E_LOCALE_INSTANCES_NOT_LOCAL",
            ErrorCode::EntityLocaleInstancesUnknownLocale => {
                "ORB_E_LOCALE_INSTANCES_UNKNOWN_LOCALE"
            }
            ErrorCode::EntityLocaleInstancesIncomplete => "ORB_E_LOCALE_INSTANCES_INCOMPLETE",
            ErrorCode::EntityInvalidRelation => "ORB_E_INVALID_RELATION",
            ErrorCode::EntityEmptyEnumValues => "ORB_E_EMPTY_ENUM_VALUES",
            ErrorCode::EntitySemanticValueInvalid => "ORB_E_SEMANTIC_VALUE_INVALID",
            ErrorCode::EntityMockNotInEnum => "ORB_E_MOCK_NOT_IN_ENUM",
            ErrorCode::LiteralUnionDefaultMismatch => "ORB_E_LITERAL_UNION_DEFAULT_MISMATCH",
            ErrorCode::EntityDefaultTypeMismatch => "ORB_E_DEFAULT_TYPE_MISMATCH",

            // Trait
            ErrorCode::TraitUndefined => "ORB_T_UNDEFINED_TRAIT",
            ErrorCode::TraitInvalidFormat => "ORB_T_INVALID_FORMAT",
            ErrorCode::TraitInvalidCategory => "ORB_T_INVALID_CATEGORY",
            ErrorCode::TraitNoInitialState => "ORB_T_NO_INITIAL_STATE",
            ErrorCode::TraitMultipleInitialStates => "ORB_T_MULTIPLE_INITIAL_STATES",
            ErrorCode::TraitDuplicateState => "ORB_T_DUPLICATE_STATE",
            ErrorCode::TraitEmptyEventName => "ORB_T_EMPTY_EVENT_NAME",
            ErrorCode::TraitInvalidTransition => "ORB_T_INVALID_TRANSITION",
            ErrorCode::TraitDuplicateTransition => "ORB_T_DUPLICATE_TRANSITION",
            ErrorCode::TraitMissingInitTransition => "ORB_T_MISSING_INIT_TRANSITION",
            ErrorCode::TraitStateUnreachable => "ORB_T_STATE_UNREACHABLE",
            ErrorCode::TraitInitMissingFetch => "ORB_T_INIT_MISSING_FETCH",
            ErrorCode::TraitMissingRenderUI => "ORB_T_MISSING_RENDER_UI",
            ErrorCode::TraitDeprecatedUI => "ORB_T_DEPRECATED_UI",
            ErrorCode::TraitDuplicateName => "ORB_T_DUPLICATE_NAME",
            ErrorCode::TraitUndefinedEvent => "ORB_T_UNDEFINED_EVENT",
            ErrorCode::TraitBehaviorRefUnresolved => "ORB_T_BEHAVIOR_REF_UNRESOLVED",
            ErrorCode::TraitBehaviorRefNotLiteral => "ORB_T_BEHAVIOR_REF_NOT_LITERAL",
            ErrorCode::TraitUnquoteOutsideQuasiquote => "ORB_T_UNQUOTE_OUTSIDE_QUASIQUOTE",
            ErrorCode::TraitInvalidSlotDefault => "ORB_T_INVALID_SLOT_DEFAULT",
            ErrorCode::TraitInvalidPatternDefault => "ORB_T_INVALID_PATTERN_DEFAULT",
            ErrorCode::TraitSecretDefaultNotString => "ORB_T_SECRET_DEFAULT_NOT_STRING",
            ErrorCode::TraitInvalidEventDefault => "ORB_T_INVALID_EVENT_DEFAULT",
            ErrorCode::TraitEventRefUnresolved => "ORB_T_EVENT_REF_UNRESOLVED",
            ErrorCode::TraitEventRefNotExternalInput => "ORB_T_EVENT_REF_NOT_EXTERNAL_INPUT",
            ErrorCode::TraitExternalInputNotResting => "ORB_T_EXTERNAL_INPUT_NOT_RESTING",
            ErrorCode::TraitConfigShapeMismatch => "ORB_T_CONFIG_SHAPE_MISMATCH",
            ErrorCode::TraitConfigObjectArrayForbidden => "ORB_T_CONFIG_OBJECT_ARRAY_FORBIDDEN",
            ErrorCode::GenericTypeDeprecated => "ORB_T_GENERIC_TYPE_DEPRECATED",
            ErrorCode::NodeFieldUncheckedContent => "ORB_RUI_NODE_FIELD_UNCHECKED_CONTENT",
            ErrorCode::RenderableContentInGenericField => "ORB_RUI_RENDERABLE_IN_GENERIC_FIELD",
            ErrorCode::TraitConfigTierInvalid => "ORB_T_CONFIG_TIER_INVALID",
            ErrorCode::TraitEntityNotRebindable => "ORB_T_ENTITY_NOT_REBINDABLE",
            ErrorCode::RebindMissingEntityFields => "ORB_T_REBIND_MISSING_FIELDS",
            ErrorCode::RebindTargetUnresolved => "ORB_T_REBIND_TARGET_UNRESOLVED",
            ErrorCode::UnresolvedTypeParam => "ORB_T_UNRESOLVED_TYPE_PARAM",
            ErrorCode::EventRenameUnknownKey => "ORB_T_EVENT_RENAME_UNKNOWN_KEY",
            ErrorCode::TraitConfigUnknownCallSiteKey => "ORB_T_CONFIG_UNKNOWN_CALL_SITE_KEY",
            ErrorCode::TraitConfigMissingRequiredKey => "ORB_T_CONFIG_MISSING_REQUIRED_KEY",
            ErrorCode::TraitLocalTouchesServer => "ORB_T_LOCAL_TRAIT_TOUCHES_SERVER",
            ErrorCode::TraitClientTickServerEffect => "ORB_T_CLIENT_TICK_SERVER_EFFECT",
            ErrorCode::TraitLocalOnNonSharedPersistent => "ORB_T_LOCAL_ON_NON_SHARED_PERSISTENT",
            ErrorCode::AuthGuardClientOnly => "ORB_AUTH_GUARD_CLIENT_ONLY",

            // Page
            ErrorCode::PageMissingName => "ORB_P_MISSING_NAME",
            ErrorCode::PageMissingPath => "ORB_P_MISSING_PATH",
            ErrorCode::PageInvalidPath => "ORB_P_INVALID_PATH",
            ErrorCode::PageDuplicatePath => "ORB_P_DUPLICATE_PATH",
            ErrorCode::PageIndexRequiresPublic => "ORB_P_INDEX_REQUIRES_PUBLIC",
            ErrorCode::PageIndexParamRoute => "ORB_P_INDEX_PARAM_ROUTE",
            ErrorCode::PageAuthNoSignIn => "ORB_P_AUTH_NO_SIGN_IN",
            ErrorCode::PageSignInUnknownRoute => "ORB_P_SIGN_IN_UNKNOWN_ROUTE",
            ErrorCode::PageHostReservedRoute => "ORB_P_HOST_RESERVED_ROUTE",
            ErrorCode::PageTranslationDuplicateLocale => "ORB_P_TRANSLATION_DUPLICATE_LOCALE",
            ErrorCode::PageMissingTraits => "ORB_P_MISSING_TRAITS",
            ErrorCode::PageEmptyTraits => "ORB_P_EMPTY_TRAITS",
            ErrorCode::OrbitalMissingPages => "ORB_O_MISSING_PAGES",
            ErrorCode::OrbitalDuplicateName => "ORB_O_DUPLICATE_NAME",
            ErrorCode::OrbitalRefConfigUnknownKey => "ORB_O_CONFIG_UNKNOWN_KEY",
            ErrorCode::OrbitalRefOmitEmbeddedTrait => "ORB_O_OMIT_EMBEDDED_TRAIT",
            ErrorCode::OrbitalConfigDeadKnob => "ORB_O_CONFIG_DEAD_KNOB",
            ErrorCode::OrbitalRefRoleTargetNotMember => "ORB_O_ROLE_TARGET_NOT_MEMBER",
            ErrorCode::OrbitalRefRoleUnknownUpstream => "ORB_O_ROLE_UNKNOWN_UPSTREAM",
            ErrorCode::OrbitalRefEntityUnmapped => "ORB_O_ENTITY_UNMAPPED",
            ErrorCode::OrbitalRefEntityUnknownKey => "ORB_O_ENTITY_UNKNOWN_KEY",
            ErrorCode::OrbitalRefEntityTargetUnknown => "ORB_O_ENTITY_TARGET_UNKNOWN",
            ErrorCode::OrbitalRefPageUnknownPath => "ORB_O_PAGE_UNKNOWN_PATH",
            ErrorCode::OrbitalRefExtendFieldCollision => "ORB_O_EXTEND_FIELD_COLLISION",
            ErrorCode::OrbitalRefExtendFieldUnread => "ORB_O_EXTEND_FIELD_UNREAD",
            ErrorCode::OrbitalRefFieldRenameUnapplied => "ORB_O_FIELD_RENAME_UNAPPLIED",
            ErrorCode::OrbitalRefMockUnknownField => "ORB_O_MOCK_UNKNOWN_FIELD",
            ErrorCode::OrbitalRefRetypeUnknownField => "ORB_O_RETYPE_UNKNOWN_FIELD",
            ErrorCode::OrbitalRefRetypeFieldConflict => "ORB_O_RETYPE_FIELD_CONFLICT",
            ErrorCode::OrbitalRefEntityPersistenceLocked => "ORB_O_ENTITY_PERSISTENCE_LOCKED",
            ErrorCode::OrbitalRefTraitsUnknownTrait => "ORB_O_TRAITS_UNKNOWN_TRAIT",
            ErrorCode::OrbitalRefTraitsOmittedTrait => "ORB_O_TRAITS_OMITTED_TRAIT",
            ErrorCode::PageSectionsForbidden => "ORB_P_SECTIONS_FORBIDDEN",
            ErrorCode::PageInvalidTraitRef => "ORB_P_INVALID_TRAIT_REF",
            ErrorCode::PageInvalidViewType => "ORB_P_INVALID_VIEW_TYPE",
            ErrorCode::PageUnreachable => "ORB_PAGE_UNREACHABLE",
            ErrorCode::PageSecondContentBody => "ORB_PAGE_SECOND_CONTENT_BODY",
            ErrorCode::PageControlOutsideShell => "ORB_PAGE_CONTROL_OUTSIDE_SHELL",
            ErrorCode::ComposedStateUnreachable => "ORB_COMPOSED_STATE_UNREACHABLE",
            ErrorCode::ComposedTransitionUnreachable => "ORB_COMPOSED_TRANSITION_UNREACHABLE",
            ErrorCode::ControlValueDiscarded => "ORB_CONTROL_VALUE_DISCARDED",
            ErrorCode::PageAmbiguousContentOwner => "ORB_PAGE_AMBIGUOUS_CONTENT_OWNER",

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
            ErrorCode::A11yUnnamed => "ORB_A11Y_UNNAMED",
            ErrorCode::RenderUIInvalidFieldPath => "ORB_RUI_INVALID_FIELD_PATH",
            ErrorCode::RenderUIFormFieldIntrinsic => "ORB_RUI_FORM_FIELD_INTRINSIC",
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
            ErrorCode::CallServiceCancelledWithoutKey => "ORB_CALL_SERVICE_CANCELLED_WITHOUT_KEY",
            ErrorCode::CallServiceKeyType => "ORB_CALL_SERVICE_KEY_TYPE",
            ErrorCode::CancelCallNoKeyedCall => "ORB_CANCEL_CALL_NO_KEYED_CALL",
            ErrorCode::CallServiceResultHasKey => "ORB_CALL_SERVICE_RESULT_HAS_KEY",
            ErrorCode::HttpEgressUndeclared => "ORB_HTTP_EGRESS_UNDECLARED",
            ErrorCode::HttpEgressEntryInvalid => "ORB_HTTP_EGRESS_ENTRY_INVALID",
            ErrorCode::InboundUndeclared => "ORB_INBOUND_UNDECLARED",
            ErrorCode::InboundEntryInvalid => "ORB_INBOUND_ENTRY_INVALID",
            ErrorCode::InboundDuplicate => "ORB_INBOUND_DUPLICATE",
            ErrorCode::InboundAnswerUndeclared => "ORB_INBOUND_ANSWER_UNDECLARED",
            ErrorCode::InboundVerifyInvalid => "ORB_INBOUND_VERIFY_INVALID",
            ErrorCode::OsGrantMissing => "ORB_OS_GRANT_MISSING",
            ErrorCode::OsTargetNotStatic => "ORB_OS_TARGET_NOT_STATIC",
            ErrorCode::OsGrantUnused => "ORB_OS_GRANT_UNUSED",
            ErrorCode::OsGrantEntryInvalid => "ORB_OS_GRANT_ENTRY_INVALID",
            ErrorCode::PlacementOrbitalUnknown => "ORB_PLACEMENT_ORBITAL_UNKNOWN",
            ErrorCode::PlacementOrbitalDuplicate => "ORB_PLACEMENT_ORBITAL_DUPLICATE",
            ErrorCode::PlacementOrbitalUnplaced => "ORB_PLACEMENT_ORBITAL_UNPLACED",
            ErrorCode::PlacementUnitEmpty => "ORB_PLACEMENT_UNIT_EMPTY",
            ErrorCode::HttpOriginNotStatic => "ORB_HTTP_ORIGIN_NOT_STATIC",
            ErrorCode::HttpMethodUnknown => "ORB_HTTP_METHOD_UNKNOWN",
            ErrorCode::HttpOptionUnknown => "ORB_HTTP_OPTION_UNKNOWN",
            ErrorCode::HttpAuthSchemeUnknown => "ORB_HTTP_AUTH_SCHEME_UNKNOWN",
            ErrorCode::HttpAuthInvalid => "ORB_HTTP_AUTH_INVALID",
            ErrorCode::HttpTokenRotateInvalid => "ORB_HTTP_TOKEN_ROTATE_INVALID",
            ErrorCode::HttpHeaderInvalid => "ORB_HTTP_HEADER_INVALID",
            ErrorCode::HttpAuthOnClient => "ORB_HTTP_AUTH_ON_CLIENT",
            ErrorCode::HttpBodyConflict => "ORB_HTTP_BODY_CONFLICT",
            ErrorCode::HostEffectArity => "ORB_HOST_EFFECT_ARITY",
            ErrorCode::HostEffectOptionUnknown => "ORB_HOST_EFFECT_OPTION_UNKNOWN",
            ErrorCode::RandomBytesCount => "ORB_RANDOM_BYTES_COUNT",
            ErrorCode::OAuthClientInvalid => "ORB_OAUTH_CLIENT_INVALID",
            ErrorCode::OAuthOpUnknown => "ORB_OAUTH_OP_UNKNOWN",
            ErrorCode::OAuthEndpointMismatch => "ORB_OAUTH_ENDPOINT_MISMATCH",
            ErrorCode::OAuthRequestConflict => "ORB_OAUTH_REQUEST_CONFLICT",
            ErrorCode::OAuthProviderUnknown => "ORB_OAUTH_PROVIDER_UNKNOWN",
            ErrorCode::OAuthOptionUnknown => "ORB_OAUTH_OPTION_UNKNOWN",
            ErrorCode::HttpPaginateMaxMissing => "ORB_HTTP_PAGINATE_MAX_MISSING",
            ErrorCode::HttpStreamWithoutOnMessage => "ORB_HTTP_STREAM_WITHOUT_ON_MESSAGE",
            ErrorCode::PageFactsFieldInvalid => "ORB_PAGE_FACTS_FIELD_INVALID",
            ErrorCode::PageFactsUnrequestedRead => "ORB_PAGE_FACTS_UNREQUESTED_READ",
            ErrorCode::BeaconOptionUnknown => "ORB_BEACON_OPTION_UNKNOWN",
            ErrorCode::BeaconTextRequired => "ORB_BEACON_TEXT_REQUIRED",
            ErrorCode::BeaconUrlNotAbsolute => "ORB_BEACON_URL_NOT_ABSOLUTE",
            ErrorCode::OnDeviceEffectOnServer => "ORB_ON_DEVICE_EFFECT_ON_SERVER",
            ErrorCode::OnDeviceSchemaInvalid => "ORB_ON_DEVICE_SCHEMA_INVALID",
            ErrorCode::EffectSetInvalidBinding => "ORB_EFF_SET_INVALID_BINDING",
            ErrorCode::EffectSetPayloadNotAllowed => "ORB_EFF_SET_PAYLOAD_NOT_ALLOWED",
            ErrorCode::EffectFetchInvalidEntity => "ORB_EFF_FETCH_INVALID_ENTITY",
            ErrorCode::EffectFetchInvalidInclude => "ORB_EFF_FETCH_INVALID_INCLUDE",
            ErrorCode::PersistBatchOpsNotArray => "ORB_PERSIST_BATCH_OPS_NOT_ARRAY",
            ErrorCode::PersistBatchBadOp => "ORB_PERSIST_BATCH_BAD_OP",
            ErrorCode::PersistBatchMixedPlacement => "ORB_PERSIST_BATCH_MIXED_PLACEMENT",
            ErrorCode::EffectSlotConflict => "ORB_EFF_SLOT_CONFLICT",
            ErrorCode::EffectUnknownType => "ORB_EFF_UNKNOWN_TYPE",
            ErrorCode::EffectNotifyRetired => "ORB_EFF_NOTIFY_RETIRED",
            ErrorCode::EffectNavigateMissingPayload => "ORB_EFF_NAVIGATE_MISSING_PAYLOAD",
            ErrorCode::EffectNavigateTargetUnreachable => "ORB_EFF_NAVIGATE_TARGET_UNREACHABLE",
            ErrorCode::EmitDeclaredButUnfired => "ORB_EMIT_DECLARED_BUT_UNFIRED",
            ErrorCode::EmitControlUnhandled => "ORB_EMIT_CONTROL_UNHANDLED",
            ErrorCode::EmitConfigRefInvalid => "ORB_EMIT_CONFIG_REF_INVALID",
            ErrorCode::EffectMissingFetchForEntityPattern => "ORB_EFF_MISSING_FETCH_FOR_ENTITY",
            ErrorCode::EffectEmitKeyInvalid => "ORB_EFF_EMIT_KEY_INVALID",
            ErrorCode::EffectEmitInvalidName => "ORB_EFF_EMIT_INVALID_NAME",
            ErrorCode::EffectEmitUndeclaredEvent => "ORB_EFF_EMIT_UNDECLARED_EVENT",
            ErrorCode::EffectProgramEvalFailureMissing => "ORB_EFF_PROGRAM_EVAL_FAILURE_MISSING",
            ErrorCode::EffectMissingRequiredEmit => "ORB_EFF_MISSING_REQUIRED_EMIT",
            ErrorCode::EffectFetchByIdPayloadList => "ORB_EFF_FETCH_BY_ID_PAYLOAD_LIST",
            ErrorCode::EffectFetchInvalidOption => "ORB_EFF_FETCH_INVALID_OPTION",
            ErrorCode::FetchDeliverIdsRowsRead => "ORB_FETCH_DELIVER_IDS_ROWS_READ",
            ErrorCode::FetchDeliverIdsEntityRefs => "ORB_FETCH_DELIVER_IDS_ENTITY_REFS",
            ErrorCode::FetchDeliverIdsNoSuccess => "ORB_FETCH_DELIVER_IDS_NO_SUCCESS",
            ErrorCode::FetchTotalNoneRead => "ORB_FETCH_TOTAL_NONE_READ",

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
            ErrorCode::CrossMixedResidence => "ORB_X_MIXED_RESIDENCE",
            ErrorCode::CrossOrbitalOrphanListener => "ORB_X_ORPHAN_LISTENER",
            ErrorCode::CrossOrbitalUnusedEmission => "ORB_X_UNUSED_EMISSION",
            ErrorCode::EmitUnroutedToHandler => "ORB_EMIT_UNROUTED_TO_HANDLER",
            ErrorCode::ClientTickNotLocal => "ORB_T_CLIENT_TICK_NOT_LOCAL",
            ErrorCode::CrossOrbitalCircularDependency => "ORB_X_CIRCULAR_DEPENDENCY",
            ErrorCode::CrossOrbitalEventCollision => "ORB_X_EVENT_COLLISION",
            ErrorCode::CrossOrbitalMissingName => "ORB_X_MISSING_ORBITAL_NAME",
            ErrorCode::CrossOrbitalPayloadMismatch => "ORB_X_PAYLOAD_MISMATCH",
            ErrorCode::CrossOrbitalListenSourceUnresolved => "ORB_X_LISTEN_SOURCE_UNRESOLVED",
            ErrorCode::CrossOrbitalListenSourceNeverProduces => {
                "ORB_X_LISTEN_SOURCE_NEVER_PRODUCES"
            }
            ErrorCode::ListenTriggerUnmatched => "ORB_X_LISTEN_TRIGGER_UNMATCHED",
            ErrorCode::FilterFieldNeverWritten => "ORB_X_FILTER_FIELD_NEVER_WRITTEN",
            ErrorCode::InternalEventExposed => "ORB_X_INTERNAL_EVENT_EXPOSED",
            ErrorCode::UnresolvedRelationTarget => "ORB_X_UNRESOLVED_RELATION_TARGET",
            ErrorCode::UnresolvedPatternFieldRef => "ORB_X_UNRESOLVED_PATTERN_FIELD_REF",
            ErrorCode::ListenRouteStarvesRequiredField => "ORB_LISTEN_ROUTE_STARVES_REQUIRED_FIELD",
            ErrorCode::ListenRedundantEmbedRoute => "ORB_LISTEN_REDUNDANT_EMBED_ROUTE",

            // Binding
            ErrorCode::BindingInvalidFormat => "ORB_BINDING_INVALID_FORMAT",
            ErrorCode::BindingUnknownRoot => "ORB_BINDING_UNKNOWN_ROOT",
            ErrorCode::ConfigDanglingRef => "ORB_CONFIG_DANGLING_REF",
            ErrorCode::BindingInvalidPath => "ORB_BINDING_INVALID_PATH",
            ErrorCode::BindingStateNoPath => "ORB_BINDING_STATE_NO_PATH",
            ErrorCode::BindingNowNoPath => "ORB_BINDING_NOW_NO_PATH",
            ErrorCode::BindingPayloadInTick => "ORB_BINDING_PAYLOAD_IN_TICK",
            ErrorCode::BindingDispatchRootInTick => "ORB_BINDING_DISPATCH_ROOT_IN_TICK",
            ErrorCode::BindingRootNoPath => "ORB_BINDING_ROOT_NO_PATH",
            ErrorCode::BindingEventInvalidPath => "ORB_BINDING_EVENT_INVALID_PATH",
            ErrorCode::BindingUnknownEntity => "ORB_BINDING_UNKNOWN_ENTITY",
            ErrorCode::BindingInvalidFieldName => "ORB_BINDING_INVALID_FIELD_NAME",
            ErrorCode::BindingSetTargetMissingPath => "ORB_BINDING_SET_TARGET_MISSING_PATH",
            ErrorCode::BindingPayloadFieldUndeclared => "ORB_BINDING_PAYLOAD_FIELD_UNDECLARED",
            ErrorCode::BindingPayloadTypeIncompatible => "ORB_BINDING_PAYLOAD_TYPE_INCOMPATIBLE",
            ErrorCode::SetValueTypeMismatch => "ORB_X_SET_VALUE_TYPE_MISMATCH",
            ErrorCode::EnumValueNotMember => "ORB_X_ENUM_VALUE_NOT_MEMBER",
            ErrorCode::SetReturnTypeMismatch => "ORB_X_SET_RETURN_TYPE_MISMATCH",
            ErrorCode::ComparisonOperandTypeMismatch => "ORB_X_COMPARISON_OPERAND_TYPE_MISMATCH",
            ErrorCode::OperatorArgTypeMismatch => "ORB_X_OPERATOR_ARG_TYPE_MISMATCH",
            ErrorCode::OperatorArgUndetermined => "ORB_X_OPERATOR_ARG_UNDETERMINED",
            ErrorCode::OrderingOptionalOperand => "ORB_X_ORDERING_OPTIONAL_OPERAND",
            ErrorCode::SetRequiredFieldFromOptionalKnob => {
                "ORB_X_SET_REQUIRED_FIELD_FROM_OPTIONAL_KNOB"
            }
            ErrorCode::EmitPayloadMissingRequired => "ORB_EMIT_PAYLOAD_MISSING_REQUIRED",
            ErrorCode::EmitPayloadExtraField => "ORB_EMIT_PAYLOAD_EXTRA_FIELD",
            ErrorCode::EmitPayloadFieldTypeMismatch => "ORB_EMIT_PAYLOAD_FIELD_TYPE_MISMATCH",
            ErrorCode::PayloadFieldNameInvalid => "ORB_X_PAYLOAD_FIELD_NAME",
            ErrorCode::UntypedArrayField => "ORB_X_UNTYPED_ARRAY",
            ErrorCode::EntityPropContract => "ORB_X_ENTITY_PROP_CONTRACT",
            ErrorCode::EmitSuccessPayloadReturnMismatch => {
                "ORB_EMIT_SUCCESS_PAYLOAD_RETURN_MISMATCH"
            }
            ErrorCode::EmitSuccessPayloadWrapped => "ORB_EMIT_SUCCESS_PAYLOAD_WRAPPED",
            ErrorCode::EmitPayloadNotProduced => "ORB_EMIT_PAYLOAD_NOT_PRODUCED",
            ErrorCode::CrossDuplicateTraitId => "ORB_X_DUPLICATE_TRAIT_ID",
            ErrorCode::CrossEmitterDeclaredPayloadMismatch => {
                "ORB_X_CROSS_EMITTER_DECLARED_PAYLOAD_MISMATCH"
            }
            ErrorCode::BindingEntityFieldNotFound => "ORB_BINDING_ENTITY_FIELD_NOT_FOUND",
            ErrorCode::BindingEntityFieldNeverSet => "ORB_BINDING_ENTITY_FIELD_NEVER_SET",
            ErrorCode::BindingEmbedReadsArmWrite => "ORB_BINDING_EMBED_READS_ARM_WRITE",
            ErrorCode::BindingPersistRowIdNeverSet => "ORB_BINDING_PERSIST_ROW_ID_NEVER_SET",
            ErrorCode::PersistCreateMissingPolicyField => "ORB_PERSIST_CREATE_MISSING_POLICY_FIELD",
            ErrorCode::PersistCreateLiteralId => "ORB_PERSIST_CREATE_LITERAL_ID",
            ErrorCode::BindingTraitMissingName => "ORB_BINDING_TRAIT_MISSING_NAME",
            ErrorCode::BindingTraitInvalidFormat => "ORB_BINDING_TRAIT_INVALID_FORMAT",
            ErrorCode::BindingTraitUnknown => "ORB_BINDING_TRAIT_UNKNOWN",
            ErrorCode::BindingTraitSelfReference => "ORB_BINDING_TRAIT_SELF_REFERENCE",
            ErrorCode::BindingTraitCycle => "ORB_BINDING_TRAIT_CYCLE",
            ErrorCode::BindingTraitInvalidPosition => "ORB_BINDING_TRAIT_INVALID_POSITION",
            ErrorCode::BindingTraitOutsideRenderUi => "ORB_BINDING_TRAIT_OUTSIDE_RENDER_UI",
            ErrorCode::RenderUnboundTraitKnobEmbedded => "ORB_RENDER_UNBOUND_TRAIT_KNOB_EMBEDDED",
            ErrorCode::RenderOrbitalValueEmbedded => "ORB_RENDER_ORBITAL_VALUE_EMBEDDED",
            ErrorCode::EmbedBridgeListensUnsent => "ORB_EMBED_BRIDGE_LISTENS_UNSENT",
            ErrorCode::EmbedBridgeEmitsUnheard => "ORB_EMBED_BRIDGE_EMITS_UNHEARD",

            // Entity Mutation / Tick
            ErrorCode::EntitySplitMutation => "ORB_ENTITY_SPLIT_MUTATION",
            ErrorCode::SharedEntityFieldConflict => "ORB_SHARED_ENTITY_FIELD_CONFLICT",
            ErrorCode::SharedEntityRenderAuthority => "ORB_SHARED_ENTITY_RENDER_AUTHORITY",
            ErrorCode::EntityMergedFieldUnreferenced => "ORB_E_MERGED_FIELD_UNREFERENCED",
            ErrorCode::TickRequiresRuntime => "ORB_TICK_REQUIRES_RUNTIME",
            ErrorCode::TickAsyncEffect => "ORB_TICK_ASYNC_EFFECT",
            ErrorCode::EntityIntrinsicFieldPersisted => "ORB_ENTITY_INTRINSIC_FIELD_PERSISTED",
            ErrorCode::GuardOnFrameFieldAuthorizes => "ORB_GUARD_ON_FRAME_FIELD_AUTHORIZES",
            ErrorCode::SetColumnWithoutRow => "ORB_SET_COLUMN_WITHOUT_ROW",

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
            ErrorCode::RenderActionLifecycleEvent => "ORB_RENDER_ACTION_LIFECYCLE_EVENT",
            ErrorCode::CircuitFailureArmMissing => "ORB_CIRCUIT_FAILURE_ARM_MISSING",
            ErrorCode::CircuitFailureArmRendersNothing => "ORB_CIRCUIT_FAILURE_ARM_RENDERS_NOTHING",
            ErrorCode::CircuitModalShellCloseDeaf => "ORB_CIRCUIT_MODAL_SHELL_CLOSE_DEAF",
            ErrorCode::CircuitSelfOverlayInShell => "ORB_CIRCUIT_SELF_OVERLAY_IN_SHELL",
            ErrorCode::CircuitLifecycleEmitUndelivered => "ORB_CIRCUIT_LIFECYCLE_EMIT_UNDELIVERED",
            ErrorCode::CircuitSelfEmitLoop => "ORB_CIRCUIT_SELF_EMIT_LOOP",
            ErrorCode::CircuitClientUnboundStateMachine => {
                "ORB_CIRCUIT_CLIENT_UNBOUND_STATE_MACHINE"
            }
            ErrorCode::CircuitInitUncoveredState => "ORB_CIRCUIT_INIT_UNCOVERED_STATE",
            ErrorCode::CircuitDeadBodilessAction => "ORB_CIRCUIT_DEAD_BODILESS_ACTION",
            ErrorCode::CircuitAsyncResultDeafTarget => "ORB_CIRCUIT_ASYNC_RESULT_DEAF_TARGET",
            ErrorCode::CircuitViewerStranded => "ORB_CIRCUIT_VIEWER_STRANDED",

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
            ErrorCode::EntityOwnerColumnUnscoped => "ORB_ENTITY_OWNER_COLUMN_UNSCOPED",
            ErrorCode::RenderGroupbyEnumColumnGap => "ORB_RENDER_GROUPBY_ENUM_COLUMN_GAP",
            ErrorCode::EmbedConfigForwardMultiReferrer => "ORB_EMBED_CONFIG_FORWARD_MULTI_REFERRER",
            ErrorCode::ThemePinContradictsApp => "ORB_THEME_PIN_CONTRADICTS_APP",
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
            ErrorCode::SchemaSharedWithoutPersistenceMode => {
                "ORB_S_SHARED_WITHOUT_PERSISTENCE_MODE"
            }

            // Identity
            ErrorCode::OrbIdUnknownRef => "ORB_ID_UNKNOWN_REF",
            ErrorCode::OrbIdNameMismatch => "ORB_ID_NAME_MISMATCH",
            ErrorCode::OrbIdKindMismatch => "ORB_ID_KIND_MISMATCH",
            ErrorCode::OrbIdLedgerOrphan => "ORB_ID_LEDGER_ORPHAN",
            ErrorCode::OrbIdDuplicateEventId => "ORB_ID_DUPLICATE_EVENT_ID",

            // Behavior Package
            ErrorCode::BehaviorPackageNotInstalled => "ORB_BEHAVIOR_PACKAGE_NOT_INSTALLED",
            ErrorCode::BehaviorNotFound => "ORB_BEHAVIOR_NOT_FOUND",
            ErrorCode::BehaviorAmbiguous => "ORB_BEHAVIOR_AMBIGUOUS",
            ErrorCode::BehaviorPrefixConflict => "ORB_BEHAVIOR_PREFIX_CONFLICT",
            ErrorCode::BehaviorNameDuplicate => "ORB_BEHAVIOR_NAME_DUPLICATE",
            ErrorCode::BehaviorPackageInvalid => "ORB_BEHAVIOR_PACKAGE_INVALID",

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
            | ErrorCode::EntityLocalRequiresPersistent
            | ErrorCode::EntityMockSeedNotLocal
            | ErrorCode::EntitySeedSourceConflict
            | ErrorCode::EntityIdentityLocal
            | ErrorCode::EntityInstanceInvalid
            | ErrorCode::EntityLocaleInstancesNotLocal
            | ErrorCode::EntityLocaleInstancesUnknownLocale
            | ErrorCode::EntityLocaleInstancesIncomplete
            | ErrorCode::EntityInvalidRelation
            | ErrorCode::EntityEmptyEnumValues
            | ErrorCode::EntitySemanticValueInvalid
            | ErrorCode::EntityMockNotInEnum
            | ErrorCode::EntityNameShadowsTraitUnion
            | ErrorCode::LiteralUnionDefaultMismatch
            | ErrorCode::EntityDefaultTypeMismatch
            | ErrorCode::UntypedArrayField => "entity",

            ErrorCode::TraitUndefined
            | ErrorCode::TraitInvalidFormat
            | ErrorCode::TraitInvalidCategory
            | ErrorCode::TraitNoInitialState
            | ErrorCode::TraitMultipleInitialStates
            | ErrorCode::TraitDuplicateState
            | ErrorCode::TraitEmptyEventName
            | ErrorCode::TraitInvalidTransition
            | ErrorCode::TraitDuplicateTransition
            | ErrorCode::TraitMissingInitTransition
            | ErrorCode::TraitStateUnreachable
            | ErrorCode::TraitInitMissingFetch
            | ErrorCode::TraitMissingRenderUI
            | ErrorCode::TraitDeprecatedUI
            | ErrorCode::TraitDuplicateName
            | ErrorCode::TraitUndefinedEvent
            | ErrorCode::TraitBehaviorRefUnresolved
            | ErrorCode::TraitBehaviorRefNotLiteral
            | ErrorCode::TraitUnquoteOutsideQuasiquote
            | ErrorCode::TraitInvalidSlotDefault
            | ErrorCode::TraitInvalidPatternDefault
            | ErrorCode::TraitSecretDefaultNotString
            | ErrorCode::TraitInvalidEventDefault
            | ErrorCode::TraitEventRefUnresolved
            | ErrorCode::TraitEventRefNotExternalInput
            | ErrorCode::TraitExternalInputNotResting
            | ErrorCode::TraitConfigShapeMismatch
            | ErrorCode::TraitConfigObjectArrayForbidden
            | ErrorCode::NodeFieldUncheckedContent
            | ErrorCode::RenderableContentInGenericField
            | ErrorCode::TraitConfigTierInvalid
            | ErrorCode::TraitEntityNotRebindable
            | ErrorCode::RebindMissingEntityFields
            | ErrorCode::RebindTargetUnresolved
            | ErrorCode::EventRenameUnknownKey
            | ErrorCode::TraitConfigUnknownCallSiteKey
            | ErrorCode::TraitConfigMissingRequiredKey
            | ErrorCode::UnresolvedTypeParam
            | ErrorCode::TraitLocalTouchesServer
            | ErrorCode::TraitClientTickServerEffect
            | ErrorCode::TraitLocalOnNonSharedPersistent
            // A generic type in a declaration position — a TYPE-system rejection
            // since the burndown reached zero (2026-09-01).
            | ErrorCode::GenericTypeDeprecated => "trait",

            ErrorCode::AuthGuardClientOnly => "dispatch",

            ErrorCode::PageMissingName
            | ErrorCode::PageMissingPath
            | ErrorCode::PageInvalidPath
            | ErrorCode::PageDuplicatePath
            | ErrorCode::PageIndexRequiresPublic
            | ErrorCode::PageIndexParamRoute
            | ErrorCode::PageAuthNoSignIn
            | ErrorCode::PageSignInUnknownRoute
            | ErrorCode::PageHostReservedRoute
            | ErrorCode::PageTranslationDuplicateLocale
            | ErrorCode::PageMissingTraits
            | ErrorCode::PageEmptyTraits
            | ErrorCode::OrbitalMissingPages
            | ErrorCode::OrbitalDuplicateName
            | ErrorCode::OrbitalRefConfigUnknownKey
            | ErrorCode::OrbitalRefOmitEmbeddedTrait
            | ErrorCode::OrbitalConfigDeadKnob
            | ErrorCode::OrbitalRefRoleTargetNotMember
            | ErrorCode::OrbitalRefRoleUnknownUpstream
            | ErrorCode::OrbitalRefEntityUnmapped
            | ErrorCode::OrbitalRefEntityUnknownKey
            | ErrorCode::OrbitalRefEntityTargetUnknown
            | ErrorCode::OrbitalRefPageUnknownPath
            | ErrorCode::OrbitalRefExtendFieldCollision
            | ErrorCode::OrbitalRefExtendFieldUnread
            | ErrorCode::OrbitalRefFieldRenameUnapplied
            | ErrorCode::OrbitalRefMockUnknownField
            | ErrorCode::OrbitalRefRetypeUnknownField
            | ErrorCode::OrbitalRefRetypeFieldConflict
            | ErrorCode::OrbitalRefEntityPersistenceLocked
            | ErrorCode::OrbitalRefTraitsUnknownTrait
            | ErrorCode::OrbitalRefTraitsOmittedTrait
            | ErrorCode::PageSectionsForbidden
            | ErrorCode::PageInvalidTraitRef
            | ErrorCode::PageInvalidViewType
            | ErrorCode::PageUnreachable
            | ErrorCode::PageSecondContentBody
            | ErrorCode::PageControlOutsideShell
            | ErrorCode::PageAmbiguousContentOwner => "page",

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
            | ErrorCode::A11yUnnamed
            | ErrorCode::RenderUIInvalidFieldPath
            | ErrorCode::RenderUIFormFieldIntrinsic
            | ErrorCode::RenderUIPropTypeMismatch
            | ErrorCode::RenderUIBindingTypeMismatch
            | ErrorCode::RenderUIElementShapeMismatch
            | ErrorCode::RenderUIEventBindingPayloadMismatch
            | ErrorCode::EntityPropContract
            | ErrorCode::RenderUIEventLiteralStale
            | ErrorCode::RenderUnboundTraitKnobEmbedded
            | ErrorCode::RenderOrbitalValueEmbedded
            | ErrorCode::EmbedBridgeListensUnsent
            | ErrorCode::EmbedBridgeEmitsUnheard => "render_ui",

            ErrorCode::EffectCallServiceMissingService
            | ErrorCode::EffectCallServiceUnknownService
            | ErrorCode::EffectCallServiceMissingAction
            | ErrorCode::EffectCallServiceMissingHandlers
            | ErrorCode::EffectCallServiceUnknownParam
            | ErrorCode::EffectCallServiceMissingParam
            | ErrorCode::EffectCallServiceParamType
            | ErrorCode::CallServiceCancelledWithoutKey
            | ErrorCode::CallServiceKeyType
            | ErrorCode::CancelCallNoKeyedCall
            | ErrorCode::CallServiceResultHasKey
            | ErrorCode::HttpEgressUndeclared
            | ErrorCode::HttpEgressEntryInvalid
            | ErrorCode::InboundUndeclared
            | ErrorCode::InboundEntryInvalid
            | ErrorCode::InboundDuplicate
            | ErrorCode::InboundAnswerUndeclared
            | ErrorCode::InboundVerifyInvalid
            | ErrorCode::OsGrantMissing
            | ErrorCode::OsTargetNotStatic
            | ErrorCode::OsGrantUnused
            | ErrorCode::OsGrantEntryInvalid
            | ErrorCode::PlacementOrbitalUnknown
            | ErrorCode::PlacementOrbitalDuplicate
            | ErrorCode::PlacementOrbitalUnplaced
            | ErrorCode::PlacementUnitEmpty
            | ErrorCode::HttpOriginNotStatic
            | ErrorCode::HttpMethodUnknown
            | ErrorCode::HttpOptionUnknown
            | ErrorCode::HttpAuthSchemeUnknown
            | ErrorCode::HttpAuthInvalid
            | ErrorCode::HttpTokenRotateInvalid
            | ErrorCode::HttpHeaderInvalid
            | ErrorCode::HttpAuthOnClient
            | ErrorCode::HttpBodyConflict
            | ErrorCode::HostEffectArity
            | ErrorCode::HostEffectOptionUnknown
            | ErrorCode::RandomBytesCount
            | ErrorCode::OAuthClientInvalid
            | ErrorCode::OAuthOpUnknown
            | ErrorCode::OAuthEndpointMismatch
            | ErrorCode::OAuthRequestConflict
            | ErrorCode::OAuthProviderUnknown
            | ErrorCode::OAuthOptionUnknown
            | ErrorCode::HttpPaginateMaxMissing
            | ErrorCode::HttpStreamWithoutOnMessage
            | ErrorCode::PageFactsFieldInvalid
            | ErrorCode::PageFactsUnrequestedRead
            | ErrorCode::BeaconOptionUnknown
            | ErrorCode::BeaconTextRequired
            | ErrorCode::BeaconUrlNotAbsolute
            | ErrorCode::OnDeviceEffectOnServer
            | ErrorCode::OnDeviceSchemaInvalid
            | ErrorCode::EffectNotifyRetired
            | ErrorCode::EffectSetInvalidBinding
            | ErrorCode::EffectSetPayloadNotAllowed
            | ErrorCode::EffectFetchInvalidEntity
            | ErrorCode::EffectFetchInvalidInclude
            | ErrorCode::PersistBatchOpsNotArray
            | ErrorCode::PersistBatchBadOp
            | ErrorCode::PersistBatchMixedPlacement
            | ErrorCode::EffectSlotConflict
            | ErrorCode::EffectUnknownType
            | ErrorCode::EffectNavigateMissingPayload
            | ErrorCode::EffectNavigateTargetUnreachable
            | ErrorCode::EffectMissingFetchForEntityPattern
            | ErrorCode::EffectEmitKeyInvalid
            | ErrorCode::EffectEmitInvalidName
            | ErrorCode::EffectEmitUndeclaredEvent
            | ErrorCode::EffectProgramEvalFailureMissing
            | ErrorCode::EffectMissingRequiredEmit
            | ErrorCode::EffectFetchByIdPayloadList
            | ErrorCode::EffectFetchInvalidOption
            | ErrorCode::FetchDeliverIdsRowsRead
            | ErrorCode::FetchDeliverIdsEntityRefs
            | ErrorCode::FetchDeliverIdsNoSuccess
            | ErrorCode::FetchTotalNoneRead
            | ErrorCode::SetValueTypeMismatch
            | ErrorCode::EnumValueNotMember
            | ErrorCode::SetReturnTypeMismatch
            | ErrorCode::ComparisonOperandTypeMismatch
            | ErrorCode::OperatorArgTypeMismatch
            | ErrorCode::OperatorArgUndetermined
            | ErrorCode::OrderingOptionalOperand
            | ErrorCode::SetRequiredFieldFromOptionalKnob
            | ErrorCode::EmitDeclaredButUnfired
            | ErrorCode::EmitControlUnhandled
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

            ErrorCode::CrossMixedResidence
            | ErrorCode::CrossOrbitalDuplicateEntity
            | ErrorCode::CrossOrbitalOrphanListener
            | ErrorCode::CrossOrbitalUnusedEmission
            | ErrorCode::EmitUnroutedToHandler
            | ErrorCode::ClientTickNotLocal
            | ErrorCode::CrossOrbitalCircularDependency
            | ErrorCode::CrossOrbitalEventCollision
            | ErrorCode::CrossOrbitalMissingName
            | ErrorCode::CrossOrbitalPayloadMismatch
            | ErrorCode::CrossOrbitalListenSourceUnresolved
            | ErrorCode::CrossOrbitalListenSourceNeverProduces
            | ErrorCode::ListenTriggerUnmatched
            | ErrorCode::InternalEventExposed
            | ErrorCode::UnresolvedRelationTarget
            | ErrorCode::UnresolvedPatternFieldRef
            | ErrorCode::ListenRouteStarvesRequiredField
            | ErrorCode::ListenRedundantEmbedRoute
            | ErrorCode::CrossEmitterDeclaredPayloadMismatch
            | ErrorCode::CrossDuplicateTraitId
            | ErrorCode::FilterFieldNeverWritten => "cross",

            ErrorCode::BindingInvalidFormat
            | ErrorCode::BindingUnknownRoot
            | ErrorCode::ConfigDanglingRef
            | ErrorCode::BindingInvalidPath
            | ErrorCode::BindingStateNoPath
            | ErrorCode::BindingNowNoPath
            | ErrorCode::BindingPayloadInTick
            | ErrorCode::BindingDispatchRootInTick
            | ErrorCode::BindingRootNoPath
            | ErrorCode::BindingEventInvalidPath
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
            | ErrorCode::BindingEmbedReadsArmWrite
            | ErrorCode::BindingTraitMissingName
            | ErrorCode::BindingTraitInvalidFormat
            | ErrorCode::BindingTraitUnknown
            | ErrorCode::BindingTraitSelfReference
            | ErrorCode::BindingTraitCycle
            | ErrorCode::BindingTraitInvalidPosition
            | ErrorCode::BindingTraitOutsideRenderUi
            | ErrorCode::PersistCreateMissingPolicyField
            | ErrorCode::PersistCreateLiteralId => "binding",

            ErrorCode::EntitySplitMutation
            | ErrorCode::SharedEntityFieldConflict
            | ErrorCode::SharedEntityRenderAuthority
            | ErrorCode::EntityMergedFieldUnreferenced
            | ErrorCode::TickRequiresRuntime
            | ErrorCode::TickAsyncEffect
            | ErrorCode::EntityIntrinsicFieldPersisted
            | ErrorCode::GuardOnFrameFieldAuthorizes
            | ErrorCode::SetColumnWithoutRow => "entity_mutation",

            ErrorCode::CircuitOrphanEvent
            | ErrorCode::CircuitDuplicateEvent
            | ErrorCode::CircuitDeadEndComponent
            | ErrorCode::CircuitUnreachableState
            | ErrorCode::CircuitDeadEndState
            | ErrorCode::CircuitActionComponentMismatch
            | ErrorCode::CircuitNoOverlayExit
            | ErrorCode::CircuitModalExitIncomplete
            | ErrorCode::CircuitDeadHandler
            | ErrorCode::RenderActionLifecycleEvent
            | ErrorCode::CircuitFailureArmMissing
            | ErrorCode::CircuitFailureArmRendersNothing
            | ErrorCode::CircuitModalShellCloseDeaf
            | ErrorCode::CircuitSelfOverlayInShell
            | ErrorCode::CircuitLifecycleEmitUndelivered
            | ErrorCode::CircuitSelfEmitLoop
            | ErrorCode::CircuitClientUnboundStateMachine
            | ErrorCode::CircuitInitUncoveredState
            | ErrorCode::CircuitDeadBodilessAction
            | ErrorCode::CircuitAsyncResultDeafTarget
            | ErrorCode::CircuitViewerStranded
            | ErrorCode::ComposedStateUnreachable
            | ErrorCode::ComposedTransitionUnreachable
            | ErrorCode::ControlValueDiscarded => "circuit",

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
            | ErrorCode::UsesConfigUnknownKey
            | ErrorCode::SchemaSharedWithoutPersistenceMode => "schema",

            ErrorCode::OrbIdUnknownRef
            | ErrorCode::OrbIdNameMismatch
            | ErrorCode::OrbIdKindMismatch
            | ErrorCode::OrbIdLedgerOrphan
            | ErrorCode::OrbIdDuplicateEventId => "identity",

            ErrorCode::BehaviorPackageNotInstalled
            | ErrorCode::BehaviorNotFound
            | ErrorCode::BehaviorAmbiguous
            | ErrorCode::BehaviorPrefixConflict
            | ErrorCode::BehaviorNameDuplicate
            | ErrorCode::BehaviorPackageInvalid => "behavior_package",

            ErrorCode::InvalidValue
            | ErrorCode::InvalidReference
            | ErrorCode::UnknownTheme
            | ErrorCode::MissingField
            | ErrorCode::DuplicateEntity => "generic",

            ErrorCode::EntityOwnerColumnUnscoped => "entity",
            ErrorCode::RenderGroupbyEnumColumnGap => "render_ui",
            ErrorCode::EmbedConfigForwardMultiReferrer => "cross",
            ErrorCode::ThemePinContradictsApp => "schema",
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
