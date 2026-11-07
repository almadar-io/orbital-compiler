//! Compiler Phases
//!
//! The Orbital compiler is organized into phases that transform the input schema
//! into the final Orbital IR (OIR) suitable for code generation.
//!
//! # Phase Pipeline
//!
//! ```text
//! .orb file → Parse → Validate → Enrich → Inline → Resolve → OIR
//! ```
//!
//! Each phase is responsible for a specific transformation:
//!
//! 1. **Parse**: Read JSON/YAML and deserialize into schema types
//! 2. **Validate**: Check structural validity and type correctness
//! 3. **Enrich**: Add derived information (defaults, cross-references)
//! 4. **Inline**: Resolve `uses` declarations and inline external references
//! 5. **Resolve**: Resolve all references and produce final OIR

// the pipeline, in order: parse → inline → compose → enrich → validate → reflect → resolve → nominalize; lazy splits a `uses lazy` import
pub mod compose;
pub mod enrich;
pub mod inline;
pub mod lazy;
pub mod nominalize;
pub mod parse;
pub mod reflect;
pub mod resolve;
pub mod sexpr_json;
pub mod validate;
pub mod validation;

// program — program-level facts and files derived from the whole schema
#[path = "program/await_render.rs"]
pub mod await_render;
#[path = "program/cross_file_ids.rs"]
pub mod cross_file_ids;
#[path = "program/embed_graph.rs"]
pub(crate) mod embed_graph;
#[path = "program/messages.rs"]
pub mod messages;
#[path = "program/public.rs"]
pub mod public;

pub use compose::{compose_schema, ComposeConfig};
pub use cross_file_ids::{resolve_cross_file_id_edges, CrossFileIdError};
pub use enrich::{enrich_schema, EnrichmentConfig};
pub use inline::{inline_schema, InlineConfig, InlineError};
pub use parse::{parse_file, parse_json, ParseError};
pub use resolve::{resolve_to_oir, ResolveError};
pub use validate::{validate_schema, ValidationError as SchemaValidationError};
pub use validation::sync::ExtraPatterns;
pub use validation::{
    validate_comprehensive_full, validate_comprehensive_without_services, SchemaLanguage,
    ValidationResult as ComprehensiveValidationResult,
};

