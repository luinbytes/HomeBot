# Product implementation audit

Audited 25 August 2026 from `main` at
`beaedd1576a02c3ec80c9c4afd395a08776d5845`, before making changes. This audit
uses source tracing and live runs; README, milestone, fixture, and snapshot
claims are not treated as acceptance evidence.

## Bottom line

HomeBot is a durable chat server with real provider adapters and broad
administration surfaces. It is not yet a usable private AI team. Direct Codex
chat works locally; one real Codex group turn and the concurrent/handoff paths
through deterministic integration tests now execute independent Bots. Codex
can now initiate a server-owned, persisted group handoff through a scoped
dynamic tool. Other assistant tools and providers are not connected to that
tool loop, Android has no physical-device acceptance,
and background restart recovery is truthful failure rather than continuation.

## Capability classification

| Area | Classification | Implementation evidence and limit |
| --- | --- | --- |
| Rust server, auth, HTTP/WebSocket protocol | Actually usable | The server starts, bearer auth is enforced, event frames stream, and cursor-backed SQLite state replays. The live probe used HTTP plus persisted timelines; a real remote phone was not used. |
| SQLite messages and direct chats | Actually usable | User messages are durable before dispatch; streamed Bot text, activities, approvals, usage, terminal state, and provider conversation IDs persist. Completed/cancelled state survived a server restart. |
| Bot identity | Actually usable after this change | Name, role, and responsibility now enter every direct and group provider turn. Previously these fields were UI/storage metadata only. Avatar shape/color persist; there is no image avatar. |
| Bot files/workspace | Partial | Each Bot now gets a neutral persistent directory and an attached repository/worktree overrides it. Previously Codex/Claude inherited a profile or server process directory, so attaching a repository did not control execution. General Bot files have no polished file-management flow. |
| Codex CLI with ChatGPT auth | Actually usable locally; broader acceptance unproven | Installed `codex-cli 0.149.1` completed authenticated turns through HomeBot. Live evidence covered streaming, a resumed follow-up, a terminal command approval in the attached repository, cancellation, two concurrent Bots, and restart persistence. Packaging, login onboarding, write approval, and Linux remain unproven. |
| Claude Code | Implemented but unproven | Structured streaming, local account health, cwd, resume, cancellation, and operation-scoped HomeBot MCP tool continuation exist. The loopback bridge is fixture-proven, but real account execution was not run because `claude` is not installed. Headless native Claude approvals remain explicitly unsupported; HomeBot tool policy still applies server-side. |
| OpenAI-compatible API | Implemented but unproven | HTTPS/loopback validation, opaque secret lookup, models, and narrow Responses/Chat Completions SSE paths exist. Both styles have fixture-proven native HomeBot tool continuation. Cross-turn Chat Completions replay, approvals, and provider-side cancellation remain absent. Live endpoint proof is still required. |
| Streaming and cancellation | Actually usable with local Codex | Live content/activity state persisted; a long Codex turn became `cancelled` 355 ms after HomeBot stop. No Android rendering or network-loss acceptance was performed. |
| Concurrent direct Bots | Actually usable with local Codex | Scout and Codey started 1 ms apart and completed independent markers in overlapping 3.3 s/4.8 s windows. This does not prove multiple accounts or heavy tool workloads. |
| Client disconnect/reconnect | Partial | Server-owned turns continued after short-lived HTTP clients exited, and later timeline reads recovered the result. WebSocket loss, Wi-Fi/Tailscale roaming, cursor expiry, and Android process death remain unproven. |
| Server restart during a turn | Partial after this change | HomeBot cannot reattach to an in-flight provider. Startup now marks orphaned direct/group streaming messages and activities failed, expires approvals, clears false direct running state, and moves affected group participants from running to failed with a retryable error. Before the fix the UI remained stuck on `streaming` forever. |
| Status / “What are you working on?” | Partial | Chat running/queue state and normalized activities exist, but there is no first-class human summary across Bots. Provider readiness shown on Bot cards only proves a configured profile record, not executable/auth health. |
| Approvals and structured input | Partial | Codex command approval was live-proven and durable. Shared capability approvals now have a ten-minute TTL, four-pending per-operation bound, request digest, policy-revision fencing, and one-time decisions. Provider-neutral confirm, pick-one, and vault-only secret cards resume only their owning turn; duplicate/stale secure submissions and transcript/SQLite secret absence are fixture-proven. Native desktop controls have rendered accessibility evidence and Android transport/Compose compile, but physical Android/offline handling, live file-write approval, and packaged-provider acceptance remain open. |
| Repository, VCS, checkpoints | Partial | Repository registration, isolated worktrees, status/diff/commit/branch/push/PR/checkpoint APIs exist and are fixture-tested. Provider cwd was disconnected until this change. A real Codex read in the attached repository now works; a real write/diff/restore/push/PR chain is still unproven. |
| Skills | Partial | Versioned skills can be assigned and are assembled into direct provider prompts. There is no live proof that a real provider uses a skill correctly, and skills do not define collaboration permissions or budgets. |
| MCP/plugins | Implemented; live providers incomplete | Registry, remote/local connection, tool discovery, assignment, opaque secret headers, policy enforcement, and untrusted-result boundaries are integrated into provider turns. Codex, Claude, OpenAI-compatible, and generic JSONL adapters have structured continuation bridges. Generic remote MCP OAuth now performs resource/auth-server discovery, PKCE S256, dynamic registration, resource binding, keychain token storage, refresh, callback expiry, and native desktop/Android setup. Composio session creation, Google Workspace preset consent, account-state preflight, explicit account switch/revoke, V3 webhook reconciliation, vault-only signing secrets, HMAC/timestamp/scope verification, and duplicate-safe routine delivery are fixture-proven with hosted workbench disabled. Pre-registered/CIMD-only MCP servers, real Composio/Google consent and events, and live external MCP acceptance remain open. |
| Browser and general tools | Provider loops implemented; live runtime incomplete | Every provider adapter receives bounded server-owned list/read/write/create-directory and structured terminal tools. Filesystem writes and commands share durable approvals, workspace scoping, cancellation, and the same structured continuation contract; an approval-gated provider write is fixture-proven. When loopback CDP is configured, adapters also receive open, HTTPS navigate, current URL, screenshot, and close. The browser reuses its persistent profile, persists screenshots as message artifacts, pauses for human takeover, and exposes no cookies, paths, raw CDP, headers, or JavaScript evaluation. Browser actions now have a 30-second server fence: a stalled runtime is detached, persisted failed, stripped of approval/takeover state, and replaceable by a fresh session. A real generic provider plus local terminal/browser acceptance remains open. |
| Routines and Assistant Packs | Partial | Scheduler, triggers, durable jobs, retries, dry runs, installed packs, and signed Composio plugin events exist. Event notifications use a 750 ms quiet window, batch at most 25 matching events, retain a monotonic cursor, and report when a 500-event inspection window leaves backlog. Duplicate, disabled, stale, tampered, restart, batched-event, and redacted-history contracts are automated. Runs can launch a direct provider turn, but real-provider scheduled execution, host reboot continuity, notifications, and a reopenable assistant-grade run history are unproven. |
| Notifications | Partial | Desktop/Android distinguish high-priority decision/secret input and approvals from low-priority completion, deduplicate sequenced events, and carry exact chat/activity or routine/run deep links. Android relies on a live process/WebSocket and reconnect-on-open; there is no push path after Android process death. |
| Remote access and pairing | Implemented but unproven | Loopback default, explicit remote bind, single-use pairing, device sessions, revocation, and Android transport policy exist. No real LAN/Tailscale/HTTPS phone acceptance was run. |
| Group chats and Bot collaboration | Partial; full Codex handoff chain usable locally | A user message starts each mentioned Bot concurrently, or the current owner when no Bot is mentioned. Streaming replies and participant operations persist; turn/parallel budgets apply; stopping a group cancels its provider operations. Codex group turns receive a participant-scoped `homebot_handoff` dynamic tool whose targets are visible teammate names resolved server-side. HomeBot validates it, persists the sender message and handoff, starts the recipient independently, and returns the result to the sender's still-running turn. The exact rebuilt server completed Scout → Codey → Reviewer → Codey with three visible handoffs and four independent completed operations ending in `CODEY_FIXED_CHAIN`. Deterministic integration covers the same server-owned flow. Concurrent real-provider groups, other provider bridges, and distinct depth/cycle policy remain unproven. |
| Desktop UX | Partial | Conversation/Bot hierarchy and composer exist, with native approval plus confirm/pick-one/secure-entry cards. The structured-input golden and accessibility tree pass, but runtime/VCS/provider detail still dominates significant space; snapshots do not prove daily interaction, VoiceOver, or background management. |
| Android UX | Implemented but unproven | Compose covers chat, stream projection, approvals, native confirm/pick-one/secure-entry cards, status, cancel, attachments, routines, groups, and reconnect logic. Interaction transport and Kotlin compilation pass. It remains a dense single-activity client with no physical-device, notification, lifecycle, TalkBack, performance, or signed-release acceptance. |
| macOS/Linux installation | Implemented but unproven | Packaging/service assets exist. macOS writes a raw LaunchAgent rather than using status-aware `SMAppService`; Linux headless continuity depends on an explicit lingering/service choice. Signing, notarization, clean install, upgrade, rollback, and distro coverage remain open. |

