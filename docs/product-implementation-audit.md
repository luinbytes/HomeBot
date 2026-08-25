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
| Claude Code | Implemented but unproven | Structured streaming, local account health, cwd, resume, and cancellation exist. Real account execution was not run because `claude` is not installed. Headless approvals are explicitly unsupported. |
| OpenAI-compatible API | Implemented but unproven | HTTPS/loopback validation, opaque secret lookup, models, and narrow Responses/Chat Completions SSE paths exist. Only fixtures prove them; Chat Completions continuation, approvals, tools, and provider-side cancellation are absent. |
| Streaming and cancellation | Actually usable with local Codex | Live content/activity state persisted; a long Codex turn became `cancelled` 355 ms after HomeBot stop. No Android rendering or network-loss acceptance was performed. |
| Concurrent direct Bots | Actually usable with local Codex | Scout and Codey started 1 ms apart and completed independent markers in overlapping 3.3 s/4.8 s windows. This does not prove multiple accounts or heavy tool workloads. |
| Client disconnect/reconnect | Partial | Server-owned turns continued after short-lived HTTP clients exited, and later timeline reads recovered the result. WebSocket loss, Wi-Fi/Tailscale roaming, cursor expiry, and Android process death remain unproven. |
| Server restart during a turn | Partial after this change | HomeBot cannot reattach to an in-flight provider. Startup now marks orphaned direct/group streaming messages and activities failed, expires approvals, clears false direct running state, and moves affected group participants from running to failed with a retryable error. Before the fix the UI remained stuck on `streaming` forever. |
| Status / “What are you working on?” | Partial | Chat running/queue state and normalized activities exist, but there is no first-class human summary across Bots. Provider readiness shown on Bot cards only proves a configured profile record, not executable/auth health. |
| Approvals | Partial | Codex command approval was live-proven and durable. Claude has no approval bridge; Android-device handling, offline decisions, duplicate decision races, and file-write approval were not accepted live. |
| Repository, VCS, checkpoints | Partial | Repository registration, isolated worktrees, status/diff/commit/branch/push/PR/checkpoint APIs exist and are fixture-tested. Provider cwd was disconnected until this change. A real Codex read in the attached repository now works; a real write/diff/restore/push/PR chain is still unproven. |
| Skills | Partial | Versioned skills can be assigned and are assembled into direct provider prompts. There is no live proof that a real provider uses a skill correctly, and skills do not define collaboration permissions or budgets. |
| MCP/plugins | Scaffold/mock/demo only | Registry, connection, tool metadata, assignment, secret boundaries, and tests exist. Direct provider execution has no tool bridge that lets Codex/Claude invoke registered HomeBot MCP tools. |
| Browser and general tools | Scaffold/mock/demo only as Bot capabilities | Browser-session/policy APIs and fixture runtimes exist, as do separate VCS endpoints. They are not a provider tool loop, so a Bot cannot naturally choose these HomeBot capabilities during a conversation. Native CLI activities are merely normalized for display. |
| Routines and Assistant Packs | Partial | Scheduler, triggers, durable jobs, retries, dry runs, and installed packs exist. Runs can launch a direct provider turn, but real-provider scheduled execution, host reboot continuity, notifications, and a reopenable assistant-grade run history are unproven. |
| Notifications | Partial | Desktop/Android notification surfaces and attention state exist. Android relies on a live process/WebSocket and reconnect-on-open; there is no push path after Android process death. |
| Remote access and pairing | Implemented but unproven | Loopback default, explicit remote bind, single-use pairing, device sessions, revocation, and Android transport policy exist. No real LAN/Tailscale/HTTPS phone acceptance was run. |
| Group chats and Bot collaboration | Partial; full Codex handoff chain usable locally | A user message starts each mentioned Bot concurrently, or the current owner when no Bot is mentioned. Streaming replies and participant operations persist; turn/parallel budgets apply; stopping a group cancels its provider operations. Codex group turns receive a participant-scoped `homebot_handoff` dynamic tool whose targets are visible teammate names resolved server-side. HomeBot validates it, persists the sender message and handoff, starts the recipient independently, and returns the result to the sender's still-running turn. The exact rebuilt server completed Scout → Codey → Reviewer → Codey with three visible handoffs and four independent completed operations ending in `CODEY_FIXED_CHAIN`. Deterministic integration covers the same server-owned flow. Concurrent real-provider groups, other provider bridges, and distinct depth/cycle policy remain unproven. |
| Desktop UX | Partial | Conversation/Bot hierarchy and composer exist, with many functional secondary panels. Runtime/VCS/provider detail still dominates significant space; snapshots prove rendering, not daily interaction, responsiveness, accessibility, or background management. |
| Android UX | Implemented but unproven | Compose covers chat, stream projection, approvals, status, cancel, attachments, routines, groups, and reconnect logic. It is a dense single-activity client with no physical-device, notification, lifecycle, TalkBack, performance, or signed-release acceptance. |
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

1. Codex can initiate one visible group handoff, but the broader HomeBot
   tools/plugin catalog and other providers are not connected to that loop.
2. HomeBot tools/plugins look broad in API and UI, but are not connected to the
   provider execution loop.
3. Provider “ready” on a Bot is configuration presence, not live health.
4. Repository/checkpoint features existed without passing the selected
   repository to the provider process.
5. Restart durability preserved stale streaming state but did not recover or
   truthfully terminate it.
6. Desktop/Android surface coverage and goldens substantially exceed runtime,
   device, accessibility, packaging, and dogfood evidence.

## Product gate before a v1 claim

Add explicit collaboration depth/cycle/permission policy beyond the existing
turn and parallel budgets. Wire the existing skills/MCP/browser/file
capabilities into provider turns, add health-driven onboarding, and run a sustained Mac/Linux
plus physical-Android dogfood lane. Until those are proven, HomeBot is a
promising assistant host—not yet the private personal AI team described by the
product goal.
