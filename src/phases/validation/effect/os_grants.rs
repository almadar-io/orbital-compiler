//! `os/` grants and placement (`docs/Almadar_Sandbox.md` Contracts §3): every `os/` call is covered by the
//! grants of the orbital that makes it (app level plus the orbital's own), its targets are literals, a declared
//! entry is well formed and used, and a `deployment` places every orbital exactly once. The matching is
//! `orbital_core_base::os_grants`'s; this module only walks the program.

use super::super::{trait_path, ErrorCode, Severity, ValidationError, ValidationResult};
use orbital_core_base::os::{split_envelope, OsOp};
use orbital_core_base::os_grants::{os_requirements_deferring, OsGrants, OsRequirement};
use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitDefinition, TraitReference};
use std::collections::BTreeSet;

fn each_os_call<'a>(expr: &'a SExpression, visit: &mut dyn FnMut(OsOp, &'a [SExpression])) {
    match expr {
        SExpression::Array(arr) => {
            if let Some(SExpression::String(head)) = arr.first() {
                if let Some(op) = OsOp::from_name(head) {
                    visit(op, &arr[1..]);
                }
            }
            for item in arr {
                each_os_call(item, visit);
            }
        }
        SExpression::Object(obj) => {
            for v in obj.values() {
                each_os_call(v, visit);
            }
        }
        _ => {}
    }
}

/// A target read from the composing call site's `config`: the binding `@config.*`, or a call whose data
/// argument (its first) is one. A config value is a literal once the program is composed, so the target is
/// not checkable here and the host and io-gate enforce the grant when the call runs.
fn config_rooted(expr: &SExpression) -> bool {
    match expr {
        SExpression::Array(arr) if expr.is_call() => arr.get(1).is_some_and(config_rooted),
        _ => expr
            .binding_path()
            .is_some_and(|path| path.split('.').next() == Some("config")),
    }
}

fn effect_sites<'a>(trait_def: &'a TraitDefinition, path: &str) -> Vec<(&'a SExpression, String)> {
    let mut sites = Vec::new();
    for (i, transition) in trait_def.state_machine.transitions.iter().enumerate() {
        for (j, effect) in transition.effects.iter().flatten().enumerate() {
            sites.push((
                effect,
                format!("{path}.stateMachine.transitions[{i}].effects[{j}]"),
            ));
        }
    }
    for (i, tick) in trait_def.ticks.iter().flatten().enumerate() {
        for (j, effect) in tick.effects.iter().enumerate() {
            sites.push((effect, format!("{path}.ticks[{i}].effects[{j}]")));
        }
    }
    sites
}

fn describe(need: &OsRequirement) -> String {
    match need {
        OsRequirement::Fs { path, mode } => format!("fs {path:?} {}", mode.name()),
        OsRequirement::Exec(command) => format!("exec {command:?}"),
        OsRequirement::Net { host, port, kinds } => {
            let kinds: Vec<&str> = kinds.iter().map(|k| k.name()).collect();
            format!("net \"{host}:{port}\" {}", kinds.join(" or "))
        }
        OsRequirement::Env(name) => format!("env {name:?}"),
        OsRequirement::Stdio(kind) => format!("stdio {}", kind.name()),
        OsRequirement::Sys(kind) => format!("sys {}", kind.name()),
        OsRequirement::Desktop(kind) => format!("desktop {}", kind.name()),
    }
}

fn suggestion(need: &OsRequirement) -> String {
    match need {
        OsRequirement::Fs { path, mode } => {
            format!("Add `fs: {{ {path:?}: [{}] }}` to the orbital's bracket (`orbital Name [ … ]`) or the app's", mode.name())
        }
        OsRequirement::Exec(command) => {
            format!("Add `exec: [{command:?}]` to the orbital's bracket")
        }
        OsRequirement::Net { host, port, kinds } => format!(
            "Add `net: {{ \"{host}:{port}\": [{}] }}` to the orbital's bracket",
            kinds.first().map_or("tcp", |k| k.name())
        ),
        OsRequirement::Env(name) => format!("Add `env: [{name:?}]` to the orbital's bracket"),
        OsRequirement::Stdio(kind) => {
            format!("Add `stdio: [{}]` to the orbital's bracket", kind.name())
        }
        OsRequirement::Sys(kind) => {
            format!("Add `sys: [{}]` to the orbital's bracket", kind.name())
        }
        OsRequirement::Desktop(kind) => {
            format!("Add `desktop: [{}]` to the orbital's bracket", kind.name())
        }
    }
}

fn unused(path: String, label: &str, owner: &str) -> ValidationError {
    let mut warning = ValidationError::error(
        ErrorCode::OsGrantUnused,
        path,
        format!("`{label}` is granted to {owner} but no `os/` call there uses it"),
    )
    .with_suggestion("Remove the entry: an unused grant is authority the program does not need");
    warning.severity = Severity::Warning;
    warning
}

