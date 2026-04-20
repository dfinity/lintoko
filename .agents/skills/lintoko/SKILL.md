---
name: lintoko
description: Write and debug lintoko lint rules for Motoko using TOML and tree-sitter queries. Use when creating, editing, or reviewing lintoko rule files, when the user mentions lintoko rules or Motoko linting rules, or when writing tree-sitter queries for Motoko code analysis.
---

# Lintoko — Writing Lint Rules for Motoko

Lintoko is an extensible linter for Motoko built on tree-sitter. Rules are TOML files containing tree-sitter queries that match the parse tree produced by the [motoko tree-sitter grammar](https://github.com/christoph-dfinity/tree-sitter-motoko).

Repo: [github.com/caffeinelabs/lintoko](https://github.com/caffeinelabs/lintoko)

## Rule TOML Format

```toml
name = "rule-name"
severity = "warning"  # optional, defaults to "error"
description = "Human-readable message. Can reference captures like @var."
query = """
(tree_sitter_query) @error
"""
fix = "@captured_replacement"  # optional
includes = ["backend/types/**"]      # optional
excludes = ["**/*.test.mo"]          # optional
```

### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Kebab-case rule identifier (used in error output) |
| `severity` | no | `"error"` (default) or `"warning"`. Warnings are reported but don't cause a non-zero exit code |
| `description` | yes | Message shown to the user. Supports `@capture` templating — capture names are replaced with matched source text at report time |
| `query` | yes | Tree-sitter query. Must contain at least one `@error` capture |
| `fix` | no | Replacement template using `@capture` references. When `--fix` is passed, the `@error` range is replaced with this expanded string |
| `includes` | no | List of globs; rule only runs on paths matching at least one. Empty/absent = match all |
| `excludes` | no | List of globs; rule is skipped on any matching path |

### Path filtering (`includes` / `excludes`)

Globs match the path string lintoko was handed (typically project-relative because `mops lint` runs from the project root). Patterns are anchored to the full path; use `**` to match any number of segments.

- Scope a rule to a directory: `includes = ["backend/types/**"]`
- Enforce a directory layout: pair `query = "(source_file) @error"` with `excludes = [...permitted paths...]` so the rule fires on every file *outside* the allowlist.
- Both fields together: file must match at least one `include` AND no `exclude`.

Patterns are validated at load time, so typos surface before linting starts.

### TOML string quoting

Use `"""` for most queries. Use `'''` when queries contain escaped quotes (e.g. matching `text_literal` content) — TOML `"""` interprets `\"` as `"`, breaking the tree-sitter predicate. Example: `query = ''' ... (#eq? @path "\"mo:core/Array\"") ... '''`

### Capture naming for templates

The template engine regex is `@([a-z-]+)` — only **lowercase letters and hyphens** work in `description`/`fix` references. Underscores are silently ignored. Use `@type-constructor` (hyphens) for template captures, `@left_var` (underscores) only for predicate-only captures.

## Special Captures

These capture names have special meaning in lintoko (from `lib.rs`):

| Capture | Required | Behavior |
|---------|----------|----------|
| `@error` | yes | Marks violation nodes. Determines diagnostic range and what `fix` replaces |
| `@trailing` | no | If the captured node has a `next_named_sibling`, the match is **skipped**. Use to enforce last-child position ([tree-sitter bug workaround](https://github.com/tree-sitter/tree-sitter/issues/4558)) |
| `@filter` | no | Suppresses `@error` matches at the same range. Use for exceptions |

## Query Pattern Reference

Lintoko uses standard [tree-sitter query syntax](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html). Below is a compressed reference of every technique available, with minimal examples.

### Basic node matching

```
(let_else_dec) @error                        ; match by node type
"flexible" @error                            ; match literal keyword token
["await*" "async*"] @error                   ; match any of several keywords
(node_type (child_type)) @error              ; parent with specific child
(bin_op "|>") @error                         ; match operator token inside bin_op
(func_dec (identifier) @ident @error         ; dual captures — @ident for predicate, @error for reporting
 (#not-match? @ident "^[a-z_][a-zA-Z0-9]*$"))
(import (text_literal) @path                 ; @error on parent, predicate on child — controls what gets highlighted
  (#match? @path "Debug")) @error
```

### Named fields and field negation

```
(if_exp then: (_) @error)                    ; named field (then:, else:, left:, right:, body:, params:, return_ty:, scrutinee:, name:, shared_pat:)
(func_dec !return_ty) @error                 ; negated field — matches only when return_ty is ABSENT
_ @error                                     ; wildcard — matches any node
```

### Alternative node types

Match any of several node structures with `[...]`. A capture after `]` captures whichever alternative matched:

```
([(dot_exp_object (var_exp))
  (dot_exp_block (var_exp))] @error          ; capture on ] applies to whichever matched
 (#eq? @error "Principal.fromText"))
```

### Anchors

`.` constrains position within siblings:

```
(tup_pat . (lit_pat (bool_literal)) @trailing)   ; must be first child AND last (via @trailing)
```

### Predicates

| Predicate | Example |
|-----------|---------|
| `#eq?` capture=capture | `(#eq? @var @left_var)` |
| `#eq?` capture=string | `(#eq? @import "Result")` |
| `#not-eq?` | `(#not-eq? @name "run")` |
| `#match?` regex | `(#match? @import "pure")` |
| `#not-match?` | `(#not-match? @ident "^[a-z_][a-zA-Z0-9]*$")` |
| `#any-of?` set | `(#any-of? @type "List" "Set" "Map")` |

Predicates go inside the outermost `()` of the pattern. Multiple `#not-eq?` predicates create an **allowlist** — everything is flagged except listed values. For path-based scoping use the `includes`/`excludes` rule fields instead of predicates.

### Multiple patterns

A single `query` field can contain multiple patterns separated by newlines. Each is matched independently. Use this for:

- **Commutative operators** — two patterns for `x := x + y` and `x := y + x`
- **Non-commutative operators** — one pattern suffices
- **Different node contexts** — same violation in `func_dec` and `class_dec`

```toml
query = """
(func_dec "shared" (var_pat) @error)
(class_dec "shared" . (var_pat) @error)
"""
```

### `@filter` patterns

A separate pattern in the same query that **suppresses** `@error` matches at the same range. Common strategies:

**Exclude a structural variant** (flag all `if` bodies, but allow `block_exp`):
```
(if_exp then: (_) @error)
(if_exp then: (block_exp) @filter)
(if_exp else: (if_exp) @filter)        ; also allow else-if chains
```

**Exclude by keyword** (flag `{ x = x }`, but not `{ var x = x }`):
```
((exp_field (identifier) @field (var_exp (identifier) @value)) @error
 (#eq? @field @value))
(exp_field "var" (identifier) @field (var_exp (identifier) @value)) @filter
```

**Exclude by content** (flag typed lambdas, but allow when body uses `return`):
```
(func_exp return_ty: (typ_annot) @error)
(func_exp return_ty: (typ_annot) @filter body: (_) @body
  (#match? @body "[^a-zA-Z_0-9]return"))
```

**Exclude by parent context** (also shows brute-force depth nesting — repeat at increasing levels since tree-sitter has no recursion):
```
(func_exp params: (_ (_ (typ_annot) @error)))
(func_exp params: (_ (_ (_ (typ_annot) @error))))
(func_exp params: (_ (_ (_ (_ (typ_annot) @error)))))
(let_dec (func_exp params: (_ (_ (typ_annot) @filter))))
```

**Allow-list via `@filter`** — To flag “anything except these shapes,” you often pair `(parent (_) @error)` with one or more `(parent (allowed_child)) @filter` patterns. **`@error` and each `@filter` must resolve to the same byte range** (see Common Pitfalls): typically both captures refer to **the same child node** (the `_` / `allowed_child` instance), not `@error` on `parent` and `@filter` only on a nested descendant (or the reverse).

**Catch-all `(_)` under a wide parent** — `(root (_) @error)` matches **every named child** of `root`. Depending on the grammar, that can include **comments**, **whitespace-related nodes**, or other **extras** as named siblings. You may need extra `@filter` patterns or a **narrower parent / explicit violation patterns** instead of a single wildcard high in the tree.

### `@trailing` for last-child

Ensures a node is the last named sibling in its parent. Stack `@trailing` at multiple nesting levels for tail-position checks:

```
(func_dec (block_exp (exp_dec (if_exp
  then: (block_exp (exp_dec (return_exp)) @error @trailing))) @trailing))
```

### Comments in queries

Use `;` for inline comments: `; this pattern handles actor classes`

### Common Motoko node types

Refer to the [tree-sitter-motoko grammar](https://github.com/christoph-dfinity/tree-sitter-motoko) for the full list.

**Declarations:** `func_dec`, `let_dec`, `var_dec`, `typ_dec`, `class_dec`, `let_else_dec`, `import`, `obj_dec`
**Expressions:** `var_exp`, `call_exp_object`, `dot_exp_object`, `dot_exp_block`, `bin_exp_object`, `assign_exp_object`, `return_exp`, `switch_exp`, `if_exp`, `block_exp`, `func_exp`, `label_exp`
**Types:** `path_typ`, `async_typ`, `typ_path`, `type_identifier`, `typ_annot`, `typ_params`, `typ_bind`
**Patterns:** `var_pat`, `tup_pat`, `lit_pat`, `wild_pat`, `obj_pat`, `annot_pat`, `quest_pat`, `val_pat_field`, `case`
**Structure:** `source_file`, `obj_body`, `dec_field`, `exp_field`, `exp_dec`, `catch`
**Operators:** `bin_op`
**Literals:** `identifier`, `text_literal`, `bool_literal`

Use `tree-sitter parse file.mo` or the tree-sitter playground to inspect the actual parse tree of Motoko code.

## Fix Templates

The `fix` field is a string template. `@capture` references are replaced with matched source text. The **entire `@error` range** is replaced.

| Pattern | Fix | Effect |
|---------|-----|--------|
| Substitute | `fix = "@field"` | `{ x = x }` → `{ x }` |
| Wrap | `fix = "{ @error }"` | `expr` → `{ expr }` |
| Delete | `fix = ""` | removes the matched node |

**Constraints:** fixes are applied in reverse byte-offset order; overlapping ranges are skipped (re-run to converge).

## Common Pitfalls

- **No recursive queries** — tree-sitter can't match "at any depth"; repeat patterns at increasing nesting: `(_ (_ (target) @error))`, `(_ (_ (_ (target) @error)))`, etc.
- **`@trailing` is global** — ANY `@trailing` capture with a `next_named_sibling` skips the ENTIRE match, not just that sub-pattern
- **`@filter` matches by byte range** — `@filter` and `@error` must produce identical byte ranges to suppress; different ranges won't cancel. For allow-lists, ensure both captures target the **same node** (same pattern depth), as in the `if_exp then:` example: `(_) @error` and `(block_exp) @filter` both refer to the **then** child, not the outer `if_exp`
- **Deduplication** — the engine deduplicates by byte range per rule, so overlapping patterns are safe

## Writing Rules — Process

1. **Identify the pattern** you want to flag in Motoko code
2. **Parse a sample** with `tree-sitter parse sample.mo` to see the concrete syntax tree
3. **Write the query** matching the violation, using `@error` on the node to highlight
4. **Add predicates** to narrow matches (equality, regex, etc.)
5. **Handle exceptions** with `@filter` if needed
6. **Add `fix`** if the correction can be expressed as a template
7. **Test** with `lintoko -r single-rule.toml sample.mo` (runs one rule on one file)

## Running Lintoko

### CLI

```bash
lintoko -r single-rule.toml file.mo         # iterate on one rule + one file
lintoko -r <rules-dir> [files/dirs/globs]   # lint files with a rule directory
lintoko -r rules --fix                      # apply auto-fixes
lintoko -r rules -f text                    # text output (vs pretty)
lintoko -r my-rules -r more-rules src/      # multiple rule dirs
lintoko -r rules -s warning src/            # treat all rules as warnings
```

When no input files are specified, lintoko lints all `**/*.mo` files under the current directory.

### Mops integration

Specify lintoko version in `mops.toml`:

```toml
[toolchain]
lintoko = "0.7.0"
```

Install via `mops install` or `mops toolchain use lintoko 0.7.0`.

## Additional resources

- Example rules: [example-rules/](https://github.com/caffeinelabs/lintoko/tree/main/example-rules)
- Grammar reference: [tree-sitter-motoko](https://github.com/christoph-dfinity/tree-sitter-motoko)
- Tree-sitter query docs: [tree-sitter queries](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html)
