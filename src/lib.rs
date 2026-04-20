mod custom_predicates;

use anyhow::{Context, Result, anyhow};
use glob::Pattern;
use miette::{LabeledSpan, NamedSource, Severity, miette};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::{fs, io::Write, path::Path};
use tracing::debug;
use tree_sitter::{Node, Parser, Query, QueryCapture, QueryCursor, Range, StreamingIterator};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Pretty,
    Text,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSeverity {
    Warning,
    #[default]
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub format: OutputFormat,
    pub fix: bool,
    pub severity_override: Option<RuleSeverity>,
}

/// Raw TOML shape for a rule before glob compilation. Kept separate from [`Rule`] so the
/// public type can carry compiled `glob::Pattern` values directly, eliminating both
/// per-call recompilation and any "validated at load time" panics.
#[derive(Debug, Default, Deserialize)]
struct RawRule {
    name: String,
    description: String,
    query: String,
    fix: Option<String>,
    #[serde(default)]
    severity: RuleSeverity,
    #[serde(default)]
    includes: Vec<String>,
    #[serde(default)]
    excludes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "RawRule")]
pub struct Rule {
    name: String,
    description: String,
    query: String,
    fix: Option<String>,
    severity: RuleSeverity,
    /// When non-empty, the rule only runs on files whose path matches at least one pattern.
    /// Patterns are anchored to the full path string lintoko was handed (typically
    /// project-relative); use `**` to match zero or more path segments.
    includes: Vec<Pattern>,
    /// When a path matches any pattern here, the rule is skipped. Same matching
    /// semantics as [`Rule::includes`]. Combined with `includes`, the rule runs
    /// when the path is included AND not excluded.
    excludes: Vec<Pattern>,
}

impl TryFrom<RawRule> for Rule {
    type Error = anyhow::Error;

    fn try_from(raw: RawRule) -> Result<Self> {
        let compile = |pats: Vec<String>| -> Result<Vec<Pattern>> {
            pats.into_iter()
                .map(|p| {
                    Pattern::new(&p).map_err(|e| {
                        anyhow!("invalid glob pattern {p:?} in rule '{}': {e}", raw.name)
                    })
                })
                .collect()
        };
        Ok(Rule {
            includes: compile(raw.includes)?,
            excludes: compile(raw.excludes)?,
            name: raw.name,
            description: raw.description,
            query: raw.query,
            fix: raw.fix,
            severity: raw.severity,
        })
    }
}

impl Rule {
    fn applies_to(&self, path: &str) -> bool {
        // Normalize Windows paths so authors can write forward-slash globs portably.
        let path = path.replace('\\', "/");
        let any = |pats: &[Pattern]| pats.iter().any(|p| p.matches(&path));
        (self.includes.is_empty() || any(&self.includes)) && !any(&self.excludes)
    }
}

#[derive(Debug, Clone)]
struct RawDiagnostic {
    rule: String,
    description: String,
    range: Range,
    fix: Option<String>,
    severity: RuleSeverity,
}

pub fn parse_rule(content: &str) -> Result<Rule> {
    let rule: Rule = toml::from_str(content)?;
    Ok(rule)
}

#[cfg(test)]
pub(crate) fn test_rule(query: &str) -> Rule {
    RawRule {
        name: "test".into(),
        description: "test".into(),
        query: query.into(),
        ..Default::default()
    }
    .try_into()
    .unwrap()
}

pub fn load_rule_from_file(path: &Path) -> Result<Rule> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read rule from '{}'", path.display()))?;
    parse_rule(&content)
        .with_context(|| format!("Failed to parse rule from '{}'", path.display()))
}

