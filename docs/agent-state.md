# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M1, Local Runtime Foundation
- Current Linear issue: 6C7-37 authenticated HTTP/WebSocket server and reconnect semantics
- Current Git branch: `feat/m0-contracts`
- Latest verified remote commit: `bf02b9913eaf0a1a46d2a329e80352659fbf033b`
- Public repository: `https://github.com/luinbytes/HomeBot`
- Repository owner and commit identity: `luinbytes <42706009+luinbytes@users.noreply.github.com>`

Architecture decisions currently frozen:

- The Rust server is authoritative; desktop and Android use one versioned HTTP/WebSocket contract.
- HomeBot identity and transcript history are independent from provider conversation mappings.
- SQLite is authoritative for structured state; large artifacts use content-addressed storage.
- Server mutations use idempotency keys and an outbox-backed monotonic event sequence.
- Server-side capability policy is the only approval authority.
- The server binds to loopback by default; remote access is explicit and pairing uses short-lived single-use credentials.
- Secret values use OS-backed credential storage and never ordinary SQLite rows.
- Codex App Server uses structured stdio JSONL initially because its WebSocket transport is documented as experimental.
- T3 Code is MIT-licensed architectural inspiration; no proprietary Grok Bot source or assets are copied.

Current blockers:

- Rust is installed in isolated task paths under `/tmp/homebot-rustup` and `/tmp/homebot-cargo`.
- Exact current-app pixel captures remain deliberately gated to 6C7-42.
- 6C7-37 is In Progress. The working tree has a server library/router, SHA-256 and constant-time bearer validation, public secret-free health, authenticated version negotiation, stale-version rejection, and server-side denial tests. WebSocket, replay, mutation, attachment, cancellation, and backpressure work remains.

## Completed work

- Public `luinbytes/HomeBot` repository created and populated.
- Initial Rust monorepo crate boundaries and baseline GitHub Actions committed.
- Architecture, protocol, provider, Android, routines, plugins, development, release, installation, and security documents created.
- Initial Grok Bot feature parity matrix created from authoritative SpaceXAI documentation.
- M0 security/capability threat model implemented in `docs/security.md`; Linear 6C7-33 is Done.
- Grok Bot behavioural parity inventory and 56-state visual reference index completed; Linear 6C7-31 is Done.
- Versioned protocol v1, machine schema, Android binding, and golden contract tests completed; Linear 6C7-32 is Done.
- M0 epic 6C7-30 is Done.
- Rust workspace and CI quality gates completed; Linear 6C7-35 is Done.
- SQLite migrations, WAL persistence, event outbox, backup/restore, and recovery tests completed; Linear 6C7-36 is Done.

## Immediate next work

1. Commit and push the authenticated HTTP/version foundation for 6C7-37.
2. Implement authenticated WebSocket hello, snapshot/replay cursors, heartbeat, and slow-client bounds.
3. Implement idempotent command lifecycle, cancellation, and attachment upload with integration tests.
4. Close 6C7-37 only after disconnect/replay and backpressure postconditions pass.

## Verification state

Verified:

- Local and remote Git trees match at `00cdca9`.
- All three remote commits are attributed to GitHub account `luinbytes`.
- Working tree was clean before `feat/m0-contracts` was created.
- All committed TOML files parse with Python `tomllib`.
- Protocol JSON schema and golden JSON fixture parse successfully.
- Local documentation link targets referenced by the README exist.
- `git diff --check` passed for the committed baseline.
- `cargo fmt --all -- --check` passes.
- `cargo test --workspace` passes with six tests.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- Rust JSON Schema and generated Android binding drift checks pass.
- `homebot-storage` has seven passing tests covering clean install/table inventory, WAL mode, restart/outbox durability, backup/restore, migration rollback, corrupted startup, and concurrent access.

Not yet verified:

- GitHub Actions run 32346392738 passed all six quality, audit/policy, Linux, macOS Intel, and macOS arm64 jobs.
- Android application build; the generated protocol source is not compiled until the M4 Gradle module lands.
- Any end-user HomeBot behaviour beyond static contract inspection.

## Known failures and incomplete implementation

- `homebot-server` has authenticated health/version foundations but not the complete 6C7-37 transport.
- WebSocket replay, providers, desktop egui, Android, routines, plugins, tools, VCS, pairing, and packaging are not implemented.
- The protocol defines product-level v1 envelopes and lifecycle contracts; server persistence and transport behaviour remain implementation work in later issues.
- No release artifact exists. Do not describe the project as installable or v1-ready.

## Environment notes

- Workspace path in the current Work Mode environment: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- For Rust commands export `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and prepend `/tmp/homebot-cargo/bin` to `PATH`.
- Shell Git cannot authenticate to GitHub in this environment. Use the connected GitHub tools for remote writes, or configure normal Git authentication in a future environment.
- The GitHub connector can create trees, commits, and update refs; remote commits created this way are correctly attributed to `luinbytes`.
- Local repository config already sets the correct `luinbytes` noreply identity.
- Preserve the local `feat/m0-foundation` branch until its work is fully represented remotely; do not force-push or delete it casually.
- Do not expose the current server beyond loopback.
