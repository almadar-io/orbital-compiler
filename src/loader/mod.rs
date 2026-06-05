//! External Orbital Loader
//!
//! Loads external .orb files from various sources:
//! - Local filesystem (relative paths)
//! - Standard library (std/...)
//! - Scoped packages (@name/...)

use orbital_core::schema::OrbitalSchema;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Invalid JSON in {0}: {1}")]
    InvalidJson(String, String),

    #[error("Invalid schema in {0}: {1}")]
    InvalidSchema(String, String),

    #[error("Circular import detected: {0}")]
    CircularImport(String),

    #[error("Path traversal outside allowed directory: {0}")]
    PathTraversal(String),

    #[error("Standard library path not configured")]
    StdLibNotConfigured,

    #[error("Scoped package not configured: {0}")]
    ScopedPackageNotConfigured(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
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
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("."),
            std_lib_path: None,
            scoped_paths: HashMap::new(),
            allow_outside_base_path: false,
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
        let canonical = canonical_key(path);
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

    pub fn clear(&mut self) {
        self.cache.clear();
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
// External Orbital Loader
// ============================================================================

/// ExternalOrbitalLoader - Loads external .orb files with security and caching
pub struct ExternalOrbitalLoader {
    config: LoaderConfig,
    cache: LoaderCache,
    /// Cache for embedded std behaviors keyed by canonical import path
    /// (e.g., `std/behaviors/std-modal`). Kept separate from `cache` because
    /// `LoaderCache` is keyed by filesystem-canonicalized paths, which is
    /// meaningless for in-memory embedded content.
    embedded_cache: HashMap<String, LoadedSchema>,
}

impl ExternalOrbitalLoader {
    pub fn new(config: LoaderConfig) -> Self {
        Self {
            config,
            cache: LoaderCache::new(),
            embedded_cache: HashMap::new(),
        }
    }

    /// Load a schema from an import path
    pub fn load(
        &mut self,
        import_path: &str,
        from_path: Option<&Path>,
        chain: &mut ImportChain,
    ) -> Result<&LoadedSchema, LoaderError> {
        // Sub-unit 3.5.D: check the embedded std behavior registry BEFORE any
        // filesystem resolution. Canonical std paths like
        // `std/behaviors/std-modal` are shipped inside the compiler binary via
        // `phf::Map` and must take precedence over any on-disk std lib.
        if let Some(content) = crate::embedded::get_std_behavior_content(import_path) {
            // Use a synthetic path for cycle detection. `ImportChain.push`
            // canonicalizes but falls back to the given path when the file
            // does not exist, so cycle detection still works for embedded
            // entries that transitively import themselves.
            let synthetic_path = PathBuf::from(format!("<embedded>/{}.orb", import_path));

            if let Some(error) = chain.push(&synthetic_path) {
                return Err(LoaderError::CircularImport(error));
            }

            // Entry API avoids the contains-then-get double lookup; the parse
            // error is matched (not `?`-propagated) so the chain still pops.
            let result = match self.embedded_cache.entry(import_path.to_string()) {
                Entry::Occupied(e) => Ok(e.into_mut()),
                Entry::Vacant(e) => match serde_json::from_str::<OrbitalSchema>(content) {
                    Ok(schema) => Ok(e.insert(LoadedSchema {
                        schema,
                        source_path: synthetic_path.clone(),
                        import_path: import_path.to_string(),
                    })),
                    Err(parse_err) => Err(LoaderError::InvalidJson(
                        synthetic_path.display().to_string(),
                        parse_err.to_string(),
                    )),
                },
            };

            chain.pop();
            return result.map(|loaded| &*loaded);
        }

        // Resolve the absolute path
        let absolute_path = self.resolve_path(import_path, from_path)?;

        // Check for circular imports
        if let Some(error) = chain.push(&absolute_path) {
            return Err(LoaderError::CircularImport(error));
        }

        let result = self.cache.get_or_insert_with(&absolute_path, || {
            Self::load_file(&absolute_path, import_path)
        });

        chain.pop();
        result
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

        // Default: treat as relative to base path
        self.resolve_relative_path(&format!("./{}", import_path), from_path)
    }

    fn resolve_std_path(&self, import_path: &str) -> Result<PathBuf, LoaderError> {
        let std_lib_path = self
            .config
            .std_lib_path
            .as_ref()
            .ok_or(LoaderError::StdLibNotConfigured)?;

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
        if path.extension().map_or(true, |ext| ext != "orb") {
            path.with_extension("orb")
        } else {
            path
        }
    }

    fn load_file(path: &Path, import_path: &str) -> Result<LoadedSchema, LoaderError> {
        // Check file exists
        if !path.exists() {
            return Err(LoaderError::FileNotFound(path.display().to_string()));
        }

        // Read file
        let content = std::fs::read_to_string(path)?;

        // Deserialize directly into the schema (single parse).
        let schema: OrbitalSchema = serde_json::from_str(&content)
            .map_err(|e| LoaderError::InvalidJson(path.display().to_string(), e.to_string()))?;

        Ok(LoadedSchema {
            schema,
            source_path: path.to_path_buf(),
            import_path: import_path.to_string(),
        })
    }

    /// Clear the cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.embedded_cache.clear();
    }

    /// Get cache size
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Get embedded-cache size (entries resolved from the embedded std
    /// behavior registry, not the filesystem).
    pub fn embedded_cache_size(&self) -> usize {
        self.embedded_cache.len()
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
            name: "Test".to_string(),
            version: None,
            description: None,
            orbitals: vec![],
            metadata: None,
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
    fn test_resolve_std_path_requires_config() {
        let config = LoaderConfig {
            base_path: fixtures_dir(),
            std_lib_path: None,
            ..Default::default()
        };
        let loader = ExternalOrbitalLoader::new(config);

        let result = loader.resolve_path("std/behaviors/game-core", None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LoaderError::StdLibNotConfigured
        ));
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
}

// ============================================================================
// Sub-unit 3.5.D: Embedded std behavior loader tests
// ============================================================================

#[cfg(test)]
mod embedded_loader_tests {
    use super::*;

    #[test]
    fn loads_std_modal_from_embedded() {
        let mut loader = ExternalOrbitalLoader::new(LoaderConfig::default());
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
    fn loads_std_list_from_embedded() {
        let mut loader = ExternalOrbitalLoader::new(LoaderConfig::default());
        let mut chain = ImportChain::new();
        let result = loader.load("std/behaviors/std-list", None, &mut chain);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().schema.name, "std-list");
    }

    #[test]
    fn unknown_std_behavior_returns_error() {
        // "std/behaviors/no-such-thing" is not in the embedded registry, so the
        // loader should fall through to the filesystem path. Because the default
        // `LoaderConfig` has no `std_lib_path`, filesystem resolution yields
        // `StdLibNotConfigured`. Either way, resolution must NOT silently succeed.
        let mut loader = ExternalOrbitalLoader::new(LoaderConfig::default());
        let mut chain = ImportChain::new();
        let result = loader.load("std/behaviors/no-such-thing", None, &mut chain);
        assert!(result.is_err(), "Expected error for unknown std behavior");
        assert!(matches!(
            result.unwrap_err(),
            LoaderError::StdLibNotConfigured | LoaderError::FileNotFound(_)
        ));
    }

    #[test]
    fn relative_path_still_falls_through_to_filesystem() {
        // Sanity check: relative imports must NOT be intercepted by the embedded
        // branch. `./nonexistent.orb` is not a std path, so it should go through
        // the filesystem resolver and produce a `FileNotFound` (or similar IO)
        // error, proving the embedded branch did not swallow it.
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
        // surfaced from embedded JSON parsing.
        assert!(matches!(
            result.unwrap_err(),
            LoaderError::FileNotFound(_) | LoaderError::IoError(_)
        ));
    }

    #[test]
    fn embedded_load_caches_result() {
        // Loading the same embedded behavior twice should hit the embedded
        // cache on the second call, not reparse the JSON.
        let mut loader = ExternalOrbitalLoader::new(LoaderConfig::default());
        let mut chain = ImportChain::new();
        let _ = loader
            .load("std/behaviors/std-modal", None, &mut chain)
            .unwrap();
        assert_eq!(loader.embedded_cache_size(), 1);
        assert_eq!(loader.cache_size(), 0);

        let mut chain2 = ImportChain::new();
        let _ = loader
            .load("std/behaviors/std-modal", None, &mut chain2)
            .unwrap();
        assert_eq!(loader.embedded_cache_size(), 1);
    }
}
