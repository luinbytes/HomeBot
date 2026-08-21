# HomeBot autonomous agent state

Updated: 22 August 2026, Europe/London

This file is operational state for coding agents. It is not user-facing product documentation.

## Current state

- Current milestone: M6, packaging, hardening and the v1 parity gate. M0 through M5 are verified complete.
- Current Linear issues: 6C7-66, real Developer ID signing/notarisation and clean Intel/Apple Silicon validation (`In Progress`); 6C7-75, physical-platform, assistive-technology and live-provider release acceptance (`In Progress`, externally blocked). Final gate 6C7-71 remains Todo and blocked by them.
- Current Git branch: public `main`; local continuity branch `audit/6c7-75-release-readiness` contains the same verified implementation tree plus this handoff update.
- Latest public and verified implementation commit: `ca8edf477380265109015241b46aa4a5b26457c4` (exact tree `daf86edd51011b5b87c310108f573bbecb16fdb2`). It includes production provider composition (6C7-77), the fail-closed macOS notarisation pipeline, the Android release-artifact pipeline (6C7-78), and consistent v1 candidate identity across every client/package (6C7-79).
- Latest verified GitHub Actions run: `32536602259`, all sixteen jobs passed, including Rust/release-version quality, Android lint/tests/debug and minified release builds/signature packaging, dependency gates, Linux and Arch 1.0.0 packaging, both macOS architectures' 1.0.0 builds/goldens/packages, and all resource probes.
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
- The production server composition root owns the real provider registry. It creates strict, bounded provider profiles from production configuration, injects the resulting `ProviderRuntime` into `AppState`, exposes only secret-free projections, and never selects fixture adapters as production defaults.
- Local MCP plugins use a provider-neutral adapter boundary and constrained direct-executable stdio lifecycle. Plugin results are explicitly untrusted data; connection and Bot assignment never grant tool capability authority.
- Filesystem authority uses `cap-std`; terminal authority uses bounded `portable-pty`; browser authority uses loopback-only CDP.
- Repository registrations and per-chat primary/isolated workspace associations are SQLite authority. `homebot-vcs` invokes a fixed Git executable without a shell, never mutates the primary checkout, and removes only clean canonical children of the server-managed worktree root.
- Coding turns use alternate-index hidden-ref checkpoints. Restore captures a safety checkpoint, preserves the real branch/index, refuses ignored-content overwrite conflicts, and explicitly forks incompatible provider conversation mappings.
- Source-control reads and mutations are server-owned and normalized. Commit/branch/push use a fixed shell-free Git executable with repository hooks suppressed; remote push and PR creation require digest-bound server approval, and exact idempotent results persist independently from remote side effects.
- Queued prompts are durable server state with typed `steering` and `follow_up` semantics. Steering retains FIFO priority ahead of ordinary follow-ups, stop/failure preserves remaining order, and SQLite write reservations prevent provider-event races during insert/promotion.
- Provider interaction mode and working-context generation/status/usage are server-owned. Capability-gated native compaction preserves provider mapping; reset removes only that mapping. Neither operation deletes HomeBot identity, transcript, attachments, Skills, checkpoints or app memory.
- Pairing offers are five-minute, single-use, endpoint-bound credentials stored only as digests. Named device sessions use the same authenticated versioned protocol as desktop, are owner-listable/revocable, and cannot administer devices. Remote binding is explicit and loopback remains the safe default.
- Shared browser profiles and sessions are server-owned, owner-scoped state. Generated profile directory references never expose native paths or credentials; group handoff preserves access. Navigation and human takeover remain digest-bound capability operations, while watch/return and live activity use the authenticated sequenced contract.
- Android release packaging accepts only a cryptographically verified, version/package-matched signed APK and emits a manifest, certificate-digest evidence, and SHA-256 checksums. CI uses an explicitly `ci-ephemeral` identity that cannot be represented as the public signing identity.
- Root `VERSION` is the single candidate identity. Cargo metadata, server/desktop negotiation, Android BuildConfig/client identity, and macOS/Arch/Android artifact names and manifests must agree; the release gate rejects divergence.

