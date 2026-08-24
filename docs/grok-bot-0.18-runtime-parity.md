# Grok Bot 0.18 runtime parity audit

This is a clean-room-oriented audit of the public evidence preserved by the
unofficial [grok-bot-0.18-reconstructed repository][repo] at commit
[`a9f633e09d49a85829b8236331b9e21f7e612634`][commit].  The commit is the only
reconstructed-repository revision used here.  It is not an Anysphere source
release and it is not proof that every reconstructed function is present in
the original application.

The contract statements below describe observable behavior and acceptance
cases, rather than implementation.  A source link is an evidence pointer, not
permission to copy code.  HomeBot must still validate the relevant behavior in
its own production stack.

## Evidence classes and provenance boundary

The labels in the tables are deliberate:

| Label | Meaning | How HomeBot should use it |
| --- | --- | --- |
| **R — recovered runtime** | A runtime, protocol, IPC, host, or coordinator behavior reconstructed from emitted code, source-path markers, extracted capsules, or repeatable observation of the pinned artifact. | A candidate behavioral contract. Validate it on the target platform before marking a HomeBot parity row `Pass`. |
| **U — reconstructed UI inference** | A readable renderer reconstruction or an exact shipped string, selector, DOM/CSS signature, or asset anchor. | A state/information-architecture hypothesis. It is not a pixel or interaction guarantee; capture the current UI for visual acceptance. |
| **X — repository-author extension** | Behavior the reconstruction explicitly adds or substitutes: local Docker hosting, routed non-Cursor providers, router settings, local usage accounting, or extra guardrails. | Keep it as a HomeBot requirement only when intentionally adopted. Do not call it Grok Bot 0.18 parity. |

The repository's own evidence-only rule says that recovered behavior needs an
inspectable artifact anchor and that renderer gaps must remain gaps rather than
being guessed.  Its notice also says that the project is unofficial, asserts no
upstream source-code license, and does not cover the preserved installers. See
[PROVENANCE.md][provenance] and [NOTICE.md][notice].

The pinned release inputs are the macOS arm64 DMG
(`a253ccd8aab01e083f9812a0264354c5034d8ba7f0610bbb557e82ae77d203eb`) and the
Windows x64 installer
(`464079a15ef5fa8b61ccea8fffcc78f63cfcf6df65fb0ad5e725d8b95f7e437e`) listed in
the [original artifact archive][archive] and its [machine-readable manifest][manifest].
The original macOS app is identified there as bundle ID `com.anysphere.sand`,
Electron 42.1.0, with an independent `app.asar` digest.  The reconstructed
build has a different bundle ID and ad-hoc signature.  Those facts establish
provenance; they do not grant rights to redistribute the original application,
renderer, names, marks, or service content.

For the product-level context, use the official [Grok Bot overview][xai-overview],
[Bots][xai-bots], [chat and collaboration][xai-chat], [files and results][xai-files],
[computer and apps][xai-computer], [skills, routines, and automations][xai-routines],
[settings and notifications][xai-settings], [approvals, security, and privacy][xai-security],
[mobile][xai-mobile], and [FAQ][xai-faq].

## Production composition

