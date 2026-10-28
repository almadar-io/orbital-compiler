//! External Orbital Loader
//!
//! Loads external .orb files from various sources:
//! - Local filesystem (relative paths)
//! - Standard library (std/...)
//! - Scoped packages (@name/...)

use orbital_core_base::behavior_packages::{BehaviorPackage, DiscoverError, RegistrySource};
pub use orbital_core_base::behavior_packages::index_registry_source;
use orbital_core_base::schema::OrbitalSchema;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use thiserror::Error;

/// Canonicalize a path for use as a cycle-detection / cache key.
///
/// Returns the original path when canonicalization fails (e.g. the file does
/// not exist yet, as with synthetic `<embedded>` paths). When the path *does*
/// exist but canonicalization still fails (permission denied, symlink loop),
/// the fallback key can diverge from another reference to the same file and
/// silently break cycle detection or cache hits — so we warn loudly instead of
/// swallowing the error.
fn canonical_key(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(err) => {
            if path.exists() {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "canonicalize failed for an existing path; falling back to the \
                     non-canonical path, which may break cycle detection or cache keys"
                );
            }
            path.to_path_buf()
        }
    }
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error(transparent)]
    BehaviorPackage(#[from] DiscoverError),
    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Invalid JSON in {0}: {1}")]
    InvalidJson(String, String),

    #[error("Failed to lower {0}: {1}")]
    SourceLowering(String, String),

    #[error("Invalid schema in {0}: {1}")]
    InvalidSchema(String, String),

    #[error("Circular import detected: {0}")]
    CircularImport(String),

    #[error("Path traversal outside allowed directory: {0}")]
    PathTraversal(String),

    #[error("Standard library path not configured")]
    StdLibNotConfigured,

    #[error(
        "Unknown std import path '{given}' — this behavior exists as '{canonical}'. \
         Std imports use the flat canonical form std/behaviors/<name>; topic/kind \
         folders from the browse tree never appear in import paths."
    )]
    StdImportNotCanonical { given: String, canonical: String },

    #[error("Scoped package not configured: {0}")]
    ScopedPackageNotConfigured(String),

    #[error(
        "Unresolvable primitive import '{0}'. Accepted forms:\n  \
         loader:  std/behaviors/<name>  (e.g. std/behaviors/std-stats)  |  \
         almadar-behaviors/<name>  (e.g. almadar-behaviors/rpg-hero)\n  \
         browse:  @std/<topic>/<kind>/<name>.lolo  (e.g. @std/ui/core/atoms/std-stats.lolo)  |  \
         @behaviors/<topic>/<kind>/<name>.lolo  (e.g. @behaviors/app/molecules/app-crud-manager.lolo)"
    )]
    InvalidPrimitiveImport(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error(
        "Behavior package with prefix '{prefix}' is not installed. Run `orb add \
         <package>` for the package that declares orb.prefix = \"{prefix}\"."
    )]
    PackageNotInstalled { prefix: String },

    #[error("{}", orbital_core_base::behavior_packages::not_installed_message(package))]
    BehaviorPackageNotInstalled { package: String },

    #[error("Behavior '{name}' not found in {searched:?}.")]
    BehaviorNotFound { name: String, searched: Vec<String> },

    #[error(
        "Behavior '{name}' is ambiguous: declared by both installed packages '{package_a}' and \
         '{package_b}'. Resolve it via its explicit '<prefix>/{name}' form."
    )]
    BehaviorAmbiguous {
        name: String,
        package_a: String,
        package_b: String,
    },

    #[error(
        "Behavior prefix '{prefix}' is declared by both {package_a} ({path_a:?}) and \
         {package_b} ({path_b:?})"
    )]
    PrefixConflict {
        prefix: String,
        package_a: String,
        path_a: PathBuf,
        package_b: String,
        path_b: PathBuf,
    },
}

// ============================================================================
// Types
// ============================================================================

/// Loader configuration
#[derive(Debug, Clone)]
pub struct LoaderConfig {
    /// Base directory for resolving relative imports
    pub base_path: PathBuf,

    /// Standard library root path
    pub std_lib_path: Option<PathBuf>,

    /// Scoped package roots (e.g., { "@game-lib": "/path/to/lib" })
    pub scoped_paths: HashMap<String, PathBuf>,

    /// Whether to allow paths outside base_path
    pub allow_outside_base_path: bool,

    /// Installed behavior packages, discovered node-style from a working
    /// directory (`orbital_core::behavior_packages::discover`). Resolution
    /// (Almadar_Behavior_Packages.md ruling 3): `<prefix>/<name>` resolves
    /// against the package declaring that `orb.prefix`; `std/behaviors/<name>`
    /// prefers an installed `almadar-std`-prefixed package (else the embedded
    /// phf map), then on a miss searches every OTHER package in this list.
    pub packages: Vec<BehaviorPackage>,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("."),
            std_lib_path: None,
            scoped_paths: HashMap::new(),
            allow_outside_base_path: false,
            packages: Vec::new(),
        }
    }
}

/// Loaded schema with metadata
#[derive(Debug, Clone)]
pub struct LoadedSchema {
    /// The loaded schema
    pub schema: OrbitalSchema,

    /// Absolute source path
    pub source_path: PathBuf,

    /// Original import path
    pub import_path: String,
}

// ============================================================================
// Import Chain (Circular Detection)
// ============================================================================

/// Tracks import chains to detect circular imports
pub struct ImportChain {
    chain: Vec<PathBuf>,
}

impl ImportChain {
    pub fn new() -> Self {
        Self { chain: Vec::new() }
    }

    /// Try to add a path to the chain
    /// Returns error message if circular, None if OK
    pub fn push(&mut self, path: &Path) -> Option<String> {
        self.push_canonical(canonical_key(path))
    }

    /// [`Self::push`] for a path the caller already canonicalized.
    fn push_canonical(&mut self, canonical: PathBuf) -> Option<String> {
        if self.chain.contains(&canonical) {
            let cycle: Vec<String> = self
                .chain
                .iter()
                .skip_while(|p| **p != canonical)
                .map(|p| p.display().to_string())
                .chain(std::iter::once(canonical.display().to_string()))
                .collect();
            return Some(format!("Circular import: {}", cycle.join(" -> ")));
        }
        self.chain.push(canonical);
        None
    }

    /// Remove the last path from the chain
    pub fn pop(&mut self) {
        self.chain.pop();
    }

    /// Clone for nested loading
    pub fn clone_chain(&self) -> Self {
        Self {
            chain: self.chain.clone(),
        }
    }
}

impl Default for ImportChain {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Cache
// ============================================================================

/// `to` relative to `from` (both canonical), `.` when equal.
fn relative_dir(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_string(), from.len() - common).collect();
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Cache for loaded schemas
pub struct LoaderCache {
    cache: HashMap<PathBuf, LoadedSchema>,
}

impl LoaderCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    pub fn get(&self, path: &Path) -> Option<&LoadedSchema> {
        let canonical = path.canonicalize().ok()?;
        self.cache.get(&canonical)
    }

    pub fn insert(&mut self, path: PathBuf, schema: LoadedSchema) {
        let key = canonical_key(&path);
        self.cache.insert(key, schema);
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.cache.contains_key(&canonical_key(path))
    }

    /// Return the cached schema for `path`, computing and inserting it with
    /// `produce` on a miss. Single canonicalization, single hash lookup via the
    /// Entry API — no contains-then-get round trip.
    pub fn get_or_insert_with<E>(
        &mut self,
        path: &Path,
        produce: impl FnOnce() -> Result<LoadedSchema, E>,
    ) -> Result<&LoadedSchema, E> {
        match self.cache.entry(canonical_key(path)) {
            Entry::Occupied(e) => Ok(e.into_mut()),
            Entry::Vacant(e) => Ok(e.insert(produce()?)),
        }
    }

