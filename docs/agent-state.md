# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M0, Product & Architecture Baseline
- Current Linear issue: 6C7-32 protocol contract
- Parallel scaffold issue: 6C7-35 remains In Progress but cannot close until compilation and CI are verified
- Current Git branch: `feat/m0-contracts`
- Latest verified remote commit: `40269eb0917f9df1c9cc7f3756a5d0b9b5d0de2c`
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
- GitHub Actions runs 1 and 2 are queued; CI success is unverified.
- Exact current-app pixel captures remain deliberately gated to 6C7-42.
- 6C7-32 implementation is locally verified and awaits commit, push, and Linear evidence.

## Completed work

- Public `luinbytes/HomeBot` repository created and populated.
- Initial Rust monorepo crate boundaries and baseline GitHub Actions committed.
- Architecture, protocol, provider, Android, routines, plugins, development, release, installation, and security documents created.
- Initial Grok Bot feature parity matrix created from authoritative SpaceXAI documentation.
- M0 security/capability threat model implemented in `docs/security.md`; Linear 6C7-33 is Done.
- Grok Bot behavioural parity inventory and 56-state visual reference index completed; Linear 6C7-31 is Done.

## Immediate next work

1. Commit and push the verified 6C7-32 protocol contract, then attach evidence and mark it Done.
2. Close epic 6C7-30 after confirming all three child issues remain Done.
3. Finish 6C7-35 by verifying GitHub Actions and all target compile jobs.
4. Refresh Linear and start 6C7-36, SQLite persistence, migrations, and event outbox.

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

Not yet verified:

- GitHub Actions jobs.
- Android application build; the generated protocol source is not compiled until the M4 Gradle module lands.
- Any end-user HomeBot behaviour beyond static contract inspection.

## Known failures and incomplete implementation

- `homebot-server` is a development-only unauthenticated loopback health stub.
- Storage, authentication, WebSocket replay, providers, desktop egui, Android, routines, plugins, tools, VCS, pairing, and packaging are not implemented.
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