Current blockers:

- All currently known non-external v1 work is complete. Final release blockers are external to this environment: neither Codex nor Claude is installed/authenticated for genuine provider smoke tests; Apple Developer ID/notarisation credentials and clean Intel/Apple Silicon machines are unavailable; and physical Arch/Omarchy and Android devices with VoiceOver/TalkBack-equivalent acceptance facilities are unavailable.
- No active M2 blocker. Corrective issue 6C7-73 and epic 6C7-41 are verified Done.
- `egui` 0.32.3 transitively uses unmaintained `ttf-parser` 0.25.1. RUSTSEC-2026-0192 reports no known vulnerability or safe upgrade; the exact-revision 6C7-69 review accepted the warning with a required pre-v1 dependency recheck.

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
- 6C7-55, owner-scoped repository registration, primary/isolated per-chat workspaces, deterministic branches, guarded cleanup, authenticated protocol/events, desktop transport/projection and Android/schema parity.
- 6C7-56, hidden-ref before/after turn checkpoints, exact binary-capable per-turn/full-chat diffs, safe restore, provider-conversation fork reconciliation, authenticated desktop/server contracts and Android/schema parity.
- 6C7-57, normalized status/staged and unstaged diff/commit/branch/push/PR workflows, server-side capability approvals, durable exact replay, hostile-repository hook denial, authenticated desktop projection and Android/schema parity.
- 6C7-58, durable typed queued steering/follow-ups, deterministic restart-safe promotion, provider-neutral plan/default modes, capability-gated compaction/reset, persistent working-context boundaries, desktop projection and Android/schema parity.
- M4 epic 6C7-54: all coding workspace, checkpoint/diff/restore, source-control and context workflow children verified complete.
- 6C7-60, short-lived owner pairing, restart-durable named/revocable device sessions, server-enforced device administration, LAN/Tailscale/custom-HTTPS classification, explicit remote bind controls and desktop projection.
- 6C7-61, real Gradle/Compose Android application, authenticated HTTP/WebSocket client, Keystore session storage, DataStore preferences, snapshot/replay recovery, deterministic fake-server tests and green Android CI/APK artifact.
- 6C7-62, native server-backed Bot roster/lifecycle, direct and group chat timelines, streamed refresh, attachments, approvals, activity, queue/steering/stop/retry, unread state, mentions/handoff and coding checkpoint/VCS surfaces.
- 6C7-63, Android routine/run/trigger controls, Skills assignment, plugin/MCP controls, provider/endpoint settings, opaque secret references and paired-device self management with owner administration still denied.
- 6C7-64, event-driven Android Bot/approval/routine/error notifications, exact deep links and connectivity-driven reconnect without permanent polling.
- M5 epic 6C7-59: native Android and secure remote parity verified complete.
- 6C7-67, AUR-ready Arch/Omarchy desktop and headless packaging with a clean install/update/uninstall CI lifecycle.
- 6C7-68, explicit manifest-driven desktop update staging, verified pre-migration backups, too-new-schema refusal and deterministic recovery coverage.
- 6C7-69, repository-wide security hardening, parser/resource bounds, hostile repository/MCP/provider fixtures, all-capability negative approvals and a clean exact-revision security scan.
- 6C7-70, measurable startup/reconnect/chat/stream/concurrency/CPU/RSS budgets, bounded local telemetry, desktop keyboard/text scaling, Android screen-reader semantics and cross-platform CI resource gates.
- 6C7-74, complete Bot lifecycle, reply/thread and reaction reconciliation, exact-target global search, demonstration-to-Skill conversion, and immutable versioned Bot/group/routine/plugin/Skill references across server, desktop and Android.
- 6C7-76, durable bounded group rename/membership, owner-managed deny-first capability rules/audit, and server-owned shared browser watch/takeover/return with approval, artifact, handoff, restart, desktop and Android parity.
- 6C7-77, production provider configuration/registry composition, real `ProviderRuntime` injection into `AppState`, safe profile projection, configured Bot-turn resolution, clean configuration failures and fixture-provider exclusion.
- 6C7-78, deterministic Android v1 version injection, minified release assembly, fail-closed APK signature/package/version verification, manifest/certificate evidence/checksums and explicitly non-release CI signing.
- 6C7-79, single `1.0.0` candidate identity across Cargo/server/desktop, Android app/protocol client, macOS, Arch and Android packaging, with a fail-closed consistency gate.
- Most recent completed issue: 6C7-79.
- Focused repository presentation pass: README badges and real desktop previews added from checked-in visual goldens; tracked-file hygiene audited with no junk removals required and `.gitignore` expanded for common Rust, Android, editor, environment, Python, Node, log and temporary outputs.