These are the production-boundary contracts exposed by the reconstruction.  The
first four are **R** candidates; the local Docker and multi-provider rows are
explicit **X** extensions.

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | The desktop process owns the privileged boundary, creates a context-isolated BrowserWindow with Node integration disabled, persists window placement/focus state, handles deep links, and forwards external navigation instead of silently navigating the app surface. | Launch twice, open a deep link during startup and after launch, restart with a non-default window position, and activate an external URL. The second process exits; the deep link is delivered once; placement survives; external navigation is handed to the OS. | [Electron main composition][main] |
| R | The packaged app requests a single-instance lock. A second instance forwards its argv/open-url payload to the first instance and quits. | Start the packaged app twice with the same and different deep links. Exactly one owner remains and the first owner receives the second payload. | [Electron main composition][main] |
| R | Electron main, host, and coordinator are separate production seams. The coordinator owns the gateway/host services and swaps renderer-facing ports without exposing stale generations. | Kill the coordinator while a request is pending, relaunch it, and reconnect the renderer. Old replies are rejected as transport failure and the new generation is usable. | [Coordinator runtime][coordinator-runtime], [coordinator source][coordinator-source] |
| R | Coordinator child relaunch uses bounded exponential delay (250 ms doubling to 10 s) and treats a child that remains unhealthy for about 30 s as unhealthy rather than reporting a false ready state. | Stop the child repeatedly and observe retry delay, stale-event rejection, and eventual unhealthy state. A late event from an old child cannot mutate the new session. | [Coordinator runtime][coordinator-runtime] |
| R | Gateway transport `down` invalidates cached health and emits a transport-down signal; `connected` refreshes local execution, seeds the roster, and emits connected. Account transitions clear account-scoped MCP state and notifications before the new account is usable. | Drop the gateway, reconnect, then switch account. The UI shows disconnected/reconnecting state, refreshes roster and local execution, and cannot display the previous account's MCP or notification state. | [Coordinator main][coordinator-main], [production provider][production-provider] |
| X | The reconstruction can substitute a locally owned Docker host for the upstream hosted box. It checks a pinned image/schema and content-addressed bundle, rejects an unowned/conflicting container, binds loopback services and owned volumes, waits for gateway readiness, and stops only its own container. | Start with no Docker, an unowned container, a stale image, and a ready owned container. Report distinct unavailable/conflict/not-running/ready states; never delete or stop the unowned container. | [Local Docker connector][docker-host] |
| R | A pause gate suppresses credential egress once and serializes gateway pause/resume across connect, recreate, and force operations. | Pause while reconnecting or recreating. No new credential-bearing request leaves the host; resume performs one ordered reconnect rather than parallel reconnects. | [Box pause gate][box-pause] |
| R | Box migration/recovery reports phases, operation IDs, resumable offsets, a 3 s reconnect watch, a 30 s stall threshold, and bounded reattach attempts; terminal status is retained for the UI. | Interrupt backup, move, cleanup, or wipe; reconnect and resume from the last offset; after the reattach limit, expose a failed terminal operation rather than looping forever. | [Box recovery][box-recovery], [migration watcher][migration-watcher] |

## Chat, replies, reactions, groups, and handoff

The official [chat and collaboration guide][xai-chat] is the product-level source;
the following details are the more specific reconstructed runtime contracts.

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | A send has a client nonce and an input digest covering agent, prompt, rich text, reply/fork metadata, and attachments. An identical in-flight nonce coalesces; an already accepted duplicate is a no-op; a digest mismatch is rejected without a second turn. | Retry the same send after an acknowledgement, retry while the first send is in flight, and reuse its nonce with changed text. The first two produce one user message/turn; the last produces a safe conflict/error and no duplicate work. | [Send pipeline][send-pipeline] |
| R | A prompt must contain text or an attachment. The user message/attachment echo is persisted before dispatch, with a batch ID for multiple attachments and delivery state that can be pending, queued, failed, or sent. | Submit whitespace only, attachment-only, and a mixed prompt; disconnect after local persistence. Whitespace is rejected; attachment-only is accepted; replay does not duplicate the durable echo and preserves delivery state. | [Send pipeline][send-pipeline], [production model][production-model] |
| R | Reply and fork metadata is stamped against an addressed transcript. A fork requires a valid reply target; an unknown or in-flight target is not silently turned into an AI `reply_to` reference. | Reply to an existing message, fork from it, and send with a stale/unknown target. Existing reply context survives reconnect; invalid fork/reply metadata is stripped or rejected deterministically. | [Thread stamping][thread-stamping] |
| R | Direct non-group sends have acknowledgement obligations; group/remote dispatch can return acceptance before detached work when `awaitTurn` is false. | Compare `awaitTurn=true` and `false`: the accepted event is immediate only in the latter, while detached completion remains observable and cannot be mistaken for a completed turn. | [Send pipeline][send-pipeline], [group fan-out][group-fanout] |
| R | Reactions are routed as a message operation, projected with aggregate reactions and the current user's reactions, and may be streamed as group activity. A group member cannot react to its own message. | Add/remove a reaction concurrently and replay the transcript. The final reaction set is durable and deduplicated; a member self-reaction is refused without failing the group turn. | [Coordinator protocol][protocol], [production model][production-model], [group glue][group-glue] |
| R | Local groups deduplicate member IDs, require at least one existing non-nested member, exclude self, and enforce the configured maximum. Duplicating an identical set returns the existing group. Shared-room membership is immutable. | Create with duplicates, a nested group, an unknown member, zero members, over-capacity, and the same set twice. Only the valid set succeeds; the duplicate returns the existing group; shared-room edits return an explanatory summary. | [Group glue][group-glue] |
| R | Group turns use bounded, cancellable round-robin scheduling. Each round resolves responders; empty/pass content stops the round; a member failure is a pass, not a room-wide failure; shared and local rooms use their respective history window. | Run a group with a silent member, a failing member, and a stopped turn. The other members can continue within the round/message/turn limits; stop cancels outstanding work and leaves completed effects. | [Group orchestrator][group-orchestrator], [group glue][group-glue] |
| R | Group sends preserve shared-room publication, membership epoch, and a scheduler lane keyed to the user/source group. Remote rooms mirror dispatch; membership or epoch changes cannot append to the wrong room. | Change group membership while a turn is pending and replay after reconnect. The turn is fenced to its original epoch and any detached completion is attributed to the original room. | [Group fan-out][group-fanout], [group glue][group-glue] |
| R | Bot-to-Bot messaging is explicitly asynchronous. Direct 1:1 messages queue for the receiver; priority 1:1 work interrupts non-user work, while group messages are text-only and priority does not interrupt a group. | Send regular and priority messages to a bot and to a group. The receiver wakes and replies in a later turn; only direct priority work preempts; image attachments to a group are dropped with a note rather than failing the room. | [Agent-to-agent messaging][agent-messaging], [agent messaging policy][agent-policy] |
| X | The reconstructed messaging prompt adds repository guardrails: `SendToAgent` is distinct from `SendMessage`, no ping-pong acknowledgements are required, private user content is not relayed verbatim, and fan-out requires explicit user approval. | Ask one bot to relay private content or fan out without approval. The guardrail refuses or summarizes safely. Treat this as a HomeBot policy choice, not upstream parity. | [Agent messaging policy][agent-policy] |

