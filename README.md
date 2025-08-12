# lintoko

An extensible linting tool for Motoko

## Installation

Download the latest release from [GitHub](https://github.com/dfinity/lintoko/releases) and put it in your PATH.

## Running

```bash
# Linting all Motoko files underneath the current directory
lintoko

# Linting a single file
lintoko src/actor.mo

# Linting all files in the `src` and `test` directories
lintoko src test
```

Specify custom rules with the `-r` flag. The tool will look for rules in the specified directory. You can pass multiple directories

```bash
lintoko -r custom-rules -r more-rules
```

## Defining Rules

Rules are specified as TOML files. For example this rule forbids the usage of `let-else`:

```toml
name = "no-let-else"
description = "Do not use let-else. Use a switch instead."
query =  """
(let_else_dec) @error
"""
```

The "query" field contains a [Tree-sitter query](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html) that matches a parse tree produced by a [Motoko tree-sitter grammar](https://github.com/christoph-dfinity/tree-sitter-motoko).
Look at the rules in [`custom-rules`](./custom-rules) or [`default-rules`](./default-rules) for more complex examples.