## Immediate next work

1. On a macOS release host with Developer ID and stored notary credentials, run the exact commands in `docs/release-acceptance.md` to build, Developer-ID sign, notarise, staple and verify both x86_64 and arm64 candidates; record the immutable artifact hashes and close 6C7-66 only after clean Intel and Apple Silicon first-run/provider-discovery checks pass.
2. On authenticated provider hosts, execute the documented Codex CLI and Claude Code smoke matrix for auth discovery, streamed Bot turns, activities/tools, approvals, cancel, restart/resume, plan mode and compaction where supported. Record versions and secret-free evidence in 6C7-75.
3. Install the exact candidate artifacts on clean Intel Mac, Apple Silicon Mac, Arch/Omarchy and Android; complete keyboard/VoiceOver/TalkBack, install/upgrade, pairing/reconnect and parity rows from `docs/release-acceptance.md`.
4. Only after 6C7-66 and 6C7-75 are genuinely Done, execute 6C7-71, create the immutable v1.0.0 tag/release, download every public artifact, reverify manifests/checksums/signatures, and close M6.

## Verification state

Verified at the latest remote baseline:

- GitHub Actions run `32536602259` passed all sixteen jobs for public 6C7-79 tree `daf86edd51011b5b87c310108f573bbecb16fdb2`, merged as `ca8edf47`. It verifies all HomeBot Rust packages/server/desktop as 1.0.0, Android BuildConfig and protocol identity from root `VERSION`, 1.0.0 minified/signed-pipeline APK, 1.0.0 Intel/Apple Silicon bundles, and clean Arch 1.0.0 install/update/uninstall with matching manifests/checksums.
- GitHub Actions run `32534100793` passed all sixteen jobs for public 6C7-78 tree `97d83a132c116fc0b205bf61bbddd31064642b8e`, merged as `84084ff8`. Android lint/tests, debug and minified release assembly, ephemeral CI signing, `apksigner` verification, package/version validation, manifest/checksum generation and artifact upload all passed. The CI-only release-pipeline artifact from run `32533932893` has workflow-archive digest `sha256:03266a922fc915aa5c69b05ca7f235323d9a9cfb679457f1c45f8d6ba5ab787e` and is explicitly not a release candidate.
- GitHub Actions run `32532554784` passed all sixteen jobs for macOS notarisation-pipeline tree `53f29dae2c1fdcf3d6b152efe24d90cdcad5e8d1`, merged as `a3f44842`. Automated evidence covers deterministic bundles, architecture validation, notarisation ZIPs, fail-closed Developer ID/notary commands, manifests and checksums; real Apple signing/notarisation and physical Macs remain open in 6C7-66/75.
- GitHub Actions run `32531703016` passed all sixteen jobs for production-provider composition tree `8bfbabf9a0cf1b2da31741f34ba13fb6b01327cf`, merged as `6fece576`. Tests exercise the production composition root, safe defaults/configuration errors, real runtime injection and a structured CLI Bot turn without fixture defaults.
- GitHub Actions run `32480288326` passed all sixteen jobs for public code tree `389b8c140fb5341c53c5ec4d266c9872c04f18a1`. It confirms the provider queue/context lifecycle fixtures no longer starve each other under the full concurrent server suite; PR #19 also synchronizes this handoff to 6C7-75.
- GitHub Actions run `32478602118` passed all sixteen jobs for exact 6C7-76 tree `ad451acede6713456801f35532cf0a22df41b3b1`, merged as public `main` commit `7391c362`. It verifies 39 server tests, 36 storage tests, Android lint/tests/APK, Rust/schema/generated-Kotlin/security gates, Linux and both macOS builds/goldens/packages/resources, and the fresh Arch install/update/uninstall lifecycle. PRs #15 through #17 cover the group, policy and shared-computer slices.
- GitHub Actions run `32471204374` passed all sixteen jobs for exact tree `e190fdebde97e97919fb01c4f8693b3aa7a4ff7c`, merged as public `main` commit `92c78fc`. It verifies 6C7-74's typed-reference migration and server/client contract alongside Android, Rust, packaging, visual, dependency and resource gates. PRs #11 through #14 collectively close every 6C7-74 acceptance row.
- GitHub Actions run `32461879059` passed all sixteen jobs for public 6C7-70 commit `bf1a613`. Android lint/tests/APK, the five-cycle rejected-event cleanup stress test, strict Rust/workspace gates, fresh Arch package lifecycle and Linux/macOS Intel/macOS Apple Silicon process-resource budgets all passed.
- GitHub Actions run `32458792462` passed all thirteen jobs for public 6C7-69 commit `4c65711`. The exact-revision canonical security scan reports complete repository coverage, zero surviving findings and no unresolved high/critical issue.
- GitHub Actions run `32456932817` passed all thirteen jobs for public 6C7-68 commit `e52bdbc`. It verified updater/migration recovery, Android, dependency policy/audit, Linux, both macOS builds and packages/goldens, and the Arch clean package lifecycle.
- GitHub Actions run `32455982765` passed all thirteen jobs for the final 6C7-67 tree, including source-archive creation, package build, clean install, update, uninstall, manifest/checksum generation and artifact upload in a fresh Arch container.
- GitHub Actions run `32451067995` passed all twelve jobs for the corrected macOS packaging branch. It uploaded `HomeBot-macOS-Intel` and `HomeBot-macOS-Apple-Silicon` artifacts with reproducible bundles, notarisation ZIPs, manifests and checksums; public `main` is `8851233`.
- GitHub Actions run `32449482551` passed all ten jobs for public 6C7-64 commit `5adeeab9`. Android tests prove authoritative event-to-notification mapping, exact deep links, duplicate-safe sequence handling and connectivity-triggered reconnect without permanent polling.
- GitHub Actions run `32448835788` passed all ten jobs for public 6C7-63 commit `f834e37`. Android lint, seven deterministic tests, debug APK assembly/artifact upload, Rust quality, dependency audit/policy, Linux and both macOS builds, and all visual-golden platforms passed.
- Paired-device self inspection/revocation is server enforced while paired credentials remain forbidden from owner-wide session administration. The attachment claim path reserves the SQLite writer before its idempotency snapshot; the exact integration test passes five consecutive runs.