    fn get_canonical(&self, canonical: &Path) -> Option<&LoadedSchema> {
        self.cache.get(canonical)
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Iterate the loaded schemas (filesystem-resolved imports).
    pub fn schemas(&self) -> impl Iterator<Item = &OrbitalSchema> {
        self.cache.values().map(|l| &l.schema)
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

impl Default for LoaderCache {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Behavior-package registry resolution
// ============================================================================

pub fn index_registry(root: &Path) -> Result<HashMap<String, PathBuf>, DiscoverError> {
    index_registry_source(&RegistrySource::Materialized(root.to_path_buf()))
}

/// Recursively search `root` for a behavior named `name` (file `<name>.orb`).
/// One full walk per call — `ExternalOrbitalLoader` keeps a per-root index
/// instead, so it walks each root once per compile rather than once per
/// import hop.
pub fn resolve_behavior_in_registry(
    name: &str,
    source: &RegistrySource,
) -> Result<Option<PathBuf>, DiscoverError> {
    Ok(index_registry_source(source)?.remove(name))
}

// ============================================================================
// Browse-form specifier mapping (FC-3)
// ============================================================================

/// Outcome of mapping a `uses ... from "<specifier>"` string to its canonical
/// loader form.
pub enum BrowseMapping {
    /// Not a browse-form specifier — use the original path unchanged.
    NotBrowseForm,
    /// A well-formed browse specifier mapped to its canonical loader form.
    Mapped(String),
    /// Browse-prefixed but malformed (no primitive name after the alias).
    Malformed,
}

/// Map a browse-form primitive specifier — the mount paths the free-compose
/// agent reads (`@std/…`, `@behaviors/…`) — to the canonical loader specifier.
///
/// Deterministic pure string transform: strip the mount alias and the
/// intermediate topic/kind path, drop a trailing `.lolo`, and route to the
/// owning registry. `@std/<...>/<name>.lolo` → `std/behaviors/<name>` (embedded
/// std registry); `@behaviors/<...>/<name>.lolo` → `almadar-behaviors/<name>`
/// (behaviors package registry). No fuzzy matching — the primitive name is the
/// path basename verbatim. A browse-prefixed specifier with no basename is
/// `Malformed`; anything not starting with a mount alias is `NotBrowseForm`.
pub fn map_browse_specifier(import_path: &str) -> BrowseMapping {
    for (prefix, package) in [
        ("@std/", "std/behaviors/"),
        ("@behaviors/", "almadar-behaviors/"),
    ] {
        let Some(rest) = import_path.strip_prefix(prefix) else {
            continue;
        };
        let basename = rest.rsplit('/').next().unwrap_or("");
        let name = basename.strip_suffix(".lolo").unwrap_or(basename);
        if name.is_empty() {
            return BrowseMapping::Malformed;
        }
        return BrowseMapping::Mapped(format!("{package}{name}"));
    }
    BrowseMapping::NotBrowseForm
}

// ============================================================================
// External Orbital Loader
// ============================================================================

/// Lowers a `.lolo` source file to a schema. orbital-compiler cannot depend on
/// orbital-lolo (they are siblings), so the binary that links both registers it.
pub type SourceLowerer = fn(&Path, &mut ExternalOrbitalLoader, &mut ImportChain) -> Result<OrbitalSchema, String>;

static SOURCE_LOWERER: OnceLock<SourceLowerer> = OnceLock::new();

/// Register the `.lolo` lowerer once per process. A relative import whose
/// `<name>.orb` does not exist then loads `<name>.lolo` beside it (G-ORB-056).
pub fn register_source_lowerer(lowerer: SourceLowerer) {
    let _ = SOURCE_LOWERER.set(lowerer);
}

/// A `.lolo` file lowered by the registered lowerer, stamped.
pub(crate) fn lower_source(path: &Path) -> Result<OrbitalSchema, String> {
    let mut loader = ExternalOrbitalLoader::new(LoaderConfig {
        base_path: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        ..Default::default()
    });
    loader.load_from_path(path.to_path_buf(), &path.to_string_lossy(), &mut ImportChain::new())
        .map(|loaded| loaded.schema.clone()).map_err(|error| error.to_string())
}

#[derive(Debug, Clone)]
pub struct SourceTraitOrigin {
    pub root_callsite: Option<(usize, String)>,
    pub trait_id: String,
    pub source_path: Option<PathBuf>,
}

pub type SourceTraitOrigins = HashMap<(PathBuf, String), SourceTraitOrigin>;

/// ExternalOrbitalLoader - Loads external .orb files with security and caching
pub struct ExternalOrbitalLoader {
    #[cfg(test)]
    source_lowerer_override: Option<SourceLowerer>,
    trait_origins: std::rc::Rc<RefCell<SourceTraitOrigins>>,
    config: LoaderConfig,
    cache: LoaderCache,
    /// Behavior name → `.orb` path per registry root, built by one walk of the
    /// root on its first lookup. Registry trees hold ~1.2k files; walking them
    /// on every `uses` hop made a project-friday resolve spend >80% of its
    /// wall time in `stat`/`getdirentries` (Compiler_Gaps §106).
    registry_index: RefCell<HashMap<PathBuf, HashMap<String, PathBuf>>>,
    /// Path → canonical path, once per path: canonicalizing on every load was
    /// a large share of a big program's resolve (`getattrlist` per hop).
    canonical_paths: HashMap<PathBuf, PathBuf>,
    /// The files the current schema loaded. One loader can serve several
    /// schemas (a program and its lazy behaviors); `begin_schema` resets this,
    /// so per-schema answers (message sources, ledger union) see only them.
    touched: HashSet<PathBuf>,
    /// Open recordings: every load is also added to each, so a caller can learn
    /// which files one piece of work loaded (`begin_recording`/`end_recording`).
    recordings: Vec<HashSet<PathBuf>>,
}

impl ExternalOrbitalLoader {
    pub fn new(config: LoaderConfig) -> Self {
        Self {
            #[cfg(test)]
            source_lowerer_override: None,
            trait_origins: Default::default(),
            config,
            cache: LoaderCache::new(),
            registry_index: RefCell::new(HashMap::new()),
            canonical_paths: HashMap::new(),
            touched: HashSet::new(),
            recordings: Vec::new(),
        }
    }

    pub(crate) fn trait_origins(&self) -> std::rc::Rc<RefCell<SourceTraitOrigins>> {
        self.trait_origins.clone()
    }

    /// Start a new schema on this loader: the files loaded so far stay cached,
    /// but no longer count as this schema's.
    pub fn begin_schema(&mut self) {
        self.touched.clear();
    }

    /// Start recording which files are loaded until the matching [`Self::end_recording`].
    pub fn begin_recording(&mut self) {
        self.recordings.push(HashSet::new());
    }

    /// The files loaded since the matching [`Self::begin_recording`].
    pub fn end_recording(&mut self) -> HashSet<PathBuf> {
        self.recordings.pop().unwrap_or_default()
    }

    /// Count files as loaded by the current schema (and any open recording)
    /// without loading them again — they come from a reused result.
    pub fn mark_loaded(&mut self, files: &HashSet<PathBuf>) {
        for file in files {
            self.note_loaded(file);
        }
    }

    fn note_loaded(&mut self, canonical: &Path) {
        self.touched.insert(canonical.to_path_buf());
        for recording in &mut self.recordings {
            recording.insert(canonical.to_path_buf());
        }
    }

    /// The canonical form of `path`, computed once per path for this loader.
    pub fn canonical(&mut self, path: &Path) -> PathBuf {
        if let Some(canonical) = self.canonical_paths.get(path) {
            return canonical.clone();
        }
        let canonical = canonical_key(path);
        self.canonical_paths
            .insert(path.to_path_buf(), canonical.clone());
        canonical
    }

    /// `<name>.orb` under `root`, through the per-root index.
    fn registry_lookup(&self, name: &str, source: &RegistrySource) -> Result<Option<PathBuf>, LoaderError> {
        let mut indexes = self.registry_index.borrow_mut();
        let index = match indexes.entry(source.root().to_path_buf()) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(index_registry_source(source)?),
        };
        Ok(index.get(name).cloned())
    }

    /// Load `absolute_path` from the filesystem into `self.cache`, keyed by
    /// its canonicalized path. Shared tail for every disk-resolved import
    /// (relative/scoped/std-lib paths and both behavior-package forms).
    fn load_from_path(
        &mut self,
        absolute_path: PathBuf,
        import_path: &str,
        chain: &mut ImportChain,
    ) -> Result<&LoadedSchema, LoaderError> {
        let source = absolute_path.with_extension("lolo");
        let absolute_path = if !absolute_path.exists() && source.exists() && self.source_lowerer().is_some() {
            source
        } else {
            absolute_path
        };
        let canonical = self.canonical(&absolute_path);
        if let Some(error) = chain.push_canonical(canonical.clone()) {
            return Err(LoaderError::CircularImport(error));
        }
        let loaded: Result<(), LoaderError> = (|| {
            if self.cache.get_canonical(&canonical).is_none() {
                let schema = self.load_file(&absolute_path, import_path, chain)?;
                self.cache.insert(canonical.clone(), schema);
            }
            Ok(())
        })();
        chain.pop();
        loaded?;
        self.note_loaded(&canonical);
        Ok(self
            .cache
            .get_canonical(&canonical)
            .expect("inserted above"))
    }

    /// `std/behaviors/<name>` resolution (Almadar_Behavior_Packages.md Phase
    /// 2, ruling 3): the installed `almadar-std`-prefixed package is the ONLY
    /// std source now (no embedded fallback) — its absence is a hard,
    /// actionable error, never a silent substitute. A miss on the installed
    /// std package's own registry is NEVER retried against std again, only
    /// against every other installed package.
    fn load_std_behavior(
        &mut self,
        import_path: &str,
        behavior_name: &str,
        chain: &mut ImportChain,
    ) -> Result<&LoadedSchema, LoaderError> {
        let std_package = self
            .config
            .packages
            .iter()
            .find(|p| p.prefix == "almadar-std")
            .cloned();

        let Some(pkg) = &std_package else {
            return Err(LoaderError::BehaviorPackageNotInstalled {
                package: orbital_core_base::behavior_packages::STD_PACKAGE.to_string(),
            });
        };
        let std_source_label = format!("{}@{}", pkg.name, pkg.version);
        if let Some(path) = self.registry_lookup(behavior_name, &pkg.registry)? {
            return self.load_from_path(path, import_path, chain);
        }

        // Miss on the std source: search every OTHER installed package.
        let mut hits: Vec<(String, PathBuf)> = Vec::new();
        for pkg in &self.config.packages {
            if pkg.prefix == "almadar-std" {
                continue;
            }
            if let Some(path) = self.registry_lookup(behavior_name, &pkg.registry)? {
                hits.push((pkg.name.clone(), path));
            }
        }

        match hits.len() {
            0 => {
                let mut searched = vec![std_source_label];
                searched.extend(
                    self.config
                        .packages
                        .iter()
                        .filter(|p| p.prefix != "almadar-std")
                        .map(|p| format!("{}@{}", p.name, p.version)),
                );
                Err(LoaderError::BehaviorNotFound {
                    name: behavior_name.to_string(),
                    searched,
                })
            }
            1 => {
                let (_, path) = hits.into_iter().next().expect("len checked above");
                self.load_from_path(path, import_path, chain)
            }
            _ => {
                let mut names = hits.into_iter().map(|(name, _)| name);
                Err(LoaderError::BehaviorAmbiguous {
                    name: behavior_name.to_string(),
                    package_a: names.next().expect("len >= 2"),
                    package_b: names.next().expect("len >= 2"),
                })
            }
        }
    }

    /// Bare `<prefix>/<name>` resolution: the package declaring `orb.prefix ==
    /// prefix` only — never any other package, never std.
    fn load_scoped_behavior(
        &mut self,
        import_path: &str,
        chain: &mut ImportChain,
    ) -> Result<&LoadedSchema, LoaderError> {
        let (prefix, name) = import_path
            .split_once('/')
            .expect("caller verified a '/' is present");
        let pkg = self
            .config
            .packages
            .iter()
            .find(|p| p.prefix == prefix)
            .cloned();
        let Some(pkg) = pkg else {
            return Err(LoaderError::PackageNotInstalled {
                prefix: prefix.to_string(),
            });
        };
        match self.registry_lookup(name, &pkg.registry)? {
            Some(path) => self.load_from_path(path, import_path, chain),
            None => Err(LoaderError::BehaviorNotFound {
                name: name.to_string(),
                searched: vec![format!("{}@{}", pkg.name, pkg.version)],
            }),
        }
    }

    /// Load a schema from an import path
    pub fn load(
        &mut self,
        import_path: &str,
        from_path: Option<&Path>,
        chain: &mut ImportChain,
    ) -> Result<&LoadedSchema, LoaderError> {
        // FC-3: accept the browse-form specifier the free-compose agent reads
        // (`@std/…`, `@behaviors/…`) by mapping it to the canonical loader form
        // BEFORE embedded/filesystem resolution, so both namespaces converge.
        let mapped = map_browse_specifier(import_path);
        let import_path = match &mapped {
            BrowseMapping::NotBrowseForm => import_path,
            BrowseMapping::Mapped(canonical) => canonical.as_str(),
            BrowseMapping::Malformed => {
                return Err(LoaderError::InvalidPrimitiveImport(import_path.to_string()))
            }
        };

        // std/behaviors/<name>: installed std (if present) is authoritative,
        // else the embedded phf map, else every other installed package.
        if let Some(behavior_name) = import_path.strip_prefix("std/behaviors/") {
            return self.load_std_behavior(import_path, behavior_name, chain);
        }

        // Bare `<prefix>/<name>` against an installed package — not std/, @,
        // ./, ../, or absolute.
        if import_path.contains('/')
            && !import_path.starts_with("./")
            && !import_path.starts_with("../")
            && !import_path.starts_with('@')
            && !import_path.starts_with("std/")
            && !Path::new(import_path).is_absolute()
        {
            return self.load_scoped_behavior(import_path, chain);
        }

        // Everything else: filesystem resolution (relative / scoped / absolute
        // / topic-qualified `std/...` paths that aren't the canonical form).
        let absolute_path = self.resolve_path(import_path, from_path)?;
        self.load_from_path(absolute_path, import_path, chain)
    }

    /// The `orb.prefix` of the installed package whose registry holds `path`.
    pub fn owning_prefix(&self, path: &Path) -> Option<&str> {
        self.config
            .packages
            .iter()
            .find(|pkg| path.starts_with(pkg.registry.root()))
            .map(|pkg| pkg.prefix.as_str())
    }

    /// Resolve `import_path` exactly as `load` would (behavior-package forms
    /// only — `std/behaviors/<name>` or `<prefix>/<name>`), for `orb behaviors
    /// which`: "which copy compiled?" without a `from_path` context. Returns
    /// the resolved absolute path plus a label for the owning package
    /// (`"<name>@<version>"`, or the local path when it resolved outside any
    /// installed package's registry).
    pub fn resolve_owned(&mut self, import_path: &str) -> Result<(PathBuf, String), LoaderError> {
        let mut chain = ImportChain::new();
        let path = self
            .load(import_path, None, &mut chain)?
            .source_path
            .clone();
        let owner = self
            .config
            .packages
            .iter()
            .find(|pkg| path.starts_with(pkg.registry.root()))
            .map(|pkg| format!("{}@{}", pkg.name, pkg.version))
            .unwrap_or_else(|| path.display().to_string());
        Ok((path, owner))
    }

    /// Resolve an import path to an absolute filesystem path
    pub fn resolve_path(
        &self,
        import_path: &str,
        from_path: Option<&Path>,
    ) -> Result<PathBuf, LoaderError> {

        // Standard library
        if import_path.starts_with("std/") {
            return self.resolve_std_path(import_path);
        }

        // Scoped packages
        if import_path.starts_with('@') {
            return self.resolve_scoped_path(import_path);
        }

        // Relative paths
        if import_path.starts_with("./") || import_path.starts_with("../") {
            return self.resolve_relative_path(import_path, from_path);
        }

        // Absolute paths
        let path = Path::new(import_path);
        if path.is_absolute() {
            if !self.config.allow_outside_base_path {
                return Err(LoaderError::PathTraversal(import_path.to_string()));
            }
            return Ok(self.add_orb_extension(path.to_path_buf()));
        }

        // A bare `<package>/<behavior-name>` import is resolved by `load()`
        // before `resolve_path` is ever reached (see `load_scoped_behavior`),
        // so it never falls through to relative-path resolution here.

        // Default: treat as relative to base path
        self.resolve_relative_path(&format!("./{}", import_path), from_path)
    }

    fn resolve_std_path(&self, import_path: &str) -> Result<PathBuf, LoaderError> {
        let std_lib_path = match self.config.std_lib_path.as_ref() {
            Some(p) => p,
            // No on-disk std lib is configured, so this `std/...` specifier has
            // fallen past every canonical resolution surface (dev registry
            // roots, the installed std package). Distinguish the AUTHORING
            // mistakes from the genuine environment gap before erroring —
            // StdLibNotConfigured is classified as validator INFRA downstream
            // ("do not edit, retry"), which turns a fixable bad import into an
            // unfixable dead end (battery 2026-07-20 free-lolo-infra-approval:
            // `std/flow/atoms/std-approval-gate` retried 9× to no avail).
            None => {
                let basename = {
                    let b = import_path.rsplit('/').next().unwrap_or("");
                    b.strip_suffix(".lolo")
                        .or_else(|| b.strip_suffix(".orb"))
                        .unwrap_or(b)
                };
                let canonical = format!("std/behaviors/{basename}");
                // Canonical-form detection now asks every installed package's
                // registry (including std's, when installed) — there is no
                // embedded phf map left to consult.
                let mut known_canonically = false;
                if canonical != import_path {
                    for pkg in &self.config.packages {
                        if self.registry_lookup(basename, &pkg.registry)?.is_some() {
                            known_canonically = true;
                            break;
                        }
                    }
                }
                if known_canonically {
                    return Err(LoaderError::StdImportNotCanonical {
                        given: import_path.to_string(),
                        canonical,
                    });
                }
                // No installed std package: the loader has no source of
                // truth left to classify this as authoring-error vs infra —
                // install std and retry (Phase 2, Almadar_Behavior_Packages.md).
                if !self
                    .config
                    .packages
                    .iter()
                    .any(|p| p.prefix == "almadar-std")
                {
                    return Err(LoaderError::BehaviorPackageNotInstalled {
                        package: orbital_core_base::behavior_packages::STD_PACKAGE.to_string(),
                    });
                }
                // std is installed (packages is therefore non-empty): the
                // full searchable universe (ruling 3) was consulted and
                // missed — the behavior does not exist under any accepted
                // form. An authoring error, not infra.
                return Err(LoaderError::InvalidPrimitiveImport(import_path.to_string()));
            }
        };

        // std/behaviors/game-core -> std_lib_path/behaviors/game-core.orb
        let relative_path = &import_path[4..]; // Remove "std/"
        let absolute_path = std_lib_path.join(relative_path);
        let absolute_path = self.add_orb_extension(absolute_path);

        // Validate it's within std library
        self.validate_path_within(&absolute_path, std_lib_path, import_path)?;

        Ok(absolute_path)
    }

    fn resolve_scoped_path(&self, import_path: &str) -> Result<PathBuf, LoaderError> {
        // Extract scope: @game-lib/enemies.orb -> @game-lib
        let scope_end = import_path.find('/').unwrap_or(import_path.len());
        let scope = &import_path[..scope_end];

        let scope_root = self
            .config
            .scoped_paths
            .get(scope)
            .ok_or_else(|| LoaderError::ScopedPackageNotConfigured(scope.to_string()))?;

        // @game-lib/enemies.orb -> scope_root/enemies.orb
        let relative_path = &import_path[scope_end + 1..];
        let absolute_path = scope_root.join(relative_path);
        let absolute_path = self.add_orb_extension(absolute_path);

        // Validate it's within scope root
        self.validate_path_within(&absolute_path, scope_root, import_path)?;

        Ok(absolute_path)
    }

    fn resolve_relative_path(
        &self,
        import_path: &str,
        from_path: Option<&Path>,
    ) -> Result<PathBuf, LoaderError> {
        let base_dir = from_path
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.config.base_path.clone());

        let absolute_path = base_dir.join(import_path);
        let absolute_path = self.add_orb_extension(absolute_path);

        // Security check: ensure within base path
        if !self.config.allow_outside_base_path {
            self.validate_path_within(&absolute_path, &self.config.base_path, import_path)?;
        }

        Ok(absolute_path)
    }

    fn validate_path_within(
        &self,
        path: &Path,
        allowed_root: &Path,
        import_path: &str,
    ) -> Result<(), LoaderError> {
        // Normalize both paths by resolving . and .. components
        let normalized = self.normalize_path(path);
        let root_normalized = self.normalize_path(allowed_root);

        // Check if normalized path starts with root
        if !normalized.starts_with(&root_normalized) {
            return Err(LoaderError::PathTraversal(import_path.to_string()));
        }

        Ok(())
    }

    /// Normalize a path by resolving . and .. components without requiring file to exist
    fn normalize_path(&self, path: &Path) -> PathBuf {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::CurDir => {
                    // Skip current dir
                }
                other => {
                    normalized.push(other);
                }
            }
        }
        normalized
    }

