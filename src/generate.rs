//! Code Generation Module
//!
//! Handles the full compilation pipeline: Schema → OIR → Shell → Generated Code
//!
//! This module provides the high-level API for generating code from Orbital schemas.

use crate::phases::{compile_file_with_source, parse_file, CompilerOptions};
use orbital_core::ir::OirModule;
use orbital_core::shell::{Shell, ShellError};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

// Re-export core generation types so existing consumers of
// `orbital_compiler::generate` continue to compile without changes.
pub use orbital_core::generate::{Backend, GeneratedFile, GeneratedOutput, ShellMode};

/// Generation error
#[derive(Debug, Error)]
pub enum GenerateError {
    #[error("Compilation error: {0}")]
    Compile(#[from] crate::phases::CompilerError),

    #[error("Shell error: {0}")]
    Shell(#[from] ShellError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Backend error: {0}")]
    Backend(String),
}

/// Result type for generation operations
pub type GenerateResult<T> = Result<T, GenerateError>;

/// Generation options
// `Clone` dropped along with `CompilerOptions`'s (see that struct's comment) —
// verified zero call sites clone a `GenerateOptions` today.
#[derive(Debug, Default)]
pub struct GenerateOptions {
    /// Compiler options
    pub compiler: CompilerOptions,
    /// Whether to copy the shell template
    pub copy_template: bool,
    /// Whether to clean existing generated files first
    pub clean: bool,
    /// Verbose output
    pub verbose: bool,
    /// Shell mode (full, client, server)
    pub mode: ShellMode,
}

/// Generate code from a schema file
///
/// This is the main entry point for code generation. It:
/// 1. Compiles the schema to OIR
/// 2. Optionally copies the shell template
/// 3. Generates code using the backend
/// 4. Writes files to the output directory
/// 5. Copies the source schema to schema.orb in the output directory
pub fn generate_from_file<S, B>(
    schema_path: impl AsRef<Path>,
    output_dir: impl AsRef<Path>,
    shell: &S,
    backend: &B,
    options: &GenerateOptions,
) -> GenerateResult<GeneratedOutput>
where
    S: Shell,
    B: Backend,
{
    let schema_path = schema_path.as_ref();
    generate_from_file_with_source(schema_path, schema_path, output_dir, shell, backend, options)
}

/// [`generate_from_file`] for a schema written somewhere other than its
/// source (a lowered `.lolo` or a translated copy): imports resolve against

/// A program declaring browser-stored entities compiles only with a backend that
/// generates their client-side data effects; any other backend is refused
/// before it writes a file.
fn ensure_backend_supports<B: Backend>(oir: &OirModule, backend: &B) -> GenerateResult<()> {
    let local: Vec<&str> = oir.entities.iter().filter(|e| e.local).map(|e| e.name.as_str()).collect();
    if local.is_empty() || backend.supports_browser_stored_entities() {
        return Ok(());
    }
    Err(GenerateError::Backend(format!(
        "ORB_SHELL_LOCAL_ENTITY_UNSUPPORTED: the {} shell does not generate browser-stored entities; {} declare `local`. Compile with --shell typescript, or drop `local`.",
        backend.name(),
        local.join(", ")
    )))
}

/// `source_path`.
pub fn generate_from_file_with_source<S, B>(
    schema_path: &Path,
    source_path: &Path,
    output_dir: impl AsRef<Path>,
    shell: &S,
    backend: &B,
    options: &GenerateOptions,
) -> GenerateResult<GeneratedOutput>
where
    S: Shell,
    B: Backend,
{
    let output_dir = output_dir.as_ref();

    if options.verbose {
        eprintln!("Compiling: {}", schema_path.display());
    }

    // Step 1: Compile schema to OIR
    let oir = compile_file_with_source(schema_path, source_path, &options.compiler)?;

    // Generate code from OIR
    let result = generate_from_oir(&oir, output_dir, shell, backend, options)?;

    // A program with `locales` ships one merged catalog per locale, where
    // the generated App imports it.
    let catalogs = if oir.locales.is_empty() {
        std::collections::BTreeMap::new()
    } else {
        let (resolved, lazy) = crate::phases::lazy::resolve_program_with_path(
            parse_file(schema_path).map_err(crate::phases::CompilerError::from)?,
            &options.compiler,
            Some(source_path),
        )?;
        let program_dir = source_path.parent().unwrap_or(Path::new("."));
        let catalogs = crate::phases::messages::program_catalogs(&resolved, &lazy, program_dir);
        let locales_dir = output_dir.join("locales");
        std::fs::create_dir_all(&locales_dir)?;
        for (locale, messages) in &catalogs {
            let json = serde_json::to_string_pretty(messages).map_err(std::io::Error::other)?;
            std::fs::write(locales_dir.join(format!("{locale}.json")), json)?;
        }
        catalogs
    };

    // Every route the app serves with its resolved page modifiers — the one
    // manifest the route documents, sitemap and prerender read.
    let manifest = route_manifest(&oir, &catalogs).map_err(std::io::Error::other)?;
    std::fs::write(
        output_dir.join("routes.json"),
        serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?,
    )?;

    // Step 6: Copy source schema to output directory for reference
    let dest_schema = output_dir.join("schema.orb");
    std::fs::copy(schema_path, &dest_schema)?;
    if options.verbose {
        eprintln!("Copied schema to: {}", dest_schema.display());
    }

    // Step 7: Emit `resolved.orb` alongside the copied source schema.
    //
    // The resolved artifact is the bundled view of the schema: post Inline +
    // Compose + Enrich, with every `uses[]` import resolved, every
    // `TraitReference::Reference` collapsed to `TraitReference::Inline`,
    // every Phase 0 composer (`behavior/compose`, `wire`, `pipe`,
    // `detect-layout`) expanded, and every `events: {}` rename applied
    // through transitions AND render-ui props.
    //
    // Structurally identical to the input `.orb` format so downstream TS
    // tools (`orbital-verify-unified/src/analyze.ts`, future `--parity`
    // mode, verifiers) can `JSON.parse` the file and consume it as an
    // `OrbitalSchema` without new parsing code.
    //
    // Skipped silently on parse / resolve errors — emission is best-effort,
    // not a blocker for codegen. `orbital resolve` exposes the same shape
    // as a dedicated CLI subcommand for tooling that wants resolved
    // visibility without generating TSX.
    if let Ok(parsed) = parse_file(schema_path) {
        if let Ok((resolved, lazy)) =
            crate::phases::lazy::resolve_program_with_path(parsed, &options.compiler, Some(source_path))
        {
            let resolved_path = output_dir.join("resolved.orb");
            let program_dir = source_path.parent().unwrap_or(Path::new("."));
            if let Err(err) = crate::phases::lazy::write_lazy(output_dir, &lazy, program_dir, &|schema| {
                serde_json::to_string_pretty(schema).map_err(std::io::Error::other)
            }) {
                eprintln!(
                    "[generate_from_file] Failed to write lazy behaviors under {}: {}",
                    output_dir.display(),
                    err
                );
            }
            if let Ok(json) = serde_json::to_string_pretty(&resolved) {
                if let Err(err) = std::fs::write(&resolved_path, json) {
                    eprintln!(
                        "[generate_from_file] Failed to write resolved.orb at {}: {}",
                        resolved_path.display(),
                        err
                    );
                } else if options.verbose {
                    eprintln!("Wrote resolved.orb to: {}", resolved_path.display());
                }
            }
        }
    }

    Ok(result)
}

/// Generate code from an OIR module
pub fn generate_from_oir<S, B>(
    oir: &OirModule,
    output_dir: impl AsRef<Path>,
    shell: &S,
    backend: &B,
    options: &GenerateOptions,
) -> GenerateResult<GeneratedOutput>
where
    S: Shell,
    B: Backend,
{
    let output_dir = output_dir.as_ref();

    if options.verbose {
        eprintln!("Output: {}", output_dir.display());
        eprintln!("Shell: {}", shell.name());
        eprintln!("Backend: {}", backend.name());
    }

    // Step 2: Copy shell template if requested
    if options.copy_template {
        if options.verbose {
            eprintln!("Copying shell template...");
        }
        shell.init_project(output_dir, &oir.name)?;
    }

    // Step 3: Clean existing generated files if requested
    if options.clean {
        if options.verbose {
            eprintln!("Cleaning generated directory...");
        }
        shell.clean_generated(output_dir)?;
    }

    // Step 4: Generate code
    if options.verbose {
        eprintln!("Generating code (mode: {:?})...", options.mode);
    }
    ensure_backend_supports(oir, backend)?;
    let output = backend.generate_with_mode(oir, options.mode);

    // Cross-shell operator-completeness backstop: no emitted file may carry the
    // shared unsupported-operator marker (Python emits it in a `#` comment, TS
    // in a `/* */` comment). Fail loudly BEFORE writing, rather than shipping a
    // broken stub the static parity gate can't see — e.g. an operator with no
    // canonical-registry entry at all.
    let unsupported: Vec<String> = output
        .files
        .iter()
        .flat_map(|file| {
            file.content
                .lines()
                .filter(|line| {
                    line.contains(orbital_core::UNSUPPORTED_OPERATOR_MARKER)
                        || line.contains(orbital_core::ir::LITERAL_TYPE_MISMATCH_MARKER)
                })
                .map(move |line| format!("{} — {}", file.path, line.trim()))
        })
        .collect();
    if !unsupported.is_empty() {
        return Err(GenerateError::Backend(format!(
            "backend emitted {} codegen marker(s) (unsupported operator / default literal \
             not renderable at its declared type):\n  {}",
            unsupported.len(),
            unsupported.join("\n  ")
        )));
    }

    // Step 5: Write files
    let generated_dir = output_dir.join(shell.generated_dir());
    std::fs::create_dir_all(&generated_dir)?;

    // §94 (`docs/Almadar_Compiler_Gaps.md`): a source-level trait/organism a
    // recompile no longer emits used to leave its LAST compile's file behind
    // forever — nothing in this loop ever deletes anything. Read the PREVIOUS
    // compile's manifest before writing (`None` on first compile / no
    // manifest / an unparsable one — never destructive on that path), then
    // prune whatever it lists that this compile did not reproduce. Scoped to
    // exactly what THIS loop writes: `generate_from_file`'s `schema.orb` /
    // `resolved.orb`, `orbital-cli`'s `.env` merge, and the lowered `.orb`
    // (`orbital-cli/src/commands/compile.rs`) are written by other call
    // sites entirely and never touched here.
    let previous_manifest = read_orb_manifest(output_dir);

    for file in &output.files {
        // Workspace-relative paths (e.g. packages/shared/...) are written
        // relative to the output root; all other generated paths live under
        // the shell's generated_dir.
        let file_path = resolve_manifest_path(output_dir, &generated_dir, &file.path);

        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&file_path, &file.content)?;

        if options.verbose {
            eprintln!("  Written: {}", file.path);
        }
    }

    let new_manifest: std::collections::BTreeSet<String> =
        output.files.iter().map(|f| f.path.clone()).collect();
    if let Some(previous) = previous_manifest {
        for stale in previous.difference(&new_manifest) {
            let stale_path = resolve_manifest_path(output_dir, &generated_dir, stale);
            if stale_path.exists() {
                std::fs::remove_file(&stale_path)?;
                if options.verbose {
                    eprintln!("  Pruned stale: {stale}");
                }
                if let Some(parent) = stale_path.parent() {
                    prune_empty_dirs(parent, output_dir);
                }
            }
        }
    }
    write_orb_manifest(output_dir, &new_manifest)?;

    if options.verbose {
        eprintln!(
            "Generated {} files to {}",
            output.files.len(),
            generated_dir.display()
        );
    }

    Ok(output)
}

/// The manifest file name `generate_from_oir` reads/writes at `output_dir`,
/// listing every logical `GeneratedFile::path` written by its OWN write loop
/// (never `.env`/lowered-`.orb`/`schema.orb`/`resolved.orb`, written by other
/// call sites).
const ORB_MANIFEST_FILE: &str = ".orb-manifest.json";

/// A generated path prefixed with one of these is "workspace-relative" —
/// already rooted at a template-declared location outside the shell's own
/// `generated_dir` tree (a sibling package root, or a native test target's
/// own `sources:`/source-set root declared in `project.yml`/`build.gradle.kts`)
/// — and is written directly under `output_dir` instead of nested under
/// `generated_dir`. `packages/` is the original TS/mobile multi-package
/// case (`orbital-shell-typescript`/`orbital-shell-mobile`); `Sources/` and
/// `app/src/test/` are the iOS/Android native test-target cases (Program C
/// wave C4 — `Sources/OrbitalAppTests/PatternSnapshotTests.swift`,
/// `app/src/test/kotlin/generated/PatternSnapshotTests.kt`); `app/src/main/assets/`
/// is the Android *main* module's asset root (Program B wave B6 —
/// `app/src/main/assets/resolved_schema.json`, the in-app kernel's bundled
/// resolved schema, mirroring iOS's `Sources/OrbitalApp/Resources/`) — added
/// because neither shell's `generated_dir` (`Sources/OrbitalApp/Generated`,
/// `app/src/main/kotlin/generated`) covers a sibling resource/test source
/// root. No existing `GeneratedFile::path` in any shell started with any of
/// these before their own addition (each backend otherwise emits bare
/// relative names under its own `generated_dir`), so each is purely additive.
const WORKSPACE_RELATIVE_PREFIXES: &[&str] =
    &["packages/", "Sources/", "app/src/test/", "app/src/main/assets/"];

/// A workspace-relative path (see `WORKSPACE_RELATIVE_PREFIXES`) is written
/// relative to the output root; every other generated path is written under
/// the shell's `generated_dir` — the ONE resolution rule the write loop and
/// the stale-prune pass both use, so a manifest entry always resolves to the
/// exact file it named.
fn resolve_manifest_path(output_dir: &Path, generated_dir: &Path, logical_path: &str) -> std::path::PathBuf {
    if WORKSPACE_RELATIVE_PREFIXES.iter().any(|prefix| logical_path.starts_with(prefix)) {
        output_dir.join(logical_path)
    } else {
        generated_dir.join(logical_path)
    }
}

/// The previous compile's manifest, if `output_dir` has one and it parses —
/// `None` on first compile, a missing file, or anything unparsable, so a
/// corrupt/foreign manifest is never destructive (no prune runs at all
/// rather than guessing at a malformed list).
fn read_orb_manifest(output_dir: &Path) -> Option<std::collections::BTreeSet<String>> {
    let content = std::fs::read_to_string(output_dir.join(ORB_MANIFEST_FILE)).ok()?;
    serde_json::from_str(&content).ok()
}

/// Persist this compile's manifest for the NEXT compile's prune pass.
fn write_orb_manifest(
    output_dir: &Path,
    manifest: &std::collections::BTreeSet<String>,
) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(manifest)?;
    std::fs::write(output_dir.join(ORB_MANIFEST_FILE), json)
}

