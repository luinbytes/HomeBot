# Architecture

Status: M0 contract, 20 August 2026.

## Product boundary

HomeBot is a messaging application for durable AI teammates. Provider sessions, agent runtimes, checkpoints, and workspaces are implementation concepts. They appear in the primary UX only when a user needs the underlying detail to make a decision.

The self-host machine replaces Grok Bot's hosted computer. This is a deployment substitution, not permission to weaken approval boundaries.

## Process topology

The headless `homebot-server` is authoritative. Desktop and Android use its authenticated HTTP and WebSocket API. Desktop can launch and supervise a bundled child server, discover its loopback endpoint, and reconnect after restart, but it has no privileged side channel.

Server-owned responsibilities:

- domain state and invariants
- provider discovery, authentication status, execution, streaming, and cancellation
- capability policy and approvals
- filesystem, PTY, browser, MCP/plugin, secret, and VCS operations
- routines, schedules, event triggers, retries, and run history
- SQLite, migrations, backups, outbox sequence, and artifact storage
- pairing, device sessions, rate limits, and audit events

Client-owned responsibilities:

- rendering, navigation, input, accessibility, and local notifications
- connection state and a replaceable local cache
- OS-protected device-session credentials
- desktop supervision of a local server process

## Bounded crates

Dependencies point inward toward `homebot-domain` and `homebot-protocol`. Domain code cannot depend on transport, SQL, egui, Android, or a provider SDK. Provider adapters cannot expose provider-native payloads through client contracts. Tools and VCS return normalised activities and structured approval requests.

## Identity and conversations

A `Bot` has a HomeBot-owned stable ID, identity, instructions, memory policy, provider profile reference, permissions, skills, and plugins. A direct chat or group chat owns HomeBot transcript history. Backend conversation IDs are mappings keyed by chat and provider profile. Provider switching retains Bot identity and transcript but creates or resumes the appropriate backend mapping.

Group coordination is an explicit server state machine with a bounded hop count, concurrency budget, one visible current owner where applicable, and an emergency stop. Bot-to-Bot messages are durable message records, not invisible in-process calls.

## Persistence

SQLite runs in WAL mode and is the source of truth for structured product state. Each schema change is an ordered, transactional migration with an upgrade fixture. Event-producing mutations update domain rows and the outbox in one transaction. Monotonic outbox sequences support WebSocket resume and client reconciliation.

Large attachments and generated artifacts live outside SQLite in a content-addressed data directory. Database rows store ownership, media type, size, digest, and lifecycle metadata. Secret rows contain opaque credential-store references only.

## Failure model

All mutations carry an idempotency key. Accepted asynchronous operations have stable operation IDs and terminal completion/failure/cancelled events. The server restores incomplete durable operations according to operation-specific recovery policy after restart. Clients hydrate a snapshot, apply strictly ordered events, detect gaps, and resnapshot when replay is no longer available.

Provider, MCP, browser, terminal, and Git failures are isolated from the server process and represented as provider-neutral error/activity events. Bounded output, cancellation, child cleanup, and backoff are mandatory.

## Decisions still requiring implementation evidence

- exact OS credential-store crate and Linux locked-session behaviour
- content-addressed artifact garbage collection policy
- supported compatibility-window duration after v1
- egui native chrome exceptions and golden rendering hosts
- the Android schema generation tool after the first complete Rust schema