    fn add_orb_extension(&self, path: PathBuf) -> PathBuf {
        if path.extension().is_none_or(|ext| ext != "orb") {
            path.with_extension("orb")
        } else {
            path
        }
    }

    fn source_lowerer(&self) -> Option<SourceLowerer> {
        #[cfg(test)]
        if self.source_lowerer_override.is_some() {
            return self.source_lowerer_override;
        }
        SOURCE_LOWERER.get().copied()
    }

    fn lower_source_file(&mut self, path: &Path, chain: &mut ImportChain) -> Result<OrbitalSchema, String> {
        let lower = self.source_lowerer()
            .ok_or_else(|| "no .lolo lowerer is registered in this process".to_string())?;
        let mut schema = lower(path, self, chain)?;
        orbital_core_base::stamp::ensure_stamped(&mut schema);
        Ok(schema)
    }

    fn load_file(&mut self, path: &Path, import_path: &str, chain: &mut ImportChain) -> Result<LoadedSchema, LoaderError> {
        // A live package's behavior is its `.lolo` source, lowered on read.
        if path.extension().is_some_and(|e| e == "lolo") {
            let schema = self.lower_source_file(path, chain)
                .map_err(|e| LoaderError::SourceLowering(path.display().to_string(), e))?;
            return Ok(LoadedSchema {
                schema,
                source_path: path.to_path_buf(),
                import_path: import_path.to_string(),
            });
        }
        if !path.exists() {
            // An `.orb` beside the importer wins; else its `.lolo` source, lowered.
            let source = path.with_extension("lolo");
            if self.source_lowerer().is_some() && source.exists() {
                let mut schema = self.lower_source_file(&source, chain)
                    .map_err(|e| LoaderError::SourceLowering(source.display().to_string(), e))?;
                orbital_core_base::stamp::ensure_stamped(&mut schema);
                return Ok(LoadedSchema {
                    schema,
                    source_path: source,
                    import_path: import_path.to_string(),
                });
            }
            return Err(LoaderError::FileNotFound(path.display().to_string()));
        }

        // Read file
        let content = std::fs::read_to_string(path)?;

        // Deserialize directly into the schema (single parse).
        let mut schema: OrbitalSchema = serde_json::from_str(&content)
            .map_err(|e| LoaderError::InvalidJson(path.display().to_string(), e.to_string()))?;
        orbital_core_base::stamp::ensure_stamped(&mut schema);

        Ok(LoadedSchema {
            schema,
            source_path: path.to_path_buf(),
            import_path: import_path.to_string(),
        })
    }

