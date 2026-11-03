//! The validation report: one pipeline over a parsed program (lazy children, inline, enrich, comprehensive
//! validation, OIR) that yields the structured findings every renderer reads. Source positions come through
//! [`SourceLocator`], so the parser that owns the source text stays a sibling, not a dependency.

use super::validate_comprehensive_full;
use crate::phases::inline::{
    inline_schema_with_orbital_diagnostics, InlineError, InlineSession,
};
use crate::phases::{compile_schema, enrich_schema, ExtraPatterns, SchemaLanguage};
use crate::CompilerOptions;
use orbital_core_base::ir::{OirValidator, ValidationSeverity};
use orbital_core_base::logging::ns;
use orbital_core_base::schema::OrbitalSchema;
use std::path::Path;

/// A place in the source text: 1-based `line` and `col`, `len` characters long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub line: u32,
    pub col: u32,
    pub len: u32,
}

/// Maps a finding's L2 path (and a failing `uses` import) back to the source the program was written in.
pub trait SourceLocator {
    /// The span for `path` and whether it is exact (`false`: the nearest tracked ancestor's).
    fn locate(&self, path: &str) -> Option<(SourceSpan, bool)>;
    /// The span of the `uses` line declaring `import`.
    fn import_span(&self, import: &str) -> Option<SourceSpan>;
}

/// [`SourceSpan`] as the 1-based range a finding reports; `approximate` when the span is the enclosing construct's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub approximate: bool,
}

impl SourcePosition {
    pub fn of(span: SourceSpan, exact: bool) -> Self {
        Self {
            line: span.line,
            column: span.col,
            end_line: span.line,
            end_column: span.col + span.len.max(1),
            approximate: !exact,
        }
    }
}

/// One validation finding: the one model every renderer reads.
#[derive(Debug, Clone, Default)]
pub struct Finding {
    pub code: String,
    pub path: String,
    pub message: String,
    pub suggestion: Option<String>,
    pub fix_guidance: Option<String>,
    pub expected_shape: Option<String>,
    pub valid_values: Option<Vec<String>>,
    pub position: Option<SourcePosition>,
    pub needs_install: Option<bool>,
    /// The entity the finding is about, set by the phase that raised it: for an unresolved-reference code, the
    /// entity name that did not resolve; otherwise the entity the finding sits on.
    pub entity: Option<String>,
    /// The field of `entity` the finding names, where the raising phase knows it.
    pub field: Option<String>,
}

impl Finding {
    pub fn new(code: String, path: String, message: String, suggestion: Option<String>) -> Self {
        Self { code, path, message, suggestion, ..Self::default() }
    }

    /// The human line: `[code] path: message @ line:col`, then the suggestion and the fix guidance. A finding with no
    /// code is its message alone.
    pub fn render(&self) -> String {
        if self.code.is_empty() {
            return self.message.clone();
        }
        let mut line = if self.path.is_empty() {
            format!("[{}] {}", self.code, self.message)
        } else {
            format!("[{}] {}: {}", self.code, self.path, self.message)
        };
        if let Some(position) = &self.position {
            line.push_str(&format!(" @ {}:{}", position.line, position.column));
        }
        if let Some(suggestion) = &self.suggestion {
            line.push_str(&format!(" \u{1f4a1} {suggestion}"));
        }
        if let Some(guidance) = &self.fix_guidance {
            line.push_str(&format!(" {guidance}"));
        }
        line
    }
}

/// Counts of what a validated program holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub orbitals: usize,
    pub entities: usize,
    pub traits: usize,
    pub pages: usize,
}

/// What validating one program found.
#[derive(Debug, Default)]
pub struct Report {
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
    pub stats: Option<Stats>,
}

impl Report {
    /// A report whose one finding stopped validation before the program was evaluated.
    pub fn refused(finding: Finding) -> Self {
        Self { errors: vec![finding], warnings: Vec::new(), stats: None }
    }
}

