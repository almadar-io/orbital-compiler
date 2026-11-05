//! Code Generation Module
//!
//! Handles the full compilation pipeline: Schema → OIR → Shell → Generated Code
//!
//! This module provides the high-level API for generating code from Orbital schemas.

use crate::phases::{compile_file_with_source, parse_file, CompilerOptions};
use orbital_core_base::ir::OirModule;
use orbital_core_base::schema::OrbitalSchema;
use orbital_core_base::logging::ns;
use orbital_core_base::shell::{Shell, ShellError};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

// Re-export core generation types so existing consumers of
// `orbital_compiler::generate` continue to compile without changes.
pub use orbital_core_base::generate::{locale_alternates, Backend, GeneratedFile, GeneratedOutput, ShellMode};

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

    /// A shell program refused the request (`orbital_core::shell_protocol`).
    #[error("{0}")]
    ShellProgram(#[from] orbital_core_base::shell_protocol::ShellFailure),
}

impl From<orbital_core_base::project::WriteError> for GenerateError {
    fn from(error: orbital_core_base::project::WriteError) -> Self {
        use orbital_core_base::project::WriteError;
        match error {
            WriteError::Shell(e) => GenerateError::Shell(e),
            WriteError::Io(e) => GenerateError::Io(e),
            WriteError::Backend(message) => GenerateError::Backend(message),
        }
    }
}

/// Generate code from an OIR module ([`orbital_core::project::write_project`]).
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
    Ok(orbital_core_base::project::write_project(oir, output_dir.as_ref(), shell, backend, &options.write_options())?)
}

impl GenerateOptions {
    /// The project-writing half of these options, as a shell receives it.
    pub fn write_options(&self) -> orbital_core_base::project::WriteOptions {
        orbital_core_base::project::WriteOptions {
            copy_template: self.copy_template,
            clean: self.clean,
            mode: self.mode,
            service_runtimes: self.compiler.service_runtimes.clone(),
        }
    }
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
    generate_from_file_with_source_using(schema_path, source_path, output_dir, options, |oir, output_dir| {
        generate_from_oir(oir, output_dir, shell, backend, options)
    })
}

/// [`generate_from_file_with_source`] with the project write delegated to
/// `write` (an in-process shell, or a shell program `orb` runs): compile to
/// IR, write, then the program-level files (catalogs, `routes.json`,
/// `schema.orb`, `resolved.orb`).
pub fn generate_from_file_with_source_using<R>(
    schema_path: &Path,
    source_path: &Path,
    output_dir: impl AsRef<Path>,
    options: &GenerateOptions,
    write: impl FnOnce(&OirModule, &Path) -> GenerateResult<R>,
) -> GenerateResult<R> {
    generate_from_file_with_source_resolved(schema_path, source_path, output_dir, options, |oir, _, out| write(oir, out))
}

/// The program resolved with its `uses lazy` behaviors resolved apart: what `resolved.orb`, the locale
/// catalogs and a browser client that opens lazy pages itself are written from.
pub type SplitProgram = (OrbitalSchema, Vec<crate::phases::lazy::ResolvedLazy>);