## Live acceptance recorded in this audit

The local server used a temporary SQLite database and disposable Git
repository. The installed Codex CLI used its existing supported account auth;
no API key was added.

* A trivial direct turn completed with `HOMEBOT_REAL_CODEX_OK` in about 5.5 s.
* A resumed follow-up received the Bot identity/responsibility and read the
  attached repository. HomeBot showed a terminal activity and approval naming
  the exact disposable repository, then persisted the successful output.
* Two independent Bots launched 1 ms apart and completed
  `SCOUT_CONCURRENT_OK` and `CODEY_CONCURRENT_OK` without crossed transcripts.
* After the group runner change, the exact rebuilt server started Scout from a
  persisted group mention, exposed its operation/status and turn budget, and
  completed `HOMEBOT_GROUP_CODEX_OK` through authenticated Codex in 7 s.
* The exact rebuilt server then supplied the scoped collaboration tool to an
  authenticated Codex turn. Scout persisted `SCOUT_FOUND_ALPHA`, initiated a
  visible handoff, Reviewer started as a separate operation and persisted
  `REVIEWER_VERIFIED_ALPHA`, and both participant states completed.
* A longer authenticated acceptance then completed Scout → Codey → Reviewer →
  Codey. HomeBot persisted all three correctly addressed handoffs by teammate
  name and four independent completed Bot messages ending in
  `CODEY_FIXED_CHAIN`.
