//! Shell Trait and Registry
//!
//! Each target platform (TypeScript, Android, Python, Rust, Swift) has a shell that
//! provides the runtime framework. The compiler generates code INTO the shell.
//!
//! The `Shell` trait and related error/config types live in `orbital-core` so
//! shell implementation crates can depend on `orbital-core` instead of the
//! compiler, avoiding circular dependencies when the compiler registers those
//! shells.


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

pub use orbital_core::shell::BaseShell;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

}
