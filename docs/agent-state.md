# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M2, Grok Bot Desktop Parity
- Current Linear issue: 6C7-44 Direct chat timeline, composer, streaming and activities
- Current Git branch: `feat/m0-contracts`
- Latest verified remote commit: `790a2854ad2ad15d55cb5f8b600da4f6ac742ba5`
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
- Claude Code uses its documented `stream-json` CLI bridge because Anthropic publishes TypeScript and Python Agent SDKs but no Rust SDK.
- BYOK profiles persist opaque secret references only. Credentials are resolved at request time, redacted and zeroized; remote endpoints require HTTPS and redirects are disabled.
- Community provider processes use a constrained direct-executable JSONL contract with a cleared environment, bounded records and no implicit shell.
- Local filesystem access uses `cap-std` capability directories plus traversal and symlink rejection; content digests bind approved writes.
- Terminal execution uses `portable-pty` with explicit executables, canonical workspace working directories, a cleared allowlisted environment, concurrency/input/output/runtime bounds and kill-and-reap cancellation.
- Browser automation uses loopback-only Chrome DevTools Protocol endpoints and targets; browser profile files remain in a server-owned local directory and are never copied into SQLite or synchronized to clients.
- T3 Code is MIT-licensed architectural inspiration; no proprietary Grok Bot source or assets are copied.

Current blockers:

- Rust is installed in isolated task paths under `/tmp/homebot-rustup` and `/tmp/homebot-cargo`.
- Exact current-app pixel captures remain required during 6C7-42; only legitimate public/current-app references may be used.
- 6C7-37 is Done. GitHub Actions run 32352700967 passed all six Linux, macOS Intel, macOS arm64, quality, dependency-policy, and audit jobs.
- 6C7-38 is Done. GitHub Actions run 32354047604 passed the full six-job matrix.
- 6C7-39 is Done. GitHub Actions run 32355805077 passed the full six-job matrix. The current environment has no `codex` binary, so the real-binary smoke test skips with an explicit reason; fake executable App Server fixtures verify structured start, resume, streaming, approval, and interruption round trips.
- 6C7-40 is Done. GitHub Actions run 32358033480 passed the full six-job matrix after the dependency policy explicitly admitted webpki-roots' permissive CDLA-Permissive-2.0 license.
- M1 is complete. 6C7-72 and epic 6C7-34 are Done after GitHub Actions run 32361115615 passed all six jobs.
- 6C7-42 is Done. GitHub Actions run 32363729132 passed all nine quality, dependency, platform-build and cross-platform visual-golden jobs after the bounded executable-busy retry fix.
- 6C7-43 is Done. GitHub Actions run 32366996632 passed all nine jobs with the native eframe executable and cross-platform visual goldens.
- 6C7-44 is In Progress. Durable direct chats/messages/rich parts/queued prompts, authenticated create/timeline/send/steer/stop operations, normalized event contracts and a reconnect-safe native timeline/composer model are implemented locally. The exact next action is to wire provider execution into the direct-chat turn lifecycle, persist activity/approval updates, add approval/retry endpoints, and verify streaming plus cancellation end to end.
- egui 0.32.3 transitively uses unmaintained `ttf-parser` 0.25.1. RUSTSEC-2026-0192 reports no known vulnerability and no safe upgrade; the exact advisory is documented and temporarily ignored, with mandatory review in 6C7-69.

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
- Authenticated HTTP/WebSocket transport, verified attachments, reconnect/replay, cancellation, heartbeat, idempotency, and bounded slow-client handling completed; Linear 6C7-37 is Done.
- ProviderAdapter lifecycle/runtime and bounded child-process supervision completed; Linear 6C7-38 is Done.
- Codex App Server adapter implemented with profile-scoped binary/environment configuration, auth and model discovery, structured threads/turns, streaming activities, approvals, usage, cancellation, compaction, normalized failures, and fixture-backed protocol tests; Linear 6C7-39 is Done.
- Claude Code, OpenAI-compatible BYOK and constrained generic process adapters implemented with normalized streaming, cancellation, failures and request-time secret references; Linear 6C7-40 is Done.
- Scoped filesystem, PTY/process and loopback CDP browser capabilities with server-side policy, approvals, activity and hostile-input tests completed; Linear 6C7-72 is Done.
- M1 epic 6C7-34 is Done.
- Semantic egui tokens, reusable shell components, deterministic CPU-rendered visual goldens and Linux/macOS visual CI are complete; Linear 6C7-42 is Done.
- Validated Bot identity, owner-scoped SQLite lifecycle, authenticated APIs, durable Bot events/snapshots, native eframe roster/editor interactions and six visual states are complete; Linear 6C7-43 is Done.

## Immediate next work

