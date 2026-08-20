# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M3, Skills, Plugins, Routines & Secrets. M2 is verified complete after corrective integration.
- Current Linear issue: 6C7-49, Skill model, library, import/export and Bot assignment (`In Progress`).
- Current Git branch: `main`.
- Latest verified implementation commit: `7405c8c47d3963085e4375eccfbce86daf246985`.
- Latest verified GitHub Actions run: `32416289234`, all nine jobs passed.
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
- No active M2 blocker. Corrective issue 6C7-73 and epic 6C7-41 are verified Done.
- `egui` 0.32.3 transitively uses unmaintained `ttf-parser` 0.25.1. RUSTSEC-2026-0192 reports no known vulnerability or safe upgrade; review remains mandatory in 6C7-69.

## Completed work

- M0 epic 6C7-30: baseline, parity inventory, protocol contract, and security model.
- M1 epic 6C7-34: Rust/CI foundation, SQLite/outbox/recovery, authenticated HTTP/WebSocket transport, provider runtime, Codex, Claude/OpenAI-compatible/community adapters, and local filesystem/PTY/browser capabilities.
- M2 epic 6C7-41: egui visual system, native shell, Bot lifecycle, direct chats, three-Bot groups/coordination, normalized activity/artifact surfaces, settings/native notifications, and the authenticated integrated application/runtime lifecycle.
- 6C7-50, local MCP/plugin registry, exact client recovery states, health/discovery, enablement/removal and per-Bot assignment.
- 6C7-52, durable routine create/edit/versioning, structured recording conversion, deterministic Bot/MCP replay, dry run, Run now, approval preservation, restart persistence, durable failures and server-driven desktop projections.
- 6C7-73, real `HomeBotApp` authenticated transport, local authoritative-server supervision, snapshot/replay reconnect, HTTP mutation routing and restart/failure verification. M2 epic 6C7-41 is Done again with corrective evidence.
- 6C7-53, durable headless one-shot/interval/timezone schedules, missed-run recovery, outbox-backed event/plugin triggers, deduplicated webhooks, version-pinned jobs, concurrency/overlap policy, retries, cancellation, redacted run history and v10→v11 migration safety.
- Most recent completed issue: 6C7-53.

## Immediate next work

1. Define 6C7-49's versioned Skill instruction/context/tool-bundle contract and migration without leaking provider-specific prompt formats.
2. Implement owner-scoped create/edit/duplicate/delete, import/export conflict handling and multi-Bot assignment through authenticated server APIs.
3. Assemble assigned Skill versions deterministically into provider turns and persist the exact applied versions on historical messages before completing the protocol/desktop/Android drift gates.

## Verification state

Verified at the latest remote baseline:

- GitHub Actions run 32409841146 passed all nine jobs: formatting/clippy/114 tests, dependency policy/audit, Linux, macOS Intel, macOS Apple Silicon, and 15 cross-platform visual goldens for commit `82a2d11b`.
- The remote commit author and committer resolve to GitHub account `luinbytes`.
- 6C7-50's authenticated integration fixture proves Connect -> Waiting -> Connected discovery, then malformed MCP health -> Error with disablement and cleared tools; the child environment is cleared and MCP results remain explicitly untrusted.

Verified locally for the active issue:

- `./scripts/check.sh` passes on Rust 1.98.0: formatting, clippy, the complete workspace suite with 123 tests, protocol/schema drift checks, generated Android binding drift checks, and all 15 exact desktop visual fixtures.
- Targeted routine/plugin/storage/server/desktop suites also pass independently.
- The routine verification patch proves durable version 2 edits, recording conversion, dry-run side-effect freedom, real manual Bot dispatch, MCP `tools/call`, approval stops, safe durable failures, editable recovery after invalid finish, restart persistence and server-event-only desktop projection updates.
- Secret-specific coverage passes: three vault tests, two policy-gated secret-tool tests, one authenticated API canary test, and two storage/migration tests.
- Secret API canary coverage proves values are absent from response bytes and message/activity/outbox JSON; locked-store mutation fails with `secret_store_locked`; delete removes the value.
- Protocol schema and generated Android bindings match the Rust-owned contract with no drift.

- Cargo-deny 0.20.2 reports advisories, bans, licenses, and sources all OK with only the existing documented warnings.
- 6C7-73 integration tests prove clean loopback supervision, authenticated version/hello/snapshot startup, real Bot/chat/message and attachment APIs, restart persistence, cursor resume without duplicate events, graceful WebSocket shutdown, and distinct authentication/version/unavailable failures. Existing server coverage proves stale-cursor snapshot fallback.
- 6C7-53's full gate passes at commit `7405c8c`: headless schedule restart, durable event restart, plugin filtering, forged-event denial, webhook deduplication, DST transitions, overlap/concurrency, exponential retry, cancellation, interrupted-job recovery, redacted history, v10 migration, generated schema/Android drift and desktop projections. GitHub Actions run `32416289234` passed all nine jobs.

6C7-73 and reopened M2 epic 6C7-41 completion evidence is recorded in Linear; both are Done. 6C7-53 is verified Done and 6C7-49 is In Progress.

## Known failures and incomplete implementation

- Android app, skills, VCS/worktrees/checkpoints, device pairing/Tailscale, packaging, and release artifacts remain incomplete roadmap work.
- 6C7-49 Skill persistence, assignment, import/export and deterministic historical prompt application are not implemented yet.
- Real authenticated Codex/Claude round trips are unavailable in this environment. OpenAI-compatible and CDP behavior use protocol-faithful local fixtures.
- No release artifact exists. Do not describe HomeBot as installable or v1-ready.

## Environment notes

- Workspace: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Rust commands require `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and `/tmp/homebot-cargo/bin` first in `PATH`.
- Local cargo-deny binary: `/tmp/cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny`.
- Shell Git read access works. If authenticated push is unavailable, publish with the connected GitHub blob/tree/commit/ref tools and then fast-forward local `main` to `origin/main`.
- Do not expose the current server beyond loopback. Preserve unrelated work and never reset/clean user repositories destructively.
