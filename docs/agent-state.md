# HomeBot autonomous agent state

Updated: 20 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M0, Product & Architecture Baseline
- Current Linear issues: 6C7-31 parity inventory and visual states; 6C7-32 protocol contract; 6C7-33 threat model pending final Linear closure
- Parallel scaffold issue: 6C7-35 remains In Progress but cannot close until compilation and CI are verified
- Current Git branch: `feat/m0-contracts`
- Latest verified remote commit: `00cdca9192d2b35e4931ec0455968e227d6f894a`
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

- This execution host has no `rustc` or `cargo`, so Rust formatting, clippy, compilation, and tests cannot run locally.
- GitHub currently exposes no Actions run or combined status for remote head `00cdca9`; CI success is unverified.
- 6C7-31 still needs a complete visible-state reference index, especially empty, loading, disconnected, approval, and error states.
- Android protocol generation or mechanical schema validation has not been implemented, so 6C7-32 is not complete.

## Completed work

- Public `luinbytes/HomeBot` repository created and populated.
- Initial Rust monorepo crate boundaries and baseline GitHub Actions committed.
- Architecture, protocol, provider, Android, routines, plugins, development, release, installation, and security documents created.
- Initial Grok Bot feature parity matrix created from authoritative SpaceXAI documentation.
- M0 security/capability threat model implemented in `docs/security.md`; Linear 6C7-33 is ready for Done after evidence is posted.

## Immediate next work

1. Complete and verify Linear 6C7-33, then mark Done.
2. Complete 6C7-31 by adding a canonical visual-state/reference index covering every visible surface and state.
3. Complete 6C7-32 with full protocol schemas, golden fixtures, malformed/skew fixtures, and Android mechanical validation scaffolding.
4. Close epic 6C7-30 only after all three children are genuinely Done.
5. Finish 6C7-35 by running rustfmt, clippy, tests, dependency policy, and all target compile jobs through a working Rust environment/CI.
6. Refresh Linear and start 6C7-36, SQLite persistence, migrations, and event outbox.

## Verification state

Verified:

- Local and remote Git trees match at `00cdca9`.
- All three remote commits are attributed to GitHub account `luinbytes`.
- Working tree was clean before `feat/m0-contracts` was created.
- All committed TOML files parse with Python `tomllib`.
- Protocol JSON schema and golden JSON fixture parse successfully.
- Local documentation link targets referenced by the README exist.
- `git diff --check` passed for the committed baseline.

Not yet verified:

- Rust formatting, clippy, compilation, unit tests, integration tests, or server health endpoint.
- GitHub Actions jobs.
- Android build or schema compatibility.
- Any end-user HomeBot behaviour beyond static contract inspection.

## Known failures and incomplete implementation

- `homebot-server` is a development-only unauthenticated loopback health stub.
- Storage, authentication, WebSocket replay, providers, desktop egui, Android, routines, plugins, tools, VCS, pairing, and packaging are not implemented.
- The committed protocol schema is an initial envelope subset, not the complete v1 schema required by 6C7-32.
- No release artifact exists. Do not describe the project as installable or v1-ready.

## Environment notes

- Workspace path in the current Work Mode environment: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Shell Git cannot authenticate to GitHub in this environment. Use the connected GitHub tools for remote writes, or configure normal Git authentication in a future environment.
- The GitHub connector can create trees, commits, and update refs; remote commits created this way are correctly attributed to `luinbytes`.
- Local repository config already sets the correct `luinbytes` noreply identity.
- Preserve the local `feat/m0-foundation` branch until its work is fully represented remotely; do not force-push or delete it casually.
- Do not expose the current server beyond loopback.
