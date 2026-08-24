# Client/server protocol v1

Status: M0 v1 contract. The Rust types in `homebot-protocol` own the wire contract. The checked JSON Schema lives in `protocol/schema/`, and the checked Android binding is generated into `android/protocol/`.

## Transport

- HTTP handles health/version, pairing exchange, snapshots, CRUD queries/commands, and resumable binary attachment upload.
- WebSocket carries authenticated live events, command lifecycle updates, streaming message parts, activities, approvals, and heartbeats.
- TLS/WSS is required outside loopback or an explicitly accepted private-network exception. Public-interface plaintext is rejected by default.

## Negotiation

The client opens with `hello(protocol_version, client_version, resume_after)`. The server accepts only versions inside its declared inclusive compatibility range. Unsupported clients receive the structured `protocol_version_unsupported` error before closure. A server may support multiple protocol versions, but never reinterpret an existing field incompatibly.

## Snapshot and resume

1. Authenticate the device session.
2. Negotiate protocol compatibility.
3. If `resume_after` remains inside the retained outbox window, replay every event with a greater sequence in order.
4. Otherwise return a complete authorised snapshot and its boundary sequence.
5. Continue with incremental events whose sequences are strictly greater than the boundary.

Sequence numbers are server-monotonic for the authenticated owner stream. Clients ignore a repeated event ID, reject an unexplained gap, and request a fresh snapshot. Reconnect never relies on message text comparison.

## Mutations and idempotency

Every mutation has a client-generated request ID and idempotency key. Repeating the same key and same canonical command returns the prior operation/result. Reusing a key for a different command is a conflict. Asynchronous commands emit accepted, then exactly one terminal completed, failed, or cancelled state. Cancellation is itself idempotent and race-safe with normal completion.

## Errors

Errors contain a stable machine code, safe user-facing message, retryable flag, and request ID where relevant. Debug details remain in redacted local diagnostics and never expose provider secrets, pairing tokens, raw environment variables, or filesystem data outside the requesting capability scope.

Pairing is owner-initiated but exchange is the sole narrowly unauthenticated mutation. `POST /api/v1/pairing` returns a five-minute single-use offer, `POST /api/v1/pairing/exchange` atomically consumes it and returns a named persistent device session once, and owner-only device routes list/revoke sessions. Raw tokens never enter SQLite or events. Device bearers authenticate the same HTTP/WebSocket protocol but cannot administer devices. See [remote-access.md](remote-access.md).

Required common codes include `unauthenticated`, `forbidden`, `approval_required`, `not_found`, `conflict`, `validation_failed`, `rate_limited`, `provider_unavailable`, `plugin_unavailable`, `secret_store_locked`, `secret_store_unavailable`, `operation_cancelled`, `resume_unavailable`, `protocol_version_unsupported`, and `internal`.

## Secret metadata

Authenticated `/api/v1/secrets` endpoints list, create, update, and delete owner-scoped secret references. Responses contain only UUID, label, availability state, and timestamps. Secret-bearing create/update request types deliberately omit Rust `Debug`/`Serialize`; generated Android request classes redact values from `toString`. Mutations emit sequenced `secret_changed` or `secret_removed` events containing no value or credential-store locator.

`ready`, `locked`, `unavailable`, and `missing` are distinct states so clients can offer the correct recovery action without receiving platform error details. See [secrets.md](secrets.md).

## Plugin registry

Authenticated `/api/v1/plugins` operations expose provider-neutral summaries and exact `connect`, `waiting`, `reopen`, `connected`, and `error` states. Summaries include authentication state, bounded discovered-tool metadata, enablement and Bot assignments, but never raw MCP frames or secret values. Mutations emit monotonic `plugin_changed` and `plugin_removed` events so desktop and Android render server state rather than inferring transport health.

## Native desktop transport

`HomeBotApp` uses `homebot-desktop::transport` as its only state-changing path. Startup probes the public health endpoint, supervises an embedded loopback server when local mode has no listener, then calls the authenticated version endpoint and opens `/api/v1/events` with the same bearer session used by remote clients. The first client frame is `hello` with protocol/client versions and an optional resume cursor. A `snapshot_required` hello is followed by a boundary snapshot; `replayed` is followed only by strictly newer retained events.