    /// Clear the cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.registry_index.borrow_mut().clear();
    }

    /// Get cache size
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Every behavior this loader loaded (plus the program at `root` itself,
    /// named `root_name`): name → the folder of its source file, relative to
    /// the program's folder. A program's message catalogs are found there.
    pub fn message_sources(
        &self,
        root: &Path,
        root_name: &str,
    ) -> std::collections::BTreeMap<String, String> {
        let base = root.parent().map(canonical_key).unwrap_or_default();
        let mut sources = std::collections::BTreeMap::new();
        sources.insert(root_name.to_string(), ".".to_string());
        for loaded in self
            .touched
            .iter()
            .filter_map(|p| self.cache.get_canonical(p))
        {
            if let Some(dir) = loaded.source_path.parent() {
                sources.insert(
                    loaded.schema.name.clone(),
                    relative_dir(&base, &canonical_key(dir)),
                );
            }
        }
        sources
    }

    /// Every file the current schema loaded, as parsed (before any inline
    /// substitution), in path order.
    pub fn each_touched_schema(&self, visit: &mut dyn FnMut(&LoadedSchema)) {
        let mut paths: Vec<&PathBuf> = self.touched.iter().collect();
        paths.sort();
        for loaded in paths.into_iter().filter_map(|p| self.cache.get_canonical(p)) {
            visit(loaded);
        }
    }

    /// Union of every loaded import's identity ledger (`id → row`). First
    /// writer wins on id collision (identical baked ids are byte-equal by
    /// construction). Used by the inline phase to preserve cross-file ids: a
    /// cloned node that references a source id gets that source's ledger row
    /// copied into the consumer schema.
    pub fn source_ledger_union(&self) -> HashMap<String, orbital_core_base::identity::LedgerEntry> {
        let mut union: HashMap<String, orbital_core_base::identity::LedgerEntry> = HashMap::new();
        let schemas = self
            .touched
            .iter()
            .filter_map(|p| self.cache.get_canonical(p))
            .map(|l| &l.schema);
        for schema in schemas {
            if let Some(ledger) = &schema.ledger {
                for (id, row) in &ledger.entries {
                    union.entry(id.clone()).or_insert_with(|| row.clone());
                }
            }
        }
        union
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Parse an import path with optional fragment
/// Example: "./game.orb#Player" -> ("./game.orb", Some("Player"))
pub fn parse_import_path(import_path: &str) -> (&str, Option<&str>) {
    if let Some(hash_idx) = import_path.find('#') {
        (&import_path[..hash_idx], Some(&import_path[hash_idx + 1..]))
    } else {
        (import_path, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loader/fixtures")
    }

    #[test]
    fn test_parse_import_path() {
        assert_eq!(parse_import_path("./health.orb"), ("./health.orb", None));
        assert_eq!(
            parse_import_path("./game.orb#Player"),
            ("./game.orb", Some("Player"))
        );
    }

    #[test]
    fn test_import_chain_circular_detection() {
        let mut chain = ImportChain::new();

        // First push should succeed
        assert!(chain.push(Path::new("/a.orb")).is_none());
        assert!(chain.push(Path::new("/b.orb")).is_none());

        // Pushing same path should fail
        assert!(chain.push(Path::new("/a.orb")).is_some());
    }

    #[test]
    fn test_import_chain_allows_revisit_after_pop() {
        let mut chain = ImportChain::new();

        chain.push(Path::new("/a.orb"));
        chain.push(Path::new("/b.orb"));
        chain.pop();
        chain.pop();

        // Should be able to visit again after popping
        assert!(chain.push(Path::new("/a.orb")).is_none());
    }

    #[test]
    fn test_loader_cache() {
        let mut cache = LoaderCache::new();
        assert!(cache.is_empty());

        // Insert and retrieve
        let schema = OrbitalSchema {
            type_contract_version: None,
            resolved: None,            types: None,
            store: None,
            theme_definitions: None,
            themes: None,
            site: None,
            public_dir: None,
            locales: None,
            lazy_pages: None,
            theme: None,
            name: "Test".to_string(),
            version: None,
            description: None,
            orbitals: vec![],
            metadata: None,
            ledger: None,
            schema_version: None,
            config: None,
    egress: None,
    inbound: None,
    os_grants: None,
    deployment: None,
        };
        let loaded = LoadedSchema {
            schema,
            source_path: PathBuf::from("/test.orb"),
            import_path: "./test.orb".to_string(),
        };

        cache.insert(PathBuf::from("/test.orb"), loaded);
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());

        // Clear
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn native_type_import_cycle_keeps_the_active_source_chain() {
        fn lower(path: &Path, loader: &mut ExternalOrbitalLoader, chain: &mut ImportChain) -> Result<OrbitalSchema, String> {
            if chain.push(path).is_none() {
                chain.pop();
                return Err("source callback lost its active import chain".to_string());
            }
            let import = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
            loader.load(import.trim(), Some(path), chain)
                .map(|loaded| loaded.schema.clone()).map_err(|error| error.to_string())
        }
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.lolo");
        let second = root.path().join("second.lolo");
        std::fs::write(&first, "./second.lolo").unwrap();
        std::fs::write(&second, "./first.lolo").unwrap();
        let mut loader = ExternalOrbitalLoader::new(LoaderConfig { base_path: root.path().to_path_buf(), ..Default::default() });
        loader.source_lowerer_override = Some(lower);
        let error = loader.load_from_path(first.clone(), "first.lolo", &mut ImportChain::new()).unwrap_err();
        assert!(error.to_string().contains("Circular import"), "{error}");
        assert!(error.to_string().contains("first.lolo") && error.to_string().contains("second.lolo"), "{error}");
        assert!(loader.cache.is_empty());
    }

    #[test]
    fn test_resolve_relative_path() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("./health.orb", None);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().contains("health.orb"));
    }

    #[test]
    fn test_resolve_adds_orb_extension() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("./health", None);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with(".orb"));
    }

    #[test]
    fn test_resolve_rejects_path_traversal() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            allow_outside_base_path: false,
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("../../etc/passwd", None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LoaderError::PathTraversal(_)));
    }

    #[test]
    fn test_resolve_rejects_absolute_path() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            allow_outside_base_path: false,
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("/etc/passwd", None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LoaderError::PathTraversal(_)));
    }

    #[test]
    fn test_resolve_std_path_with_no_std_installed_reports_not_installed() {
        // No installed std package and no `std_lib_path` configured: the
        // loader has no std source left at all (Phase 2 removed the
        // embedded fallback that used to make this StdLibNotConfigured) —
        // the actionable not-installed error, naming @almadar/std.
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            std_lib_path: None,
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("std/behaviors/game-core", None);
        match result.unwrap_err() {
            LoaderError::BehaviorPackageNotInstalled { package } => {
                assert_eq!(package, "@almadar/std");
            }
            other => panic!("expected BehaviorPackageNotInstalled, got {other:?}"),
        }
    }

    #[test]
    fn test_topic_qualified_std_import_of_known_behavior_is_authoring_error() {
        // `std/<topic>/<kind>/<name>` copied from the browse tree must surface
        // the canonical correction, not the infra-classified StdLibNotConfigured
        // — checked against the installed std package's own registry now that
        // there is no embedded phf map to consult.
        let tmp = tempfile::tempdir().unwrap();
        let std_pkg = fake_package(
            tmp.path(),
            "@almadar/std",
            "almadar-std",
            "16.223.0",
            &["std-modal"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            std_lib_path: None,
            packages: vec![std_pkg],
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("std/ui/core/atoms/std-modal", None);
        match result.unwrap_err() {
            LoaderError::StdImportNotCanonical { given, canonical } => {
                assert_eq!(given, "std/ui/core/atoms/std-modal");
                assert_eq!(canonical, "std/behaviors/std-modal");
            }
            other => panic!("expected StdImportNotCanonical, got {other:?}"),
        }
    }

    #[test]
    fn test_installed_packages_unknown_std_import_is_authoring_error() {
        // std IS installed (so this doesn't short-circuit on
        // BehaviorPackageNotInstalled) alongside another package: the full
        // searchable universe (ruling 3) was consulted and missed — an
        // authoring error, never infra.
        let tmp = tempfile::tempdir().unwrap();
        let std_pkg = fake_package(tmp.path(), "@almadar/std", "almadar-std", "16.223.0", &[]);
        let io_pkg = fake_package(
            tmp.path(),
            "@almadar-io/behaviors",
            "almadar-behaviors",
            "1.0.0",
            &["some-behavior"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            std_lib_path: None,
            packages: vec![std_pkg, io_pkg],
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("std/nowhere/no-such-behavior", None);
        assert!(matches!(
            result.unwrap_err(),
            LoaderError::InvalidPrimitiveImport(_)
        ));
    }

    fn tempdir_registry() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("app/organisms");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(dir.path().join("ui/core/atoms")).unwrap();
        std::fs::write(nested.join("std-realtime-chat.orb"), "{}").unwrap();
        std::fs::write(dir.path().join("ui/core/atoms/std-modal.orb"), "{}").unwrap();
        std::fs::write(dir.path().join("ui/core/atoms/notes.md"), "").unwrap();
        dir
    }

    #[test]
    fn index_registry_maps_every_nested_orb_by_stem() {
        let dir = tempdir_registry();
        let index = index_registry(dir.path()).unwrap();
        assert_eq!(index.len(), 2);
        assert!(index["std-realtime-chat"].ends_with("app/organisms/std-realtime-chat.orb"));
        assert!(index["std-modal"].ends_with("ui/core/atoms/std-modal.orb"));
        let source = RegistrySource::Materialized(dir.path().to_path_buf());
        assert_eq!(
            resolve_behavior_in_registry("std-modal", &source).unwrap(),
            Some(index["std-modal"].clone())
        );
        assert_eq!(resolve_behavior_in_registry("notes", &source).unwrap(), None);
    }

    #[test]
    fn index_registry_refuses_duplicate_names_instead_of_selecting_one() {
        let dir = tempdir_registry();
        std::fs::write(dir.path().join("app/organisms/std-modal.orb"), "{}").unwrap();
        let error = index_registry(dir.path()).unwrap_err();
        assert!(matches!(error, DiscoverError::DuplicateBehaviorName { .. }));
        let source = RegistrySource::Materialized(dir.path().to_path_buf());
        assert!(resolve_behavior_in_registry("std-modal", &source).is_err());
    }

    #[test]
    fn loader_walks_each_registry_root_once_per_lookup_series() {
        let dir = tempdir_registry();
        let pkg = BehaviorPackage {
            name: "@almadar-io/behaviors".to_string(),
            version: "1.0.0".to_string(),
            prefix: "almadar-behaviors".to_string(),
            root: dir.path().to_path_buf(),
            registry: RegistrySource::Materialized(dir.path().to_path_buf()),
            files: Vec::new(),
            lolo_source: None,
            vectors: None,
            sidecars: vec![],
            compiler_range: ">=0.0.0".to_string(),
            std_peer_range: None,
            core_range: None,
            is_link: false,
        };
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![pkg.clone()],
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let first = loader.registry_lookup("std-modal", &pkg.registry).unwrap().unwrap();
        assert!(first.ends_with("ui/core/atoms/std-modal.orb"));
        // A file written after the index was built is invisible to this
        // loader — one walk per root per compile, same freshness as a
        // process that read the directory once.
        std::fs::write(dir.path().join("ui/core/atoms/std-late.orb"), "{}").unwrap();
        assert_eq!(loader.registry_lookup("std-late", &pkg.registry).unwrap(), None);
        assert_eq!(loader.registry_index.borrow().len(), 1);
        let second = loader
            .registry_lookup("std-realtime-chat", &pkg.registry)
            .unwrap()
            .unwrap();
        assert!(second.ends_with("app/organisms/std-realtime-chat.orb"));
        assert_eq!(loader.registry_index.borrow()[pkg.registry.root()].len(), 2);
    }

    /// Writes an installable behavior package under `tmp/node_modules/<name>`
    /// (a real `package.json` `orb` block + one `.orb` file per name in
    /// `behaviors`, each a copy of `fixtures/health.orb` with its top-level
    /// `name` replaced) and returns the `BehaviorPackage` `discover()` resolves
    /// for it — the same struct `LoaderConfig::packages` carries in production.
    fn fake_package(
        tmp: &Path,
        name: &str,
        prefix: &str,
        version: &str,
        behaviors: &[&str],
    ) -> BehaviorPackage {
        let pkg_dir = tmp.join("node_modules").join(name);
        let registry_dir = pkg_dir.join("behaviors/registry");
        std::fs::create_dir_all(&registry_dir).unwrap();
        let health = std::fs::read_to_string(fixtures_dir().join("health.orb")).unwrap();
        for b in behaviors {
            let content = health.replacen("\"HealthModule\"", &format!("\"{b}\""), 1);
            std::fs::write(registry_dir.join(format!("{b}.orb")), content).unwrap();
        }
        let pkg_json = serde_json::json!({
            "name": name,
            "version": version,
            "orb": {
                "prefix": prefix,
                "registry": "behaviors/registry",
                "compiler": ">=0.0.0",
            },
        });
        std::fs::write(
            pkg_dir.join("package.json"),
            serde_json::to_string_pretty(&pkg_json).unwrap(),
        )
        .unwrap();
        // A manifest above node_modules so `discover()` treats `tmp` as a
        // project root rather than falling back to a user store.
        let _ = std::fs::write(tmp.join("package.json"), "{}");

        orbital_core_base::behavior_packages::discover(tmp, &tmp.join("unused-user-store"))
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .expect("fake_package: discover() did not find the package it just wrote")
    }

    #[test]
    fn prefix_hit_loads_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_package(
            tmp.path(),
            "@almadar-io/behaviors",
            "almadar-behaviors",
            "1.0.0",
            &["rpg-hero"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![pkg],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let loaded = loader
            .load("almadar-behaviors/rpg-hero", None, &mut chain)
            .expect("prefix hit should load");
        assert_eq!(loaded.schema.name, "rpg-hero");
    }

    #[test]
    fn unknown_prefix_is_package_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_package(
            tmp.path(),
            "@almadar-io/behaviors",
            "almadar-behaviors",
            "1.0.0",
            &["rpg-hero"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![pkg],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let err = loader
            .load("no-such-package/rpg-hero", None, &mut chain)
            .unwrap_err();
        match err {
            LoaderError::PackageNotInstalled { prefix } => {
                assert_eq!(prefix, "no-such-package");
            }
            other => panic!("expected PackageNotInstalled, got {other:?}"),
        }
    }

    #[test]
    fn unknown_name_in_known_prefix_is_behavior_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_package(
            tmp.path(),
            "@almadar-io/behaviors",
            "almadar-behaviors",
            "1.0.0",
            &["rpg-hero"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![pkg],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let err = loader
            .load("almadar-behaviors/no-such-behavior", None, &mut chain)
            .unwrap_err();
        match err {
            LoaderError::BehaviorNotFound { name, searched } => {
                assert_eq!(name, "no-such-behavior");
                assert_eq!(searched, vec!["@almadar-io/behaviors@1.0.0".to_string()]);
            }
            other => panic!("expected BehaviorNotFound, got {other:?}"),
        }
    }

    #[test]
    fn std_behaviors_path_found_only_via_a_non_std_package() {
        // std IS installed (so `load_std_behavior` doesn't short-circuit on
        // BehaviorPackageNotInstalled) but doesn't carry `rpg-hero` — a miss
        // on the installed std still falls through to every other installed
        // package (Phase 1 ruling 3, preserved by Phase 2).
        let tmp = tempfile::tempdir().unwrap();
        let std_pkg = fake_package(tmp.path(), "@almadar/std", "almadar-std", "16.223.0", &[]);
        let io_pkg = fake_package(
            tmp.path(),
            "@almadar-io/behaviors",
            "almadar-behaviors",
            "1.0.0",
            &["rpg-hero"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![std_pkg, io_pkg],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let loaded = loader
            .load("std/behaviors/rpg-hero", None, &mut chain)
            .expect("io behavior should be found via the std/behaviors/ walk");
        assert_eq!(loaded.schema.name, "rpg-hero");
    }

    #[test]
    fn installed_std_package_is_authoritative() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_dir = tmp.path().join("node_modules/@almadar/std");
        let registry_dir = pkg_dir.join("behaviors/registry");
        std::fs::create_dir_all(&registry_dir).unwrap();
        let health = std::fs::read_to_string(fixtures_dir().join("health.orb")).unwrap();
        let content = health
            .replacen("\"HealthModule\"", "\"std-modal\"", 1)
            .replacen(
                "\"version\": \"1.0.0\",",
                "\"version\": \"1.0.0\", \"description\": \"disk-installed-std-modal\",",
                1,
            );
        std::fs::write(registry_dir.join("std-modal.orb"), content).unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            serde_json::json!({
                "name": "@almadar/std",
                "version": "16.223.0",
                "orb": {
                    "prefix": "almadar-std",
                    "registry": "behaviors/registry",
                        "compiler": ">=0.0.0",
                },
            })
            .to_string(),
        )
        .unwrap();
        let _ = std::fs::write(tmp.path().join("package.json"), "{}");
        let pkg = orbital_core_base::behavior_packages::discover(
            tmp.path(),
            &tmp.path().join("unused-user-store"),
        )
        .unwrap()
        .into_iter()
        .find(|p| p.name == "@almadar/std")
        .expect("discover should find the fake std package");

        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![pkg],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let loaded = loader
            .load("std/behaviors/std-modal", None, &mut chain)
            .expect("installed std-modal should load");
        // The embedded std-modal fixture carries no description; the
        // disk-installed copy does — proving the installed package won,
        // not the embedded phf map.
        assert_eq!(
            loaded.schema.description.as_deref(),
            Some("disk-installed-std-modal")
        );
    }

    #[test]
    fn installed_std_miss_is_behavior_not_found() {
        // `std-modal` doesn't exist in this installed (but otherwise empty)
        // std package and there is no other package to fall through to — a
        // miss on an installed std must be BehaviorNotFound, not any other
        // error shape.
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_package(tmp.path(), "@almadar/std", "almadar-std", "16.223.0", &[]);
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![pkg],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let err = loader
            .load("std/behaviors/std-modal", None, &mut chain)
            .unwrap_err();
        assert!(
            matches!(err, LoaderError::BehaviorNotFound { .. }),
            "expected BehaviorNotFound, got {err:?}"
        );
    }

    #[test]
    fn std_missing_reports_not_installed() {
        // No `almadar-std`-prefixed package on the walk — even though an
        // unrelated package IS installed — must report the actionable
        // not-installed error, never BehaviorNotFound/StdLibNotConfigured.
        let tmp = tempfile::tempdir().unwrap();
        let pkg = fake_package(
            tmp.path(),
            "@almadar-io/behaviors",
            "almadar-behaviors",
            "1.0.0",
            &["rpg-hero"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![pkg],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let err = loader
            .load("std/behaviors/std-modal", None, &mut chain)
            .unwrap_err();
        let message = err.to_string();
        match err {
            LoaderError::BehaviorPackageNotInstalled { package } => {
                assert_eq!(package, "@almadar/std");
                assert!(
                    message.contains("run `orb update`"),
                    "message: {message}"
                );
            }
            other => panic!("expected BehaviorPackageNotInstalled, got {other:?}"),
        }
    }

    #[test]
    fn same_name_in_two_non_std_packages_is_ambiguous() {
        // std IS installed (so resolution doesn't short-circuit on
        // BehaviorPackageNotInstalled) but doesn't carry `shared-name`.
        let tmp = tempfile::tempdir().unwrap();
        let std_pkg = fake_package(
            &tmp.path().join("std"),
            "@almadar/std",
            "almadar-std",
            "16.223.0",
            &[],
        );
        let pkg_a = fake_package(
            &tmp.path().join("a"),
            "pkg-a",
            "pkg-a-prefix",
            "1.0.0",
            &["shared-name"],
        );
        let pkg_b = fake_package(
            &tmp.path().join("b"),
            "pkg-b",
            "pkg-b-prefix",
            "1.0.0",
            &["shared-name"],
        );
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            packages: vec![std_pkg, pkg_a, pkg_b],
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let err = loader
            .load("std/behaviors/shared-name", None, &mut chain)
            .unwrap_err();
        match err {
            LoaderError::BehaviorAmbiguous {
                name,
                package_a,
                package_b,
            } => {
                assert_eq!(name, "shared-name");
                let mut names = [package_a, package_b];
                names.sort();
                assert_eq!(names, ["pkg-a".to_string(), "pkg-b".to_string()]);
            }
            other => panic!("expected BehaviorAmbiguous, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_std_path_when_configured() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            std_lib_path: Some(PathBuf::from("/project/std")),
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("std/behaviors/game-core", None);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().contains("game-core.orb"));
        assert!(path.to_string_lossy().contains("behaviors"));
    }

    #[test]
    fn test_resolve_scoped_path_requires_config() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            scoped_paths: HashMap::new(),
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("@game-lib/enemies", None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LoaderError::ScopedPackageNotConfigured(_)
        ));
    }

    #[test]
    fn test_resolve_scoped_path_when_configured() {
        let mut scoped_paths = HashMap::new();
        scoped_paths.insert("@game-lib".to_string(), PathBuf::from("/project/game-lib"));

        let config = LoaderConfig {
            base_path: fixtures_dir(),
            scoped_paths,
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("@game-lib/enemies", None);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().contains("enemies.orb"));
    }

    #[test]
    fn test_load_health_orb() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let result = loader.load("./health.orb", None, &mut chain);
        assert!(result.is_ok());

        let loaded = result.unwrap();
        assert_eq!(loaded.schema.name, "HealthModule");
        assert_eq!(loaded.schema.orbitals.len(), 1);
        assert_eq!(loaded.schema.orbitals[0].name, "Health");
    }

    #[test]
    fn test_load_player_orb_with_uses() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let result = loader.load("./player.orb", None, &mut chain);
        assert!(result.is_ok(), "Load failed: {:?}", result.err());

        let loaded = result.unwrap();
        assert_eq!(loaded.schema.name, "PlayerModule");

        let orbital = &loaded.schema.orbitals[0];
        assert!(orbital.uses.is_some());
        let uses = orbital.uses.as_ref().unwrap();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].from, "./health.orb");
        assert_eq!(uses[0].alias, "Health");

        // Entity should be a reference
        assert!(orbital.entity.is_reference());
        assert_eq!(orbital.entity.as_reference(), Some("Health.entity"));
    }

    #[test]
    fn test_load_caches_schemas() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        // First load
        let _ = loader.load("./health.orb", None, &mut chain);
        assert_eq!(loader.cache_size(), 1);

        // Second load should use cache
        let mut chain2 = ImportChain::new();
        let _ = loader.load("./health.orb", None, &mut chain2);
        assert_eq!(loader.cache_size(), 1);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let result = loader.load("./nonexistent.orb", None, &mut chain);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LoaderError::FileNotFound(_)));
    }

    /// G-ORB-056: a relative import whose target exists only as `.lolo` is
    /// lowered through the registered source lowerer (the real one; the
    /// process registers it once, and the installed-std tests need it too).

    fn sibling_dir(tag: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "orb-loader-lolo-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        dir
    }

    fn named(name: &str) -> String {
        format!(r#"{{"name":"{name}","orbitals":[]}}"#)
    }

    fn named_lolo(name: &str) -> String {
        format!("orbital {name} {{\n  entity Probe [runtime] {{\n    id : string!\n  }}\n}}\n")
    }

    fn sibling_loader(dir: &Path) -> ExternalOrbitalLoader {
        register_source_lowerer(|path, _, _| orbital_test_support::lower_lolo_file(path));
        ExternalOrbitalLoader::new(LoaderConfig {
            base_path: dir.to_path_buf(),
            ..Default::default()
        })
    }

    #[test]
    fn relative_import_lowers_a_lolo_only_sibling() {
        let dir = sibling_dir("only", &[("A.lolo", &named_lolo("FromLolo"))]);
        let mut loader = sibling_loader(&dir);
        let mut chain = ImportChain::new();
        let loaded = loader
            .load("./A", Some(&dir.join("B.lolo")), &mut chain)
            .expect("a .lolo-only sibling loads");
        assert_eq!(loaded.schema.name, "FromLolo");
        assert_eq!(loaded.source_path, dir.join("A.lolo"));
    }

    #[test]
    fn explicit_lolo_import_lowers_the_lolo() {
        let dir = sibling_dir("explicit", &[("A.lolo", &named_lolo("FromLolo"))]);
        let mut loader = sibling_loader(&dir);
        let mut chain = ImportChain::new();
        let loaded = loader
            .load("./A.lolo", Some(&dir.join("B.lolo")), &mut chain)
            .expect("an explicit ./A.lolo import loads");
        assert_eq!(loaded.schema.name, "FromLolo");
    }

    #[test]
    fn orb_sibling_wins_over_lolo() {
        let dir = sibling_dir(
            "both",
            &[
                ("A.orb", &named("FromOrb")),
                ("A.lolo", &named_lolo("FromLolo")),
            ],
        );
        let mut loader = sibling_loader(&dir);
        let mut chain = ImportChain::new();
        let loaded = loader
            .load("./A", Some(&dir.join("B.lolo")), &mut chain)
            .expect("loads");
        assert_eq!(loaded.schema.name, "FromOrb");
    }

    #[test]
    fn missing_sibling_is_still_file_not_found() {
        let dir = sibling_dir("none", &[]);
        let mut loader = sibling_loader(&dir);
        let mut chain = ImportChain::new();
        let err = loader
            .load("./A", Some(&dir.join("B.lolo")), &mut chain)
            .expect_err("nothing to load");
        assert!(matches!(err, LoaderError::FileNotFound(_)));
    }

    #[test]
    fn lowering_failure_names_the_lolo_file() {
        let dir = sibling_dir("broken", &[("A.lolo", "not lolo {")]);
        let mut loader = sibling_loader(&dir);
        let mut chain = ImportChain::new();
        let err = loader
            .load("./A", Some(&dir.join("B.lolo")), &mut chain)
            .expect_err("lowering fails");
        match err {
            LoaderError::SourceLowering(path, _) => assert!(path.ends_with("A.lolo")),
            other => panic!("expected SourceLowering, got {other:?}"),
        }
    }
}

