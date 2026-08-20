# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M3, Skills, Plugins, Routines & Secrets.
- Current Linear issue: 6C7-52, routine recorder/editor and deterministic replay (`In Progress`).
- Current Git branch: `feat/m0-contracts`.
- Latest verified remote commit: `e0c9014551133aaab9873538c54f8a4a758a5d0d`.
- Latest verified GitHub Actions run: `32385491987`, all nine jobs passed.
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
- `egui` 0.32.3 transitively uses unmaintained `ttf-parser` 0.25.1. RUSTSEC-2026-0192 reports no known vulnerability or safe upgrade; review remains mandatory in 6C7-69.

## Completed work

- M0 epic 6C7-30: baseline, parity inventory, protocol contract, and security model.
- M1 epic 6C7-34: Rust/CI foundation, SQLite/outbox/recovery, authenticated HTTP/WebSocket transport, provider runtime, Codex, Claude/OpenAI-compatible/community adapters, and local filesystem/PTY/browser capabilities.
- M2 epic 6C7-41: egui visual system, native shell, Bot lifecycle, direct chats, three-Bot groups/coordination, normalized activity/artifact surfaces, and settings/native notifications.
- 6C7-50, local MCP/plugin registry, exact client recovery states, health/discovery, enablement/removal and per-Bot assignment.
- Most recent completed issue: 6C7-50.

## Immediate next work

1. For 6C7-52, define versioned routine/draft/recording contracts with structured steps, typed inputs, expected outputs, plugin/tool references, explicit approvals and deterministic replay semantics.
2. Add durable create/edit/rename/duplicate/delete/enable-disable operations plus recording-to-editable-draft conversion, dry-run and manual Run now execution.
3. Add authenticated APIs/events, desktop routine list/editor/recording projections and goldens, protocol/schema/Android models, restart/error/approval fixtures and docs; then pass the complete gate and remote CI.

## Verification state

Verified at the last remote baseline:

- GitHub Actions run 32385491987 passed formatting/clippy/107 tests, dependency policy/audit, Linux, macOS Intel, macOS Apple Silicon, and 12 cross-platform visual goldens.
- The remote commit author and committer resolve to GitHub account `luinbytes`.
- 6C7-50's authenticated integration fixture proves Connect -> Waiting -> Connected discovery, then malformed MCP health -> Error with disablement and cleared tools; the child environment is cleared and MCP results remain explicitly untrusted.

Verified locally for the active issue:

- `cargo check --workspace --all-targets` passes with `keyring` 3.6.3 on Linux.
- `./scripts/check.sh` passes with 107 Rust tests, 12 exact desktop visual fixtures, full workspace clippy, and both protocol drift checks.
- Secret-specific coverage passes: three vault tests, two policy-gated secret-tool tests, one authenticated API canary test, and two storage/migration tests.
- Secret API canary coverage proves values are absent from response bytes and message/activity/outbox JSON; locked-store mutation fails with `secret_store_locked`; delete removes the value.
- Protocol schema and generated Android bindings were regenerated with redacted Android secret-request `toString` behavior.

- Cargo-deny 0.20.2 reports advisories, bans, licenses, and sources all OK with only the existing documented warnings.

6C7-51 completion evidence is recorded in Linear and the issue is Done.
6C7-50 completion evidence is recorded in Linear and the issue is Done.

## Known failures and incomplete implementation

- Android app, routines, skills, VCS/worktrees/checkpoints, device pairing/Tailscale, packaging, and release artifacts remain incomplete roadmap work.
- Real authenticated Codex/Claude round trips are unavailable in this environment. OpenAI-compatible and CDP behavior use protocol-faithful local fixtures.
- No release artifact exists. Do not describe HomeBot as installable or v1-ready.

## Environment notes

- Workspace: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Rust commands require `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and `/tmp/homebot-cargo/bin` first in `PATH`.
- Local cargo-deny binary: `/tmp/cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny`.
- Shell Git cannot authenticate to GitHub. Push with the connected GitHub blob/tree/commit/ref tools, then align local `feat/m0-contracts` to `origin/main` without force-pushing.
- Do not expose the current server beyond loopback. Preserve unrelated work and never reset/clean user repositories destructively.