The transport advances its durable cursor only for accepted state events. Heartbeat pings are answered without advancing it, duplicate or older sequences are ignored, and a sequence gap forces reconnect/replay. If replay retention has advanced, the server sends a fresh snapshot and the client atomically replaces its projections. Bot create/edit/archive/read, direct-chat creation/messages/steering/retry, approvals, stop, and the three-step attachment upload/finalize flow all use authenticated HTTP with fresh request and idempotency identifiers. Commands queued while disconnected remain pending until an authenticated connection returns.

## Routines

Routine definitions contain typed inputs, expected outputs and tagged structured steps. Recording actions use the same step representation, so replay never depends on UI coordinates. Create/edit/duplicate/enable/disable/delete, recording append/finish/cancel, dry run, Run now and run history are authenticated server operations. `routine_changed`, `routine_removed`, `routine_recording_changed` and `routine_run_changed` events are durable and sequenced.

Each run carries the immutable `routine_version_id`, step status and redacted structured output. Dry-run results are `planned` and have no output. Approval-required replay stops before executing the marked step.

Routine triggers and jobs use authenticated HTTP mutations plus durable `routine_trigger_changed`, `routine_trigger_removed`, and `routine_job_changed` events. Trigger definitions carry timezone/missed-run/overlap/retry policy; job summaries expose only redacted input metadata. Schedule delivery keys and external webhook/event delivery keys are idempotent. Event triggers advance a durable outbox sequence only after all preceding events have been examined, which provides restart-safe delivery without treating WebSocket broadcasts as authority.

Skills use authenticated CRUD, duplication, assignment, and versioned import/export endpoints. The initial snapshot carries the Skill library; `skill_changed` and `skill_removed` keep client projections current. `SendMessageRequest.skill_ids` adds turn-specific Skills, while assigned and explicit active versions are resolved once and persisted in `MessageSummary.applied_skills`. Queued prompts similarly pin version IDs internally, so edit, reconnect, retry, or restart cannot silently change accepted context. Portable tool references remain subject to server capability and approval policy.

Assistant Packs expose a server-owned curated catalog at `GET /api/v1/assistant-packs`. Installing through `POST /api/v1/assistant-packs/{pack_id}/install` atomically creates and assigns the pack's Skill, enabled routine, and timezone-safe trigger for one Bot. The response returns all three authoritative summaries; normal Skill, routine, and trigger events keep connected clients current.

## Repository workspaces

Authenticated workspace endpoints register canonical Git repositories, list local branches, attach a chat in primary or isolated mode, and detach it. Snapshot fields and sequenced `repository_workspace_changed`, `chat_workspace_changed`, and `chat_workspace_removed` events keep desktop and Android projections aligned with SQLite authority. Summaries report the effective path, selected branch/base ref and `clean`, `dirty`, `conflicted`, or `unavailable` condition. Clients never construct managed paths or perform worktree lifecycle locally.

Coding-chat timelines include opaque turn-checkpoint summaries. Authenticated endpoints list checkpoints, return an explicit-pair or full-chat binary-capable diff, and restore a stopped chat to a checkpoint with an idempotency key. `turn_checkpoint_changed` and `checkpoint_restored` events are durable and sequenced. Restore responses state whether provider context was unchanged or forked; hidden refs, object IDs and provider conversation IDs never cross the client contract.

Authenticated chat-scoped VCS endpoints return normalized porcelain status, staged/unstaged binary-capable diffs, exact commit results, clean branch transitions, push outcomes and pull-request metadata/actions. `vcs_status_changed` is durable and sequenced. Push and pull-request creation return `approval_required` until the server capability engine consumes an allowed, exact-request approval. Mutation responses are durably idempotent; remote URLs, credentials and raw Git/GitHub output never cross the contract.

Owner-authenticated capability-rule endpoints list, upsert, delete, and audit narrow allow/require-approval/deny policy. Current safe rule summaries are included in reconnect snapshots and reconciled through sequenced `capability_rule_changed` and `capability_rule_removed` events. Android treats this projection as read-only because a paired device session cannot grant itself authority; the owner desktop performs administration through the same server contract.

