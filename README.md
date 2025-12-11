# lintoko

An extensible linting tool for Motoko

## Installation

Download the latest release from [GitHub](https://github.com/caffeinelabs/lintoko/releases)

## Running

```bash
# Linting all Motoko files underneath the current directory
lintoko -r rules

# Linting a single file
lintoko -r rules src/actor.mo

# Linting all files in the `src` and `test` directories
lintoko -r rules src test
```

Specify rules with the `-r` flag. The tool will look for rules in the specified directory. You can pass multiple directories

```bash
lintoko -r my-rules -r more-rules
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

The "query" field contains a [Tree-sitter query](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html) that matches a parse tree produced by the [motoko tree-sitter grammar](https://github.com/christoph-dfinity/tree-sitter-motoko).
Look at the rules in [`example-rules`](./example-rules) for more complex examples.


## LICENSE

Copyright 2025 DFINITY Stiftung

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