## Routines and event automations

The product concept is documented by [skills, routines, and automations][xai-routines].
The reconstructed runtime gives the following acceptance-level detail.

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | A routine stores a name, saved prompt, trigger, enabled state, timestamps, and bounded run history. Names are capped at 80 characters, each agent has at most 50 routines, and history is retained to 20 runs. | Create at the limits, exceed each limit, restart, and inspect history. The limit error is safe and deterministic; valid records and the bounded history survive restart. | [Automation model][automation] |
| R | Scheduled triggers accept five-field cron plus the supported `@hourly`, `@daily`, and `@every` forms and an optional `CRON_TZ` timezone. Event triggers have typed provider filters for Slack, GitHub, Microsoft Teams, Linear, Sentry, and PagerDuty. | Parse valid/invalid schedules and events, including timezone and provider filter mismatches. Invalid triggers are rejected before persistence; a matching event fires and a nonmatching event does not. | [Automation triggers][automation-trigger] |
| R | Event delivery debounces for 750 ms, caps queued events per automation at 500, batches at most 25 wake events, coalesces run IDs, and reports overflow. | Send a burst and then more than 500 events. One bounded batch wakes the bot; overflow is reported/dropped according to policy and does not create unbounded work. | [Automation event fires][automation-fires] |
| R | Schedule/manual runs are deduplicated per agent and automation. Disabled, missing, changed, or mismatched backend definitions are rejected; a manual run is exposed separately from lifecycle mutation. | Race Run-now with a schedule and update/disable during delivery. Only one run is active; stale/disabled definitions do not execute; the run result is recorded with its status. | [Automation run path][automation-run], [automation runtime][automation-runtime], [Sand event consumer][sand-event-consumer] |
| R | Background wakes may be silent, have a spend guard, and retry transient stream failures with bounded backoff. Foreground-trigger errors notify; background errors are recorded without a tray storm and are deduplicated. | Run while the user is away, force a transient stream error, and trigger the same failure repeatedly. Backoff is bounded; the run ends in a visible history error; background notification policy remains quiet. | [Automation run path][automation-run] |
| X | “User away” cadence guidance and any HomeBot-specific approval before changing a routine are repository policy, not recovered Grok Bot behavior. | Keep this policy in a separate acceptance row and do not use it to mark the upstream routine contract complete. | [Automation model][automation] |