/// [`generate_from_file_with_source_using`] that also hands `write` the split program, resolved once for the
/// whole generate.
pub fn generate_from_file_with_source_resolved<R>(
    schema_path: &Path,
    source_path: &Path,
    output_dir: impl AsRef<Path>,
    options: &GenerateOptions,
    write: impl FnOnce(&OirModule, &SplitProgram, &Path) -> GenerateResult<R>,
) -> GenerateResult<R> {
    let output_dir = output_dir.as_ref();
    let _span = tracing::info_span!(target: ns::COMPILER, "generate", file = %schema_path.display()).entered();

    // Step 1: Compile schema to OIR
    let oir = compile_file_with_source(schema_path, source_path, &options.compiler)?;
    let split: SplitProgram = {
        let _span = tracing::info_span!(target: ns::COMPILER, "resolve_split").entered();
        crate::phases::lazy::resolve_program_with_path(
            parse_file(schema_path).map_err(crate::phases::CompilerError::from)?,
            &options.compiler,
            Some(source_path),
        )?
    };

    // Generate code from OIR
    let result = write(&oir, &split, output_dir)?;
    let (resolved, lazy) = &split;
    let program_dir = source_path.parent().unwrap_or(Path::new("."));

    // A program with `locales` ships one merged catalog per locale, where
    // the generated App imports it.
    let catalogs = if oir.locales.is_empty() {
        std::collections::BTreeMap::new()
    } else {
        let catalogs = crate::phases::messages::program_catalogs(resolved, lazy, program_dir);
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
    tracing::info!(target: ns::COMPILER, path = %dest_schema.display(), "copied schema");

    // Step 7: `resolved.orb` (post Inline + Compose + Enrich, every import resolved) and each lazy behavior beside
    // it: the bundled view downstream tools parse as an `OrbitalSchema`, from the same resolution as the catalogs.
    crate::phases::lazy::write_lazy(output_dir, lazy, program_dir, &|schema| {
        orbital_core_eval::util::canonical_json::to_canonical_string_pretty(schema).map_err(std::io::Error::other)
    })?;
    let resolved_path = output_dir.join("resolved.orb");
    std::fs::write(
        &resolved_path,
        orbital_core_eval::util::canonical_json::to_canonical_string_pretty(resolved).map_err(std::io::Error::other)?,
    )?;
    tracing::info!(target: ns::COMPILER, path = %resolved_path.display(), "wrote resolved.orb");

    Ok(result)
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
    oir: &orbital_core_base::ir::OirModule,
    catalogs: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
) -> Result<serde_json::Value, String> {
    use orbital_core_base::schema::{PageAccess, PageIndexing, PageMessage, PageMeta};
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
        .find(|p| orbital_core_base::schema::is_host_sign_in_route(&p.path))
    {
        return Err(format!(
            "ORB_P_HOST_RESERVED_ROUTE: page {} is declared on route {}, the host's own sign-in route",
            page.name, page.path
        ));
    }
    if let Some((group, locale, paths)) = orbital_core_base::messages::duplicate_translation_locales(
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
            obj.insert("dir".into(), orbital_core_base::messages::locale_direction(l).into());
        }
        if page.is_not_found {
            obj.insert("notFound".into(), true.into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::BaseShell;
    use orbital_core_base::ir::OirModule;

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
        let mut invoice = orbital_core_base::ir::OirEntity::new("Invoice");
        invoice.browser = true;
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

    fn module_calling(service: &str) -> OirModule {
        use orbital_core_base::ir::{OirEffect, OirExpr, OirState, OirTrait, OirTransition};
        let mut t = OirTrait::new("Caller");
        t.states.push(OirState::initial("idle"));
        t.transitions.push(OirTransition::new("idle", "idle", "INIT").with_effects(vec![OirExpr::effect(
            OirEffect::call_service(service, "run", OirExpr::null()),
        )]));
        let mut module = OirModule::new("Test", "0.1.0");
        module.traits.push(t);
        module
    }

    fn generate_calling(service: &str, mode: ShellMode, browser_storage: bool) -> (Result<GeneratedOutput, String>, bool) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let runtimes = [("analytics", vec!["browser"]), ("github", vec!["node"]), ("llm", vec!["node", "browser"])]
            .into_iter()
            .map(|(n, r)| (n.to_string(), r.into_iter().map(String::from).collect()))
            .collect();
        let options = GenerateOptions {
            copy_template: false,
            clean: false,
            mode,
            compiler: crate::CompilerOptions { service_runtimes: Some(runtimes), ..Default::default() },
            ..Default::default()
        };
        let result = generate_from_oir(&module_calling(service), tmp.path(), &fixture_shell("generated"), &CapabilityBackend { browser_storage }, &options)
            .map_err(|e| e.to_string());
        (result, tmp.path().join("generated/a.ts").exists())
    }

    #[test]
    fn a_full_build_without_a_browser_leg_refuses_a_browser_only_service() {
        let (result, written) = generate_calling("analytics", ShellMode::Full, false);
        let err = result.unwrap_err();
        assert!(err.contains("ORB_SHELL_SERVICE_RUNTIME_UNSUPPORTED") && err.contains("analytics"), "{err}");
        assert!(!written, "nothing is written");
    }

    #[test]
    fn control_a_full_build_with_a_browser_leg_hosts_it() {
        assert!(generate_calling("analytics", ShellMode::Full, true).0.is_ok());
    }

    #[test]
    fn control_node_services_and_split_builds_are_not_judged() {
        assert!(generate_calling("github", ShellMode::Full, false).0.is_ok());
        assert!(generate_calling("llm", ShellMode::Full, false).0.is_ok(), "node+browser runs on the server");
        assert!(generate_calling("analytics", ShellMode::Client, false).0.is_ok(), "the other half of a split deploy may host it");
        assert!(generate_calling("github", ShellMode::Client, true).0.is_ok(), "a client half posts to its server half");
        assert!(generate_calling("analytics", ShellMode::Server, false).0.is_ok());
        assert!(generate_calling("custom-bearer-api", ShellMode::Full, false).0.is_ok(), "an unregistered service is validation's business");
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
            &std::fs::read_to_string(output_dir.join(orbital_core_base::project::ORB_MANIFEST_FILE)).unwrap(),
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
        assert!(output_dir.join(orbital_core_base::project::ORB_MANIFEST_FILE).exists());
    }
}
