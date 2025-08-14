use anyhow::{Context, Result, anyhow};
use miette::{LabeledSpan, NamedSource, Severity, miette};
use regex::Regex;
use serde::Deserialize;
use std::{collections::HashSet, fs, io::Write, path::Path};
use tracing::debug;
use tree_sitter::{Node, Parser, Query, QueryCapture, QueryCursor, Range, StreamingIterator};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Pretty,
    Text,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub format: OutputFormat,
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    name: String,
    description: String,
    query: String,
}

#[derive(Debug, Clone)]
struct RawDiagnostic {
    rule: String,
    description: String,
    range: Range,
}

pub fn default_rules() -> Vec<Rule> {
    vec![
        toml::from_str(include_str!("../default-rules/no-flexible.toml"))
            .expect("Failed to parse no-flexible rule"),
        toml::from_str(include_str!("../default-rules/no-stable.toml"))
            .expect("Failed to parse no-stable rule"),
        toml::from_str(include_str!("../default-rules/only-persistent-actor.toml"))
            .expect("Failed to parse only-persistent-actor rule"),
        toml::from_str(include_str!("../default-rules/pun-fields.toml"))
            .expect("Failed to parse pun-fields rule"),
        toml::from_str(include_str!("../default-rules/no-bool-switch.toml"))
            .expect("Failed to parse no-bool-switch rule"),
        toml::from_str(include_str!("../default-rules/assign-plus.toml"))
            .expect("Failed to parse assign-plus rule"),
        toml::from_str(include_str!("../default-rules/assign-minus.toml"))
            .expect("Failed to parse assign-minus rule"),
        toml::from_str(include_str!("../default-rules/unneeded-return.toml"))
            .expect("Failed to parse unneeded-return rule"),
        toml::from_str(include_str!("../default-rules/case-types.toml"))
            .expect("Failed to parse case-types rule"),
        toml::from_str(include_str!("../default-rules/case-functions.toml"))
            .expect("Failed to parse case-functions rule"),
    ]
}

pub fn load_rule_from_file(path: &Path) -> Result<Rule> {
    let content = std::fs::read_to_string(path)?;
    let rule = toml::from_str(&content)?;
    Ok(rule)
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
fn template_description(
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
    let filter_capture_index = query.capture_index_for_name("filter");
    let trailing_capture_index = query.capture_index_for_name("trailing");

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree, input.as_bytes());
    let mut filtered: HashSet<Range> = HashSet::new();
    let mut errors = Vec::new();
    while let Some(m) = matches.next() {
        // Works around a tree-sitter bug that doesn't let us use trailing anchors: https://github.com/tree-sitter/tree-sitter/issues/4558
        if let Some(trailing_capture_index) = trailing_capture_index
            && m.nodes_for_capture_index(trailing_capture_index)
                .any(|n| n.next_named_sibling().is_some())
        {
            continue;
        };
        for error_node in m.nodes_for_capture_index(error_capture_index) {
            // NOTE: We have to use `to_vec` here, or tree-sitter will silently swap the captures under our feet.
            errors.push((error_node.range(), m.captures.to_vec()));
        }

        if let Some(filter_capture_index) = filter_capture_index {
            for filter_node in m.nodes_for_capture_index(filter_capture_index) {
                filtered.insert(filter_node.range());
            }
        }
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
        let description = template_description(&rule.description, &query, &captures, input)?;
        let diagnostic = RawDiagnostic {
            rule: rule.name.to_string(),
            description,
            range,
        };
        diagnostics.push(diagnostic);
    }
    Ok(diagnostics)
}

fn print_pretty_diagnostic(path: &str, source_code: &str, diagnostic: RawDiagnostic) -> String {
    let source_code = NamedSource::new(path, source_code.to_string());
    let report = miette!(
        severity = Severity::Error,
        labels = vec![LabeledSpan::new_primary_with_span(
            Some(diagnostic.description),
            (
                diagnostic.range.start_byte,
                diagnostic.range.end_byte - diagnostic.range.start_byte
            )
        )],
        "[ERROR]: {}",
        diagnostic.rule
    )
    .with_source_code(source_code);
    format!("{report:?}")
}

fn print_text_diagnostic(path: &str, source_code: &str, diagnostic: RawDiagnostic) -> String {
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

    let start = format!("{start_line}:{}", diagnostic.range.start_point.column);
    format!(
        "{path}:{start} Error: {description}\nFound in:\n{snippet}",
        description = diagnostic.description
    )
}

pub fn lint_file(
    config: &Config,
    path: &str,
    input: &str,
    rules: &[Rule],
    mut out: impl Write,
) -> Result<usize> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_motoko::LANGUAGE.into())
        .expect("Error loading Motoko grammar");
    let tree = parser.parse(input.as_bytes(), None).unwrap();
    let mut diagnostics = Vec::new();
    for rule in rules {
        diagnostics.extend(apply_rule(rule, tree.root_node(), input)?);
    }
    let count = diagnostics.len();
    diagnostics.sort_by_key(|d| d.range.start_byte);
    for diagnostic in diagnostics {
        let output = match config.format {
            OutputFormat::Pretty => print_pretty_diagnostic(path, input, diagnostic),
            OutputFormat::Text => print_text_diagnostic(path, input, diagnostic),
        };
        writeln!(&mut out, "{output}")?
    }
    Ok(count)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn no_rules() {
        let mut out: Vec<u8> = vec![];
        let err_count = lint_file(
            &Config::default(),
            "<input_path>",
            include_str!("../test-data.mo"),
            &[],
            &mut out,
        )
        .unwrap();
        assert_eq!(err_count, 0);
        assert_eq!(str::from_utf8(&out).unwrap(), "");
    }

    #[test]
    fn it_lints_default_rules() {
        let mut out: Vec<u8> = vec![];
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        let rules = default_rules();
        let _err_count = lint_file(
            &Config::default(),
            "<input_path>",
            include_str!("../test-data.mo"),
            &rules,
            &mut out,
        )
        .unwrap();
        let lint_output = str::from_utf8(&out).unwrap();
        insta::assert_snapshot!(lint_output);
    }

    #[test]
    fn it_lints_custom_rules() {
        let mut out: Vec<u8> = vec![];
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        let mut rules = default_rules();
        rules.extend(load_rules_from_directory(Path::new("custom-rules")).unwrap());
        let _err_count = lint_file(
            &Config::default(),
            "<input_path>",
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
        let mut rules = default_rules();
        rules.extend(load_rules_from_directory(Path::new("custom-rules")).unwrap());
        let _err_count = lint_file(
            &Config {
                format: OutputFormat::Text,
            },
            "<input_path>",
            include_str!("../test-data.mo"),
            &rules,
            &mut out,
        )
        .unwrap();
        let lint_output = str::from_utf8(&out).unwrap();
        insta::assert_snapshot!(lint_output);
    }
}