## Approvals, local permissions, and secrets

The security concepts should be reconciled with [approvals, security, and
privacy][xai-security], but the reconstructed runtime supplies the operation
semantics below.

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | Auto-review requests have a fingerprint, reason, safe summary, command/scope, proposed rule, and a ten-minute TTL. At most four are pending per agent. Modes are off, shadow, and enforce; resolutions are approved or denied. | Create five requests, wait past TTL, and switch mode. The fifth is bounded/rejected, an expired request cannot approve work, and shadow/off do not silently claim an enforce approval. | [Sand auto-review][auto-review], [auto-review service][auto-review-service] |
| R | Approval cards are stale when host generation or user-message epoch changes. Redirect, settings change, session end, cancellation, and quiesce have distinct expiry causes; host update/quiesce tells the user to rerun after resume rather than pretending to be a denial. | Approve a card after redirect, host update, and expiry. Only the current operation can consume approval; each stale cause is safe and retryable where specified. | [Sand auto-review][auto-review], [auto-review service][auto-review-service] |
| R | Local tool permission is action- and target-scoped (`run-command`, `send-input`, `read-file`, `list-directory`, `write-file`) with default `ask`, standing `always`/`never`, ten-minute ask TTL, target/path coverage, and an abort path. A denial is remembered for the direction epoch and is not retried. | Ask for a command, then change only its target; deny, allow once, choose always, abort, and restart a turn. The exact-target rule is enforced; denial does not loop; standing rules persist only in their scope. | [Local permission controller][permission-controller], [permission machinery][permission-machinery], [local approvals][local-approvals] |
| R | Secret requests have bounded labels/descriptions and a secure acknowledgement that the value is not visible in transcript. A submitted secret must match a pending request card; stale cards expire and resume only the owning agent. | Submit to a stale card, submit twice, and inspect transcript/logs. No secret value appears; only the secure acknowledgement is recorded; duplicate/stale submissions do not resume unrelated work. | [Secret request][secret-request], [widget responses][widget-responses] |
| R | Credentials use OS-encrypted storage when available, an in-memory session-only fallback otherwise, atomic writes, legacy plaintext migration, access scoping, and a privileged IPC guard. | Start with encrypted storage, unavailable secure storage, and a legacy record. Migrate atomically, warn on fallback, and reject untrusted IPC access without exposing credentials to renderer content. | [Secret store][secret-store], [secrets IPC guard][secrets-guard] |

## MCP, plugins, and skills

The official [computer and apps guide][xai-computer] supplies the product
surface; these rows describe the reconstructed manager's observable states.

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | The MCP catalog exposes installed summaries, transport, server status, tool count, disabled count, and custom instructions. Management includes install/uninstall, add/remove, restart, authenticate/logout, rename, and account removal. Built-ins are excluded from this managed catalog. | Install, restart, authenticate, rename, remove, and reinstall a server. Each state transition is visible and idempotent; removing a server removes its live references and does not mutate an unrelated built-in. | [MCP service][mcp-service] |
| R | Auth outcomes distinguish started, already-authenticated, not-configured, and error. Tool discovery and execution are routed through the manager and can report an auth slot requirement. | Invoke an unauthenticated, already-authenticated, unconfigured, and failing connector. The UI receives the correct outcome and never treats an auth-required tool as an ordinary tool failure. | [MCP service][mcp-service] |
| R | Plugin-provided skills sync at startup and on a 24-hour interval when authenticated. A failed load keeps unchanged prior cache, prunes obsolete entries after successful reconciliation, and serializes concurrent syncs. | Run startup, concurrent, expired, auth-blocked, and failed syncs. No duplicate sync corrupts the catalog; a transient failure does not erase the last good skill; obsolete entries are pruned after a successful load. | [Plugin skills][plugin-skills] |
| R | Account transition resets account-scoped MCP custom instructions, disabled tools, manager state, and notifications before exposing the new account. | Switch accounts with a connected plugin and a disabled tool. The new account cannot inherit the previous account's authorization or disablement. | [Production provider][production-provider] |
| X | Any HomeBot plugin marketplace, connector policy, or additional skills registry that differs from the above is an extension. It must retain explicit auth/status/revoke states and historical skill-version determinism if adopted. | Disable a plugin and replay an old message. The old message keeps its applied skill version while new messages use current catalog state. | [Plugin skills][plugin-skills], [skill publish][skill-publish] |

