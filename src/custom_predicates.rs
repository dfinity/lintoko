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
    Ok(depth_fn(node, types) > threshold)
}

fn evaluate_predicates<'q>(
    predicates: &'q [QueryPredicate],
    captures: &[QueryCapture<'_>],
    types_cache: &mut HashMap<&'q str, HashSet<&'q str>>,
) -> Result<bool> {
    for pred in predicates {
        let pass = match pred.operator.as_ref() {
            "nesting-depth?" => eval_depth_predicate(pred, captures, types_cache, ancestor_depth)?,
            "max-depth?" => eval_depth_predicate(pred, captures, types_cache, subtree_depth)?,
            unknown => bail!("Unknown custom predicate: #{unknown}"),
        };
        if !pass {
            return Ok(false);
        }
    }
    Ok(true)
}

// Workaround for tree-sitter bug: https://github.com/tree-sitter/tree-sitter/issues/4558
fn is_trailing(m: &tree_sitter::QueryMatch, trailing_capture_index: u32) -> bool {
    m.nodes_for_capture_index(trailing_capture_index)
        .any(|n| n.next_named_sibling().is_some())
}

pub fn should_skip_match<'q>(
    m: &tree_sitter::QueryMatch,
    query: &'q tree_sitter::Query,
    trailing_capture_index: Option<u32>,
    types_cache: &mut HashMap<&'q str, HashSet<&'q str>>,
) -> Result<bool> {
    if let Some(idx) = trailing_capture_index
        && is_trailing(m, idx)
    {
        return Ok(true);
    }
    let predicates = query.general_predicates(m.pattern_index);
    if !predicates.is_empty() && !evaluate_predicates(predicates, m.captures, types_cache)? {
        return Ok(true);
    }
    Ok(false)
}

pub fn collect_filter_ranges(
    m: &tree_sitter::QueryMatch,
    filter_capture_index: u32,
    out: &mut HashSet<tree_sitter::Range>,
) {
    for node in m.nodes_for_capture_index(filter_capture_index) {
        out.insert(node.range());
    }
}

#[cfg(test)]
mod test {
    use crate::{Config, Rule, lint_file};

    #[test]
    fn max_depth_fires_above_threshold() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "too-deep".into(),
            description: "nesting too deep".into(),
            query: r#"((obj_body) @error (#max-depth? @error "obj_body,block_exp" "2"))"#.into(),
            fix: None,
        };
        let input = "actor { func f() { if (true) { 0 } } };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn max_depth_does_not_leak_into_siblings() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "too-deep".into(),
            description: "nesting too deep".into(),
            query: r#"((obj_body) @error (#max-depth? @error "block_exp" "1"))"#.into(),
            fix: None,
        };
        let input = "actor { func f() { 0 } }; actor { func g() { if (true) { 0 } } };";
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
            query: r#"((block_exp) @error (#nesting-depth? @error "obj_body,block_exp" "2"))"#
                .into(),
            fix: None,
        };
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
            query: r#"((block_exp) @error (#nesting-depth? @error "obj_body,block_exp" "1"))"#
                .into(),
            fix: None,
        };
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
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "eq-test".into(),
            description: "pun: @field".into(),
            query: r#"((exp_field (identifier) @field (var_exp (identifier) @value)) @error (#eq? @field @value))"#.into(),
            fix: None,
        };
        let input = "{ x = x }";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn trailing_skips_match_with_next_sibling() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "unneeded-return".into(),
            description: "unneeded return".into(),
            query: r#"(func_dec (block_exp (exp_dec (return_exp)) @error @trailing))"#.into(),
            fix: None,
        };
        let input = "actor { func f() { return 10; 20 }; };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 0);
    }

    #[test]
    fn trailing_flags_match_without_next_sibling() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "unneeded-return".into(),
            description: "unneeded return".into(),
            query: r#"(func_dec (block_exp (exp_dec (return_exp)) @error @trailing))"#.into(),
            fix: None,
        };
        let input = "actor { func f() { return 10 }; };";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }

    #[test]
    fn filter_suppresses_matching_error_range() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "pun-fields".into(),
            description: "use punning".into(),
            query: r#"
                ((exp_field (identifier) @field (var_exp (identifier) @value)) @error (#eq? @field @value))
                (exp_field "var" (identifier) @field (var_exp (identifier) @value)) @filter
            "#.into(),
            fix: None,
        };
        let input = "{ var x = x }";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 0);
    }

    #[test]
    fn filter_does_not_suppress_non_matching_range() {
        let mut out: Vec<u8> = vec![];
        let rule = Rule {
            name: "pun-fields".into(),
            description: "use punning".into(),
            query: r#"
                ((exp_field (identifier) @field (var_exp (identifier) @value)) @error (#eq? @field @value))
                (exp_field "var" (identifier) @field (var_exp (identifier) @value)) @filter
            "#.into(),
            fix: None,
        };
        let input = "{ x = x }";
        let res = lint_file(&Config::default(), "<test>", input, &[rule], &mut out).unwrap();
        assert_eq!(res.error_count, 1);
    }
}
