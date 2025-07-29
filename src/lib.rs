use std::{collections::HashSet, fs, io::Write, path::Path};

use anyhow::{Context, Result, anyhow};
use miette::{LabeledSpan, NamedSource, Report, Severity, miette};
use serde::Deserialize;
use tracing::debug;
use tree_sitter::{Node, Parser, Query, QueryCursor, Range, StreamingIterator};

#[derive(Debug, Deserialize)]
pub struct Rule {
    name: String,
    description: String,
    query: String,
}

pub fn default_rules() -> Vec<Rule> {
    vec![
        toml::from_str(include_str!("../default-rules/no-flexible.toml"))
            .expect("Failed to parse no-flexible rule"),
        toml::from_str(include_str!("../default-rules/no-stable.toml"))
            .expect("Failed to parse no-stable rule"),
        toml::from_str(include_str!("../default-rules/only-persistent-actor.toml"))
            .expect("Failed to parse only-persistent-actor rule"),
    ]
}

pub fn load_rule_from_file(path: &Path) -> Result<Rule> {
    let content = std::fs::read_to_string(path)?;
    let rule = toml::from_str(&content)?;
    Ok(rule)
}

pub fn load_rules_from_directory(dir: &Path) -> Result<Vec<Rule>> {
    let mut rules = vec![];
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
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

fn apply_rule(rule: &Rule, tree: Node, input: &str) -> Result<Vec<Report>> {
    let query = Query::new(&tree_sitter_motoko::LANGUAGE.into(), &rule.query)
        .with_context(|| format!("Failed to create query for rule '{}'", rule.name))?;
    let error_capture_index = query.capture_index_for_name("error").with_context(|| {
        anyhow!(
            "Expected query to contain `@error` captures:\n{}",
            rule.query
        )
    })?;
    let filter_capture_index = query.capture_index_for_name("filter");

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree, input.as_bytes());
    let mut filtered: HashSet<Range> = HashSet::new();
    let mut errors: Vec<Range> = Vec::new();
    while let Some(m) = matches.next() {
        for error_node in m.nodes_for_capture_index(error_capture_index) {
            errors.push(error_node.range());
        }
        if let Some(filter_capture_index) = filter_capture_index {
            for filter_node in m.nodes_for_capture_index(filter_capture_index) {
                filtered.insert(filter_node.range());
            }
        }
    }
    let diagnostics = errors
        .into_iter()
        .filter(|range| !filtered.contains(range))
        .map(|range| {
            miette!(
                severity = Severity::Error,
                labels = vec![LabeledSpan::new_primary_with_span(
                    Some(rule.description.clone()),
                    (range.start_byte, range.end_byte - range.start_byte)
                )],
                "[ERROR]: {}",
                rule.name
            )
        })
        .collect();
    Ok(diagnostics)
}

pub fn lint_file(path: &str, input: &str, rules: Vec<Rule>, mut out: impl Write) -> Result<usize> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_motoko::LANGUAGE.into())
        .expect("Error loading Motoko grammar");
    let tree = parser.parse(input.as_bytes(), None).unwrap();
    let mut diagnostics = Vec::new();
    for rule in &rules {
        diagnostics.extend(apply_rule(rule, tree.root_node(), input)?);
    }
    let count = diagnostics.len();
    diagnostics.sort_by_key(|d| d.labels().unwrap().next().unwrap().offset());
    for diagnostic in diagnostics {
        let pretty = diagnostic.with_source_code(NamedSource::new(path, input.to_string()));
        writeln!(&mut out, "{pretty:?}")?
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
            "<input_path>",
            include_str!("../test-data.mo"),
            vec![],
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
            "<input_path>",
            include_str!("../test-data.mo"),
            rules,
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
            "<input_path>",
            include_str!("../test-data.mo"),
            rules,
            &mut out,
        )
        .unwrap();
        let lint_output = str::from_utf8(&out).unwrap();
        insta::assert_snapshot!(lint_output);
    }
}
