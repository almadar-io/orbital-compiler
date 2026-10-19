//! `os/` grants and placement (`docs/Almadar_Sandbox.md` Contracts §3): every `os/` call is covered by the
//! grants of the orbital that makes it (app level plus the orbital's own), its targets are literals, a declared
//! entry is well formed and used, and a `deployment` places every orbital exactly once. The matching is
//! `orbital_core_base::os_grants`'s; this module only walks the program.

use super::super::{trait_path, ErrorCode, Severity, ValidationError, ValidationResult};
use orbital_core_base::os::{split_envelope, OsOp};
use orbital_core_base::os_grants::{
    concat_static_prefix, os_requirements_deferring, OsGrants, OsRequirement,
};
use orbital_core_base::schema::{
    ConfigField, OrbitalSchema, SExpression, TraitDefinition, TraitReference,
};
use std::collections::{BTreeSet, HashMap};

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
        if transition
            .guard
            .as_ref()
            .is_some_and(SExpression::is_statically_false)
        {
            continue;
        }
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
        OsRequirement::FsTree { root, mode } => format!("fs \"{root}**\" {}", mode.name()),
        OsRequirement::Exec { command } => format!("exec {command:?}"),
        OsRequirement::Net { host, port, kinds } => {
            let kinds: Vec<&str> = kinds.iter().map(|k| k.name()).collect();
            format!("net \"{host}:{port}\" {}", kinds.join(" or "))
        }
        OsRequirement::Env { name } => format!("env {name:?}"),
        OsRequirement::Stdio { kind } => format!("stdio {}", kind.name()),
        OsRequirement::Sys { kind } => format!("sys {}", kind.name()),
        OsRequirement::Desktop { kind } => format!("desktop {}", kind.name()),
    }
}

