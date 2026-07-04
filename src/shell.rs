//! Shell Trait and Registry
//!
//! Each target platform (TypeScript, Android, Python, Rust, Swift) has a shell that
//! provides the runtime framework. The compiler generates code INTO the shell.
//!
//! The `Shell` trait and related error/config types live in `orbital-core` so
//! shell implementation crates can depend on `orbital-core` instead of the
//! compiler, avoiding circular dependencies when the compiler registers those
//! shells.

use std::path::{Path, PathBuf};

// Re-export core shell types so existing consumers of `orbital_compiler::shell`
// continue to compile without changes.
pub use orbital_core::shell::{Shell, ShellConfig, ShellError, ShellResult};

/// Registry of all available shells
pub struct ShellRegistry {
    shells: Vec<Box<dyn Shell>>,
}

impl ShellRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self { shells: vec![] }
    }

    /// Register a shell
    pub fn register(&mut self, shell: Box<dyn Shell>) {
        self.shells.push(shell);
    }

    /// Get a shell by name
    pub fn get(&self, name: &str) -> Option<&dyn Shell> {
        self.shells
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.as_ref())
    }

    /// List all registered shells
    pub fn list(&self) -> Vec<&str> {
        self.shells.iter().map(|s| s.name()).collect()
    }

    /// Get all registered shells
    pub fn all(&self) -> impl Iterator<Item = &dyn Shell> {
        self.shells.iter().map(|s| s.as_ref())
    }
}

impl Default for ShellRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Base shell implementation that can be used by specific shells
pub struct BaseShell {
    name: &'static str,
    description: &'static str,
    extensions: &'static [&'static str],
    template_path: PathBuf,
    generated_dir: &'static str,
    build_cmd: &'static str,
    dev_cmd: &'static str,
    test_cmd: &'static str,
}

impl BaseShell {
    /// Create a new base shell
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &'static str,
        description: &'static str,
        extensions: &'static [&'static str],
        template_path: PathBuf,
        generated_dir: &'static str,
        build_cmd: &'static str,
        dev_cmd: &'static str,
        test_cmd: &'static str,
    ) -> Self {
        Self {
            name,
            description,
            extensions,
            template_path,
            generated_dir,
            build_cmd,
            dev_cmd,
            test_cmd,
        }
    }
}

impl Shell for BaseShell {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn extensions(&self) -> &[&'static str] {
        self.extensions
    }

    fn template_path(&self) -> &Path {
        &self.template_path
    }

    fn generated_dir(&self) -> &str {
        self.generated_dir
    }

    fn build_command(&self) -> &str {
        self.build_cmd
    }

    fn dev_command(&self) -> &str {
        self.dev_cmd
    }

    fn test_command(&self) -> &str {
        self.test_cmd
    }

    fn init_project(&self, target_dir: &Path, project_name: &str) -> ShellResult<()> {
        // Copy template to target with placeholder replacement
        copy_dir_with_replacements(&self.template_path, target_dir, project_name)?;

        // Create generated directory
        let gen_dir = target_dir.join(self.generated_dir);
        std::fs::create_dir_all(&gen_dir)?;

        Ok(())
    }

    fn copy_template(&self, output_dir: &Path) -> ShellResult<()> {
        if !self.template_path.exists() {
            return Err(ShellError::NotFound(
                self.template_path.display().to_string(),
            ));
        }

        copy_dir_recursive(&self.template_path, output_dir)?;
        Ok(())
    }

    fn validate_structure(&self, output_dir: &Path) -> ShellResult<()> {
        if !output_dir.exists() {
            return Err(ShellError::InvalidStructure {
                path: output_dir.to_path_buf(),
                message: "Directory does not exist".to_string(),
            });
        }

        let gen_dir = output_dir.join(self.generated_dir);
        if !gen_dir.exists() {
            return Err(ShellError::InvalidStructure {
                path: gen_dir,
                message: "Generated directory does not exist".to_string(),
            });
        }

        Ok(())
    }
}

