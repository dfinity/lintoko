use anyhow::{Context, Result, bail};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, QueryCapture, QueryPredicate, QueryPredicateArg};

fn resolve_capture_idx(args: &[QueryPredicateArg], idx: usize) -> Result<u32> {
    match args.get(idx) {
        Some(QueryPredicateArg::Capture(capture_idx)) => Ok(*capture_idx),
        Some(_) => bail!("expected capture argument at position {idx}"),
        None => bail!("missing argument at position {idx}"),
    }
}

fn find_capture_node<'a>(captures: &[QueryCapture<'a>], capture_idx: u32) -> Option<Node<'a>> {
    captures
        .iter()
        .find(|c| c.index == capture_idx)
        .map(|c| c.node)
}

fn resolve_string_arg(args: &[QueryPredicateArg], idx: usize) -> Result<&str> {
    match args.get(idx) {
        Some(QueryPredicateArg::String(s)) => Ok(s),
        Some(_) => bail!("expected string argument at position {idx}"),
        None => bail!("missing argument at position {idx}"),
    }
}

fn ancestor_depth(node: Node, types: &HashSet<&str>) -> usize {
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

// Iterative DFS to avoid stack overflow on deep trees.
fn subtree_depth(node: Node, types: &HashSet<&str>) -> usize {
    let start_id = node.id();
    let mut max = 0;
    let mut current_depth = 0;
    let mut cursor = node.walk();
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
            if cursor.node().id() == start_id {
                break 'walk;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    max
}

fn eval_depth_predicate<'q>(
    pred: &'q QueryPredicate,
    captures: &[QueryCapture<'_>],
    types_cache: &mut HashMap<&'q str, HashSet<&'q str>>,
    depth_fn: fn(Node, &HashSet<&str>) -> usize,
) -> Result<bool> {
    let capture_idx = resolve_capture_idx(&pred.args, 0)?;
    let node = find_capture_node(captures, capture_idx)
        .ok_or_else(|| anyhow::anyhow!("capture not found in match"))?;
    let types_str = resolve_string_arg(&pred.args, 1)?;
    let threshold: usize = resolve_string_arg(&pred.args, 2)?
        .parse()
        .with_context(|| format!("{} threshold must be a number", pred.operator.as_ref()))?;
    let types = match types_cache.entry(types_str) {
        Entry::Occupied(e) => e.into_mut(),
        Entry::Vacant(e) => e.insert(types_str.split(',').map(str::trim).collect()),
    };
    Ok(depth_fn(node, types) >= threshold)
}

fn evaluate_predicates<'q>(
    predicates: &'q [QueryPredicate],
    captures: &[QueryCapture<'_>],
    types_cache: &mut HashMap<&'q str, HashSet<&'q str>>,
) -> Result<bool> {
    for pred in predicates {
        let op = pred.operator.as_ref();
        let pass = match op {
            "ancestor-depth?" | "subtree-depth?" => {
                let depth_fn = if op == "ancestor-depth?" {
                    ancestor_depth as fn(Node, &HashSet<&str>) -> usize
                } else {
                    subtree_depth
                };
                eval_depth_predicate(pred, captures, types_cache, depth_fn)
                    .with_context(|| format!("in #{op}"))?
            }
            unknown => bail!("Unknown custom predicate: #{unknown}"),
        };
        if !pass {
            return Ok(false);
        }
    }
    Ok(true)
}

// Workaround for tree-sitter bug: https://github.com/tree-sitter/tree-sitter/issues/4558
fn is_trailing(m: &tree_sitter::QueryMatch, idx: u32) -> bool {
    m.nodes_for_capture_index(idx)
        .any(|n| n.next_named_sibling().is_some())
}

pub struct MatchEvaluator<'q> {
    query: &'q tree_sitter::Query,
    trailing_idx: Option<u32>,
    filter_idx: Option<u32>,
    types_cache: HashMap<&'q str, HashSet<&'q str>>,
}

impl<'q> MatchEvaluator<'q> {
    pub fn new(query: &'q tree_sitter::Query) -> Self {
        Self {
            trailing_idx: query.capture_index_for_name("trailing"),
            filter_idx: query.capture_index_for_name("filter"),
            types_cache: HashMap::new(),
            query,
        }
    }

    pub fn should_skip(&mut self, m: &tree_sitter::QueryMatch) -> Result<bool> {
        if let Some(idx) = self.trailing_idx
            && is_trailing(m, idx)
        {
            return Ok(true);
        }
        let predicates = self.query.general_predicates(m.pattern_index);
        if !predicates.is_empty()
            && !evaluate_predicates(predicates, m.captures, &mut self.types_cache)?
        {
            return Ok(true);
        }
        Ok(false)
    }

    pub fn collect_filter_ranges(
        &self,
        m: &tree_sitter::QueryMatch,
        out: &mut HashSet<tree_sitter::Range>,
    ) {
        if let Some(idx) = self.filter_idx {
            for node in m.nodes_for_capture_index(idx) {
                out.insert(node.range());
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{Config, Rule, lint_file};

    fn assert_lint_count(query: &str, input: &str, expected: usize) {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "test".into(),
            description: "test".into(),
            query: query.into(),
            fix: None,
            severity: Default::default(),
            includes: vec![],
            excludes: vec![],
        };
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, expected);
    }

    fn assert_lint_errors(query: &str, input: &str, expected_err: &str) {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "test".into(),
            description: "test".into(),
            query: query.into(),
            fix: None,
            severity: Default::default(),
            includes: vec![],
            excludes: vec![],
        };
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains(expected_err));
    }

    #[test]
    fn subtree_depth_fires_above_threshold() {
        assert_lint_count(
            r#"((obj_body) @error (#subtree-depth? @error "obj_body,block_exp" "2"))"#,
            "actor { func f() { if (true) { 0 } } };",
            1,
        );
    }

    #[test]
    fn subtree_depth_does_not_leak_into_siblings() {
        assert_lint_count(
            r#"((obj_body) @error (#subtree-depth? @error "block_exp" "2"))"#,
            "actor { func f() { 0 } }; actor { func g() { if (true) { 0 } } };",
            1,
        );
    }

    #[test]
    fn subtree_depth_silent_at_threshold() {
        assert_lint_count(
            r#"((obj_body) @error (#subtree-depth? @error "obj_body,block_exp" "5"))"#,
            "actor { func f() { 0 } };",
            0,
        );
    }

    #[test]
    fn ancestor_depth_fires_on_deep_block() {
        assert_lint_count(
            r#"((block_exp) @error (#ancestor-depth? @error "obj_body,block_exp" "3"))"#,
            "actor { func f() { if (true) { if (true) { 0 } } } };",
            1,
        );
    }

    #[test]
    fn ancestor_depth_silent_on_shallow_block() {
        assert_lint_count(
            r#"((block_exp) @error (#ancestor-depth? @error "obj_body,block_exp" "8"))"#,
            "actor { func f() { 0 } };",
            0,
        );
    }

    #[test]
    fn ancestor_depth_flags_only_deep_blocks() {
        assert_lint_count(
            r#"((block_exp) @error (#ancestor-depth? @error "obj_body,block_exp" "2"))"#,
            "actor { func f() { if (true) { 0 } } };",
            1,
        );
    }

    #[test]
    fn unknown_predicate_errors() {
        assert_lint_errors(
            r#"((func_dec) @error (#bogus-pred? @error "x"))"#,
            "actor { func f() {} };",
            "bogus-pred?",
        );
    }

    #[test]
    fn builtin_predicates_do_not_leak_into_general_predicates() {
        assert_lint_count(
            r#"((exp_field (identifier) @field (var_exp (identifier) @value)) @error (#eq? @field @value))"#,
            "{ x = x }",
            1,
        );
    }

    #[test]
    fn trailing_skips_match_with_next_sibling() {
        assert_lint_count(
            r#"(func_dec (block_exp (exp_dec (return_exp)) @error @trailing))"#,
            "actor { func f() { return 10; 20 }; };",
            0,
        );
    }

    #[test]
    fn trailing_flags_match_without_next_sibling() {
        assert_lint_count(
            r#"(func_dec (block_exp (exp_dec (return_exp)) @error @trailing))"#,
            "actor { func f() { return 10 }; };",
            1,
        );
    }

    #[test]
    fn filter_suppresses_matching_error_range() {
        assert_lint_count(
            r#"
                ((exp_field (identifier) @field (var_exp (identifier) @value)) @error (#eq? @field @value))
                (exp_field "var" (identifier) @field (var_exp (identifier) @value)) @filter
            "#,
            "{ var x = x }",
            0,
        );
    }

    #[test]
    fn filter_does_not_suppress_non_matching_range() {
        assert_lint_count(
            r#"
                ((exp_field (identifier) @field (var_exp (identifier) @value)) @error (#eq? @field @value))
                (exp_field "var" (identifier) @field (var_exp (identifier) @value)) @filter
            "#,
            "{ x = x }",
            1,
        );
    }
}
