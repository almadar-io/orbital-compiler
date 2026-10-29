//! What a compile reports and what a mixed frontend/backend compile adds around the shells' own files: the
//! docker-compose file, the frontend's `.env` and the steps to run both.

/// A shell as a mixed compile names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedShell {
    Typescript,
    Python,
    Rust,
    Android,
    Ios,
    ChromeExtension,
    Os,
}

/// What one compile reports: the stdout lines, and the parts a program reads on their own.
#[derive(Debug, Default)]
pub struct CompileReport {
    pub lines: Vec<String>,
    pub files: usize,
    pub warnings: Vec<String>,
    pub next_steps: Vec<String>,
}

impl MixedShell {
    fn port_url(self) -> &'static str {
        match self {
            Self::Python | Self::Typescript | Self::Rust => "http://localhost:3030",
            Self::Android | Self::Ios | Self::ChromeExtension | Self::Os => "http://localhost:8000",
        }
    }
}

/// The API URL a browser reaches `backend` at; the default backend port when there is none.
pub fn backend_url(backend: Option<MixedShell>) -> &'static str {
    backend.map_or("http://localhost:8000", MixedShell::port_url)
}

/// Why this pair cannot be compiled together, `None` when it can.
pub fn refuse_pair(
    frontend: Option<MixedShell>,
    backend: Option<MixedShell>,
) -> Option<&'static str> {
    if [frontend, backend].contains(&Some(MixedShell::ChromeExtension)) {
        return Some("The chrome-extension shell is a whole program; compile it without --frontend/--backend");
    }
    match backend {
        Some(MixedShell::Android) => Some("Android shell cannot be used as a backend"),
        Some(MixedShell::Ios) => Some("iOS shell cannot be used as a backend"),
        _ => None,
    }
}

/// `docker-compose.yaml` for a frontend/backend pair.
pub fn docker_compose(frontend: Option<MixedShell>, backend: Option<MixedShell>) -> String {
    let mut compose = String::from("version: '3.8'\n\nservices:\n");
    match backend {
        Some(MixedShell::Python) => compose.push_str("  backend:\n    build: ./backend\n    ports:\n      - \"3030:3030\"\n\n"),
        Some(MixedShell::Typescript | MixedShell::Rust) => {
            compose.push_str("  backend:\n    build: ./backend\n    ports:\n      - \"3030:3030\"\n    environment:\n\n")
        }
        _ => {}
    }
    let url = backend_url(backend);
    match frontend {
        Some(MixedShell::Typescript) => compose.push_str(&format!(
            "  frontend:\n    build:\n      context: ./frontend\n      args:\n        VITE_API_URL: {url}\n    ports:\n      - \"3000:3000\"\n    depends_on:\n      - backend\n"
        )),
        Some(MixedShell::Python) => compose.push_str(&format!(
            "  frontend:\n    build: ./frontend\n    ports:\n      - \"8080:8080\"\n    environment:\n      - API_URL={url}\n    depends_on:\n      - backend\n"
        )),
        Some(MixedShell::Rust) => compose.push_str(&format!(
            "  frontend:\n    build:\n      context: ./frontend\n      args:\n        API_URL: {url}\n    ports:\n      - \"3000:3000\"\n    depends_on:\n      - backend\n"
        )),
        Some(MixedShell::Android) => compose.push_str(&format!(
            "  # Android app - run locally with:\n  #   cd frontend && ./gradlew assembleDebug && ./gradlew installDebug\n  # Backend URL for Android app:\n  #   API_URL={url}\n"
        )),
        Some(MixedShell::Ios) => compose.push_str(&format!(
            "  # iOS app - run locally with:\n  #   cd frontend && ./orb-build.sh\n  # Backend URL for iOS app:\n  #   API_URL={url}\n"
        )),
        Some(MixedShell::ChromeExtension) => {
            compose.push_str("  # Chrome extension - build with: pnpm build, then load dist/ unpacked\n")
        }
        Some(MixedShell::Os) => compose.push_str(
            "  # Orbital OS image - build with ./build.sh in the seL4 container, boot with ./run-qemu.sh\n",
        ),
        None => {}
    }
    compose
}

/// The frontend's `.env` text; empty when the frontend reads no API URL from one.
pub fn frontend_env(frontend: Option<MixedShell>, backend: Option<MixedShell>) -> String {
    let url = backend_url(backend);
    match frontend {
        Some(MixedShell::Typescript) => {
            format!("# API URL for backend server\nVITE_API_URL={url}\n")
        }
        Some(MixedShell::Python | MixedShell::Rust | MixedShell::Android | MixedShell::Ios) => {
            format!("# API URL for backend server\nAPI_URL={url}\n")
        }
        Some(MixedShell::ChromeExtension | MixedShell::Os) | None => String::new(),
    }
}

/// The commands that run a generated pair; a blank entry separates the groups.
pub fn run_steps(output: &str) -> Vec<String> {
    [
        format!("cd {output}"),
        "docker-compose up".to_string(),
        String::new(),
        "Or run separately:".to_string(),
        "# Terminal 1: Backend".to_string(),
        "cd backend && python -m venv .venv && source .venv/bin/activate && pip install -e . && python -m orbital_app".to_string(),
        "# Terminal 2: Frontend".to_string(),
        "cd frontend && npm install && npm run dev".to_string(),
    ]
    .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use MixedShell::*;

    #[test]
    fn a_typescript_frontend_over_a_python_backend_builds_both_services_against_the_backend_port() {
        let compose = docker_compose(Some(Typescript), Some(Python));
        assert!(
            compose.starts_with("version: '3.8'\n\nservices:\n  backend:\n    build: ./backend\n")
        );
        assert!(compose.contains("VITE_API_URL: http://localhost:3030"));
        assert!(compose.contains("depends_on:\n      - backend"));
        assert_eq!(
            frontend_env(Some(Typescript), Some(Python)),
            "# API URL for backend server\nVITE_API_URL=http://localhost:3030\n"
        );
    }

    #[test]
    fn control_a_native_frontend_gets_a_comment_not_a_service_and_the_other_ports() {
        let compose = docker_compose(Some(Android), Some(Typescript));
        assert!(compose.contains("#   API_URL=http://localhost:3030"));
        assert!(!compose.contains("  frontend:"));
        assert_eq!(
            frontend_env(Some(Android), Some(Os)),
            "# API URL for backend server\nAPI_URL=http://localhost:8000\n"
        );
        assert_eq!(frontend_env(Some(ChromeExtension), Some(Python)), "");
        assert_eq!(backend_url(None), "http://localhost:8000");
    }

    #[test]
    fn edge_the_pairs_that_cannot_be_compiled_together_are_refused() {
        assert!(refuse_pair(Some(ChromeExtension), None)
            .unwrap()
            .contains("whole program"));
        assert!(refuse_pair(None, Some(ChromeExtension)).is_some());
        assert_eq!(
            refuse_pair(Some(Typescript), Some(Android)),
            Some("Android shell cannot be used as a backend")
        );
        assert_eq!(
            refuse_pair(Some(Typescript), Some(Ios)),
            Some("iOS shell cannot be used as a backend")
        );
        assert_eq!(refuse_pair(Some(Android), Some(Python)), None);
    }

    #[test]
    fn the_run_steps_start_from_the_output_directory() {
        assert_eq!(run_steps("/out")[0], "cd /out");
    }
}