1. Wire provider execution into the direct-chat turn lifecycle and persist normalized streaming messages and activities.
2. Add server approval/retry endpoints and durable approval/activity projections with end-to-end cancellation tests.
3. Complete native timeline visual fixtures and verify attachment, queue, steering, retry, scroll-anchor and reconnect postconditions.

## Verification state

Verified:

- Local and remote Git trees match at `790a285`. GitHub Actions run 32366996632 passed all nine jobs, including native eframe workspace builds and exact visual comparison on Linux, macOS Intel and macOS Apple Silicon.
- Remote commits are attributed to GitHub account `luinbytes`; the latest verified commit exposes the expected noreply identity.
- Working tree was clean before `feat/m0-contracts` was created.
- All committed TOML files parse with Python `tomllib`.
- Protocol JSON schema and golden JSON fixture parse successfully.
- Local documentation link targets referenced by the README exist.
- `git diff --check` passed for the committed baseline.
- `cargo fmt --all -- --check` passes.
- `cargo test -p homebot-storage -p homebot-server` passes with 23 tests. Cancellation and slow-client reconnect tests additionally passed three consecutive focused runs.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- Rust JSON Schema and generated Android binding drift checks pass.
- `homebot-storage` has ten passing tests covering clean install/table inventory, WAL mode, restart/outbox durability, backup/restore, migration rollback, corrupted startup, concurrent access, idempotency, replay retention, and attachment transitions.
- `./scripts/check.sh` passes locally with formatting, workspace clippy, 51 Rust tests, protocol schema drift and generated Android binding drift checks.
- `homebot-providers` has 22 passing tests. Codex coverage includes JSONL fixture normalization, profile isolation and redaction, explicit binary discovery, auth/error and interruption normalization, fake App Server start/resume, streaming, approval resolution, cancellation, and an explicit real-binary skip when Codex is absent. Claude fixtures verify normalized init/content/tool/usage/result records, resume-oriented CLI arguments, streaming and cancellation. OpenAI-compatible tests verify request-time bearer resolution, model discovery, streamed Responses events, HTTPS/loopback policy, redaction and cancellation. Generic process tests verify its JSONL contract, bounded normalized streaming, redaction and cancellation.
- `homebot-tools` has 12 unit and 3 hostile-input integration tests covering actor-scoped deny/allow policy, single-use/expiring/digest-bound approvals, policy revision invalidation, content substitution, traversal, symlinks, atomic and bounded filesystem operations, real PTY output/input lifecycle, duplicate IDs, filtered environment, cancellation, timeout, output limits, loopback-only browser control, profile confinement and normalized CDP actions.
- The full workspace now has 77 passing Rust test cases, including six pixel-exact CPU-rendered desktop goldens and a deterministic transient executable-busy regression. The full 23-test provider suite also passed 100 consecutive stress iterations. Local cargo-deny 0.20.2 reports advisories, bans, licenses and sources all OK with the documented exact unmaintained-font-parser exception.

Not yet verified:

- GitHub Actions run 32346392738 passed all six quality, audit/policy, Linux, macOS Intel, and macOS arm64 jobs.
- Android application build; the generated protocol source is not compiled until the M4 Gradle module lands.
- Any end-user HomeBot behaviour beyond static contract inspection.

## Known failures and incomplete implementation

- `homebot-server` covers every stated 6C7-37 transport behaviour locally and in the passing remote matrix.
- The first-class Codex and Claude adapters cannot perform real authenticated provider messages in this environment because neither CLI binary is installed. Their structured protocol fixtures pass. The OpenAI-compatible adapter is verified against a local protocol-faithful HTTP/SSE fixture, not a user credential. The local computer capability layer is implemented but no real Chrome process is installed in this environment, so CDP behavior is verified against a protocol-faithful loopback fixture. Desktop egui, Android, routines, plugins, VCS, pairing, and packaging are not implemented.
- Direct chat persistence, timeline streaming reconciliation, composer operations and native chat interactions remain unimplemented and are the active 6C7-44 work.
- No release artifact exists. Do not describe the project as installable or v1-ready.

## Environment notes

- Workspace path in the current Work Mode environment: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- For Rust commands export `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and prepend `/tmp/homebot-cargo/bin` to `PATH`.
- Shell Git cannot authenticate to GitHub in this environment. Use the connected GitHub tools for remote writes, or configure normal Git authentication in a future environment.
- The GitHub connector can create trees, commits, and update refs; remote commits created this way are correctly attributed to `luinbytes`.
- Local repository config already sets the correct `luinbytes` noreply identity.
- Preserve the local `feat/m0-foundation` branch until its work is fully represented remotely; do not force-push or delete it casually.
- Do not expose the current server beyond loopback.