/// Copy directory recursively
fn copy_dir_recursive(src: &Path, dst: &Path) -> ShellResult<()> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if path.is_dir() {
            // Skip generated directory when copying
            if entry.file_name() == "generated" {
                continue;
            }
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            std::fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}

/// Copy directory recursively with placeholder replacement
fn copy_dir_with_replacements(src: &Path, dst: &Path, project_name: &str) -> ShellResult<()> {
    if !src.exists() {
        return Err(ShellError::NotFound(src.display().to_string()));
    }

    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    // Build replacement map
    let replacements = build_replacements(project_name);

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if path.is_dir() {
            // Skip node_modules and generated directories
            let dir_name = entry.file_name();
            if dir_name == "node_modules" || dir_name == "generated" || dir_name == ".git" {
                continue;
            }
            copy_dir_with_replacements(&path, &dest_path, project_name)?;
        } else {
            // Check if this is a text file that needs placeholder replacement
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let should_process = matches!(
                ext,
                // TypeScript/JavaScript
                "ts" | "tsx"
                    | "js"
                    | "jsx"
                    // Python
                    | "py"
                    | "pyi"
                    | "toml"
                    // Common
                    | "json"
                    | "md"
                    | "html"
                    | "css"
                    | "yaml"
                    | "yml"
                    | "env"
                    | "txt"
                    | "sh"
                    // Rust
                    | "rs"
                    // Swift
                    | "swift"
                    // Kotlin
                    | "kt"
                    | "kts"
            ) || path.file_name().is_some_and(|n| {
                let name = n.to_string_lossy();
                name.starts_with('.') && !name.contains("lock")
            });

            if should_process {
                // Read, replace, and write
                let content = std::fs::read_to_string(&path)?;
                let processed = replace_placeholders(&content, &replacements);
                std::fs::write(&dest_path, processed)?;
            } else {
                // Binary file - just copy
                std::fs::copy(&path, &dest_path)?;
            }
        }
    }

    Ok(())
}

/// Build replacement map from project name
fn build_replacements(project_name: &str) -> std::collections::HashMap<&'static str, String> {
    let mut map = std::collections::HashMap::new();

    // App naming
    map.insert("{{APP_NAME}}", project_name.to_string());
    map.insert("{{APP_TITLE}}", orbital_core::str_utils::to_title_case(project_name));

    // Firebase/hosting (use kebab-case for URLs)
    let kebab_name = orbital_core::str_utils::to_kebab_case_simple(project_name);
    map.insert("{{SITE_NAME}}", kebab_name.clone());
    map.insert("{{BACKEND_NAME}}", format!("{}-backend", kebab_name));
    map.insert("{{FIREBASE_PROJECT_ID}}", format!("{}-project", kebab_name));

    // Ports
    map.insert("{{SERVER_PORT}}", "3030".to_string());
    map.insert("{{CLIENT_PORT}}", "5173".to_string());

    map
}

/// Replace all placeholders in content
fn replace_placeholders(
    content: &str,
    replacements: &std::collections::HashMap<&str, String>,
) -> String {
    let mut result = content.to_string();
    for (placeholder, value) in replacements {
        result = result.replace(placeholder, value);
    }
    result
}

/// Create a TypeScript shell configuration
///
/// Supports both "almadar-shell" (new naming) and "typescript-shell" (legacy)
pub fn typescript_shell(shells_dir: &Path) -> BaseShell {
    // Prefer almadar-shell (new naming), fall back to typescript-shell (legacy)
    let template_path = {
        let almadar_path = shells_dir.join("almadar-shell");
        if almadar_path.exists() {
            almadar_path
        } else {
            shells_dir.join("typescript-shell")
        }
    };

    BaseShell::new(
        "typescript",
        "TypeScript/React frontend with Express backend",
        &["ts", "tsx", "js", "jsx"],
        template_path,
        ".", // Generated code goes directly into packages/
        "npm run build",
        "npm run dev",
        "npm test",
    )
}

