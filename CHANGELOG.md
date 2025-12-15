# Unreleased

# 0.7.0
- feat: allows specifying fixes in rules, and automatically applies with the `--fix` flag

# 0.6.0
- breaking: default rules are no longer a thing. All rules need to be passed via command line flags

# 0.5.1
- chore: updates grammar version

# 0.5.0
- feat: allow passing a single rule to iterate on
- feat: adds rules for binary assignment operators *, /, and #
- chore: updates grammar version

# 0.4.3
- chore: updates tree-sitter version to support mixins and weak references

# 0.4.2
- fix: Don't error on non-persistent actors, the compiler takes care of that

# 0.4.1
- fix: also check casing on classes and type parameters
- feat: adds a textual output format that's easier to consume with AI or screen readers

# 0.4.0
- feat: lint casing for type and function definitions
- fix: print the correct version number when calling `lintoko --version`
- chore: infra/license/etc changes to support Open Sourcing

# 0.3.2
- fix: Don't suggest punning for var fields

# 0.3.1
- feat: allows passing directories, files and globs to the CLI
    Also makes it so no arguments expand to all Motoko files underneath
    the current directory

# 0.3.0
- feat: lints unneeded returns
- fix: make linting for switches over booleans more precise (#2)

# 0.2.1
- chore: Updates release process

# 0.2.0
- Makes assign-minus, assign-plus, no-bool-switch, and pun-fields rules default
- Adds rule guarding against pure/ imports
- Adds rule to disallow non-primitive return types from public functions