/// The one pass rule: no errors, and no warnings either under the 0/0 bar.
pub fn passes(errors: usize, warnings: usize, deny_warnings: bool) -> bool {
    errors == 0 && (!deny_warnings || warnings == 0)
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    Error,
    Warning,
}

/// A finding a std lint program reports for the file, placed in the report beside the compiler's.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProgramFinding {
    pub code: String,
    pub path: String,
    pub message: String,
    #[serde(default)]
    pub suggestion: String,
    pub severity: FindingSeverity,
    /// The accepted values, when the finding names a closed set (a field type, an enum member).
    #[serde(default, rename = "validValues")]
    pub valid_values: Option<Vec<String>>,
    #[serde(default)]
    pub entity: Option<String>,
    #[serde(default)]
    pub field: Option<String>,
}

/// Everything the pipeline reads about one parsed program.
pub struct ValidationInput<'a> {
    pub schema: OrbitalSchema,
    /// The folder whose `locales/` and `public` dir the program is checked against; `None` skips both.
    pub resources_beside: Option<&'a Path>,
    pub locator: Option<&'a dyn SourceLocator>,
    pub language: SchemaLanguage,
    pub path: &'a Path,
    pub extra_patterns: Option<ExtraPatterns>,
    pub service_registry: crate::embedded::ServiceRegistryData,
    pub program_findings: &'a [ProgramFinding],
}

/// The finding for a failed behavior-package discovery, checked before a program is parsed: `from_env` runs discovery
/// unconditionally and an empty package list must not read as "none installed".
pub fn discovery_refusal(options: &CompilerOptions) -> Option<Finding> {
    let error = options.discovery_error.as_ref()?;
    Some(Finding {
        code: discovery_error_code(error).to_string(),
        message: error.to_string(),
        suggestion: Some(
            "Validator environment error \u{2014} the program was NOT evaluated; retry, do not edit the program".to_string(),
        ),
        ..Finding::default()
    })
}

fn discovery_error_code(e: &orbital_core_base::behavior_packages::DiscoverError) -> &'static str {
    e.code()
}

/// True when an inline error names a real `uses` import nothing on the `discover()` walk answers, as opposed to a
/// broken loader environment: the install-actionable case.
fn inline_error_needs_install(e: &InlineError) -> bool {
    use crate::loader::LoaderError;
    matches!(
        e,
        InlineError::Loader(LoaderError::PackageNotInstalled { .. })
            | InlineError::Loader(LoaderError::BehaviorPackageNotInstalled { .. })
            | InlineError::Loader(LoaderError::BehaviorNotFound { .. })
    )
}

/// A finding's fix guidance, with the approximate-location note when its span is only the nearest tracked ancestor.
fn located_guidance(resolved: Option<&(SourceSpan, bool)>, existing: Option<&str>) -> Option<String> {
    match resolved {
        Some((_, false)) => {
            let note = "Location is approximate (nearest tracked source line). If the shown line only references a config knob or another trait, the failing expression lives where that value is DECLARED (e.g. the trait's config block default) \u{2014} search the file for the expression named in the message.";
            Some(match existing {
                Some(existing) => format!("{existing} {note}"),
                None => note.to_string(),
            })
        }
        _ => existing.map(str::to_string),
    }
}

fn located_finding(
    locator: Option<&dyn SourceLocator>,
    code: String,
    path: String,
    message: String,
    suggestion: Option<String>,
    guidance: Option<&str>,
) -> Finding {
    let resolved = locator.and_then(|l| l.locate(&path));
    Finding {
        fix_guidance: located_guidance(resolved.as_ref(), guidance),
        position: resolved.map(|(span, exact)| SourcePosition::of(span, exact)),
        code,
        path,
        message,
        suggestion,
        ..Finding::default()
    }
}

