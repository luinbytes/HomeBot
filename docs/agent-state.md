# HomeBot autonomous agent state

Updated: 22 August 2026, Europe/London

This is operational handoff state for coding agents, not user-facing product documentation.

## Current state

- Current milestone: M6 packaging, hardening and the v1 parity gate.
- Current issues: 6C7-69, 6C7-85, 6C7-86 and 6C7-87 are In Progress for the final coherence/security corrections. 6C7-66 and 6C7-75 remain In Progress on external acceptance. 6C7-71 is Todo and must not start until its blockers pass.
- Public branch: `main` at `1a14dfd9eb2e4689c1fc752fb144cb4758bdd314` before the pending audit PRs merge.
- Active publication branches:
  - PR #36, browser isolation/takeover: head `bb63dba88c1f5570b15798d8cba3bfb65891fd30`, tree `a5a36961a4e4ef7e7483b2fa9c991e1e69fa7f39`, CI `32582654212`.
  - PR #40, packaging-gate executable mode: head `7f552d1240a91873a61f73b808ca16cd8ace20d6`, tree `f229bf21a3080b9a48e2f115c33c46ca2dd1526d`, CI `32583350119`.
  - PR #39, pairing provenance/throttling plus the mode correction: head `fa953f311fcfad2f4675204401f2e2e67a672b40`, tree `9b33de06043ee63397bfdc7e7ba1b828f7c0b6ae`, CI `32583521104`; it is stacked on PR #36.
- Required commit identity: `luinbytes <42706009+luinbytes@users.noreply.github.com>`.

Architecture invariants remain unchanged:

- The Rust server is authoritative. Desktop and Android use the same authenticated, versioned HTTP/WebSocket contract and server-owned policy decisions.
- SQLite owns durable application state and sequenced replay. Secret values remain in OS-backed credential storage with no plaintext SQLite fallback.
- Production provider profiles are composed only through the real production registry; fixtures cannot become production defaults.
- Privileged local executable and Git mutations are owner-only or require exact server-side, device-scoped capability policy.
- Browser profiles use separate server-owned CDP contexts and target-membership checks. This isolates cookies while the external browser is alive; HomeBot does not claim disk-persistent browser credentials after that process restarts.
- Updates require a canonical Ed25519 signature verified against a compile-time pinned public key. CI-only/ad-hoc artifacts cannot become update candidates.

## Completed audit corrections

- 6C7-77: production provider configuration/registry/runtime is composed into `AppState`; production Bot-turn resolution and safe configuration failure are tested.
- 6C7-83: paired devices cannot configure host plugin executables; privileged Git policy uses the real authenticated device; routine plugin authority is server-derived.
- 6C7-84: Codex, Claude and generic provider stdout frames are bounded before allocation; retained stderr is bounded and redacted.
- 6C7-86 implementation: updater manifests use pinned-key Ed25519 verification and reject CI-only signing classifications. The issue is temporarily reopened only for PR #40's tracked-mode regression.
- Repository-wide security review produced five tracked findings (6C7-83 through 6C7-87); no finding was hidden in a TODO or handoff note.

All earlier M0-M5 product work remains complete unless new concrete evidence proves otherwise. Production desktop visual/routine corrections 6C7-80/81/41/45/52 remain backed by their audited implementation and prior 16-job CI.

## Verification state

- Provider-frame correction: CI `32579386951`, all 16 jobs passed; merged as `a1020fbde423ea092d67b6cf19357a13cc29af74`.
- Privileged-authority correction: exact implementation tree passed all 16 jobs in CI `32579470436`; merged as `8ef092d4943453d9118ab99ec918f59fc374e5f7`.
- Signed-updater correction: exact tree `5d50935d853820363a1bd2884873117738919287` passed all 16 jobs in CI `32579858638`; merged as `1a14dfd9eb2e4689c1fc752fb144cb4758bdd314`.
- Local exact final pairing/browser tree:
  - formatting and strict storage/tools/server clippy passed;
  - `cargo test --workspace --all-features` passed, including 46 server, 36 storage, 32 desktop, provider/tool/VCS suites and production visual goldens;
  - focused browser takeover, pairing, browser CDP, migration and Rust-to-schema/generated-Kotlin checks passed;
  - packaging, security and performance/accessibility gates passed after restoring `check-packaging.sh` mode `100755`.
- The first pairing CI run failed for real v14 migration-fixture drift. The fixture now applies schema 24 before calling the current pairing API; the exact failed test, all storage tests and the replacement remote Rust-quality job pass.
- One later local all-in-one gate run saw the demonstration-to-Skill server test receive an error envelope under full concurrent execution. It passed in the earlier complete workspace run, 20 immediate focused repetitions, and the exact replacement remote Rust-quality job. No test or CI check was weakened.
- At this update, all Linux/Rust/Android/dependency/Arch jobs on PRs #36/#40/#39 are green. Their eight macOS jobs per run remain queued, not passed; do not merge or close the issues until all 16 jobs complete successfully.

## Known blockers

- CI infrastructure: GitHub's macOS jobs for the exact pending trees are queued. A queued job is not release evidence.
- 6C7-66: real Developer ID signing, notarisation, stapling/Gatekeeper verification, and clean Intel/Apple Silicon first-run/provider validation have not occurred.
- 6C7-75: genuine authenticated Codex and Claude round trips plus physical Intel Mac, Apple Silicon Mac, Arch/Omarchy, Android, VoiceOver, TalkBack, keyboard-only, install/upgrade, pairing and reconnect acceptance are unavailable here.
- Android's production signing identity and physical device are unavailable. CI's `ci-ephemeral` APK must never be published as v1.
- 6C7-71 and v1.0.0 remain blocked. There is no truthful public v1 release or tag.

## Exact next actions

1. Wait for all 16 jobs in CI `32582654212`, `32583350119` and `32583521104`; inspect any failure rather than rerunning blindly.
2. Merge PR #40 and close 6C7-86 only after its exact tree is green.
3. Merge PR #36 and close 6C7-85 only after its exact tree is green.
4. Retarget/rebase PR #39 onto the resulting `main` without changing its verified final tree, merge after exact-tree CI, then close 6C7-87.
5. Run or observe CI on final `main`, update this file to the real public commit/run, add exact evidence to Linear, and close 6C7-69 only if every corrective child is Done and no high/critical finding remains.
6. Execute the external operator matrix in `docs/release-acceptance.md` for 6C7-66/75. Only then execute 6C7-71 and publish/reverify v1.0.0.

## Environment notes

- Workspace: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Local Rust: `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, prepend `/tmp/homebot-cargo/bin` to `PATH`.
- Large test links use `CARGO_TARGET_DIR=/tmp/homebot-target`, `CARGO_BUILD_JOBS=1`, and `RUSTFLAGS='-C linker=cc -C link-arg=-fuse-ld=bfd -C codegen-units=1'` in this container.
- Local Android Gradle cannot download its distribution from this sandbox; remote Android CI is the build/lint/test authority.
- Preserve unrelated work. Never reset, clean or remove user repositories destructively. Keep server binding on loopback in this environment.