/// Remove `dir` and every now-empty ancestor up to (but never including)
/// `stop_at`, bottom-up — a stale file's directory (and that directory's own
/// now-empty parent, …) left behind by a dropped trait/organism. Stops the
/// instant a directory is missing, not a directory, non-empty, or `stop_at`
/// itself; never touches `stop_at` or anything above it.
fn prune_empty_dirs(dir: &Path, stop_at: &Path) {
    let mut current = dir;
    loop {
        if current == stop_at || !current.starts_with(stop_at) {
            return;
        }
        let Ok(mut entries) = std::fs::read_dir(current) else {
            return;
        };
        if entries.next().is_some() {
            return;
        }
        if std::fs::remove_dir(current).is_err() {
            return;
        }
        let Some(parent) = current.parent() else {
            return;
        };
        current = parent;
    }
}

/// Registry of backends
pub struct BackendRegistry {
    backends: HashMap<String, Box<dyn Backend>>,
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
        }
    }

    pub fn register(&mut self, backend: Box<dyn Backend>) {
        self.backends.insert(backend.name().to_string(), backend);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Backend> {
        self.backends.get(name).map(|b| b.as_ref())
    }

    pub fn list(&self) -> Vec<&str> {
        self.backends.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// `routes.json`: the site modifiers and, per route, its locale, text
/// direction, access/indexing and its title/description resolved for that
/// route's locale (a literal as written, `i18n/t` from the merged catalog).
/// Absent modifiers stay absent. An `i18n/t` key missing from the route's
/// catalog is an error — validation guarantees every key, so this never
/// guesses.
pub fn route_manifest(
    oir: &orbital_core::ir::OirModule,
    catalogs: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
) -> Result<serde_json::Value, String> {
    use orbital_core::schema::{PageAccess, PageIndexing, PageMessage, PageMeta};
    let resolve = |meta: &PageMeta, locale: Option<&str>, path: &str| -> Result<String, String> {
        match meta {
            PageMeta::Literal(s) => Ok(s.clone()),
            PageMeta::Message(PageMessage(_, key)) => locale
                .and_then(|l| catalogs.get(l))
                .and_then(|c| c.get(key))
                .cloned()
                .ok_or_else(|| format!("route {path}: no {} message for `{key}`", locale.unwrap_or("default-locale"))),
        }
    };
    if let Some(page) = oir
        .pages
        .iter()
        .find(|p| orbital_core::schema::is_host_sign_in_route(&p.path))
    {
        return Err(format!(
            "ORB_P_HOST_RESERVED_ROUTE: page {} is declared on route {}, the host's own sign-in route",
            page.name, page.path
        ));
    }
    if let Some((group, locale, paths)) = orbital_core::messages::duplicate_translation_locales(
        oir.pages
            .iter()
            .map(|p| (p.path.as_str(), p.translation_of.as_deref())),
        &oir.locales,
    )
    .into_iter()
    .next()
    {
        return Err(format!(
            "ORB_P_TRANSLATION_DUPLICATE_LOCALE: translation group '{group}' holds {} pages in locale '{locale}' ({})",
            paths.len(),
            paths.join(", ")
        ));
    }
    let mut routes = Vec::new();
    for page in &oir.pages {
        let locale = oir.locale_of_path(&page.path);
        let mut route = serde_json::json!({
            "path": page.path,
            "page": page.name,
            "locale": locale,
        });
        let obj = route.as_object_mut().expect("route is an object");
        if let Some(l) = locale {
            obj.insert("dir".into(), orbital_core::messages::locale_direction(l).into());
        }
        if let Some(access) = page.access {
            obj.insert(
                "access".into(),
                match access {
                    PageAccess::Public => "public",
                    PageAccess::Authenticated => "authenticated",
                }
                .into(),
            );
        }
        if let Some(indexing) = page.indexing {
            obj.insert(
                "indexing".into(),
                match indexing {
                    PageIndexing::Index => "index",
                    PageIndexing::Noindex => "noindex",
                }
                .into(),
            );
        }
        if let Some(title) = &page.title {
            obj.insert("title".into(), resolve(title, locale, &page.path)?.into());
        }
        if let Some(description) = &page.description {
            obj.insert("description".into(), resolve(description, locale, &page.path)?.into());
        }
        routes.push(route);
    }
    let origin = oir.site.as_ref().and_then(|s| s.origin.as_ref());
    if origin.is_none() {
        if let Some(page) = oir
            .pages
            .iter()
            .find(|p| p.indexing == Some(PageIndexing::Index))
        {
            return Err(format!(
                "ORB_P_INDEX_NO_ORIGIN: route {} is `indexing: index` but the app declares no `origin:` — its canonical URL and sitemap entry need one",
                page.path
            ));
        }
    }
    for (page, route) in oir.pages.iter().zip(routes.iter_mut()) {
        if let Some(alternates) = locale_alternates(oir, page) {
            route["alternates"] = alternates;
        }
    }
    let mut manifest = serde_json::json!({ "routes": routes });
    if let Some(default_locale) = oir.locales.first() {
        manifest["defaultLocale"] = default_locale.clone().into();
    }
    if let Some(site) = &oir.site {
        manifest["site"] = serde_json::to_value(site).map_err(|e| e.to_string())?;
    }
    Ok(manifest)
}

/// The indexable public routes of `page`'s translation group (its
/// `translationOf`, else its upstream `sourcePage`) across the app's locales,
/// in locale order, as `[{ locale, path }]` — its hreflang alternates. `None`
/// when `page` is not itself indexable and public, has no group, appears in fewer than two locales, or one locale holds it
/// twice (no single alternate per language).
pub fn locale_alternates(
    oir: &orbital_core::ir::OirModule,
    page: &orbital_core::ir::OirPage,
) -> Option<serde_json::Value> {
    use orbital_core::schema::{PageAccess, PageIndexing};
    let indexable = |p: &orbital_core::ir::OirPage| {
        p.access == Some(PageAccess::Public) && p.indexing != Some(PageIndexing::Noindex)
    };
    if !indexable(page) {
        return None;
    }
    let group = |p: &orbital_core::ir::OirPage| -> Option<String> {
        p.translation_of.clone().or_else(|| p.source_page.clone())
    };
    let key = group(page)?;
    let mut by_locale: Vec<(&str, &str)> = Vec::new();
    for other in oir
        .pages
        .iter()
        .filter(|p| group(p).as_deref() == Some(key.as_str()) && indexable(p))
    {
        let locale = oir.locale_of_path(&other.path)?;
        if by_locale.iter().any(|(l, _)| *l == locale) {
            return None;
        }
        by_locale.push((locale, other.path.as_str()));
    }
    if by_locale.len() < 2 {
        return None;
    }
    by_locale.sort_by_key(|(l, _)| oir.locales.iter().position(|x| x == l));
    Some(
        by_locale
            .into_iter()
            .map(|(locale, path)| serde_json::json!({ "locale": locale, "path": path }))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::BaseShell;
    use orbital_core::ir::OirModule;

    #[test]
    fn test_generated_file() {
        let file = GeneratedFile::new("entities/task.ts", "export interface Task {}");
        assert_eq!(file.path, "entities/task.ts");
        assert_eq!(file.content, "export interface Task {}");
    }

    /// A backend whose file set is controlled by the test, standing in for
    /// "organism A stayed, organism B was dropped from the source between
    /// two compiles".
    struct FixtureBackend {
        include_b: bool,
    }

    impl Backend for FixtureBackend {
        fn generate(&self, _module: &OirModule) -> GeneratedOutput {
            let mut files = vec![
                GeneratedFile::new("a.ts", "export const a = 1;"),
                GeneratedFile::new("packages/server/x.ts", "export const x = 1;"),
            ];
            if self.include_b {
                files.push(GeneratedFile::new("nested/b.ts", "export const b = 1;"));
            }
            GeneratedOutput { files, warnings: vec![] }
        }
        fn name(&self) -> &'static str {
            "fixture"
        }
    }

    /// A backend declaring whether it generates browser-stored entities.
    struct CapabilityBackend {
        browser_storage: bool,
    }

    impl Backend for CapabilityBackend {
        fn generate(&self, _module: &OirModule) -> GeneratedOutput {
            GeneratedOutput { files: vec![GeneratedFile::new("a.ts", "export const a = 1;")], warnings: vec![] }
        }
        fn name(&self) -> &'static str {
            "fixture"
        }
        fn supports_browser_stored_entities(&self) -> bool {
            self.browser_storage
        }
    }

    fn module_with_local_entity() -> OirModule {
        let mut module = OirModule::new("Test", "0.1.0");
        let mut invoice = orbital_core::ir::OirEntity::new("Invoice");
        invoice.local = true;
        module.entities.push(invoice);
        module
    }

    #[test]
    fn a_backend_without_browser_storage_refuses_a_local_entity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let options = GenerateOptions { copy_template: false, clean: false, ..Default::default() };
        let err = generate_from_oir(&module_with_local_entity(), tmp.path(), &fixture_shell("generated"), &CapabilityBackend { browser_storage: false }, &options)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ORB_SHELL_LOCAL_ENTITY_UNSUPPORTED") && err.contains("Invoice"), "{err}");
        assert!(!tmp.path().join("generated/a.ts").exists(), "nothing is written");
    }

    #[test]
    fn control_a_backend_with_browser_storage_compiles_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let options = GenerateOptions { copy_template: false, clean: false, ..Default::default() };
        generate_from_oir(&module_with_local_entity(), tmp.path(), &fixture_shell("generated"), &CapabilityBackend { browser_storage: true }, &options)
            .expect("compiles");
        assert!(tmp.path().join("generated/a.ts").exists());
    }

    fn fixture_shell(generated_dir: &'static str) -> BaseShell {
        BaseShell::new(
            "fixture",
            "fixture shell",
            &[],
            std::path::PathBuf::new(),
            generated_dir,
            "",
            "",
            "",
        )
    }

    /// §94: recompiling without `nested/b.ts` must delete it (and prune the
    /// now-empty `nested/` directory), the manifest must list exactly the two
    /// survivors, and a sibling `.env` the test plants (standing in for the
    /// CLI's own `.env` merge — never touched by this loop) must survive
    /// untouched.
    #[test]
    fn recompile_prunes_stale_output_and_leaves_unlisted_files_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let output_dir = tmp.path();
        let shell = fixture_shell("generated");
        let module = OirModule::new("Test", "0.1.0");
        let options = GenerateOptions {
            copy_template: false,
            clean: false,
            ..Default::default()
        };

        // A sibling `.env` this loop must never see, let alone delete.
        std::fs::write(output_dir.join(".env"), "VITE_API_URL=http://localhost\n").unwrap();

        // First compile: A + B + packages/server/x.ts.
        let backend_with_b = FixtureBackend { include_b: true };
        generate_from_oir(&module, output_dir, &shell, &backend_with_b, &options)
            .expect("first compile");
        assert!(output_dir.join("generated/a.ts").exists());
        assert!(output_dir.join("generated/nested/b.ts").exists());
        assert!(output_dir.join("packages/server/x.ts").exists());

        // Second compile: B dropped from the source.
        let backend_without_b = FixtureBackend { include_b: false };
        generate_from_oir(&module, output_dir, &shell, &backend_without_b, &options)
            .expect("second compile");

        assert!(output_dir.join("generated/a.ts").exists(), "A survives");
        assert!(
            output_dir.join("packages/server/x.ts").exists(),
            "packages/-relative survivor stays"
        );
        assert!(
            !output_dir.join("generated/nested/b.ts").exists(),
            "B must be pruned once the compile no longer produces it"
        );
        assert!(
            !output_dir.join("generated/nested").exists(),
            "B's now-empty directory must be pruned bottom-up"
        );
        assert!(
            output_dir.join("generated").exists(),
            "pruning must stop at output_dir/generated, never remove it"
        );

        let manifest: std::collections::BTreeSet<String> = serde_json::from_str(
            &std::fs::read_to_string(output_dir.join(ORB_MANIFEST_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest,
            std::collections::BTreeSet::from([
                "a.ts".to_string(),
                "packages/server/x.ts".to_string(),
            ]),
            "the manifest must list exactly what THIS compile produced"
        );

        assert_eq!(
            std::fs::read_to_string(output_dir.join(".env")).unwrap(),
            "VITE_API_URL=http://localhost\n",
            "an unlisted sibling file (the CLI's own .env merge) must never be touched"
        );
    }

    /// §94: a first compile (no manifest yet) must never attempt a prune —
    /// nothing to diff against, so nothing is deleted.
    #[test]
    fn first_compile_with_no_manifest_prunes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let output_dir = tmp.path();
        let shell = fixture_shell("generated");
        let module = OirModule::new("Test", "0.1.0");
        let options = GenerateOptions {
            copy_template: false,
            clean: false,
            ..Default::default()
        };

        let backend = FixtureBackend { include_b: true };
        generate_from_oir(&module, output_dir, &shell, &backend, &options).expect("compiles");
        assert!(output_dir.join("generated/a.ts").exists());
        assert!(output_dir.join("generated/nested/b.ts").exists());
        assert!(output_dir.join(ORB_MANIFEST_FILE).exists());
    }
}
