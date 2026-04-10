use anyhow::{Context, Result, anyhow, bail};
use miette::{LabeledSpan, NamedSource, Severity, miette};
use regex::Regex;
use serde::Deserialize;
use std::collections::hash_map::Entry;
use std::{collections::{HashMap, HashSet}, fs, io::Write, path::Path};
use tracing::debug;
use tree_sitter::{
    Node, Parser, Query, QueryCapture, QueryCursor, QueryPredicate, QueryPredicateArg, Range,
    StreamingIterator,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Pretty,
    Text,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub format: OutputFormat,
    pub fix: bool,
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    name: String,
    description: String,
    query: String,
    fix: Option<String>,
}

#[derive(Debug, Clone)]
struct RawDiagnostic {
    rule: String,
    description: String,
    range: Range,
    fix: Option<String>,
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

fn resolve_capture_node<'a>(
    args: &[QueryPredicateArg],
    idx: usize,
    captures: &[QueryCapture<'a>],
) -> Result<Node<'a>> {
    match args.get(idx) {
        Some(QueryPredicateArg::Capture(capture_idx)) => captures
            .iter()
            .find(|c| c.index == *capture_idx)
            .map(|c| c.node)
            .context("capture not found in match"),
        Some(_) => bail!("expected capture argument at position {idx}"),
        None => bail!("missing argument at position {idx}"),
    }
}

fn resolve_string_arg<'a>(args: &'a [QueryPredicateArg], idx: usize) -> Result<&'a str> {
    match args.get(idx) {
        Some(QueryPredicateArg::String(s)) => Ok(s),
        Some(_) => bail!("expected string argument at position {idx}"),
        None => bail!("missing argument at position {idx}"),
    }
}

fn nesting_depth(node: Node, types: &HashSet<&str>) -> usize {
    let mut count = 0;
    let mut current = node.parent();
    while let Some(parent) = current {
        if types.contains(parent.kind()) {
            count += 1;
        }
        current = parent.parent();
    }
    count
}