fn check_entries(grants: &OsGrants, at: &str, result: &mut ValidationResult) {
    for problem in grants.entry_problems() {
        result.add_error(ValidationError::error(
            ErrorCode::OsGrantEntryInvalid,
            format!("{at}.{}", problem.path),
            problem.message,
        ));
    }
}

fn used_by_any(atom: &OsGrants, needs: &[OsRequirement]) -> bool {
    needs.iter().any(|need| atom.allows(need))
}

pub(crate) fn validate_os_grants(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    if let Some(app) = &schema.os_grants {
        check_entries(app, "osGrants", &mut result);
    }
    let mut needs_by_orbital: Vec<Vec<OsRequirement>> = Vec::new();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let at = format!("orbitals[{i} ({})]", orbital.name);
        if let Some(own) = &orbital.os_grants {
            check_entries(own, &format!("{at}.osGrants"), &mut result);
        }
        let grants = schema.orbital_os_grants(orbital);
        let mut needs: Vec<OsRequirement> = Vec::new();
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let path = trait_path(i, &orbital.name, j, trait_ref);
            for (effect, site) in effect_sites(trait_def, &path) {
                each_os_call(effect, &mut |op, args| {
                    let (positional, _) = split_envelope(op, args);
                    match os_requirements_deferring(op, positional, &config_rooted) {
                        Err(arg) => result.add_error(
                            ValidationError::error(
                                ErrorCode::OsTargetNotStatic,
                                site.clone(),
                                format!(
                                    "`{}` argument {} is computed — an `os/` target must be a literal so the grant can be checked at build time",
                                    op.name(),
                                    arg + 1
                                ),
                            )
                            .with_suggestion("Write the path, command, host, port or name as a literal"),
                        ),
                        Ok(found) => {
                            for need in found {
                                if !grants.allows(&need) {
                                    result.add_error(
                                        ValidationError::error(
                                            ErrorCode::OsGrantMissing,
                                            site.clone(),
                                            format!(
                                                "`{}` needs {} but orbital {} does not grant it",
                                                op.name(),
                                                describe(&need),
                                                orbital.name
                                            ),
                                        )
                                        .with_suggestion(suggestion(&need)),
                                    );
                                }
                                needs.push(need);
                            }
                        }
                    }
                });
            }
        }
        if let Some(own) = &orbital.os_grants {
            for atom in own.atoms() {
                if !used_by_any(&atom.grants, &needs) {
                    result.add_error(unused(
                        format!("{at}.osGrants.{}", atom.path),
                        &atom.label,
                        &format!("orbital {}", orbital.name),
                    ));
                }
            }
        }
        needs_by_orbital.push(needs);
    }
    if let Some(app) = &schema.os_grants {
        let all: Vec<OsRequirement> = needs_by_orbital.into_iter().flatten().collect();
        for atom in app.atoms() {
            if !used_by_any(&atom.grants, &all) {
                result.add_error(unused(
                    format!("osGrants.{}", atom.path),
                    &atom.label,
                    "the program",
                ));
            }
        }
    }
    result
}

pub(crate) fn validate_placement(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    let Some(units) = &schema.deployment else {
        return result;
    };
    let declared: BTreeSet<&str> = schema.orbitals.iter().map(|o| o.name.as_str()).collect();
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    for (unit, orbitals) in units {
        let at = format!("deployment.{unit}");
        if orbitals.is_empty() {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PlacementUnitEmpty,
                    at.clone(),
                    format!("deployment unit `{unit}` lists no orbital"),
                )
                .with_suggestion("List the orbitals the unit runs, or remove the unit"),
            );
        }
        for name in orbitals {
            if !declared.contains(name.as_str()) {
                result.add_error(ValidationError::error(
                    ErrorCode::PlacementOrbitalUnknown,
                    at.clone(),
                    format!("deployment unit `{unit}` lists `{name}`, which is not an orbital of this program"),
                ));
            } else if !placed.insert(name.as_str()) {
                result.add_error(
                    ValidationError::error(
                        ErrorCode::PlacementOrbitalDuplicate,
                        at.clone(),
                        format!("orbital `{name}` is placed more than once in `deployment`"),
                    )
                    .with_suggestion("An orbital runs in exactly one unit"),
                );
            }
        }
    }
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        if !placed.contains(orbital.name.as_str()) {
            result.add_error(
                ValidationError::error(
                    ErrorCode::PlacementOrbitalUnplaced,
                    format!("orbitals[{i} ({})]", orbital.name),
                    format!(
                        "`deployment` is declared but orbital `{}` is in none of its units",
                        orbital.name
                    ),
                )
                .with_suggestion(
                    "Place every orbital in a unit; nothing is placed on the program's behalf",
                ),
            );
        }
    }
    result
}