- GitHub Actions run `32447728686` passed all ten jobs for public 6C7-62 commit `ca6d4fa`. Android lint, six deterministic transport tests, debug APK assembly/artifact upload, Rust quality, dependency audit/policy, Linux and both macOS builds, and all three visual-golden platforms passed.
- Android product fixtures prove authenticated Bot mutations, typed queued steering, bounded attachment create/upload/finalize, and sequenced snapshot/replay behavior. Compose product screens call server APIs for durable state and expose direct/group chat, approval, activity, handoff and coding checkpoint/VCS controls.

- GitHub Actions run `32442039471` passed all nine jobs for public commit `a684abb`: Rust quality, dependency audit/policy, Linux, macOS Intel, macOS Apple Silicon and all three visual-golden platforms. Remote tree `1d1eb73101e04c286b9447c92faa70d43e1becd0` exactly matches the locally verified 6C7-60 tree; author and committer resolve to `luinbytes`.
- 6C7-60 fixtures prove short-lived single-use digest-only pairing, endpoint/origin validation, durable rate limits, restart authentication, named list/revoke, administration denial, post-revocation HTTP/WebSocket denial, desktop server-authoritative projection and explicit remote-listener controls. Rust schema and generated Kotlin remain mechanically identical.
- GitHub Actions run `32435023890` passed all nine jobs for commit `0856e56`: formatting/strict clippy/workspace tests, dependency audit/policy, Linux, macOS Intel, macOS Apple Silicon, and all three visual-golden platforms.
- Remote tree `d1feb6a9e0c3fe522b63b1953a69bc1020b79ea9` exactly matches the locally verified 6C7-58 implementation tree. GitHub resolves both author and committer to account `luinbytes`.