Authenticated browser-session endpoints expose an owner-scoped safe projection of server-local profiles and live targets. Clients may list/watch sessions, request takeover, return control to a Bot, navigate, capture a screenshot artifact, and close through normalized commands. Cookies, login values, CDP addresses, profile paths, and evaluated page data never cross this contract. `browser_session_changed` is durable and sequenced, and browser sessions are included in reconnect snapshots so desktop and Android reconcile the same controller/status/current-URL projection. Sensitive actions return the ordinary structured approval and resume only with its digest-bound ID.

## Attachments

Attachments are uploaded over authenticated HTTP with size/type limits, streaming digest verification, an idempotency key, and an explicit finalise step. WebSocket messages refer to completed attachment IDs. Partial uploads expire and cannot be consumed.

## Activity and generated artifacts

Execution activity crosses the protocol as a provider-neutral kind, lifecycle status, risk level and typed presentation detail. File and terminal locations must be normalized workspace-relative display paths. Browser screenshots and generated outputs use artifact UUIDs; clients never receive or construct a server filesystem path.

Generated artifact metadata and content are fetched through authenticated owner-scoped HTTP routes. `ArtifactSummary` exposes safe name, kind, media type, size and SHA-256 digest while deliberately omitting the internal content-addressed storage path. See [activity-surfaces.md](activity-surfaces.md).

## Bot lifecycle

The authenticated Bot collection exposes list, create, update, archive, restore and mark-read operations under /api/v1/bots. Create and update bodies carry the user-facing identity plus advanced provider-profile and permission settings. Responses normalize provider health and include unread, attention and archive state. Successful changes emit durable bot-changed events and appear in subsequent reconnect snapshots.

Bot mutation idempotency hashes include the operation and target Bot as well as the request body. This prevents a key reused across different routes from being mistaken for a replay.

## Direct chat timeline

Direct-chat HTTP operations create or recover the one chat for a Bot, load a complete timeline, submit or queue a message, steer active work and stop active work. Rich message parts cover text, attachments and notices with stable IDs and ordinals. Reply IDs, Bot mention IDs, reactions and typed Bot/group/routine/plugin references are first-class metadata. Typed references are owner-resolved when accepted, retain an immutable label snapshot, pin the active routine version, and survive idempotent replay, queued promotion, rename and restart. Applied Skill slash references similarly expose the exact immutable Skill version used for a turn.

Authenticated global search is owner-scoped and returns exact immutable `homebot://` navigation targets for messages, uploaded files, generated artifacts, links and routines. Clients treat search and timeline results as replaceable server projections; they do not rebuild an independent index.

Timeline snapshots contain canonical messages, normalized execution activities, approvals and typed `follow_up`/`steering` queued prompts at a durable outbox boundary. Incremental Chat-changed, Message-changed, Message-delta, Activity-changed, Approval-changed and Queued-prompt-changed events reconcile that state. Clients deduplicate event IDs and sequences and replace the timeline when a gap is detected.

Timeline snapshots also include optional provider-neutral working-context state. Authenticated mode and compact/reset mutations return that state directly and publish `working_context_changed`; atomic queue promotion publishes `queued_prompt_removed`, the durable user message and changed chat state in sequence. Capability flags distinguish unsupported provider features from transient failures.

## Heartbeats and backpressure

The server sends nonce-bearing pings. A client responds with pong before the advertised timeout. Slow clients have a bounded queue; the server closes them with a resumable cursor instead of allowing unbounded memory growth. Streamed content can be coalesced only when doing so preserves the final canonical message parts.

## Compatibility rules

- Additive optional fields are permitted within a protocol version.
- Changing meaning, type, required status, ordering guarantees, or security semantics requires a new version.
- Unknown event kinds are ignored only if the negotiated version marks them ignorable; otherwise the client resnapshots or fails closed.
- Android models must be generated from or mechanically checked against the committed schema in CI.
- Golden fixtures cover every envelope, malformed input, unknown fields, skew, reconnect, duplicate mutation, cancellation race, and slow-client path.

## Contract drift checks

Run both checks before changing a wire type:

```sh
cargo run -p homebot-protocol --example export_schema -- --check
cargo run -p homebot-protocol --example export_android -- --check
```

Regenerate without `--check` after an intentional compatible contract change. A breaking change requires a new protocol version and parallel schema/binding rather than rewriting v1 semantics.