// ============================================================================
// Sub-unit 3.5.D: Installed std behavior loader tests
// ============================================================================
//
// These resolve the REAL `@almadar/std` package discovered from this crate's
// `CARGO_MANIFEST_DIR` (the monorepo workspace link), not a fabricated
// fixture — the embedded phf map these tests used to exercise was removed in
// Phase 2 (`docs/Almadar_Behavior_Packages.md` §10).

#[cfg(test)]
mod embedded_loader_tests {
    use super::*;

    fn std_loader_config() -> LoaderConfig {
        register_source_lowerer(|path, _, _| orbital_test_support::lower_lolo_file(path));
        let std_pkg = orbital_test_support::installed_package(
            "almadar-std",
            Path::new(env!("CARGO_MANIFEST_DIR")),
        );
        LoaderConfig {
            base_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loader/fixtures"),
            packages: vec![std_pkg],
            ..Default::default()
        }
    }

    #[test]
    fn loads_std_modal_from_installed_std() {
        let mut loader = ExternalOrbitalLoader::new(std_loader_config());
        let mut chain = ImportChain::new();
        let result = loader.load("std/behaviors/std-modal", None, &mut chain);
        assert!(
            result.is_ok(),
            "Loading std-modal failed: {:?}",
            result.err()
        );
        let loaded = result.unwrap();
        assert_eq!(loaded.schema.name, "std-modal");
    }

