// The compiler stamps the program's browser-only services (registry `runtimes`) onto the module, so
// every `Residence::of_module` sends their calls to the browser leg.

use orbital_compiler::{compile_json, CompilerOptions};
use std::collections::HashMap;

const PROGRAM: &str = r#"{
  "name": "Site",
  "version": "1.0.0",
  "orbitals": [{
    "name": "Visits",
    "entity": { "name": "Visit", "persistence": "runtime", "fields": [ { "name": "id", "type": "string", "required": true } ] },
    "traits": [{
      "name": "Pageview",
      "linkedEntity": "Visit",
      "stateMachine": {
        "states": [ { "name": "active", "isInitial": true } ],
        "events": [ { "key": "INIT" }, { "key": "ASK" }, { "key": "SENT" }, { "key": "NOT_SENT" } ],
        "transitions": [
          { "from": "active", "to": "active", "event": "INIT",
            "effects": [ ["call-service", "browser-svc", "open", { "url": "https://example.com", "active": true }, { "emit": { "success": "SENT", "failure": "NOT_SENT" } }] ] },
          { "from": "active", "to": "active", "event": "ASK",
            "effects": [ ["call-service", "test-svc", "run", { "userPrompt": "hi" }, { "emit": { "success": "SENT", "failure": "NOT_SENT" } }] ] },
          { "from": "active", "to": "active", "event": "SENT" },
          { "from": "active", "to": "active", "event": "NOT_SENT" }
        ]
      }
    }],
    "pages": [ { "name": "Home", "path": "/", "traits": [ { "ref": "Pageview" } ] } ]
  }]
}"#;

fn options(runtimes: &[(&str, &[&str])]) -> CompilerOptions {
    CompilerOptions {
        skip_validation: true,
        service_runtimes: Some(
            runtimes
                .iter()
                .map(|(n, r)| (n.to_string(), r.iter().map(|x| x.to_string()).collect()))
                .collect::<HashMap<_, _>>(),
        ),
        ..CompilerOptions::default()
    }
}

#[test]
fn a_called_browser_only_service_is_stamped() {
    let oir = compile_json(PROGRAM, &options(&[("browser-svc", &["browser"]), ("test-svc", &["node", "browser"])])).expect("compiles");
    assert_eq!(oir.browser_services, vec!["browser-svc".to_string()]);
}

#[test]
fn control_a_service_that_also_runs_on_node_or_is_undeclared_is_not() {
    let oir = compile_json(PROGRAM, &options(&[("test-svc", &["node", "browser"])])).expect("compiles");
    assert!(oir.browser_services.is_empty(), "{:?}", oir.browser_services);
}

#[test]
fn control_an_uncalled_browser_only_service_is_not_stamped() {
    let oir = compile_json(PROGRAM, &options(&[("browser-svc", &["browser"]), ("clipboard", &["browser"])])).expect("compiles");
    assert_eq!(oir.browser_services, vec!["browser-svc".to_string()]);
}

fn credential_program(credential: &str) -> String {
    format!(
        r#"{{
  "name": "Site", "version": "1.0.0",
  "orbitals": [{{
    "name": "Visits",
    "entity": {{ "name": "Visit", "persistence": "runtime", "fields": [ {{ "name": "id", "type": "string", "required": true }} ] }},
    "traits": [{{
      "name": "Pageview", "linkedEntity": "Visit", 
      "config": {{ "token": {{ "type": "secret", "default": "SITE_TOKEN", "description": "The site token." }} }},
      "stateMachine": {{
        "states": [ {{ "name": "active", "isInitial": true }} ],
        "events": [ {{ "key": "INIT" }}, {{ "key": "OK" }} ],
        "transitions": [
          {{ "from": "active", "to": "active", "event": "INIT",
            "effects": [ ["http", "GET", "https://api.site.example", "/me", {{ "auth": {{ "scheme": "bearer", "credential": "{credential}" }} }}, {{ "emit": {{ "success": "OK" }} }}] ] }},
          {{ "from": "active", "to": "active", "event": "OK" }}
        ]
      }}
    }}],
    "pages": [ {{ "name": "Home", "path": "/", "traits": [ {{ "ref": "Pageview" }} ] }} ]
  }}]
}}"#
    )
}

#[test]
fn the_program_secret_knobs_its_http_auth_reads_are_stamped_as_declared_credentials() {
    let named = |credential: &str| -> Vec<(String, String, String)> {
        let oir = compile_json(&credential_program(credential), &options(&[])).expect("compiles");
        oir.program_credentials
            .iter()
            .map(|c| (c.group.clone(), c.env_var.clone(), c.description.clone()))
            .collect()
    };
    assert_eq!(
        named("@config.token"),
        vec![("Visits".to_string(), "SITE_TOKEN".to_string(), "The site token.".to_string())]
    );
    // A literal is never matched to a knob by its default (a name match is a heuristic; resolve keeps `@config.` reads).
    assert!(named("SITE_TOKEN").is_empty(), "control: a literal credential names no declared knob");
}

#[test]
fn control_a_program_with_no_http_auth_declares_no_credentials() {
    let oir = compile_json(PROGRAM, &options(&[])).expect("compiles");
    assert!(oir.program_credentials.is_empty());
}