pub fn load_rules_from_directory(dir: &Path) -> Result<Vec<Rule>> {
    let mut rules = vec![];
    let entries = fs::read_dir(dir)
        .with_context(|| anyhow!("Failed to read rules from {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| anyhow!("Invalid entry"))?;
        let path = entry.path();
        if path.is_file() && path.extension().unwrap_or_default() == "toml" {
            debug!("Parsing extra rule at: {}", path.display());
            let rule = load_rule_from_file(&path)
                .with_context(|| anyhow!("Failed to parse rule from: '{}'", path.display()))?;
            rules.push(rule)
        }
    }
    Ok(rules)
}

/// Allows mentioning captures in rule descriptions, and templates them with the actual captures when reporting errors.
fn template(
    template: &str,
    query: &Query,
    captures: &[QueryCapture<'_>],
    input: &str,
) -> Result<String> {
    let re = Regex::new(r"@([a-z-]+)").unwrap();
    let mut new = String::with_capacity(template.len());
    let mut last_match = 0;
    for caps in re.captures_iter(template) {
        let m = caps.get(0).unwrap();
        let name = &caps[1];
        new.push_str(&template[last_match..m.start()]);
        let capture = captures.iter().find(|c| query.capture_names()[c.index as usize] == name).with_context(|| {
            anyhow!("Failed to find capture with name '{name}', when templating error description:\n\n'{template}'")
        })?;
        new.push_str(
            capture
                .node
                .utf8_text(input.as_bytes())
                .context("Non utf-8 text input")?,
        );
        last_match = m.end();
    }
    new.push_str(&template[last_match..]);
    Ok(new)
}

fn apply_rule(rule: &Rule, tree: Node, input: &str) -> Result<Vec<RawDiagnostic>> {
    let query = Query::new(&tree_sitter_motoko::LANGUAGE.into(), &rule.query)
        .with_context(|| format!("Failed to create query for rule '{}'", rule.name))?;
    let error_capture_index = query.capture_index_for_name("error").with_context(|| {
        anyhow!(
            "Expected query to contain `@error` captures:\n{}",
            rule.query
        )
    })?;
    let mut evaluator = custom_predicates::MatchEvaluator::new(&query);
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree, input.as_bytes());
    let mut filtered: HashSet<Range> = HashSet::new();
    let mut errors = Vec::new();
    while let Some(m) = matches.next() {
        if evaluator.should_skip(m)? {
            continue;
        }
        for error_node in m.nodes_for_capture_index(error_capture_index) {
            // NOTE: We have to use `to_vec` here, or tree-sitter will silently swap the captures under our feet.
            errors.push((error_node.range(), m.captures.to_vec()));
        }
        evaluator.collect_filter_ranges(m, &mut filtered);
    }
    let mut seen = HashSet::new();
    let mut diagnostics = vec![];
    for (range, captures) in errors {
        if filtered.contains(&range) {
            continue;
        }
        // Avoid reporting the same diagnostic twice on the same range
        if !seen.insert(range) {
            continue;
        }
        let description = template(&rule.description, &query, &captures, input)?;
        let fix = if let Some(ref fix_template) = rule.fix {
            Some(template(fix_template, &query, &captures, input)?)
        } else {
            None
        };

        let diagnostic = RawDiagnostic {
            rule: rule.name.to_string(),
            description,
            range,
            fix,
            severity: rule.severity,
        };
        diagnostics.push(diagnostic);
    }
    Ok(diagnostics)
}

fn print_pretty_diagnostic(path: &str, source_code: &str, diagnostic: &RawDiagnostic) -> String {
    let source_code = NamedSource::new(path, source_code.to_string());
    let (miette_severity, label) = match diagnostic.severity {
        RuleSeverity::Warning => (Severity::Warning, "[WARNING]"),
        RuleSeverity::Error => (Severity::Error, "[ERROR]"),
    };
    let report = miette!(
        severity = miette_severity,
        labels = vec![LabeledSpan::new_primary_with_span(
            Some(diagnostic.description.clone()),
            (
                diagnostic.range.start_byte,
                diagnostic.range.end_byte - diagnostic.range.start_byte
            )
        )],
        "{label}: {}",
        diagnostic.rule
    )
    .with_source_code(source_code);
    format!("{report:?}")
}

fn print_text_diagnostic(path: &str, source_code: &str, diagnostic: &RawDiagnostic) -> String {
    let mut snippet = String::new();
    let start_line = diagnostic.range.start_point.row + 1;
    let end_line = diagnostic.range.end_point.row + 1;
    let max_line_chars = end_line.ilog(10);
    let snippet_lines = source_code
        .lines()
        .skip(start_line - 1)
        .take(end_line - start_line + 1);
    for (i, line) in snippet_lines.enumerate() {
        let l = start_line + i;
        let line_chars = l.ilog(10);
        let padding = " ".repeat((max_line_chars - line_chars) as usize);
        snippet += &format!("{padding}{l} {line}\n");
    }

    let severity_label = match diagnostic.severity {
        RuleSeverity::Warning => "Warning",
        RuleSeverity::Error => "Error",
    };
    let start = format!("{start_line}:{}", diagnostic.range.start_point.column);
    format!(
        "{path}:{start} {severity_label}: {description}\nFound in:\n{snippet}",
        description = diagnostic.description
    )
}

#[derive(Debug)]
pub struct LintResult {
    pub error_count: usize,
    pub warning_count: usize,
    pub fixed_file: Option<String>,
}

pub fn lint_file(
    config: &Config,
    path: &str,
    input: &str,
    rules: &[Rule],
    mut out: impl Write,
) -> Result<LintResult> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_motoko::LANGUAGE.into())
        .expect("Error loading Motoko grammar");
    let tree = parser.parse(input.as_bytes(), None).unwrap();
    let mut diagnostics = Vec::new();
    for rule in rules {
        if !rule.applies_to(path) {
            continue;
        }
        diagnostics.extend(apply_rule(rule, tree.root_node(), input)?);
    }
    if let Some(severity) = config.severity_override {
        for d in &mut diagnostics {
            d.severity = severity;
        }
    }
    diagnostics.sort_by_key(|d| d.range.start_byte);
    for diagnostic in &diagnostics {
        let output = match config.format {
            OutputFormat::Pretty => print_pretty_diagnostic(path, input, diagnostic),
            OutputFormat::Text => print_text_diagnostic(path, input, diagnostic),
        };
        writeln!(&mut out, "{output}")?
    }
    let mut fixed_file = None;
    let mut overlaps = false;
    if config.fix {
        diagnostics.reverse();
        let mut output = input.to_string();
        let mut last_range: Option<Range> = None;
        for diagnostic in &diagnostics {
            if let Some(fixed) = &diagnostic.fix {
                // NOTE: Don't try to fix overlapping ranges. Instead requires running the tool to a fixpoint
                // Would be nice to automate in the future
                if let Some(last_range) = last_range
                    && diagnostic.range.end_byte >= last_range.start_byte
                {
                    overlaps = true;
                    continue;
                }
                output.replace_range(
                    diagnostic.range.start_byte..diagnostic.range.end_byte,
                    fixed,
                );
                last_range = Some(diagnostic.range)
            }
        }
        if output != input {
            fixed_file = Some(output)
        }
    }
    if overlaps {
        writeln!(
            &mut out,
            "Spotted overlaps when applying fixes. Re-run the command to make progress"
        )?
    }

    let (error_count, warning_count) =
        diagnostics
            .iter()
            .fold((0, 0), |(e, w), d| match d.severity {
                RuleSeverity::Error => (e + 1, w),
                RuleSeverity::Warning => (e, w + 1),
            });

    Ok(LintResult {
        error_count,
        warning_count,
        fixed_file,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn no_rules() {
        let mut out: Vec<u8> = vec![];
        let res = lint_file(
            &Config::default(),
            "<input_path>",
            include_str!("../test-data.mo"),
            &[],
            &mut out,
        )
        .unwrap();
        assert_eq!(res.error_count, 0);
        assert_eq!(res.warning_count, 0);
        assert_eq!(str::from_utf8(&out).unwrap(), "");
    }

    /// Snapshot tests use `backend/main.mo` so that path-filtered example rules
    /// (`allowed-directories`, `types-only`) do not fire and we exercise only
    /// the structural rules.
    const SNAPSHOT_PATH: &str = "backend/main.mo";

    #[test]
    fn it_lints_example_rules() {
        let mut out: Vec<u8> = vec![];
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        let rules = load_rules_from_directory(Path::new("example-rules")).unwrap();
        let _ = lint_file(
            &Config::default(),
            SNAPSHOT_PATH,
            include_str!("../test-data.mo"),
            &rules,
            &mut out,
        )
        .unwrap();
        let lint_output = str::from_utf8(&out).unwrap();
        insta::assert_snapshot!(lint_output);
    }

    #[test]
    fn it_lints_with_textual_output() {
        let mut out: Vec<u8> = vec![];
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        let rules = load_rules_from_directory(Path::new("example-rules")).unwrap();
        let _err_count = lint_file(
            &Config {
                fix: false,
                format: OutputFormat::Text,
                ..Config::default()
            },
            SNAPSHOT_PATH,
            include_str!("../test-data.mo"),
            &rules,
            &mut out,
        )
        .unwrap();
        let lint_output = str::from_utf8(&out).unwrap();
        insta::assert_snapshot!(lint_output);
    }

    fn rule_with_filters(includes: &[&str], excludes: &[&str]) -> Rule {
        RawRule {
            name: "test".into(),
            description: "test".into(),
            query: "(source_file) @error".into(),
            includes: includes.iter().map(|s| (*s).to_string()).collect(),
            excludes: excludes.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn applies_to_handles_includes_excludes() {
        let no_filters = rule_with_filters(&[], &[]);
        assert!(no_filters.applies_to("anywhere/foo.mo"));

        let only_types = rule_with_filters(&["backend/types/**"], &[]);
        assert!(only_types.applies_to("backend/types/Foo.mo"));
        assert!(only_types.applies_to("backend/types/sub/Foo.mo"));
        assert!(!only_types.applies_to("backend/lib/Foo.mo"));

        let except_lib = rule_with_filters(&[], &["backend/lib/**"]);
        assert!(except_lib.applies_to("backend/types/Foo.mo"));
        assert!(!except_lib.applies_to("backend/lib/Foo.mo"));

        // includes wins as a coarse filter; excludes punches a hole inside it.
        let lib_except_internal = rule_with_filters(&["backend/lib/**"], &["**/internal/**"]);
        assert!(lib_except_internal.applies_to("backend/lib/Foo.mo"));
        assert!(!lib_except_internal.applies_to("backend/lib/internal/Foo.mo"));
        assert!(!lib_except_internal.applies_to("backend/types/Foo.mo"));
    }

    #[test]
    fn invalid_glob_pattern_fails_at_parse() {
        let toml_src = r#"
name = "bad"
description = "bad"
query = "(source_file) @error"
includes = ["[unterminated"]
"#;
        let err = parse_rule(toml_src).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("invalid glob"), "unexpected error: {msg}");
    }

    fn assert_rule_count(rule_path: &str, source: &str, file_path: &str, expected: usize) {
        let mut out: Vec<u8> = vec![];
        let rule = load_rule_from_file(Path::new(rule_path)).unwrap();
        let res = lint_file(&Config::default(), file_path, source, &[rule], &mut out).unwrap();
        assert_eq!(
            res.error_count, expected,
            "rule {rule_path} at {file_path}: expected {expected} errors"
        );
    }

    #[test]
    fn allowed_directories_rule() {
        let rule = "example-rules/allowed-directories.toml";
        let src = "actor { };";
        for (path, expected) in [
            ("backend/lib/foo.mo", 0),
            ("backend/types/foo.mo", 0),
            ("backend/mixins/foo.mo", 0),
            ("backend/migrations/001.mo", 0),
            ("backend/next-migration/foo.mo", 0),
            ("backend/main.mo", 0),
            ("src/foo.mo", 1),
            ("backend/other/foo.mo", 1),
            ("backend/main2.mo", 1),
        ] {
            assert_rule_count(rule, src, path, expected);
        }
    }

    #[test]
    fn types_only_scoped_by_path() {
        let rule = "example-rules/types-only.toml";
        let mixed_src = "module { public type T = Nat; public func f() {} };";
        assert_rule_count(rule, mixed_src, "backend/types/foo.mo", 1);
        assert_rule_count(rule, mixed_src, "backend/lib/foo.mo", 0);

        let only_types = "module { public type T = Nat };";
        assert_rule_count(rule, only_types, "backend/types/foo.mo", 0);
    }

    #[test]
    fn it_applies_fixes() {
        let mut out: Vec<u8> = vec![];
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        let rule = load_rule_from_file(Path::new("example-rules/pun-fields.toml")).unwrap();
        let res = lint_file(
            &Config {
                fix: true,
                format: OutputFormat::Text,
                ..Config::default()
            },
            "<input_path>",
            "{ x = x }",
            &[rule],
            &mut out,
        )
        .unwrap();
        assert_eq!(res.fixed_file.unwrap(), "{ x }".to_string())
    }

    #[test]
    fn warning_severity_does_not_count_as_error() {
        let mut out: Vec<u8> = vec![];
        let rule = load_rule_from_file(Path::new("example-rules/pun-fields.toml")).unwrap();
        assert_eq!(rule.severity, RuleSeverity::Warning);
        let res = lint_file(
            &Config::default(),
            "<input_path>",
            "{ x = x }",
            &[rule],
            &mut out,
        )
        .unwrap();
        assert_eq!(res.error_count, 0);
        assert_eq!(res.warning_count, 1);
    }

    #[test]
    fn severity_override_promotes_warnings_to_errors() {
        let mut out: Vec<u8> = vec![];
        let rule = load_rule_from_file(Path::new("example-rules/pun-fields.toml")).unwrap();
        assert_eq!(rule.severity, RuleSeverity::Warning);
        let res = lint_file(
            &Config {
                severity_override: Some(RuleSeverity::Error),
                ..Config::default()
            },
            "<input_path>",
            "{ x = x }",
            &[rule],
            &mut out,
        )
        .unwrap();
        assert_eq!(res.error_count, 1);
        assert_eq!(res.warning_count, 0);
    }

    #[test]
    fn severity_override_demotes_errors_to_warnings() {
        let mut out: Vec<u8> = vec![];
        let rule = load_rule_from_file(Path::new("example-rules/no-let-else.toml")).unwrap();
        assert_eq!(rule.severity, RuleSeverity::Error);
        let res = lint_file(
            &Config {
                severity_override: Some(RuleSeverity::Warning),
                ..Config::default()
            },
            "<input_path>",
            "let ?x = foo() else { return }",
            &[rule],
            &mut out,
        )
        .unwrap();
        assert_eq!(res.error_count, 0);
        assert_eq!(res.warning_count, 1);
    }
}
