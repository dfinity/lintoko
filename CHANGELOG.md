# Unreleased

- fix: print the correct version number when calling `lintoko --version`

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