## Provider routing and usage

The reconstructed router is explicitly an **X** surface. It is useful for
HomeBot's multi-provider requirements, but it must not be presented as a
Grok Bot 0.18 upstream contract.

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| X | A selected per-agent route can target Cursor, Claude Code, Codex, or OpenRouter. Non-Cursor sends are queued per agent, accepted promptly, and merged with remote transcript tail plus local routed transcript. | Send two prompts to one routed agent and one to another. Each agent remains FIFO; the first acknowledgement does not imply the stream is finished; merged replay has no duplicate tail. | [Shared inference router][inference-router], [coordinator inference router][coordinator-inference] |
| X | Routed local streams emit a running pulse about every 250 ms and wait about 1.2 s before the first delta so composing state is observable. Local transcript storage is schema-versioned, atomic, permission-restricted, and capped per agent. | Start a slow stream, disconnect/reconnect, and exceed the local entry cap. The UI enters composing/running before text, storage remains private/atomic, and old entries are trimmed deterministically. | [Coordinator inference router][coordinator-inference] |
| X | Codex direct routing reads an authenticated `CODEX_HOME/auth.json` only when it is a private regular file without a symlink, refreshes on 401, streams Responses API events, and caps tool steps at eight. | Use a missing, symlinked, world-readable, expired, and valid auth file; force a 401 and nine tool steps. Invalid auth is refused, 401 refresh is bounded, and the ninth step fails safely. | [Provider session][provider-session], [Codex direct responses][codex-responses] |
| X | Claude Code requires its executable and a strict MCP HTTP configuration, permits at most eight turns with MCP, and does not persist a provider session. OpenRouter takes its key from environment or box secrets, has a configurable/default model, and caps tool steps at eight. | Exercise missing executable/config/key, a successful stream, and a tool-loop overflow. Errors identify the safe settings action; no secret or provider payload leaks into the transcript. | [Provider session][provider-session] |
| X | Usage accounting is local router state, not evidence of upstream billing semantics. | Keep usage totals clearly labeled as HomeBot/provider-route accounting and do not use them as an upstream parity assertion. | [Inference router][inference-router] |

## Reconnect and recovery

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | The renderer-facing coordinator source supports lifecycle hello/ready/shutdown, cancellable requests, transport-state subscription, transport swap, and rejection of all pending requests when a transport settles. A swap emits connected only for the new transport. | Abort a request, drop the port, and swap to a new port. The aborted request does not retry; all other pending requests reject as transport failure; the new generation becomes connected once. | [Coordinator source][coordinator-source] |
| R | Failure adaptation distinguishes cancellation, no cloud storage, blocked box, access denied, nonce mismatch, stale auto-review, unknown gateway method, agent-limit, and refused skill publish. Only the explicitly retryable owner-client transport failures retry. | Inject each error kind. The UI receives a safe title/detail and retry policy; a malformed/nonce mismatch error is never retried as if it were a transient network outage. | [Coordinator source][coordinator-source] |
| R | WebAuthn reconnect backs off from 1 s to 30 s with an infinite reconnect policy; delivery retries are bounded to five attempts with 250 ms-to-4 s backoff. | Disconnect the auth provider and fail deliveries. Reconnect continues with bounded delay; a delivery stops after five attempts and remains diagnosable. | [Coordinator main][coordinator-main] |
| R | Recovery operations expose started/operation ID, started-untrackable, development fallback, and rejected outcomes rather than a generic success. | Request recovery without an operation ID, with a rejected capability, and through the development fallback. The caller receives the exact outcome and never polls a phantom operation. | [Box recreate commands][box-recreate] |

## Desktop lifecycle and UI states

The desktop state model below is split intentionally. Process and activity
states are **R**. Renderer labels, selectors, and screen composition are **U**
unless an independent runtime contract above establishes their behavior.