- GitHub Actions run `32429140362` passed all nine jobs for commit `3224de0`: formatting, strict clippy, 152 Rust tests, dependency policy/audit, Linux, macOS Intel, macOS Apple Silicon, and 15 cross-platform visual fixtures.
- Remote tree `b8e637cc89d69e19c060a30402628ed65b390022` exactly matches the locally verified implementation tree. The remote author and committer resolve to GitHub account `luinbytes`.
- 6C7-50's authenticated integration fixture proves Connect -> Waiting -> Connected discovery, then malformed MCP health -> Error with disablement and cleared tools; the child environment is cleared and MCP results remain explicitly untrusted.

Verified locally at the current baseline:

- 6C7-70 targeted budgets pass: real cold start/authenticated probes, replay and stale-cursor reconnect, 10,000-message chat hydration, eight-Bot/2,000-event streaming projection, bounded local-only telemetry and 80%-200% text scaling. Desktop/Android accessibility semantics and cross-platform process-resource CI are implemented; the Appearance visual golden was intentionally updated and reverified.
- 6C7-69's Rust security gate passes: bounded protocol/WebSocket/MCP inputs, hostile Git configuration denial, provider URL credential rejection, all capability-class approval negatives, deterministic parser properties, secret-leak scanning, strict clippy, schema/Android drift and visual goldens. The authenticated invalid-token fixture passes five consecutive runs after keeping its temporary SQLite directory alive for the Router lifetime.
- 6C7-68's complete `./scripts/check.sh` gate passes with 169 Rust tests, strict clippy, all visual goldens, schema/Android drift checks and deterministic packaging-contract checks. A follow-up symlink-negative test also passes, bringing the targeted current total to 170. Storage coverage upgrades and verifies backups for every prior schema v1–v16, reuses an interrupted-launch backup, refuses v18, fails closed on corruption/backup failure/symlink substitution and preserves transactional rollback. Desktop coverage proves compatible same-origin manifests, explicit staging, exact streamed size/SHA-256, traversal denial and partial-file cleanup.
- 6C7-61 has a real Gradle application module, Compose shell, Coroutines/Flow state machine, OkHttp HTTP/WebSocket transport, Keystore-encrypted device session, DataStore endpoint preferences, generated Rust-owned snapshot/version/attachment models and Android CI artifact job.
- Deterministic Android tests cover one-time pairing, credential redaction, version skew, revocation, snapshot hydration, cursor resume, stale-cursor fallback, replay and duplicate-sequence suppression. Run `32446363611` compiled against Android SDK 36 and Kotlin 2.3, passed lint and all tests, assembled `app-debug.apk`, and uploaded `HomeBot-Android-debug`.