    #[test]
    fn loads_std_browse_from_installed_std() {
        let mut loader = ExternalOrbitalLoader::new(std_loader_config());
        let mut chain = ImportChain::new();
        let result = loader.load("std/behaviors/std-browse", None, &mut chain);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().schema.name, "std-browse");
    }

    #[test]
    fn unknown_std_behavior_returns_error() {
        // "std/behaviors/no-such-thing" is not in the installed std
        // package's registry, so `load_std_behavior` reports
        // `BehaviorNotFound` directly — resolution must NOT silently
        // succeed, and must NOT fall through to filesystem `std/` resolution
        // (that branch is only reached for topic-qualified `std/...` paths now).
        let mut loader = ExternalOrbitalLoader::new(std_loader_config());
        let mut chain = ImportChain::new();
        let result = loader.load("std/behaviors/no-such-thing", None, &mut chain);
        assert!(result.is_err(), "Expected error for unknown std behavior");
        assert!(matches!(
            result.unwrap_err(),
            LoaderError::BehaviorNotFound { .. }
        ));
    }

    #[test]
    fn relative_path_still_falls_through_to_filesystem() {
        // Sanity check: relative imports must NOT be intercepted by the std
        // branch. `./nonexistent.orb` is not a std path, so it should go
        // through the filesystem resolver and produce a `FileNotFound` (or
        // similar IO) error, proving the std branch did not swallow it.
        let config = LoaderConfig {
            base_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loader/fixtures"),
            ..Default::default()
        };
        let mut loader = ExternalOrbitalLoader::new(config);
        let mut chain = ImportChain::new();

        let result = loader.load("./nonexistent.orb", None, &mut chain);
        assert!(
            result.is_err(),
            "Relative path should still fail through filesystem"
        );
        // Confirm the error is filesystem-related (FileNotFound), not something
        // surfaced from std-registry resolution.
        assert!(matches!(
            result.unwrap_err(),
            LoaderError::FileNotFound(_) | LoaderError::IoError(_)
        ));
    }

    // FC-3: browse-form (`@std/…`, `@behaviors/…`) specifier mapping.

    fn mapped(import_path: &str) -> Option<String> {
        match map_browse_specifier(import_path) {
            BrowseMapping::Mapped(s) => Some(s),
            _ => None,
        }
    }

    #[test]
    fn browse_form_std_maps_to_canonical_std_path() {
        assert_eq!(
            mapped("@std/ui/core/atoms/std-stats.lolo").as_deref(),
            Some("std/behaviors/std-stats")
        );
        assert_eq!(
            mapped("@std/ui/core/molecules/ui-calendar-grid.lolo").as_deref(),
            Some("std/behaviors/ui-calendar-grid")
        );
    }

    #[test]
    fn browse_form_behaviors_maps_to_package_registry() {
        assert_eq!(
            mapped("@behaviors/app/molecules/app-crud-manager.lolo").as_deref(),
            Some("almadar-behaviors/app-crud-manager")
        );
        assert_eq!(
            mapped("@behaviors/game/organisms/rpg-hero.lolo").as_deref(),
            Some("almadar-behaviors/rpg-hero")
        );
    }

    #[test]
    fn canonical_form_is_left_unchanged() {
        assert!(matches!(
            map_browse_specifier("std/behaviors/std-stats"),
            BrowseMapping::NotBrowseForm
        ));
        assert!(matches!(
            map_browse_specifier("almadar-behaviors/rpg-hero"),
            BrowseMapping::NotBrowseForm
        ));
        assert!(matches!(
            map_browse_specifier("./health.orb"),
            BrowseMapping::NotBrowseForm
        ));
    }

    #[test]
    fn malformed_browse_form_is_flagged() {
        assert!(matches!(
            map_browse_specifier("@std/"),
            BrowseMapping::Malformed
        ));
        assert!(matches!(
            map_browse_specifier("@behaviors/app/molecules/"),
            BrowseMapping::Malformed
        ));
    }

    #[test]
    fn malformed_browse_form_load_errors_with_dual_form_message() {
        let mut loader = ExternalOrbitalLoader::new(LoaderConfig::default());
        let mut chain = ImportChain::new();
        let result = loader.load("@std/", None, &mut chain);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, LoaderError::InvalidPrimitiveImport(_)));
        let msg = err.to_string();
        assert!(msg.contains("std/behaviors/std-stats"), "message: {msg}");
        assert!(
            msg.contains("@std/ui/core/atoms/std-stats.lolo"),
            "message: {msg}"
        );
        assert!(
            msg.contains("@behaviors/app/molecules/app-crud-manager.lolo"),
            "message: {msg}"
        );
    }

    #[test]
    fn browse_form_std_loads_from_installed_std() {
        // The browse alias must resolve the SAME installed-std schema the
        // canonical `std/behaviors/std-modal` import does.
        let mut loader = ExternalOrbitalLoader::new(std_loader_config());
        let mut chain = ImportChain::new();
        let result = loader.load("@std/ui/core/organisms/std-modal.lolo", None, &mut chain);
        assert!(
            result.is_ok(),
            "browse-form std-modal failed: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().schema.name, "std-modal");
    }

    #[test]
    fn std_load_caches_result() {
        // Loading the same installed-std behavior twice should hit the
        // filesystem cache on the second call, not reparse the JSON.
        let mut loader = ExternalOrbitalLoader::new(std_loader_config());
        let mut chain = ImportChain::new();
        let _ = loader
            .load("std/behaviors/std-modal", None, &mut chain)
            .unwrap();
        assert_eq!(loader.cache_size(), 1);

        let mut chain2 = ImportChain::new();
        let _ = loader
            .load("std/behaviors/std-modal", None, &mut chain2)
            .unwrap();
        assert_eq!(loader.cache_size(), 1);
    }
}
