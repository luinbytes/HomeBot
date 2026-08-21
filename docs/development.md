# Development

Performance and accessibility budgets, automated gates and physical-machine measurement steps are defined in [performance-accessibility.md](performance-accessibility.md).

## Reproducible quality check

From the repository root, run:

```sh
./scripts/check.sh
```

This is the same formatting, clippy, workspace test, JSON Schema drift, and Android binding drift sequence enforced by CI.

CI uses GitHub's current `macos-15-intel` label for x86_64 compilation and `macos-14` for Apple Silicon. Retired macOS runner labels must not be retained merely because their jobs remain queued.

Use Rust stable from `rust-toolchain.toml`. Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features` before committing.

## Android

Install JDK 17 and Android SDK 36, then run the Android gate from the repository root:

```sh
cd android
./gradlew lintDebug testDebugUnitTest assembleDebug
```

The build uses the checked-in Gradle wrapper. Do not hand-edit `android/protocol`; update the Rust protocol exporter and regenerate it with:

```sh
cargo run -p homebot-protocol --example export_android
cargo run -p homebot-protocol --example export_android -- --check
```

CI repeats lint, unit tests, schema drift verification, and debug APK assembly. Android tests use a deterministic MockWebServer rather than a developer's HomeBot instance.

Git/worktree fixtures use a real installed Git binary and temporary repositories. They must prove preservation of dirty and untracked primary-tree data as well as successful cleanup; never replace these postcondition checks with mocked command strings.

Fixtures belong under `tests/fixtures` or the owning crate. Persistent changes require migration-upgrade fixtures. Protocol changes require schema and golden fixture updates. Visible client changes require deterministic screenshots. Security boundaries require a negative test that proves denial at the server.

Do not use a real user repository as a VCS fixture. Create isolated fixture repositories representing clean, dirty, conflicted, detached, untracked, renamed, binary, and symlink-hostile states.