| Class | Observable state/contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | Agent summaries expose group/hidden flags, members, conversation partners, unread/running/composing state, waiting reason, current activity, and drafts; sorting is by updated time. | Reopen after unread, running, composing, waiting, hidden, and draft changes. The sidebar and transcript agree on the same state and stale roster updates cannot move an older row ahead of a newer one. | [Production model][production-model], [agents control feed][control-feed] |
| R | Transcript projection retains notices, permission/approval cards, computer handoff, tool-call pending/running/done/failed/error/aborted, thinking, attachments, delivery state, streaming, reply metadata, reactions, thread summaries, and offline-send timestamps. | Disconnect at each phase and replay. The timeline has one stable event for each phase, preserves terminal effects, and exposes a safe waiting/error state rather than dropping the card. | [Production model][production-model] |
| R | Computer status distinguishes off, starting, sleeping, local, running, and pulling, with VNC URL, pull progress, and handoff state. | Start, sleep, resume, pull, hand off, and return. Each transition is explicit; a stale pull result cannot turn an off computer into running. | [Production model][production-model] |
| U | Shipped-renderer evidence anchors include reconnect copy and Retry action, onboarding, composer placeholder/attachment/send controls, plugin Marketplace/Yours/Filter/Connectors/Skills controls, global search tabs (Messages, Files, Links, Actions), no-results/search-unavailable states, and computer surfaces. | Add these as named UI states and capture golden references for each state. Do not infer unanchored routes, controls, labels, or pixels from the readable reconstruction. | [Production evidence][production-evidence], [production CSS][production-css], [UI audit][ui-audit] |
| U | The recovered command-palette model names tabs for all, messages, agents, groups, files, links, routines, and actions and debounces message search by 150 ms. | Type into each tab, pause through debounce, and show empty/unavailable results. Record this as UI evidence until the underlying search behavior is independently validated. | [Command palette model][palette-model], [message provider][message-provider] |
| U | The renderer entrypoint has explicit recovery levels (exact placeholder, semantic model, named upstream module) and overlays for plugins, settings, network, and computer. | Every HomeBot screen declares its recovery level and evidence anchor. An exact-placeholder or semantic-model screen cannot be accepted as pixel parity without a capture. | [Renderer entrypoints][entrypoints] |

## Notifications, attachments, results, and search

| Class | Observable contract | Acceptance case | Evidence |
| --- | --- | --- | --- |
| R | Desktop notification decisions require a supported desktop/window and compare agent state. Needs-input is critical; agent-done is silent; clicking focuses the exact agent. Dock unread counts aggregate roster state and fence stale rows. | Trigger needs-input, done, error, and stale roster events while focused/unfocused. Only eligible events notify; click selects the exact agent; stale updates cannot increase the badge. | [OS notifications][os-notifications], [dock badge][dock-badge] |
| R | Mobile push seeds a baseline, buffers deltas before seeding, includes turn-finished preview/ID/awaiting-input, and suppresses notifications for a focused window with a five-minute freshness window. | Reconnect a mobile client with queued deltas, then focus/unfocus it around completion. Baseline is not double-counted; pre-seed events are replayed once; focused completion is suppressed only within the freshness window. | [Mobile push notifier][mobile-push] |
| R | Attachment staging rejects empty input, path separators/NULs, unsafe names, missing extensions, and over-limit bytes; committed staging paths remain inside the staging directory. Gateway transfers use 4 MiB chunks. | Upload safe/unsafe names, empty files, over-limit files, and a traversal path. Only valid files stage/commit; no path escapes; a retry does not duplicate the upload. | [Attachments][attachments] |
| R | Media uses a privileged `sand-media` scheme, supports local/remote range reads with 206/416 semantics, 4 MiB remote reads, accept-ranges, and no-cache headers. Downloads write chunks and remove a partial file on failure. | Seek a media preview, request an invalid range, interrupt a download, and retry. The preview returns correct range status; the partial file is removed and a generic safe error is shown. | [Media protocol][media-protocol], [attachments][attachments] |
| R | Search uses SQLite FTS5 for user/assistant text and media metadata, Unicode tokenization, 2/3-character prefixes, at most eight terms, bounded snippets, and a per-agent result cap; an empty media query uses recency. | Search eight terms, nine terms, prefixes, attachment metadata, and an empty media query. Results are scoped, snippets are bounded, and unsupported/too-long input is reported rather than generating an unbounded query. | [Search index DB][search-db] |
| R | Search reconciles in a background worker before ready, exposes readiness, incrementally upserts/deletes/clears/reindexes, rebuilds corruption up to three times, respawns a worker up to three times, and marks unavailable after repeated failure. | Search during reconciliation, corrupt the index, kill the worker repeatedly, and dispose during work. The UI shows not-ready/unavailable accurately; recovery is bounded and disposal finishes within its deadline. | [Search index service][search-service] |

