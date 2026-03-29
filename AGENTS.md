# AGENTS.md

## Release Process

For a new release:

1. Update the version in `Cargo.toml` and `Cargo.lock`.
2. Commit the release version changes before packaging, publishing, or tagging.
3. Run `cargo test`.
4. Run `cargo build --release`.
5. Run `cargo package`.
6. Run `cargo publish --dry-run`.
7. Run `cargo publish`.
8. After the new version appears on crates.io, create a matching git tag such as `v0.1.1` on the release commit, then push the commit and tag.

Publish to crates.io before creating and pushing the release tag. `cargo publish` is the irreversible step, and the release tag must point at the committed version bump.
