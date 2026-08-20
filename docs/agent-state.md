# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M4, T3 Code Developer Superpowers. M2 and M3 are verified complete.
- Current Linear issue: 6C7-55, coding workspaces and isolated Git worktrees per chat (`In Progress`).
- Current Git branch: `main`.
- Latest verified implementation commit: `323660afb543b6869dc17a22fba7f7d65cf6d4f4`.
- Latest verified GitHub Actions run: `32419854283`, all nine jobs passed.
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
- 6C7-49, provider-neutral versioned Skills, authenticated library/import/export/assignment, deterministic provider assembly, exact historical message versions, desktop projection and Android/schema contract.
- M3 epic 6C7-48: plugins/MCP, OS-backed secrets, routine recording/replay, schedules/triggers/history, and reusable Skills.
- Most recent completed issue: 6C7-49.

## Immediate next work

1. Inventory the existing minimal `homebot-vcs` crate and define owner-scoped repository/workspace/worktree records plus the authenticated protocol contract for 6C7-55.
2. Implement safe repository discovery/validation and optional deterministic per-chat worktree creation without changing or cleaning the user's primary working tree.
3. Add branch/conflict/lifecycle handling, server APIs/events, SQLite migration/restart tests, hostile dirty-repository fixtures, and desktop/Android projections.

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
- 6C7-49's full gate passes with 130 Rust tests and all 15 desktop visual fixtures. Coverage proves CRUD/duplicate/delete, import conflict policy, multi-Bot assignment, immutable idempotent edit replay, deterministic provider assembly, exact historical version retention after edit/delete/restart, v11→v12 migration, and scheduler/provider write concurrency.

- Cargo-deny 0.20.2 reports advisories, bans, licenses, and sources all OK with only the existing documented warnings.
- 6C7-73 integration tests prove clean loopback supervision, authenticated version/hello/snapshot startup, real Bot/chat/message and attachment APIs, restart persistence, cursor resume without duplicate events, graceful WebSocket shutdown, and distinct authentication/version/unavailable failures. Existing server coverage proves stale-cursor snapshot fallback.
- 6C7-53's full gate passes at commit `7405c8c`: headless schedule restart, durable event restart, plugin filtering, forged-event denial, webhook deduplication, DST transitions, overlap/concurrency, exponential retry, cancellation, interrupted-job recovery, redacted history, v10 migration, generated schema/Android drift and desktop projections. GitHub Actions run `32416289234` passed all nine jobs.
- GitHub Actions run `32419854283` passed all nine jobs for 6C7-49 commit `323660a`: Rust quality, dependency audit/policy, Linux, macOS Intel/Apple Silicon builds, and all three visual-golden platforms.

6C7-73 and reopened M2 epic 6C7-41 completion evidence is recorded in Linear; both are Done. 6C7-49 and M3 epic 6C7-48 are verified Done. M4 epic 6C7-54 and 6C7-55 are In Progress.

## Known failures and incomplete implementation

- Android app, VCS/worktrees/checkpoints, device pairing/Tailscale, packaging, and release artifacts remain incomplete roadmap work.
- 6C7-55 repository workspace association and isolated worktree lifecycle are not implemented yet; `homebot-vcs` currently contains only a working-tree condition enum.
- Real authenticated Codex/Claude round trips are unavailable in this environment. OpenAI-compatible and CDP behavior use protocol-faithful local fixtures.
- No release artifact exists. Do not describe HomeBot as installable or v1-ready.

## Environment notes

- Workspace: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Rust commands require `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and `/tmp/homebot-cargo/bin` first in `PATH`.
- Local cargo-deny binary: `/tmp/cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny`.
- Shell Git read access works. If authenticated push is unavailable, publish with the connected GitHub blob/tree/commit/ref tools and then fast-forward local `main` to `origin/main`.
- Do not expose the current server beyond loopback. Preserve unrelated work and never reset/clean user repositories destructively.