/// Create a TypeScript shell configuration for Hono backend
pub fn typescript_shell_hono(shells_dir: &Path) -> BaseShell {
    let template_path = {
        let hono_path = shells_dir.join("almadar-shell-hono");
        if hono_path.exists() {
            hono_path
        } else {
            // Fall back to Express shell if Hono shell not found
            let almadar_path = shells_dir.join("almadar-shell");
            if almadar_path.exists() {
                almadar_path
            } else {
                shells_dir.join("typescript-shell")
            }
        }
    };

    BaseShell::new(
        "typescript",
        "TypeScript/React frontend with Hono backend",
        &["ts", "tsx", "js", "jsx"],
        template_path,
        ".",
        "npm run build",
        "npm run dev",
        "npm test",
    )
}

/// Create an Android shell configuration
pub fn android_shell(shells_dir: &Path) -> BaseShell {
    BaseShell::new(
        "android",
        "Android with Kotlin and Jetpack Compose",
        &["kt"],
        shells_dir.join("android-shell"),
        "app/src/main/kotlin/com/orbital/app/generated",
        "gradle build",
        "gradle run",
        "gradle test",
    )
}

/// Create a Python shell configuration.
/// Searches multiple locations: shells_dir/python-shell, then
/// orbital-rust/templates/python-shell from repo root (for monorepo dev
/// where shells_dir points to packages/ for the TypeScript shell).
pub fn python_shell(shells_dir: &Path) -> BaseShell {
    let primary = shells_dir.join("python-shell");
    let template_path = if primary.exists() {
        primary
    } else {
        // Try sibling: if shells_dir is packages/, check ../orbital-rust/templates/python-shell
        let from_packages = shells_dir.join("../orbital-rust/templates/python-shell");
        if from_packages.exists() {
            from_packages
        } else {
            // Fall back to PythonShell::find_template_path() from the shell crate
            primary
        }
    };
    BaseShell::new(
        "python",
        "Python with FastAPI and PyTorch",
        &["py"],
        template_path,
        "src/orbital_app/generated",
        "pip install -e .",
        "python -m uvicorn orbital_app.main:app --reload --port 8000",
        "pytest",
    )
}

/// Create a Rust shell configuration
pub fn rust_shell(shells_dir: &Path) -> BaseShell {
    BaseShell::new(
        "rust",
        "Rust with Axum and egui",
        &["rs"],
        shells_dir.join("rust-shell"),
        "src/generated",
        "cargo build",
        "cargo run",
        "cargo test",
    )
}

/// Create an iOS shell configuration
pub fn ios_shell(shells_dir: &Path) -> BaseShell {
    BaseShell::new(
        "ios",
        "Native iOS application with SwiftUI",
        &["swift"],
        shells_dir.join("ios-shell"),
        "Sources/OrbitalApp/Generated",
        "./orb-build.sh",
        "./orb-build.sh --dev",
        "xcodebuild test",
    )
}

/// Create a registry with all default shells
pub fn create_default_registry(shells_dir: &Path) -> ShellRegistry {
    let mut registry = ShellRegistry::new();
    registry.register(Box::new(typescript_shell(shells_dir)));
    registry.register(Box::new(android_shell(shells_dir)));
    registry.register(Box::new(python_shell(shells_dir)));
    registry.register(Box::new(rust_shell(shells_dir)));
    registry.register(Box::new(ios_shell(shells_dir)));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_registry() {
        let mut registry = ShellRegistry::new();

        let shell = BaseShell::new(
            "test",
            "Test shell",
            &["txt"],
            PathBuf::from("/tmp/test-shell"),
            "generated",
            "echo build",
            "echo dev",
            "echo test",
        );

        registry.register(Box::new(shell));

        assert_eq!(registry.list(), vec!["test"]);
        assert!(registry.get("test").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_shell_properties() {
        let shell = typescript_shell(&PathBuf::from("/shells"));

        assert_eq!(shell.name(), "typescript");
        assert_eq!(shell.extensions(), &["ts", "tsx", "js", "jsx"]);
        assert_eq!(shell.generated_dir(), "."); // Generated code goes directly into packages/
        assert_eq!(shell.build_command(), "npm run build");
    }
}
