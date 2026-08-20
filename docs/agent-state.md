# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M3, Skills, Plugins, Routines & Secrets.
- Current Linear issue: 6C7-51, OS-backed secret store and safe tool injection (`In Progress`).
- Current Git branch: `feat/m0-contracts`.
- Latest verified remote commit: `3215f4ddb3a19955a1e0440512f1fd74eea4d769`.
- Latest verified GitHub Actions run: `32379291068`, all nine jobs passed.
- Public repository: `https://github.com/luinbytes/HomeBot`.
- Required commit identity: `luinbytes <42706009+luinbytes@users.noreply.github.com>`.

Architecture decisions currently frozen:

- The Rust server is authoritative; desktop and Android use one versioned HTTP/WebSocket contract.
- HomeBot Bot identity and transcript history are independent from provider conversation mappings.
- SQLite owns structured state and a monotonic outbox; large artifacts use content-addressed server storage.
- The server is the sole capability/approval authority and binds to loopback by default.
- Secret values use macOS Keychain or Linux Secret Service through `homebot-secrets`; SQLite, protocol events, chat/routine context, and normal provider configuration hold opaque references only.
- OS credential calls run on Tokio's blocking pool. Locked/unavailable stores fail closed with no plaintext fallback. Resolved provider values are redacted and zeroized.
- Codex uses structured App Server stdio JSONL. Claude uses its documented stream-JSON CLI surface. Community processes use a constrained direct-executable JSONL contract.
- Filesystem authority uses `cap-std`; terminal authority uses bounded `portable-pty`; browser authority uses loopback-only CDP.

Current blockers:

- No external blocker. Neither Codex nor Claude is installed/authenticated in this execution environment, so real provider smoke tests explicitly skip while protocol-faithful fixtures pass.
- `egui` 0.32.3 transitively uses unmaintained `ttf-parser` 0.25.1. RUSTSEC-2026-0192 reports no known vulnerability or safe upgrade; review remains mandatory in 6C7-69.

## Completed work

- M0 epic 6C7-30: baseline, parity inventory, protocol contract, and security model.
- M1 epic 6C7-34: Rust/CI foundation, SQLite/outbox/recovery, authenticated HTTP/WebSocket transport, provider runtime, Codex, Claude/OpenAI-compatible/community adapters, and local filesystem/PTY/browser capabilities.
- M2 epic 6C7-41: egui visual system, native shell, Bot lifecycle, direct chats, three-Bot groups/coordination, normalized activity/artifact surfaces, and settings/native notifications.
- Most recent completed issues: 6C7-46 and 6C7-47; M2 epic 6C7-41 is Done.

## Immediate next work

1. Finish 6C7-51 by running the full local gate and dependency policy against the new `homebot-secrets` crate, migration 0008, secret protocol/API, canary tests, and documentation.
2. Commit and push the coherent issue under the required `luinbytes` identity; wait for all GitHub Actions jobs, attach evidence to Linear 6C7-51, and mark it Done only on green CI.
3. Refresh Linear and begin the highest-priority unblocked child of 6C7-48 (expected 6C7-50 plugin/MCP registry unless dependencies changed).

## Verification state

Verified at the last remote baseline:

- GitHub Actions run 32379291068 passed formatting/clippy/tests, dependency policy/audit, Linux, macOS Intel, macOS Apple Silicon, and cross-platform visual goldens.
- The remote commit author and committer resolve to GitHub account `luinbytes`.
- `./scripts/check.sh` passed with 92 Rust tests and 11 exact desktop visual fixtures before 6C7-51 began.

Verified locally for the active issue:

- `cargo check --workspace --all-targets` passes with `keyring` 3.6.3 on Linux.
- `./scripts/check.sh` passes with 101 Rust tests, 11 exact desktop visual fixtures, full workspace clippy, and both protocol drift checks.
- Secret-specific coverage passes: three vault tests, two policy-gated secret-tool tests, one authenticated API canary test, and two storage/migration tests.
- Secret API canary coverage proves values are absent from response bytes and message/activity/outbox JSON; locked-store mutation fails with `secret_store_locked`; delete removes the value.
- Protocol schema and generated Android bindings were regenerated with redacted Android secret-request `toString` behavior.

- Cargo-deny 0.20.2 reports advisories, bans, licenses, and sources all OK with only the existing documented warnings.

Pending before the issue commit:

- Run the final formatting/diff check after the last validation-order edit, then commit/push and wait for remote CI.

## Known failures and incomplete implementation

- 6C7-51 is not yet complete or pushed. Its worktree changes are coherent but still require the full gate and remote CI.
- Android app, routines, plugins/MCP, VCS/worktrees/checkpoints, device pairing/Tailscale, packaging, and release artifacts remain incomplete roadmap work.
- Real authenticated Codex/Claude round trips are unavailable in this environment. OpenAI-compatible and CDP behavior use protocol-faithful local fixtures.
- No release artifact exists. Do not describe HomeBot as installable or v1-ready.

## Environment notes

- Workspace: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Rust commands require `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and `/tmp/homebot-cargo/bin` first in `PATH`.
- Local cargo-deny binary: `/tmp/cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny`.
- Shell Git cannot authenticate to GitHub. Push with the connected GitHub blob/tree/commit/ref tools, then align local `feat/m0-contracts` to `origin/main` without force-pushing.
- Do not expose the current server beyond loopback. Preserve unrelated work and never reset/clean user repositories destructively.
