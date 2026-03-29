# AGENTS.md

## Release Process

For a new release:

1. Update the version in `Cargo.toml`.
2. Run `cargo test`.
3. Run `cargo build --release`.
4. Run `cargo package`.
5. Run `cargo publish --dry-run`.
6. Run `cargo publish`.
7. After the new version appears on crates.io, create and push a matching git tag such as `v0.1.1`.

Publish to crates.io before creating and pushing the release tag. `cargo publish` is the irreversible step.
