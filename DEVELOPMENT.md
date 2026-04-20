# Building

Requirements:
- Rust toolchain (install via rustup)

Nice-to-have:
- [`just` task runner](https://just.systems/)
- [`cargo-insta` testing tool](http://insta.rs/docs/quickstart/)

Check the [`justfile`](./justfile) for commands to run common tasks.

# Making releases

For version `X.Y.Z`:

1. `git checkout -b release/X.Y.Z`
2. In [`CHANGELOG.md`](./CHANGELOG.md): rename `# Unreleased` to `# X.Y.Z` and add a fresh empty `# Unreleased` above it.
3. Bump `version` in [`Cargo.toml`](./Cargo.toml); run `cargo build` to refresh `Cargo.lock`.
4. Commit `release: X.Y.Z`, open a PR, get it reviewed, merge.
5. Push the tag from your machine:

    ```sh
    git fetch origin main
    git tag vX.Y.Z origin/main
    git push origin vX.Y.Z
    ```

The tag triggers [`release.yml`](./.github/workflows/release.yml) (cargo-dist,
configured in [`dist-workspace.toml`](./dist-workspace.toml)) to build the
binaries and publish the GitHub Release.