- `./scripts/check.sh` passes on Rust 1.98.0: formatting, strict clippy, the complete workspace suite with 152 tests, protocol/schema drift checks, generated Android binding drift checks, and all 15 exact desktop visual fixtures. This environment requires `CARGO_BUILD_JOBS=1` for the test build after a transient parallel egui archive-mapping failure; the serial rerun passed fully.
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
- 6C7-55 fixtures prove dirty/untracked primary preservation, primary and isolated associations, deterministic branches, detached HEAD, hostile-ref denial, clean removal, dirty/out-of-root cleanup denial, duplicate conflicts, unavailable-path projection, restart durability and v12→v13 migration. A real desktop-supervised server fixture registers, attaches and detaches a repository through authenticated APIs.
- GitHub Actions run `32422748219` passed all nine jobs for exact remote tree `61f0efa`: Rust quality, dependency audit/policy, Linux and both macOS builds, and all three visual-golden platforms. Both remote commits resolve to GitHub account `luinbytes` as author and committer.
- 6C7-56's full gate passes with 142 Rust tests and all 15 visual fixtures. Real-Git/server coverage proves dirty/staged/untracked/binary/rename capture, index preservation, exact diffs, ignored-content conflict denial, safety restore, idempotent audit persistence, and provider-conversation fork reconciliation.
- GitHub Actions run `32425371202` passed all nine jobs for public commit `20412a0` and exact remote tree `229c7d8`: Rust quality, dependency audit/policy, Linux and both macOS builds, and all three visual-golden platforms. Author and committer resolve to GitHub account `luinbytes`.
- 6C7-57's real Git/server fixtures prove normalized staged/unstaged/untracked/conflicted/detached status, bounded exact diffs, commit/clean branch/local bare push, no-remote and redacted auth failure, duplicate/deny/approve approval semantics, exact durable replay, PR metadata/create, migration restart safety and hostile Git-hook denial.
- GitHub Actions run `32429140362` passed all nine jobs for public commit `3224de0` and exact remote tree `b8e637c`; both author and committer resolve to GitHub account `luinbytes`.
- 6C7-58's complete local gate passes: strict all-target clippy, every workspace test suite, all 15 visual fixtures, schema drift, generated Android binding drift and cargo-deny advisories/bans/licenses/sources.
- Its server fixtures prove typed steering priority/FIFO follow-ups, duplicate replay, cancel-order stability, restart durability, three automatic queued turns, default/plan/default capability routing, unsupported-mode denial, compaction/reset concurrency exclusion, transcript preservation, fresh provider-context isolation and restart recovery. The 32-test server suite passed three consecutive 16-thread stress runs after the SQLite write-reservation fix.

6C7-73 and reopened M2 epic 6C7-41 completion evidence is recorded in Linear; both are Done. M3 epic 6C7-48, M4 issues 6C7-55 through 6C7-58, M4 epic 6C7-54, M5 children 6C7-60 through 6C7-64, and M5 epic 6C7-59 are verified Done. M6 epic 6C7-65 and macOS packaging issue 6C7-66 are In Progress.

## Known failures and incomplete implementation

- 6C7-66 remains In Progress because real Developer ID signing/notarisation/stapling and clean Intel/Apple Silicon first-run/provider discovery have not occurred. CI's ad-hoc artifacts and simulated notary responses do not satisfy it.
- 6C7-75 remains In Progress because real authenticated Codex/Claude round trips and physical Intel Mac, Apple Silicon Mac, Arch/Omarchy and Android install/upgrade/accessibility checks are unavailable in this environment.
- The Android production signing keystore and physical device are unavailable. CI's `ci-ephemeral` APK proves the pipeline only and must never be published as v1.
- 6C7-71 and the public v1.0.0 tag/release remain blocked by 6C7-66 and 6C7-75. No signed/notarised public v1 release exists; do not describe HomeBot as v1-ready.

## Environment notes

- Workspace: `/workspace/scratch/e0bbfdbe8a8b/HomeBot`.
- Rust commands require `RUSTUP_HOME=/tmp/homebot-rustup`, `CARGO_HOME=/tmp/homebot-cargo`, and `/tmp/homebot-cargo/bin` first in `PATH`.
- Rust 1.98's bundled `rust-lld` fails to link the large server test binary in this container. Local verification uses `CARGO_TARGET_DIR=/tmp/homebot-target` and `RUSTFLAGS='-C linker=cc -C link-arg=-fuse-ld=bfd -C codegen-units=1'`; GitHub CI uses the normal stable toolchain and is green.
- Local cargo-deny binary: `/tmp/cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny`.
- Shell Git read access works. If authenticated push is unavailable, publish with the connected GitHub blob/tree/commit/ref tools and then fast-forward local `main` to `origin/main`.
- Do not expose the current server beyond loopback. Preserve unrelated work and never reset/clean user repositories destructively.