fn max_depth(node: Node, types: &HashSet<&str>) -> usize {
    let start_id = node.id();
    let mut max = 0;
    let mut current_depth = 0;
    let mut cursor = node.walk();
    // Iterative DFS using TreeCursor to avoid stack overflow on deep trees.
    // We track start_id to avoid walking above the starting node.
    'walk: loop {
        if types.contains(cursor.node().kind()) {
            current_depth += 1;
            max = max.max(current_depth);
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if types.contains(cursor.node().kind()) {
                current_depth -= 1;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if cursor.node().id() == start_id || !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    max
}

fn evaluate_predicates(
    predicates: &[QueryPredicate],
    captures: &[QueryCapture<'_>],
    input: &str,
    sub_query_cache: &mut HashMap<String, Query>,
) -> Result<bool> {
    for pred in predicates {
        let op = pred.operator.as_ref();
        match op {
            "has-descendant?" | "not-has-descendant?" => {
                let node = resolve_capture_node(&pred.args, 0, captures)?;
                let sub_query_str = resolve_string_arg(&pred.args, 1)?;
                let sub_query = match sub_query_cache.entry(sub_query_str.to_string()) {
                    Entry::Occupied(e) => e.into_mut(),
                    Entry::Vacant(e) => e.insert(
                        Query::new(&tree_sitter_motoko::LANGUAGE.into(), sub_query_str)
                            .with_context(|| {
                                format!("invalid sub-query in #{op}: {sub_query_str}")
                            })?,
                    ),
                };
                let negate = op == "not-has-descendant?";
                let mut cursor = QueryCursor::new();
                let mut sub_matches = cursor.matches(sub_query, node, input.as_bytes());
                let found = sub_matches.next().is_some();
                if found == negate {
                    return Ok(false);
                }
            }
            "nesting-depth?" | "max-depth?" => {
                let node = resolve_capture_node(&pred.args, 0, captures)?;
                let types_str = resolve_string_arg(&pred.args, 1)?;
                let threshold: usize = resolve_string_arg(&pred.args, 2)?
                    .parse()
                    .with_context(|| format!("{op} threshold must be a number"))?;
                let types: HashSet<&str> = types_str.split(',').map(str::trim).collect();
                let depth = if op == "nesting-depth?" {
                    nesting_depth(node, &types)
                } else {
                    max_depth(node, &types)
                };
                if depth <= threshold {
                    return Ok(false);
                }
            }
            unknown => {
                bail!("Unknown custom predicate: #{unknown}");
            }
        }
    }
    Ok(true)
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
    let mut sub_query_cache: HashMap<String, Query> = HashMap::new();
    while let Some(m) = matches.next() {
        // Works around a tree-sitter bug that doesn't let us use trailing anchors: https://github.com/tree-sitter/tree-sitter/issues/4558
        if let Some(trailing_capture_index) = trailing_capture_index
            && m.nodes_for_capture_index(trailing_capture_index)
                .any(|n| n.next_named_sibling().is_some())
        {
            continue;
        };
        // Evaluate custom predicates
        let predicates = query.general_predicates(m.pattern_index);
        if !predicates.is_empty() {
            // Clone captures for evaluate_predicates — it needs owned data for sub-query cursors.
            let captures = m.captures.to_vec();
            if !evaluate_predicates(predicates, &captures, input, &mut sub_query_cache)? {
                continue;
            }
        }
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
        };
        diagnostics.push(diagnostic);
    }
    Ok(diagnostics)
}

fn print_pretty_diagnostic(path: &str, source_code: &str, diagnostic: &RawDiagnostic) -> String {
    let source_code = NamedSource::new(path, source_code.to_string());
    let report = miette!(
        severity = Severity::Error,
        labels = vec![LabeledSpan::new_primary_with_span(
            Some(diagnostic.description.clone()),
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

    let start = format!("{start_line}:{}", diagnostic.range.start_point.column);
    format!(
        "{path}:{start} Error: {description}\nFound in:\n{snippet}",
        description = diagnostic.description
    )
}

#[derive(Debug)]
pub struct LintResult {
    pub error_count: usize,
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
        diagnostics.extend(apply_rule(rule, tree.root_node(), input)?);
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

    Ok(LintResult {
        error_count: diagnostics.len(),
        fixed_file,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    fn has_descendant_rule() -> Rule {
        Rule {
            name: "has-return".into(),
            description: "function body contains a return".into(),
            query: r#"(func_dec (block_exp) @error (#has-descendant? @error "(return_exp)"))"#
                .into(),
            fix: None,
        }
    }

    fn not_has_descendant_rule() -> Rule {
        Rule {
            name: "no-return".into(),
            description: "function body lacks a return".into(),
            query:
                r#"(func_dec (block_exp) @error (#not-has-descendant? @error "(return_exp)"))"#
                    .into(),
            fix: None,
        }
    }

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
        assert_eq!(str::from_utf8(&out).unwrap(), "");
    }

    #[test]
    fn it_lints_example_rules() {
        let mut out: Vec<u8> = vec![];
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        let rules = load_rules_from_directory(Path::new("example-rules")).unwrap();
        let _ = lint_file(
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
        let rules = load_rules_from_directory(Path::new("example-rules")).unwrap();
        let _err_count = lint_file(
            &Config {
                fix: false,
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
    fn has_descendant_matches_when_present() {
        let mut out: Vec<u8> = vec![];
        let input = "actor { func f() { return 10 }; };";
        let res = lint_file(&Config::default(), "<test>", input, &[has_descendant_rule()], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn has_descendant_skips_when_absent() {
        let mut out: Vec<u8> = vec![];
        let input = "actor { func f() { 10 }; };";
        let res = lint_file(&Config::default(), "<test>", input, &[has_descendant_rule()], &mut out).unwrap();
        assert_eq!(res.error_count, 0);
    }

    #[test]
    fn not_has_descendant_matches_when_absent() {
        let mut out: Vec<u8> = vec![];
        let input = "actor { func f() { 10 }; };";
        let res = lint_file(&Config::default(), "<test>", input, &[not_has_descendant_rule()], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn not_has_descendant_skips_when_present() {
        let mut out: Vec<u8> = vec![];
        let input = "actor { func f() { return 10 }; };";
        let res = lint_file(&Config::default(), "<test>", input, &[not_has_descendant_rule()], &mut out).unwrap();
        assert_eq!(res.error_count, 0);
    }

    #[test]
    fn has_descendant_finds_deeply_nested() {
        let mut out: Vec<u8> = vec![];
        let input = "actor { func f() { if (true) { if (true) { if (true) { return 1 } } } }; };";
        let res = lint_file(&Config::default(), "<test>", input, &[has_descendant_rule()], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn max_depth_fires_above_threshold() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "too-deep".into(),
            description: "nesting too deep".into(),
            query: r#"((obj_body) @error (#max-depth? @error "obj_body,block_exp" "2"))"#.into(),
            fix: None,
        };
        // depth: obj_body(1) > block_exp(2) > block_exp(3) — exceeds threshold of 2
        let input = "actor { func f() { if (true) { 0 } } };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn max_depth_silent_at_threshold() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "too-deep".into(),
            description: "nesting too deep".into(),
            query: r#"((obj_body) @error (#max-depth? @error "obj_body,block_exp" "5"))"#.into(),
            fix: None,
        };
        // Simple actor with one function — depth well under 5
        let input = "actor { func f() { 0 } };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 0);
    }

    #[test]
    fn nesting_depth_fires_on_deep_block() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "too-nested".into(),
            description: "too nested".into(),
            // threshold 2: blocks with >2 ancestors of these types get flagged
            query: r#"((block_exp) @error (#nesting-depth? @error "obj_body,block_exp" "2"))"#
                .into(),
            fix: None,
        };
        // Ancestors for innermost block_exp { 0 }: obj_body, block_exp, block_exp = 3 > 2
        let input = "actor { func f() { if (true) { if (true) { 0 } } } };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn nesting_depth_silent_on_shallow_block() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "too-nested".into(),
            description: "too nested".into(),
            query: r#"((block_exp) @error (#nesting-depth? @error "obj_body,block_exp" "8"))"#
                .into(),
            fix: None,
        };
        let input = "actor { func f() { 0 } };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 0);
    }

    #[test]
    fn nesting_depth_flags_only_deep_blocks_not_ancestors() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "too-nested".into(),
            description: "too nested".into(),
            // threshold 1: blocks with >1 ancestor of these types get flagged
            query: r#"((block_exp) @error (#nesting-depth? @error "obj_body,block_exp" "1"))"#
                .into(),
            fix: None,
        };
        // actor { func f() { if (true) { 0 } } };
        // block_exp "{ if... }" has ancestors: obj_body → count=1, not > 1 → not flagged
        // block_exp "{ 0 }" has ancestors: obj_body, block_exp → count=2 > 1 → flagged
        let input = "actor { func f() { if (true) { 0 } } };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn unknown_predicate_errors() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "bad".into(),
            description: "bad".into(),
            query: r#"((func_dec) @error (#bogus-pred? @error "x"))"#.into(),
            fix: None,
        };
        let input = "actor { func f() {} };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("bogus-pred?"));
    }

    #[test]
    fn builtin_predicates_do_not_leak_into_general_predicates() {
        // Verify that tree-sitter's built-in predicates (#eq?, #match?, etc.)
        // are NOT returned by general_predicates() and thus don't hit our
        // "Unknown custom predicate" error path.
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "eq-test".into(),
            description: "pun: @field".into(),
            query: r#"((exp_field (identifier) @field (var_exp (identifier) @value)) @error (#eq? @field @value))"#.into(),
            fix: None,
        };
        let input = "{ x = x }";
        // This should succeed (not error with "Unknown custom predicate: #eq?")
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }
}
