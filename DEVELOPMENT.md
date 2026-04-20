# Building

Requirements:
- Rust toolchain (install via rustup)

Nice-to-have:
- [`just` task runner](https://just.systems/)
- [`cargo-insta` testing tool](http://insta.rs/docs/quickstart/)

Check the [`justfile`](./justfile) for commands to run common tasks

# Making releases

When updating to version vX.Y.Z

1. Make sure the Changelog is up-to-date, and update its latest header with the new version
2. Update the version in Cargo.toml + run `cargo build` to update the lockfile
3. git commit -m "release: X.Y.Z" on a release branch, push, `gh pr create --fill`
4. Get the PR reviewed and merged
5. git fetch origin main
6. git tag vX.Y.Z origin/main
7. git push origin vX.Y.Z

Automation will take care of creating the release and its binaries. This is using `cargo-dist` and configured in [`dist-workspace.toml`](./dist-workspace.toml)
