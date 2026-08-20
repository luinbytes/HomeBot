# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M3, Skills, Plugins, Routines & Secrets.
- Current Linear issue: 6C7-52, routine recorder/editor and deterministic replay (`In Progress`).
- Current Git branch: `main`.
- Latest verified remote commit: `4a0736f51c540cce6ff47677453df6dceae5c6d6`.
- Latest verified GitHub Actions run: `32389096872`, all nine jobs passed.
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
- Local MCP plugins use a provider-neutral adapter boundary and constrained direct-executable stdio lifecycle. Plugin results are explicitly untrusted data; connection and Bot assignment never grant tool capability authority.
- Filesystem authority uses `cap-std`; terminal authority uses bounded `portable-pty`; browser authority uses loopback-only CDP.

Current blockers:

- No external blocker. Neither Codex nor Claude is installed/authenticated in this execution environment, so real provider smoke tests explicitly skip while protocol-faithful fixtures pass.
- M2 epic 6C7-41 is reopened. Urgent corrective issue 6C7-73 must wire `HomeBotApp` to authenticated HTTP/WebSocket state and bundled-server supervision before M2 can return to Done.
- `egui` 0.32.3 transitively uses unmaintained `ttf-parser` 0.25.1. RUSTSEC-2026-0192 reports no known vulnerability or safe upgrade; review remains mandatory in 6C7-69.

## Completed work

- M0 epic 6C7-30: baseline, parity inventory, protocol contract, and security model.
- M1 epic 6C7-34: Rust/CI foundation, SQLite/outbox/recovery, authenticated HTTP/WebSocket transport, provider runtime, Codex, Claude/OpenAI-compatible/community adapters, and local filesystem/PTY/browser capabilities.
- M2 component work: egui visual system, native shell, Bot lifecycle, direct chats, three-Bot groups/coordination, normalized activity/artifact surfaces, and settings/native notifications. Epic 6C7-41 is not complete until 6C7-73 passes.
- 6C7-50, local MCP/plugin registry, exact client recovery states, health/discovery, enablement/removal and per-Bot assignment.
- Most recent completed issue: 6C7-50.

## Immediate next work

1. Commit and push the verified 6C7-52 patch: structured MCP tool execution, correctable invalid recordings, durable failed runs, preserved approval stops, and server-event-only desktop projections.
2. Verify remote CI, record evidence and close 6C7-52 only if every postcondition passes.
3. Refresh Linear and immediately start the next unblocked issue. 6C7-73 remains an urgent required correction before M2 can close.

## Verification state

Verified at the reconciled remote baseline:

- GitHub Actions run 32389096872 passed formatting/clippy/112 tests, dependency policy/audit, Linux, macOS Intel, macOS Apple Silicon, and 15 cross-platform visual goldens for commit `4a0736f`.
- The remote commit author and committer resolve to GitHub account `luinbytes`.
- 6C7-50's authenticated integration fixture proves Connect -> Waiting -> Connected discovery, then malformed MCP health -> Error with disablement and cleared tools; the child environment is cleared and MCP results remain explicitly untrusted.

Verified locally for the active issue:

- `./scripts/check.sh` passes on Rust 1.98.0: formatting, clippy, the complete workspace suite with 114 tests, protocol/schema drift checks, generated Android binding drift checks, and all 15 exact desktop visual fixtures.
- Targeted routine/plugin/storage/server/desktop suites also pass independently.
- The routine verification patch proves durable version 2 edits, recording conversion, dry-run side-effect freedom, real manual Bot dispatch, MCP `tools/call`, approval stops, safe durable failures, editable recovery after invalid finish, restart persistence and server-event-only desktop projection updates.
- Secret-specific coverage passes: three vault tests, two policy-gated secret-tool tests, one authenticated API canary test, and two storage/migration tests.
- Secret API canary coverage proves values are absent from response bytes and message/activity/outbox JSON; locked-store mutation fails with `secret_store_locked`; delete removes the value.
- Protocol schema and generated Android bindings match the Rust-owned contract with no drift.

- Cargo-deny 0.20.2 reports advisories, bans, licenses, and sources all OK with only the existing documented warnings.

6C7-51 completion evidence is recorded in Linear and the issue is Done.
6C7-50 completion evidence is recorded in Linear and the issue is Done.

## Known failures and incomplete implementation

- Android app, skills, schedules/triggers, VCS/worktrees/checkpoints, device pairing/Tailscale, packaging, and release artifacts remain incomplete roadmap work.
- 6C7-52 is still In Progress until the verification patch passes the full gate and remote CI. M2 is reopened and 6C7-73 is Todo.
- Real authenticated Codex/Claude round trips are unavailable in this environment. OpenAI-compatible and CDP behavior use protocol-faithful local fixtures.
- No release artifact exists. Do not describe HomeBot as installable or v1-ready.

## Environment notes

- Workspace: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Rust commands require `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and `/tmp/homebot-cargo/bin` first in `PATH`.
- Local cargo-deny binary: `/tmp/cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny`.
- Shell Git read access works. If authenticated push is unavailable, publish with the connected GitHub blob/tree/commit/ref tools and then fast-forward local `main` to `origin/main`.
- Do not expose the current server beyond loopback. Preserve unrelated work and never reset/clean user repositories destructively.