/// `locales/<locale>/<app>.json` beside `path`, one entry per locale folder plus every declared locale.
fn load_catalogs(
    path: &Path,
    name: &str,
    declared: &[String],
) -> std::collections::BTreeMap<String, Result<Option<serde_json::Value>, String>> {
    let dir = path.parent().unwrap_or(Path::new(".")).join("locales");
    let mut locales: std::collections::BTreeSet<String> = declared.iter().cloned().collect();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().join(format!("{name}.json")).is_file() {
                locales.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    locales
        .into_iter()
        .map(|locale| {
            let file = dir.join(&locale).join(format!("{name}.json"));
            let parsed = if file.is_file() {
                std::fs::read_to_string(&file)
                    .map_err(|e| e.to_string())
                    .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
                    .map(Some)
            } else {
                Ok(None)
            };
            (locale, parsed)
        })
        .collect()
}

fn secret_flow_finding(violation: &orbital_core_base::secrets::SecretFlowViolation, path: String) -> Finding {
    Finding::new(
        violation.code().to_string(),
        path,
        violation.message(),
        Some(violation.suggestion().to_string()),
    )
}

fn own_secret_findings(schema: &OrbitalSchema) -> Vec<Finding> {
    let Ok(program) = serde_json::to_value(schema) else {
        return Vec::new();
    };
    orbital_core_base::secrets::secret_flow_violations(&program)
        .iter()
        .map(|v| secret_flow_finding(v, v.path().to_string()))
        .collect()
}

/// The secret-flow rules over every behavior file `uses` loaded: an atom types its own knobs, so it is walked as
/// parsed, not as inlined.
fn used_secret_findings(session: &InlineSession) -> Vec<Finding> {
    let mut findings = Vec::new();
    session.each_loaded_schema(&mut |loaded| {
        let Ok(program) = serde_json::to_value(&loaded.schema) else {
            return;
        };
        for v in orbital_core_base::secrets::secret_flow_violations(&program) {
            findings.push(secret_flow_finding(&v, format!("{}: {}", loaded.schema.name, v.path())));
        }
    });
    findings
}

/// The `ORB_I18N_*` rules over a program's own `i18n/t` calls and its catalogs.
fn catalog_findings(schema: &OrbitalSchema, path: &Path) -> Vec<Finding> {
    use orbital_core_base::messages::{check_catalogs, CatalogIssue};
    let declared = schema.locales.clone().unwrap_or_default();
    let mut catalogs = std::collections::BTreeMap::new();
    let mut issues = Vec::new();
    for (locale, loaded) in load_catalogs(path, &schema.name, &declared) {
        match loaded {
            Ok(value) => {
                catalogs.insert(locale, value);
            }
            Err(detail) => issues.push(CatalogIssue::CatalogInvalid { locale, detail }),
        }
    }
    let Ok(program) = serde_json::to_value(schema) else {
        return Vec::new();
    };
    issues.extend(check_catalogs(&program, &schema.name, schema.locales.as_deref(), &catalogs));
    issues
        .into_iter()
        .map(|issue| Finding::new(issue.code().to_string(), "locales".to_string(), issue.message(), None))
        .collect()
}

struct Core {
    errors: Vec<Finding>,
    warnings: Vec<Finding>,
    inlined: Option<OrbitalSchema>,
}

/// Validates every `uses lazy` behavior split out of `schema`, records its pages on `schema`, and returns its
/// findings with paths prefixed `lazy[<Orbital>]`.
fn validate_lazy_children(
    schema: &mut OrbitalSchema,
    language: &SchemaLanguage,
    path: &Path,
    extra_patterns: &Option<ExtraPatterns>,
    service_registry: &crate::embedded::ServiceRegistryData,
    options: &CompilerOptions,
) -> (Vec<Finding>, Vec<Finding>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for artifact in crate::phases::lazy::split_lazy(schema) {
        let child = ValidationInput {
            schema: artifact.schema.clone(),
            resources_beside: None,
            locator: None,
            language: language.clone(),
            path,
            extra_patterns: extra_patterns.clone(),
            service_registry: service_registry.clone(),
            program_findings: &[],
        };
        let core = validate_core(child, options);
        let prefix = |f: Finding| Finding { path: format!("lazy[{}].{}", artifact.orbital, f.path), ..f };
        errors.extend(core.errors.into_iter().map(prefix));
        warnings.extend(core.warnings.into_iter().map(prefix));
        if let Some(inlined) = &core.inlined {
            if let Err(e) = crate::phases::lazy::record_lazy_pages(schema, &artifact, inlined) {
                errors.push(Finding::new(
                    e.structured_code().to_string(),
                    format!("lazy[{}]", artifact.orbital),
                    e.to_string(),
                    None,
                ));
            }
        }
    }
    (errors, warnings)
}

fn inline_failure(e: &InlineError, options: &CompilerOptions, locator: Option<&dyn SourceLocator>) -> Finding {
    use crate::loader::LoaderError;
    let is_infra = matches!(
        e,
        InlineError::Loader(LoaderError::StdLibNotConfigured)
            | InlineError::Loader(LoaderError::ScopedPackageNotConfigured(_))
            | InlineError::Loader(LoaderError::BehaviorAmbiguous { .. })
            | InlineError::Loader(LoaderError::PrefixConflict { .. })
    );
    let needs_install = inline_error_needs_install(e);
    let mut message = format!("Failed to resolve external references: {e}");
    if is_infra {
        let lc = &options.inline.loader_config;
        message.push_str(&format!(
            " [loader env: installed_packages={}, std_lib_path_set={}]",
            lc.packages.len(),
            lc.std_lib_path.is_some(),
        ));
    }
    let import = match e {
        InlineError::Loader(LoaderError::InvalidPrimitiveImport(s)) => Some(s.as_str()),
        InlineError::Loader(LoaderError::StdImportNotCanonical { given, .. }) => Some(given.as_str()),
        InlineError::Loader(LoaderError::BehaviorNotFound { name, .. }) => Some(name.as_str()),
        InlineError::Loader(LoaderError::PackageNotInstalled { prefix }) => Some(prefix.as_str()),
        _ => None,
    };
    let position = import
        .and_then(|import| locator?.import_span(import))
        .map(|span| SourcePosition::of(span, false));
    Finding {
        code: if is_infra { "INLINE_INFRA_ERROR".to_string() } else { e.structured_code().to_string() },
        path: e.structured_path().to_string(),
        message,
        suggestion: Some(if is_infra {
            "Validator environment error \u{2014} the schema was NOT evaluated; retry, do not edit the schema".to_string()
        } else if needs_install {
            "Run `orb add <package>` for the package that declares this behavior, then retry \u{2014} the schema was NOT evaluated.".to_string()
        } else {
            "Check `uses:` imports resolve to known behaviors".to_string()
        }),
        fix_guidance: Some(
            "(schema not fully resolved \u{2014} downstream validation suppressed until this is fixed)".to_string(),
        ),
        position,
        needs_install: needs_install.then_some(true),
        ..Finding::default()
    }
}

fn validate_core(input: ValidationInput<'_>, options: &CompilerOptions) -> Core {
    let ValidationInput {
        mut schema,
        resources_beside,
        locator,
        language,
        path,
        extra_patterns,
        service_registry,
        program_findings,
    } = input;
    let (mut errors, mut warnings) = tracing::info_span!(target: ns::COMPILER, "lazy children").in_scope(|| {
        validate_lazy_children(&mut schema, &language, path, &extra_patterns, &service_registry, options)
    });
    errors.extend(own_secret_findings(&schema));
    if let Some(resources) = resources_beside {
        errors.extend(catalog_findings(&schema, resources));
        if let Some(public) = &schema.public_dir {
            if let Err(message) = crate::phases::public::resolve_public_dir(resources, public) {
                errors.push(Finding::new("ORB_PUBLIC_DIR_INVALID".to_string(), "publicDir".to_string(), message, None));
            }
        }
    }

    // An inline failure is fatal for this run: validators index inline traits only, so validating a half-inlined
    // schema manufactures spurious findings that bury the real cause.
    let session = InlineSession::default();
    let mut inline_opts = options.inline.clone();
    inline_opts.session = Some(session.clone());
    let resolved_input = match crate::phases::already_resolved(&schema, Some(path)) {
        Ok(resolved) => resolved,
        Err(e) => {
            errors.push(Finding::new("ORB_RESOLVED_CONTRACT".to_string(), String::new(), e.to_string(), None));
            return Core { errors, warnings, inlined: None };
        }
    };
    if !resolved_input {
        let (mut orbital_refs, mut uses_configs) = (Vec::new(), Vec::new());
        let inlined = tracing::info_span!(target: ns::COMPILER, "inline").in_scope(|| {
            inline_schema_with_orbital_diagnostics(&mut schema, &inline_opts, Some(path), &mut orbital_refs, &mut uses_configs)
        });
        if let Err(e) = inlined {
            errors.push(inline_failure(&e, options, locator));
            return Core { errors, warnings, inlined: None };
        }
        tracing::info_span!(target: ns::COMPILER, "enrich").in_scope(|| enrich_schema(&mut schema, &options.enrichment));
    }

    errors.extend(used_secret_findings(&session));

    if let Ok(program) = serde_json::to_value(&schema) {
        for path in orbital_core_base::messages::render_only_violations(&program) {
            errors.push(Finding::new(
                "ORB_I18N_RENDER_ONLY".to_string(),
                path,
                "`i18n/t` and `@locale` are render-time \u{2014} use them only inside `render-ui`".to_string(),
                None,
            ));
        }
    }

    let comprehensive = tracing::info_span!(target: ns::COMPILER, "validate").in_scope(|| {
        validate_comprehensive_full(&schema, extra_patterns, language, service_registry, options.inline.loader_config.packages.clone())
    });
    errors.extend(comprehensive.errors.iter().map(|e| {
        let mut f = located_finding(
            locator,
            e.code.to_string(),
            e.path.clone(),
            e.message.clone(),
            e.suggestion.clone(),
            e.fix_guidance.as_deref(),
        );
        f.expected_shape = e.expected_shape.clone();
        f.valid_values = e.valid_values.clone();
        f.entity = e.entity.clone();
        f.field = e.field.clone();
        f
    }));
    warnings.extend(comprehensive.warnings.iter().map(|w| {
        let mut f =
            located_finding(locator, w.code.to_string(), w.path.clone(), w.message.clone(), w.suggestion.clone(), None);
        f.entity = w.entity.clone();
        f.field = w.field.clone();
        f
    }));

    for pf in program_findings {
        let mut f = located_finding(
            locator,
            pf.code.clone(),
            pf.path.clone(),
            pf.message.clone(),
            Some(pf.suggestion.clone()).filter(|s| !s.is_empty()),
            None,
        );
        f.valid_values = pf.valid_values.clone();
        f.entity = pf.entity.clone();
        f.field = pf.field.clone();
        match pf.severity {
            FindingSeverity::Error => errors.push(f),
            FindingSeverity::Warning => warnings.push(f),
        }
    }

    // Compile runs only on an accepted program: a rejected one would get its root cause restated as COMPILATION_ERROR.
    let inlined = schema.clone();
    let mut compile_options = CompilerOptions::from_env();
    compile_options.inline.skip_inline = true;
    if errors.is_empty() {
        match tracing::info_span!(target: ns::COMPILER, "compile").in_scope(|| compile_schema(schema, &compile_options)) {
            Ok(module) => {
                for e in &OirValidator::new(&module).validate().errors {
                    let is_error = e.severity == ValidationSeverity::Error;
                    let finding = Finding {
                        code: if is_error { "OIR_ERROR" } else { "OIR_WARNING" }.to_string(),
                        path: e.path.clone(),
                        message: e.message.clone(),
                        ..Finding::default()
                    };
                    if is_error { errors.push(finding) } else { warnings.push(finding) }
                }
                for w in crate::type_infer::collect_guard_type_warnings(&module) {
                    warnings.push(Finding { message: w, ..Finding::default() });
                }
                for e in crate::type_infer::collect_listen_key_type_errors(&module) {
                    errors.push(Finding { code: "ORB_X_LISTEN_KEY_NOT_STRING".to_string(), message: e, ..Finding::default() });
                }
            }
            Err(e) => errors.push(Finding::new(
                "COMPILATION_ERROR".to_string(),
                String::new(),
                format!("Compilation error: {e}"),
                None,
            )),
        }
    }
    Core { errors, warnings, inlined: Some(inlined) }
}

/// Validates one parsed program: the report every renderer (text, JSON, `orb mcp`, `orb compile`'s gate) reads.
/// Never bails on a finding; a failure that stops validation is the report's one error.
pub fn validate_parsed(input: ValidationInput<'_>, options: &CompilerOptions) -> Report {
    if let Some(refused) = discovery_refusal(options) {
        return Report::refused(refused);
    }
    let core = validate_core(input, options);
    let stats = core.inlined.as_ref().map(|inlined| Stats {
        orbitals: inlined.orbitals.len(),
        entities: inlined.orbitals.len(),
        traits: inlined.orbitals.iter().map(|o| o.traits.len()).sum(),
        pages: inlined.orbitals.iter().filter_map(|o| o.pages.as_ref()).map(|p| p.len()).sum(),
    });
    Report { errors: core.errors, warnings: core.warnings, stats }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orb(transition_to: &str) -> OrbitalSchema {
        let text = format!(
            r#"{{"name":"probe","version":"1.0.0","orbitals":[{{"name":"Probe",
            "entity":{{"name":"Item","persistence":"runtime","fields":[{{"name":"id","type":"string","required":true}}]}},
            "traits":[{{"name":"Counter","linkedEntity":"Item","stateMachine":{{
              "states":[{{"name":"idle","isInitial":true}}],"events":[{{"key":"GO"}}],
              "transitions":[{{"from":"idle","to":"{transition_to}","event":"GO","effects":[]}}]}}}}]}}]}}"#
        );
        orbital_core_base::schema::parse_schema(&text).expect("fixture parses")
    }

    struct EveryPathAt(SourceSpan, bool);

    impl SourceLocator for EveryPathAt {
        fn locate(&self, _: &str) -> Option<(SourceSpan, bool)> {
            Some((self.0, self.1))
        }
        fn import_span(&self, _: &str) -> Option<SourceSpan> {
            None
        }
    }

    fn report(schema: OrbitalSchema, locator: Option<&dyn SourceLocator>, findings: &[ProgramFinding]) -> Report {
        let path = std::env::temp_dir().join("orb-validation-report.orb");
        validate_parsed(
            ValidationInput {
                schema,
                resources_beside: None,
                locator,
                language: SchemaLanguage::English,
                path: &path,
                extra_patterns: None,
                service_registry: crate::embedded::baked_service_registry().clone(),
                program_findings: findings,
            },
            &CompilerOptions::from_env(),
        )
    }

    #[test]
    fn an_undefined_target_state_is_an_error_and_a_defined_one_is_not() {
        let broken = report(orb("ghost"), None, &[]);
        assert!(!broken.errors.is_empty());
        let clean = report(orb("idle"), None, &[]);
        assert!(clean.errors.is_empty(), "{:?}", clean.errors.iter().map(Finding::render).collect::<Vec<_>>());
        assert_eq!(clean.stats.map(|s| s.orbitals), Some(1));
    }

    fn lint() -> ProgramFinding {
        ProgramFinding {
            code: "LINT_X".into(),
            path: "orbitals[0]".into(),
            message: "lint".into(),
            suggestion: String::new(),
            severity: FindingSeverity::Warning,
            valid_values: None,
            entity: None,
            field: None,
        }
    }

    #[test]
    fn findings_take_their_position_from_the_locator() {
        let span = SourceSpan { line: 7, col: 3, len: 4 };
        let exact = report(orb("idle"), Some(&EveryPathAt(span, true)), &[lint()]);
        let position = exact.warnings.iter().find(|f| f.code == "LINT_X").and_then(|f| f.position).expect("located");
        assert_eq!((position.line, position.column, position.end_column, position.approximate), (7, 3, 7, false));
        let near = report(orb("idle"), Some(&EveryPathAt(span, false)), &[lint()]);
        let located = near.warnings.iter().find(|f| f.code == "LINT_X").expect("lint");
        assert!(located.position.expect("located").approximate);
        let control = report(orb("idle"), None, &[lint()]);
        assert!(control.warnings.iter().find(|f| f.code == "LINT_X").expect("lint").position.is_none());
    }

    #[test]
    fn a_program_finding_joins_the_errors_or_the_warnings_by_its_severity() {
        let finding = |severity| ProgramFinding {
            code: "LINT_X".into(),
            path: "orbitals[0]".into(),
            message: "lint".into(),
            suggestion: String::new(),
            severity,
            valid_values: None,
            entity: None,
            field: None,
        };
        let as_error = report(orb("idle"), None, &[finding(FindingSeverity::Error)]);
        assert_eq!(as_error.errors.iter().filter(|f| f.code == "LINT_X").count(), 1);
        let as_warning = report(orb("idle"), None, &[finding(FindingSeverity::Warning)]);
        assert!(as_warning.errors.is_empty());
        assert_eq!(as_warning.warnings.iter().filter(|f| f.code == "LINT_X").count(), 1);
    }

    #[test]
    fn a_program_finding_keeps_its_entity_and_field_in_the_report() {
        let named: ProgramFinding = serde_json::from_str(
            r#"{"code":"LINT_X","path":"orbitals[0]","message":"m","severity":"error","entity":"Other","field":"ownerId"}"#,
        )
        .unwrap();
        let bare: ProgramFinding =
            serde_json::from_str(r#"{"code":"LINT_Y","path":"orbitals[0]","message":"m","severity":"error"}"#).unwrap();
        let found = report(orb("idle"), None, &[named, bare]);
        let named = found.errors.iter().find(|f| f.code == "LINT_X").expect("named");
        assert_eq!(
            (named.entity.as_deref(), named.field.as_deref()),
            (Some("Other"), Some("ownerId"))
        );
        let bare = found.errors.iter().find(|f| f.code == "LINT_Y").expect("control");
        assert!(bare.entity.is_none() && bare.field.is_none());
    }

    #[test]
    fn warnings_fail_only_under_the_zero_zero_bar() {
        assert!(passes(0, 3, false));
        assert!(!passes(0, 1, true));
        assert!(!passes(1, 0, false));
        assert!(passes(0, 0, true));
    }

    #[test]
    fn the_text_line_is_rendered_from_the_finding_alone() {
        let finding = Finding {
            code: "ORB_X".into(),
            path: "orbitals.A".into(),
            message: "broken".into(),
            suggestion: Some("fix it".into()),
            ..Finding::default()
        };
        assert_eq!(finding.render(), "[ORB_X] orbitals.A: broken \u{1f4a1} fix it");
        let pathless = Finding { path: String::new(), suggestion: None, ..finding };
        assert_eq!(pathless.render(), "[ORB_X] broken");
        let codeless = Finding { code: String::new(), message: "guard".into(), ..pathless };
        assert_eq!(codeless.render(), "guard");
    }
}
