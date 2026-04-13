use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use tree_sitter::{Node, QueryCapture, QueryPredicate, QueryPredicateArg};

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

fn resolve_string_arg(args: &[QueryPredicateArg], idx: usize) -> Result<&str> {
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

pub fn evaluate_predicates(
    predicates: &[QueryPredicate],
    captures: &[QueryCapture<'_>],
) -> Result<bool> {
    for pred in predicates {
        let op = pred.operator.as_ref();
        match op {
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
