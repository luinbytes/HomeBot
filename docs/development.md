# Development

## Reproducible quality check

From the repository root, run:

```sh
./scripts/check.sh
```

This is the same formatting, clippy, workspace test, JSON Schema drift, and Android binding drift sequence enforced by CI.

Use Rust stable from `rust-toolchain.toml`. Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features` before committing.

Fixtures belong under `tests/fixtures` or the owning crate. Persistent changes require migration-upgrade fixtures. Protocol changes require schema and golden fixture updates. Visible client changes require deterministic screenshots. Security boundaries require a negative test that proves denial at the server.

Do not use a real user repository as a VCS fixture. Create isolated fixture repositories representing clean, dirty, conflicted, detached, untracked, renamed, binary, and symlink-hostile states.