* A long turn cancelled through HomeBot and persisted as `cancelled`.
* Completed output, activity, approval, and cancellation survived restart.
* A deliberately interrupted server restart reproduced a permanently
  `streaming` orphan. The new startup reconciliation converted that exact row
  to a retryable `provider_unavailable` failure on the next live restart.

This is local real-provider evidence, not macOS package, Linux host, desktop
GUI, WebSocket roaming, Android-device, or release acceptance. The first tiny
turn also reported 27,151 input tokens with no context-window total, which is
not an acceptable speed/observability baseline. The new neutral Bot directory
prevents an ordinary assistant from accidentally inheriting the server's
repository, but token/time budgets still need measured release-build work.

## Largest paper-versus-product gaps

1. Group handoff, MCP, and shared-browser tools use provider-neutral turn contracts, but real-provider acceptance remains narrower than the implemented adapter set.
2. Generic filesystem, terminal, and browser contracts are fixture-proven but still need sustained real-provider and packaged-runtime acceptance.
3. Provider health is live-probed for Bot status, but packaged Codex, Claude,
   OpenAI-compatible, and generic-process acceptance remains incomplete.
4. Repository/checkpoint features existed without passing the selected
   repository to the provider process.
5. Restart durability preserved stale streaming state but did not recover or
   truthfully terminate it.
6. Desktop/Android surface coverage and goldens substantially exceed runtime,
   device, accessibility, packaging, and dogfood evidence.

## Product gate before a v1 claim

Direct collaboration now has an eight-turn ceiling and one contribution per Bot; group
collaboration deliberately permits review hand-backs within persisted turn and parallel
budgets, and only configured group members are eligible recipients. Add health-driven
onboarding and run a sustained Mac/Linux plus physical-Android dogfood lane. Until those are proven, HomeBot is a
promising assistant host—not yet the private personal AI team described by the
product goal.