fn suggestion(need: &OsRequirement) -> String {
    match need {
        OsRequirement::Fs { path, mode } => {
            format!("Add `fs: {{ {path:?}: [{}] }}` to the orbital's bracket (`orbital Name [ … ]`) or the app's", mode.name())
        }
        OsRequirement::FsTree { root, mode } => {
            format!("Add `fs: {{ \"{root}**\": [{}] }}` to the orbital's bracket (`orbital Name [ … ]`) or the app's", mode.name())
        }
        OsRequirement::Exec { command } => {
            format!("Add `exec: [{command:?}]` to the orbital's bracket")
        }
        OsRequirement::Net { host, port, kinds } => format!(
            "Add `net: {{ \"{host}:{port}\": [{}] }}` to the orbital's bracket",
            kinds.first().map_or("tcp", |k| k.name())
        ),
        OsRequirement::Env { name } => format!("Add `env: [{name:?}]` to the orbital's bracket"),
        OsRequirement::Stdio { kind } => {
            format!("Add `stdio: [{}]` to the orbital's bracket", kind.name())
        }
        OsRequirement::Sys { kind } => {
            format!("Add `sys: [{}]` to the orbital's bracket", kind.name())
        }
        OsRequirement::Desktop { kind } => {
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
                            .with_suggestion("Write the path, command, host, port or name as a literal; a file path may instead be a `str/concat` that starts with a literal or `@config.<knob>` directory ending in `/`, covered by an fs grant ending in `/**`"),
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

/// The values a config-fed `os/` target can take in the finished program, read from the trait's composed knob
/// defaults (a call-site override has already replaced the default). `@config.k.p` reads a knob and a path into it;
/// `array/nth` with a literal index takes that element and with any other index every element; `object/get` with a
/// literal key takes that field of each object. `None` for anything else; a knob with no value yields no candidate.
fn config_values(
    expr: &SExpression,
    config: &HashMap<String, ConfigField>,
) -> Option<Vec<SExpression>> {
    if let SExpression::Array(arr) = expr {
        if expr.is_call() {
            return match arr.first().and_then(SExpression::as_str)? {
                "array/nth" | "nth" => {
                    let mut found = Vec::new();
                    for source in config_values(arr.get(1)?, config)? {
                        let SExpression::Array(items) = source else {
                            return None;
                        };
                        match arr.get(2) {
                            Some(SExpression::Number(at)) if *at >= 0.0 && at.fract() == 0.0 => {
                                found.extend(items.get(*at as usize).cloned());
                            }
                            _ => found.extend(items),
                        }
                    }
                    Some(found)
                }
                "object/get" => {
                    let key = arr.get(2)?.as_str()?;
                    let mut found = Vec::new();
                    for source in config_values(arr.get(1)?, config)? {
                        let SExpression::Object(fields) = source else {
                            return None;
                        };
                        found.extend(fields.get(key).cloned());
                    }
                    Some(found)
                }
                _ => None,
            };
        }
    }
    let path = expr.binding_path()?;
    let mut parts = path.split('.');
    if parts.next() != Some("config") {
        return None;
    }
    let Some(mut value) = config.get(parts.next()?)?.default.clone() else {
        return Some(Vec::new());
    };
    for part in parts {
        let SExpression::Object(fields) = value else {
            return None;
        };
        value = fields.get(part)?.clone();
    }
    Some(vec![value])
}

/// The directory prefixes a `str/concat` file target can take in the finished program: its static prefix with
/// every config piece replaced by each string the knob can hold, as a `str/concat` of literals. `None` when a
/// piece cannot be resolved or a value is not a string.
fn concat_roots(
    expr: &SExpression,
    config: &HashMap<String, ConfigField>,
) -> Option<Vec<SExpression>> {
    let mut roots: Vec<Vec<SExpression>> = vec![vec![SExpression::String("str/concat".to_string())]];
    for piece in concat_static_prefix(expr, &config_rooted)? {
        let options = if config_rooted(piece) {
            config_values(piece, config)?
        } else {
            vec![piece.clone()]
        };
        let mut next = Vec::new();
        for root in &roots {
            for option in &options {
                match option {
                    SExpression::String(text) if !text.starts_with('@') => {
                        let mut grown = root.clone();
                        grown.push(option.clone());
                        next.push(grown);
                    }
                    _ => return None,
                }
            }
        }
        roots = next;
    }
    Some(roots.into_iter().map(SExpression::Array).collect())
}

/// `orb validate --final`: a config-fed `os/` target is a literal once the program is composed, so every value it
/// can take is checked against the orbital's grants like a written one; one the pass cannot resolve is not static.
pub(crate) fn validate_final_os_targets(schema: &OrbitalSchema) -> ValidationResult {
    let mut result = ValidationResult::new();
    for (i, orbital) in schema.orbitals.iter().enumerate() {
        let grants = schema.orbital_os_grants(orbital);
        for (j, trait_ref) in orbital.traits.iter().enumerate() {
            let TraitReference::Inline(trait_def) = trait_ref else {
                continue;
            };
            let config = trait_def.config.clone().unwrap_or_default();
            let path = trait_path(i, &orbital.name, j, trait_ref);
            for (effect, site) in effect_sites(trait_def, &path) {
                each_os_call(effect, &mut |op, args| {
                    let (positional, _) = split_envelope(op, args);
                    let written = os_requirements_deferring(op, positional, &config_rooted)
                        .unwrap_or_default();
                    for (at, arg) in positional.iter().enumerate().filter(|(_, a)| {
                        config_rooted(a)
                            || concat_static_prefix(a, &config_rooted)
                                .is_some_and(|prefix| prefix.iter().any(config_rooted))
                    }) {
                        let concat = concat_static_prefix(arg, &config_rooted).is_some();
                        let resolved = if concat {
                            concat_roots(arg, &config)
                        } else {
                            config_values(arg, &config)
                        };
                        let Some(values) = resolved else {
                            result.add_error(
                                ValidationError::error(
                                    ErrorCode::OsTargetNotStatic,
                                    site.clone(),
                                    format!(
                                        "`{}` argument {} is read from config in a way the finished program cannot resolve, so its grant cannot be checked",
                                        op.name(),
                                        at + 1
                                    ),
                                )
                                .with_suggestion("Read the target with `@config.<knob>`, `array/nth` and `object/get`, or write it as a literal; a `str/concat` directory prefix must be string knobs and literals ending in `/`"),
                            );
                            continue;
                        };
                        for value in values {
                            let mut substituted = positional.to_vec();
                            substituted[at] = value;
                            let found = match os_requirements_deferring(op, &substituted, &config_rooted) {
                                Ok(found) if !(concat && found.is_empty()) => found,
                                Err(_) if !concat => continue,
                                _ => {
                                    result.add_error(
                                        ValidationError::error(
                                            ErrorCode::OsTargetNotStatic,
                                            site.clone(),
                                            format!(
                                                "`{}` argument {} is a computed path the grant cannot cover: only a file path may be a `str/concat`",
                                                op.name(),
                                                at + 1
                                            ),
                                        )
                                        .with_suggestion("Write the target as a literal"),
                                    );
                                    break;
                                }
                            };
                            for need in found.into_iter().filter(|need| !written.contains(need)) {
                                if !grants.allows(&need) {
                                    result.add_error(
                                        ValidationError::error(
                                            ErrorCode::OsGrantMissing,
                                            site.clone(),
                                            format!(
                                                "`{}` can be given {} by its config, but orbital {} does not grant it",
                                                op.name(),
                                                describe(&need),
                                                orbital.name
                                            ),
                                        )
                                        .with_suggestion(suggestion(&need)),
                                    );
                                }
                            }
                        }
                    }
                });
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