use crate::composition::ComposeError;
use orbital_core_base::ir::OirModule;
use orbital_core_base::logging::ns;
use orbital_core_base::schema::OrbitalSchema;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Compiler error
#[derive(Debug, Error)]
pub enum CompilerError {
    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("Validation error: {0}")]
    Validation(#[from] SchemaValidationError),

    #[error("Inline error: {0}")]
    Inline(#[from] InlineError),

    #[error("Compose error: {0}")]
    Compose(#[from] ComposeError),

    #[error("Resolution error: {0}")]
    Resolution(#[from] ResolveError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Reflection error: {0}")]
    Reflection(String),

    #[error("Catalog error: {0}")]
    Catalog(String),

    #[error("{path} was resolved by orb {orb} for type contract {resolved}; this orb reads {current} — resolve it again from source")]
    ResolvedContract {
        path: String,
        orb: String,
        resolved: u32,
        current: u32,
    },
}

/// Whether `schema` is already resolved (carries `resolved`): `Ok(true)` when its type contract is the one this
/// orb reads, an error naming both when it is not. The one place a resolved input is judged.
pub fn already_resolved(schema: &OrbitalSchema, source_path: Option<&Path>) -> CompileResult<bool> {
    let Some(mark) = &schema.resolved else {
        return Ok(false);
    };
    check_resolved_mark(mark, source_path.map_or_else(|| schema.name.clone(), |p| p.display().to_string()))?;
    Ok(true)
}

/// A resolved program's stamp judged against the type contract this orb reads; `path` names the program in the error.
pub fn check_resolved_mark(mark: &orbital_core_base::schema::ResolvedMark, path: String) -> CompileResult<()> {
    if mark.type_contract_version != orbital_core_base::schema::TYPE_CONTRACT_VERSION {
        return Err(CompilerError::ResolvedContract {
            path,
            orb: mark.orb.clone(),
            resolved: mark.type_contract_version,
            current: orbital_core_base::schema::TYPE_CONTRACT_VERSION,
        });
    }
    Ok(())
}

/// Compilation result
pub type CompileResult<T> = Result<T, CompilerError>;

/// Compiler options
// `Clone` dropped: `discovery_error` carries `orbital_core::behavior_packages::
// DiscoverError`, which is not `Clone` (its `Io` variant wraps `std::io::Error`)
// — verified zero call sites clone a `CompilerOptions` today.
#[derive(Debug, Default)]
pub struct CompilerOptions {
    /// Skip validation phase
    pub skip_validation: bool,
    /// Run comprehensive validation (includes all semantic checks)
    pub comprehensive_validation: bool,
    /// Enrichment configuration
    pub enrichment: EnrichmentConfig,
    /// Inline configuration
    pub inline: InlineConfig,
    /// Compose configuration
    pub compose: ComposeConfig,
    /// Run the Phase-3D nominalization pass post-Resolve: replace every
    /// anonymous `OirType::Struct`/`Union` with a `TypeRef` registered in
    /// `OirModule::named_types`. Default OFF — every shell's `type_ref`
    /// readiness landed in the same change set, but the owner flips the
    /// default only after reviewing the TS before/after diff.
    pub nominalize: bool,
    /// Set when behavior-package discovery (`orbital_core::behavior_packages::
    /// discover`, run from the process cwd) failed — a `PrefixConflict` or a
    /// malformed installed package. `packages` is then empty, which callers
    /// must not silently treat as "no packages installed": `orb validate` /
    /// `compile` report this before parsing, as validator infrastructure, not
    /// a schema authoring error.
    pub discovery_error: Option<orbital_core_base::behavior_packages::DiscoverError>,
    /// Service name → declared runtimes, from the services registry the caller
    /// loaded; `None` reads the baked registry. Decides which calls run in the
    /// browser (`OirModule::browser_services`).
    pub service_runtimes: Option<std::collections::HashMap<String, Vec<String>>>,
}

impl CompilerOptions {
    /// Discover installed behavior packages node-style from the process
    /// working directory (`orbital_core::behavior_packages::discover`) and
    /// wire them into the loader config both phases share. No environment
    /// variables — the prior dev-mode toggles are retired
    /// (Almadar_Behavior_Packages.md Phase 1) — a workspace-linked
    /// `node_modules` (or `~/.orb` with none above cwd) already puts the
    /// live package sources on the walk.
    pub fn from_env() -> Self {
        // `ORBITAL_NOMINALIZE=1` opts into the Phase-3D nominalization pass —
        // it changes codegen output for every shell, so it stays a dedicated
        // toggle rather than folding into package discovery.
        let nominalize = std::env::var("ORBITAL_NOMINALIZE")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        let mut opts = Self {
            nominalize,
            ..Self::default()
        };

        let cwd = match std::env::current_dir() {
            Ok(d) => d,
            Err(_) => return opts,
        };
        let user_store =
            orbital_core_base::behavior_packages::user_store_dir().unwrap_or_else(|| cwd.clone());
        match orbital_core_base::behavior_packages::discover(&cwd, &user_store) {
            Ok(packages) => opts.inline.loader_config.packages = packages,
            Err(e) => opts.discovery_error = Some(e),
        }
        if let Some(root) = app_root(std::env::var("ORB_APP_ROOT").ok().as_deref(), &cwd) {
            opts.inline
                .loader_config
                .scoped_paths
                .insert(APP_SCOPE.to_string(), root);
        }
        opts
    }
}

/// The import scope of the app being compiled: `uses X from "@app/blocks/<Name>"`
/// resolves against the app's workspace root from any file in it.
pub const APP_SCOPE: &str = "@app";

/// The app's workspace root: `ORB_APP_ROOT` when a caller names it (a server
/// validating a temp copy), else the nearest ancestor of `cwd` holding the
/// workspace marker `.almadar/app-marker.json`.
pub fn app_root(explicit: Option<&str>, cwd: &Path) -> Option<PathBuf> {
    if let Some(root) = explicit.filter(|r| !r.is_empty()) {
        return Some(PathBuf::from(root));
    }
    cwd.ancestors()
        .find(|dir| dir.join(".almadar").join("app-marker.json").is_file())
        .map(Path::to_path_buf)
}

/// Compile a schema file to OIR
pub fn compile_file(path: impl AsRef<Path>, options: &CompilerOptions) -> CompileResult<OirModule> {
    let path = path.as_ref();
    let _span = tracing::info_span!(target: ns::COMPILER, "compile", file = %path.display()).entered();

    // Phase 1: Parse
    let schema = tracing::info_span!(target: ns::COMPILER, "parse").in_scope(|| parse_file(path))?;

    // Continue with common compilation, passing source path for inline phase
    compile_schema_with_path(schema, options, Some(path))
}

/// [`compile_file`] for a schema written somewhere other than its source (a
/// lowered or translated copy): imports resolve against `source_path`.
pub fn compile_file_with_source(
    path: impl AsRef<Path>,
    source_path: &Path,
    options: &CompilerOptions,
) -> CompileResult<OirModule> {
    let schema = parse_file(path.as_ref())?;
    compile_schema_with_path(schema, options, Some(source_path))
}

/// Resolve a parsed schema: run Inline → Compose → Enrich and return the
/// fully-inlined `OrbitalSchema`. This is the pre-OIR "resolved" form — the
/// same JSON shape authors write, but with every import expanded, every
/// trait ref flattened to `TraitReference::Inline`, every `behavior/*`
/// composer collapsed, every `events: { OLD: NEW }` rename applied through
/// transitions AND render-ui props.
///
/// Consumers (the `orbital resolve` CLI, `generate_from_file`'s resolved.orb
/// emission, future `--parity` mode) use this shape as the canonical
/// "bundled" view of the schema. Structurally identical to the input so
/// every downstream TS tool that already understands `OrbitalSchema` can
/// consume it without new parsing code.
///
/// Skips Validate and Resolve-to-OIR on purpose — those happen inside
/// `compile_schema_with_path` which calls this helper first, then finishes.
pub fn resolve_schema_with_path(
    schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    resolve_schema_with_inline(schema, options, &options.inline, source_path)
}

/// [`resolve_schema_with_path`] inlining with `inline` instead of
/// `options.inline` — how a program shares one inline session across itself
/// and its lazy behaviors.
pub(crate) fn resolve_schema_with_inline(
    mut schema: OrbitalSchema,
    options: &CompilerOptions,
    inline: &inline::InlineConfig,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    if let Some(path) = source_path {
        crate::loader::anchor_relative_uses(&mut schema, path);
    }
    if already_resolved(&schema, source_path)? {
        return Ok(schema);
    }
    // Intrinsic identity: a raw/name-only `.orb` reaching the compiler (e.g.
    // `orb emit ts <name-only.orb>`, which parses through the raw core
    // deserializer) is stamped here — the single pre-inline chokepoint — so
    // inline/compose/resolve/codegen never see a ledger-less schema and the
    // name-fallback rewriters are dead. `.lolo`-emitted and factory-stamped
    // schemas already carry a ledger and pass through untouched (zero churn).
    orbital_core_base::stamp::ensure_stamped(&mut schema);

    // Phase 2: Inline (resolve uses declarations — must happen before validation
    //           so that trait/entity/page references are present when checked).
    //           `inline_schema` runs the V4-W4 identity normalization internally
    //           (right after its own `ensure_stamped`), so `orb validate` — which
    //           calls the inline entry directly — is covered by the same pass.
    tracing::info_span!(target: ns::COMPILER, "inline").in_scope(|| inline_schema(&mut schema, inline, source_path))?;

    // Phase 2.5: Compose (depends on inline having resolved aliases)
    tracing::info_span!(target: ns::COMPILER, "compose").in_scope(|| compose_schema(&mut schema, &options.compose))?;

    // Phase 3: Enrich (add defaults like collection names — runs on resolved schema
    //           so inlined entities are enriched too)
    tracing::info_span!(target: ns::COMPILER, "enrich").in_scope(|| enrich_schema(&mut schema, &options.enrichment));

    // The theme presets the program references travel with it, so a runtime
    // with no installed `@almadar/core` (wasm, mobile) resolves its theme.
    orbital_core_base::theme::carry_theme_definitions(
        &mut schema,
        orbital_core_base::theme::ThemePresets::installed(),
    );

    // The declared type of every `json/parse` / `as` shape naming a program alias, and the knob types that name aliases.
    validation::effect::record_declared_shapes(&mut schema).map_err(ResolveError::from)?;

    if let (Some(path), true) = (source_path, schema.locales.as_ref().is_some_and(|l| !l.is_empty())) {
        let messages = messages::merged_catalogs(&schema, path.parent().unwrap_or(Path::new("."))).map_err(CompilerError::Catalog)?;
        schema.messages = Some(messages);
    }

    tracing::info_span!(target: ns::COMPILER, "callbacks").in_scope(|| raw_callbacks_as_lambdas(&mut schema));

    schema.resolved = Some(orbital_core_base::schema::ResolvedMark {
        orb: env!("CARGO_PKG_VERSION").to_string(),
        type_contract_version: orbital_core_base::schema::TYPE_CONTRACT_VERSION,
    });
    Ok(schema)
}

/// A raw callback body is lowered to the lambda its operator binds (core's `raw_callback_params`); the resolved
/// program carries that lambda, so the kernel, which runs the resolved program, binds the element as the compiled
/// path does.
fn raw_callbacks_as_lambdas(schema: &mut OrbitalSchema) {
    use orbital_core_eval::lower::raw_callbacks_as_lambdas as wrap;
    for orbital in &mut schema.orbitals {
        for t in &mut orbital.traits {
            let orbital_core_base::schema::TraitReference::Inline(td) = t else {
                continue;
            };
            let td = std::sync::Arc::make_mut(td);
            td.state_machine.edit(|machine| {
                for transition in &mut machine.transitions {
                    transition.guard.iter_mut().for_each(wrap);
                    transition.effects.iter_mut().flatten().for_each(wrap);
                    transition.ensures.iter_mut().flatten().for_each(wrap);
                }
            });
            for tick in td.ticks.iter_mut().flatten() {
                tick.guard.iter_mut().for_each(wrap);
                tick.effects.iter_mut().for_each(wrap);
            }
            if let Some(listens) = td.listens.as_mut() {
                listens.edit(|listens| {
                    for listen in listens.iter_mut() {
                        listen.guard.iter_mut().for_each(wrap);
                        listen.key.iter_mut().for_each(wrap);
                    }
                });
            }
        }
    }
}

/// The program exactly as the compiled path compiles it: lazy behaviors compiled in
/// as route chunks, then Inline, Compose, Enrich. What a compiled client's own
/// runtime (the browser store for `[persistent, local]` entities) runs.
pub fn resolve_compiled_program_with_path(
    schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    let mut program = resolve_compiled_program_as_written(schema, options, source_path)?;
    reflect::fold_literal_reflection(&mut program, source_path)?;
    Ok(program)
}

/// [`resolve_compiled_program_with_path`] before reflection is answered: what validation judges.
fn resolve_compiled_program_as_written(
    mut schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OrbitalSchema> {
    lazy::compile_lazy_as_routes(&mut schema);
    // Phases 2–3: Inline, Compose, Enrich.
    let mut schema = resolve_schema_with_path(schema, options, source_path)?;

    // Canonicalize flat `fn` lambdas post-inline so raw `.orb` input (which skips
    // the lolo lowerer) is normalized before validation + codegen, and inlined
    // reference traits are caught too.
    orbital_core_base::schema::normalize_fn_shapes(&mut schema);
    Ok(schema)
}

/// Phase 4b: the comprehensive pass against the installed services registry
/// and the packages the compile discovered.
fn validate_for_compile(
    schema: &OrbitalSchema,
    options: &CompilerOptions,
) -> ComprehensiveValidationResult {
    validate_comprehensive_full(
        schema,
        None,
        SchemaLanguage::English,
        crate::embedded::baked_service_registry().clone(),
        options.inline.loader_config.packages.clone(),
    )
}

/// Compile a schema file to OIR with source path for resolving imports
fn compile_schema_with_path(
    schema: OrbitalSchema,
    options: &CompilerOptions,
    source_path: Option<&Path>,
) -> CompileResult<OirModule> {
    let mut schema = resolve_compiled_program_as_written(schema, options, source_path)?;

    // Phase 4: Validate (runs on the fully inlined + enriched schema so reference
    //           checks and collection-presence checks are accurate)
    if !options.skip_validation {
        tracing::info_span!(target: ns::COMPILER, "validate").in_scope(|| validate_schema(&schema))?;

        // Run comprehensive validation if enabled
        if options.comprehensive_validation {
            let result = tracing::info_span!(target: ns::COMPILER, "comprehensive validation")
                .in_scope(|| validate_for_compile(&schema, options));
            // Comprehensive validation is advisory here: its errors are reported, not fatal.
            for error in &result.errors {
                tracing::warn!(target: ns::COMPILER, code = %error.code, path = %error.path, "{}", error.message);
            }
            for warning in &result.warnings {
                tracing::info!(target: ns::COMPILER, path = %warning.path, "{}", warning.message);
            }
        }
    }

    // Phase 4c: answer the reflection the build already knows, on the validated program.
    reflect::fold_literal_reflection(&mut schema, source_path)?;

    // Phase 5: Resolve to OIR
    let mut oir = tracing::info_span!(target: ns::COMPILER, "resolve").in_scope(|| resolve_to_oir(&schema))?;
    oir.egress = schema.egress.clone().unwrap_or_default();
    oir.program_credentials = orbital_core_base::secrets::program_credentials(
        &serde_json::to_value(&schema).map_err(std::io::Error::from)?,
    );
    oir.inbound = orbital_core_base::inbound::inbound_routes_of(&schema);
    oir.os_grants = schema.program_os_grants();
    let runtimes = options
        .service_runtimes
        .as_ref()
        .unwrap_or(&crate::embedded::baked_service_registry().runtimes);
    oir.browser_services = oir
        .invoked_services()
        .into_iter()
        .filter(|s| {
            runtimes
                .get(s)
                .is_some_and(|r| orbital_core_base::runs_on::is_browser_only(r))
        })
        .collect();

    // Phase 5b (opt-in): nominalize anonymous struct/union shapes into
    // `named_types` + `TypeRef`s — post-Resolve, pre-codegen, so every
    // backend sees the same table.
    if options.nominalize {
        let stats = tracing::info_span!(target: ns::COMPILER, "nominalize").in_scope(|| nominalize::nominalize_module(&mut oir));
        tracing::debug!(
            target: ns::COMPILER,
            registered = stats.registered,
            reused_existing = stats.reused_existing,
            reused_dedup = stats.reused_dedup,
            skipped_collisions = stats.skipped_collisions,
            "nominalized"
        );
    }

    Ok(oir)
}

/// Compile a JSON string to OIR
pub fn compile_json(json: &str, options: &CompilerOptions) -> CompileResult<OirModule> {
    // Phase 1: Parse
    let schema = tracing::info_span!(target: ns::COMPILER, "parse").in_scope(|| parse_json(json))?;

    // Continue with common compilation
    compile_schema(schema, options)
}

/// Compile a parsed schema to OIR
pub fn compile_schema(
    schema: OrbitalSchema,
    options: &CompilerOptions,
) -> CompileResult<OirModule> {
    // Delegate to the path-aware version with no source path
    compile_schema_with_path(schema, options, None)
}

#[cfg(test)]
mod app_root_tests {
    use super::*;

    #[test]
    fn an_explicit_root_wins_over_the_marker() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join(".almadar")).expect("mkdir");
        std::fs::write(dir.path().join(".almadar/app-marker.json"), "{}").expect("marker");
        assert_eq!(
            app_root(Some("/elsewhere"), dir.path()),
            Some(PathBuf::from("/elsewhere"))
        );
    }

    #[test]
    fn the_nearest_ancestor_with_the_workspace_marker_is_the_root() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join(".almadar")).expect("mkdir");
        std::fs::write(dir.path().join(".almadar/app-marker.json"), "{}").expect("marker");
        let inner = dir.path().join("orbitals/deep");
        std::fs::create_dir_all(&inner).expect("mkdir");
        assert_eq!(app_root(None, &inner), Some(dir.path().to_path_buf()));
        assert_eq!(
            app_root(Some(""), &inner),
            Some(dir.path().to_path_buf()),
            "an empty value is unset"
        );
    }

    #[test]
    fn control_no_marker_and_no_explicit_root_is_no_app_scope() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join(".almadar")).expect("mkdir");
        assert_eq!(
            app_root(None, dir.path()),
            None,
            "a .almadar without the marker is not a workspace"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn watch_program(event: &str) -> OrbitalSchema {
        serde_json::from_value(json!({
            "name": "fixture", "version": "1.0.0",
            "orbitals": [{
                "name": "Feed",
                "entity": { "name": "Row", "persistence": "runtime",
                    "fields": [{ "name": "id", "type": "string", "required": true }] },
                "traits": [{
                    "name": "Watcher", "linkedEntity": "Row", 
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }],
                        "events": [{ "key": "INIT" }, { "key": "SEEN", "input": true }],
                        "transitions": [
                            { "from": "idle", "to": "idle", "event": "INIT",
                              "effects": [["browser/tab-watch",
                                { "match": "https://example.com/*", "selector": "a", "fields": {}, "event": event }]] },
                            { "from": "idle", "to": "idle", "event": "SEEN", "effects": [] }
                        ]
                    }
                }],
                "pages": []
            }]
        }))
        .expect("schema")
    }

    fn has_event_address_error(result: &ComprehensiveValidationResult) -> bool {
        result.errors.iter().chain(result.warnings.iter()).any(|e| {
            matches!(
                e.code,
                validation::ErrorCode::TraitEventRefUnresolved
                    | validation::ErrorCode::TraitEventRefNotExternalInput
            )
        })
    }

    #[test]
    fn a_tab_watch_event_address_is_checked_against_the_declared_inputs_in_every_validation() {
        let misspelled = watch_program("Watcher.SEN");
        let result = validate_for_compile(&misspelled, &CompilerOptions::default());
        assert!(has_event_address_error(&result), "{:?}", result.errors);
        assert!(has_event_address_error(&validate_comprehensive_without_services(&misspelled)));
        let declared = watch_program("Watcher.SEEN");
        assert!(!has_event_address_error(&validate_comprehensive_without_services(&declared)));
        assert!(!has_event_address_error(&validate_for_compile(&declared, &CompilerOptions::default())));
    }

    fn program_importing_a_missing_file() -> OrbitalSchema {
        let mut value = serde_json::to_value(watch_program("Watcher.SEEN")).expect("value");
        value["orbitals"][0]["uses"] = json!([{ "from": "./does-not-exist.orb", "as": "Missing" }]);
        serde_json::from_value(value).expect("schema")
    }

    #[test]
    fn resolving_stamps_the_program_and_a_resolved_program_is_returned_unchanged_without_inlining() {
        let options = CompilerOptions::default();
        let resolved = resolve_schema_with_path(watch_program("Watcher.SEEN"), &options, None).expect("resolves");
        let mark = resolved.resolved.clone().expect("stamped");
        assert_eq!(mark.type_contract_version, orbital_core_base::schema::TYPE_CONTRACT_VERSION);
        assert_eq!(mark.orb, env!("CARGO_PKG_VERSION"));

        let before = serde_json::to_string(&resolved).expect("json");
        let again = resolve_schema_with_path(resolved, &options, None).expect("resolved again");
        assert_eq!(serde_json::to_string(&again).expect("json"), before);

        let mut importing = program_importing_a_missing_file();
        importing.resolved = Some(mark);
        let before = serde_json::to_string(&importing).expect("json");
        let untouched = resolve_schema_with_path(importing, &options, None).expect("an import would fail if inlined");
        assert_eq!(serde_json::to_string(&untouched).expect("json"), before);
    }

    #[test]
    fn control_an_unresolved_program_is_inlined() {
        let err = resolve_schema_with_path(program_importing_a_missing_file(), &CompilerOptions::default(), None);
        assert!(err.is_err(), "the missing import is only reached when the program is not resolved");
    }

    #[test]
    fn a_program_resolved_under_another_type_contract_is_refused() {
        let mut program = watch_program("Watcher.SEEN");
        program.resolved = Some(orbital_core_base::schema::ResolvedMark {
            orb: "0.0.1".to_string(),
            type_contract_version: orbital_core_base::schema::TYPE_CONTRACT_VERSION + 1,
        });
        let err = resolve_schema_with_path(program, &CompilerOptions::default(), Some(Path::new("/app/app.orb")))
            .expect_err("refused")
            .to_string();
        let (theirs, ours) = (orbital_core_base::schema::TYPE_CONTRACT_VERSION + 1, orbital_core_base::schema::TYPE_CONTRACT_VERSION);
        assert!(err.contains(&format!("/app/app.orb was resolved by orb 0.0.1 for type contract {theirs}; this orb reads {ours}")), "{err}");
        assert!(err.contains("resolve it again from source"), "{err}");
    }

    #[test]
    fn test_compile_simple_schema() {
        let json = r#"{
            "name": "TestApp",
            "version": "1.0.0",
            "orbitals": []
        }"#;

        let options = CompilerOptions::default();
        let result = compile_json(json, &options);
        assert!(result.is_ok());

        let oir = result.unwrap();
        assert_eq!(oir.name, "TestApp");
        assert_eq!(oir.version, "1.0.0");
    }

    #[test]
    fn a_resolved_program_carries_the_lambda_a_raw_callback_body_lowers_to() {
        use std::path::Path;
        let schema = orbital_test_support::installed_schema(
            "almadar-std",
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "std-learn-trace-player",
        );
        let resolved = resolve_schema_with_path(schema, &CompilerOptions::default(), None)
            .expect("resolve std-learn-trace-player");
        let text = serde_json::to_string(&resolved).unwrap();
        let wrapped = r#"["array/map",["object/get","@fr","values"],["fn",["item","index"],"#;
        assert!(text.contains(wrapped), "the bars map's raw body is a lambda in the resolved program");
        assert!(
            !text.contains(r#"["array/map",["object/get","@fr","values"],{"#),
            "control: no raw body is left in its callback slot"
        );
    }

    // Wave-8: the wasm mount resolves its schema through `resolve_schema_with_path(.., None)`
    // and relies on std peers resolving from the installed `@almadar/std` package (no
    // filesystem source path). std-export `uses` a handful of atoms as
    // `TraitReference::Reference`; after resolve with `source_path=None` every one must
    // become `TraitReference::Inline`.
    #[test]
    fn resolves_installed_std_uses_with_no_source_path() {
        use orbital_core_base::schema::TraitReference;
        use std::path::Path;

        let schema = orbital_test_support::installed_schema(
            "almadar-std",
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "std-export",
        );

        // Pre-condition: the peers are unresolved References before inline.
        let orbital = &schema.orbitals[0];
        let refs_before = orbital
            .traits
            .iter()
            .filter(|t| matches!(t, TraitReference::Reference { .. }))
            .count();
        // The installed std-export's peer count drifts across registry
        // publishes; one unresolved Reference is enough to exercise
        // no-source-path resolution.
        assert!(
            refs_before >= 1,
            "expected >=1 unresolved ref, got {refs_before}"
        );

        // Discover installed behavior packages from this crate's manifest
        // dir (the monorepo workspace link), not the process cwd — mirrors
        // `CompilerOptions::from_env` rooted at a stable path for tests.
        crate::loader::register_source_lowerer(|path, _, _| orbital_test_support::lower_lolo_file(path));
        let start = Path::new(env!("CARGO_MANIFEST_DIR"));
        let user_store = orbital_core_base::behavior_packages::user_store_dir()
            .unwrap_or_else(|| start.to_path_buf());
        let packages = orbital_core_base::behavior_packages::discover(start, &user_store)
            .expect("behavior package discovery failed");
        let mut options = CompilerOptions::default();
        options.inline.loader_config.packages = packages;

        let resolved = resolve_schema_with_path(schema, &options, None)
            .expect("resolve std-export with no source path");

        for orbital in &resolved.orbitals {
            for t in &orbital.traits {
                assert!(
                    matches!(t, TraitReference::Inline(_)),
                    "every trait must inline after resolve; found unresolved Reference"
                );
            }
        }
    }
}