## Concrete acceptance gaps in `docs/parity-matrix.md`

The existing [parity matrix](parity-matrix.md) is a good product-level map, but
its rows generally state a broad outcome (“round trip”, “exact lifecycle”, or
“recover”) rather than the observable edge cases recovered above.  The list
below names only concrete acceptance details that are currently missing or
under-specified; it does not claim that HomeBot implementation is absent.

| Matrix row | Add this explicit acceptance case |
| --- | --- |
| Direct chat text/links/images; streaming response | Same-nonce in-flight coalescing, accepted-duplicate no-op, nonce/digest mismatch rejection, durable-before-dispatch echo, offline delivery states, and replay with no duplicate deltas. |
| File attachments; files/results preview cards | Safe filename/path and byte checks, 4 MiB transfer chunks, staging-directory containment, range response semantics, and partial-download deletion on failure. |
| Reply/thread; redirect while working | Valid-target stamping, fork-only-on-valid-target, stale/in-flight target stripping, cancellation of pending approval/tool work on redirect, and preservation of reply context after reconnect. |
| Reactions | Durable add/remove reconciliation, current-user reaction projection, and refusal of group-member self-reactions. |
| Group create, rename, membership; group routing and `@everyone` | Nested-group rejection, existing-member requirement, duplicate-set identity, minimum/max member bounds, shared-room immutability, membership epoch fencing, bounded rounds, and member failure as pass. |
| Bot-to-Bot handoff; ownership and parallel work | Explicit asynchronous return semantics, priority interruption only for direct 1:1, group text-only behavior, bounded redrive, and the no-ping-pong/private-content policy as a separate HomeBot extension. |
| Routine create/manual run/test; scheduled routine/time zone; event-triggered routine | Trigger grammar/timezone validation, provider filter matching, 750 ms debounce, 25-event wake batches, 500-event queue bound, per-agent/automation deduplication, and stale/disabled-definition rejection. |
| Routine enable/pause/delete/history | Run status/history bounds, foreground versus background notification policy, bounded transient retry, and a run that remains inspectable after a restart or host update. |
| Structured approval once/deny; persistent capability rules | Ten-minute TTL, four-pending bound, operation fingerprint, host/message generation staleness, distinct quiesce/cancel/redirect expiry, exact action+target/path scope, and denial non-retry behavior. |
| Secure secret entry | Pending-card ownership, stale/duplicate submission behavior, secure acknowledgement only, no value in transcript/logs, encrypted-store fallback warning, and privileged IPC rejection. |
| Plugin discovery/connect/status/remove; skills create/edit/enable | Status taxonomy, built-in exclusion, auth outcomes, account-scope reset, startup/24-hour skill sync, stale-cache retention, successful prune, and historical applied-version determinism. |
| Attention, unread, working states; notifications/deep links | Roster stale-row fencing, exact-agent click target, critical needs-input versus silent done, mobile baseline/delta buffering, five-minute focus freshness, and single-instance/deep-link delivery. |
| Desktop/mobile continuity | Transport swap lifecycle, pending-request rejection, generation fencing, coordinator relaunch backoff, migration offset resume, stall/reattach bounds, and pause-gated credential egress. |
| Search prior conversations/results | Eight-term limit, FTS prefix/snippet behavior, media-recency query, ready/unavailable state, corruption/worker retry bounds, and result scope. |
| Appearance and settings; error notices | Treat shipped strings/selectors and each reconnect, unavailable, approval, permission, no-result, and empty state as capture-required UI evidence; do not promote reconstructed UI inference to a pixel pass. |

These additions should be kept separate from the matrix's “Specified”/“Pass”
status language: a source contract is not a platform test, and a clean merge or
renderer build is not runtime proof.

## Reference links

[repo]: https://github.com/b-nnett/grok-bot-0.18-reconstructed
[commit]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/tree/a9f633e09d49a85829b8236331b9e21f7e612634
[provenance]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/PROVENANCE.md
[notice]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/NOTICE.md
[archive]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/research-archives/README.md
[manifest]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/research-archives/original/0.18.0/artifacts.json
[main]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/main.ts
[coordinator-runtime]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/coordinator/coordinator-runtime.ts
[coordinator-main]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/node-agent-coordinator/main.ts
[coordinator-source]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/frontend/src/recovered/runtime/coordinator-source.ts
[production-provider]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/coordinator/production-provider.ts
[docker-host]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/box/local-docker-host-connector.ts
[box-pause]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/box/box-client-pause.ts
[box-recovery]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/box/box-recovery.ts
[migration-watcher]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/box/box-migration-watcher.ts
[box-recreate]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/box/box-recreate-commands.ts
[protocol]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/shared/rpc/coordinator.ts
[send-pipeline]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/send-pipeline.ts
[thread-stamping]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/send-thread-stamping.ts
[group-fanout]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/send-group-fanout.ts
[group-orchestrator]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/group-chat-orchestrator.ts
[group-glue]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/group-chat-glue.ts
[agent-messaging]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/agent-to-agent-messaging.ts
[agent-policy]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/agents/agent-messaging.ts
[automation]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/automations/automation.ts
[automation-trigger]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/automations/automation-trigger.ts
[automation-fires]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/automation-event-fires.ts
[automation-run]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/automation-run-path.ts
[automation-runtime]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/automation-runtime.ts
[sand-event-consumer]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/automations/sand-automation-fire-consumer.ts
[auto-review]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/runner/sand-auto-review.ts
[auto-review-service]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/auto-review/auto-review-service.ts
[permission-controller]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/local-tool-permission/local-tool-permission-controller.ts
[permission-machinery]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/shared/local-tool-permission-machinery.ts
[local-approvals]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/local-exec/local-tool-approvals.ts
[secret-request]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/runner/tools/sand-secret-request.ts
[widget-responses]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/transcript/widget-responses.ts
[secret-store]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/secrets/secret-store.ts
[secrets-guard]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/secrets/secrets-ipc-guard.ts
[mcp-service]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/mcp/mcp-service.ts
[plugin-skills]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/mcp/plugin-skills.ts
[skill-publish]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/mcp/skill-publish.ts
[inference-router]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/shared/inference-router.ts
[coordinator-inference]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/node-agent-coordinator/inference-router.ts
[provider-session]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/inference/provider-session.ts
[codex-responses]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/inference/codex-direct-responses.ts
[production-model]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/frontend/src/production/model.ts
[production-evidence]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/frontend/src/production/evidence.ts
[production-css]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/frontend/src/production/production.css
[ui-audit]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/scripts/audit-ui-provenance.mjs
[palette-model]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/frontend/src/production/command-palette-model.ts
[message-provider]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/frontend/src/production/command-palette-message-provider.ts
[entrypoints]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/frontend/src/recovered/runtime/entrypoints.ts
[control-feed]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/notifications/agents-control-feed.ts
[os-notifications]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/notifications/os-notification-manager.ts
[dock-badge]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/notifications/dock-badge-manager.ts
[mobile-push]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/notifications/mobile-push-notifier.ts
[attachments]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/attachments/attachments.ts
[media-protocol]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/electron-main/media/media-protocol.ts
[search-db]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/content-search/search-index-db.ts
[search-service]: https://github.com/b-nnett/grok-bot-0.18-reconstructed/blob/a9f633e09d49a85829b8236331b9e21f7e612634/source/host/extensions/content-search/search-index-service.ts

[xai-overview]: https://docs.x.ai/grok-bot/overview
[xai-bots]: https://docs.x.ai/grok-bot/bots
[xai-chat]: https://docs.x.ai/grok-bot/chat-and-collaboration
[xai-files]: https://docs.x.ai/grok-bot/files-and-results
[xai-computer]: https://docs.x.ai/grok-bot/computer-and-apps
[xai-routines]: https://docs.x.ai/grok-bot/skills-routines-and-automations
[xai-settings]: https://docs.x.ai/grok-bot/settings-and-notifications
[xai-security]: https://docs.x.ai/grok-bot/approvals-security-and-privacy
[xai-mobile]: https://docs.x.ai/grok-bot/mobile
[xai-faq]: https://docs.x.ai/grok-bot/faq
