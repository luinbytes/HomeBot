# Architecture

Status: M3 automation and extension foundation, 20 August 2026.

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

`homebot-tools` owns the local-computer authority boundary. Its policy engine evaluates authenticated owner/device, Bot, chat, workspace, capability, canonical target and action before any side effect. Filesystem operations use a capability directory, terminal operations use a supervised real PTY with a cleared environment, and browser operations use a loopback-only CDP session whose profile remains on the server. See [tools.md](tools.md).

## Identity and conversations

A `Bot` has a HomeBot-owned stable ID, identity, instructions, memory policy, provider profile reference, permissions, skills, and plugins. A direct chat or group chat owns HomeBot transcript history. Backend conversation IDs are mappings keyed by chat and provider profile. Provider switching retains Bot identity and transcript but creates or resumes the appropriate backend mapping.

Group coordination is an explicit server state machine with a bounded hop count, concurrency budget, one visible current owner where applicable, and an emergency stop. Bot-to-Bot messages are durable message records, not invisible in-process calls.

Bot lifecycle is owner-scoped and server-authoritative. The domain validates identity fields and explicit archive/restore transitions. SQLite persists visual identity, advanced provider/permission references, unread count and normalized attention. Desktop keeps only a replaceable roster projection. See [bots.md](bots.md).

Direct chats persist rich message parts, reply/mention metadata and queued prompts independently of provider conversations. The desktop timeline is a replaceable projection built from an HTTP boundary snapshot plus strictly sequenced provider-neutral events. See [chats.md](chats.md).

## Persistence

SQLite runs in WAL mode and is the source of truth for structured product state. Each schema change is an ordered, transactional migration with an upgrade fixture. Event-producing mutations update domain rows and the outbox in one transaction. Monotonic outbox sequences support WebSocket resume and client reconciliation.

`homebot-storage` opens SQLite with foreign keys, a bounded busy timeout, WAL journalling, and startup integrity checks. Startup fails closed on migration, quick-check, or foreign-key failures. Backups use SQLite's consistent `VACUUM INTO` operation, and restore refuses to overwrite an existing destination.

Large attachments and generated artifacts live outside SQLite in a content-addressed data directory. Database rows store ownership, media type, size, digest, and lifecycle metadata. Secret rows contain owner-scoped opaque credential-store references only. `homebot-secrets` performs blocking macOS Keychain/Linux Secret Service calls on Tokio's blocking pool and returns zeroizing, redacted leases only through explicit secret-aware adapters.

## Failure model

All mutations carry an idempotency key. Accepted asynchronous operations have stable operation IDs and terminal completion/failure/cancelled events. The server restores incomplete durable operations according to operation-specific recovery policy after restart. Clients hydrate a snapshot, apply strictly ordered events, detect gaps, and resnapshot when replay is no longer available.

Provider, MCP, browser, terminal, and Git failures are isolated from the server process and represented as provider-neutral error/activity events. Bounded output, cancellation, child cleanup, and backoff are mandatory.

Routine demonstration records only typed server actions. Editing appends an immutable version; Run now and dry run bind to that exact version and use the same sequential executor boundary. This keeps historical context reproducible and prevents desktop/Android from becoming independent automation engines.

## Decisions still requiring implementation evidence

- content-addressed artifact garbage collection policy
- supported compatibility-window duration after v1
- egui native chrome exceptions and golden rendering hosts
- the Android schema generation tool after the first complete Rust schema

Desktop styling is centralized in a semantic egui token layer. Deterministic visual fixtures use egui tessellation plus a platform-independent CPU renderer so Linux and macOS compare the same checked-in pixels. A passing HomeBot golden detects regressions but does not alone establish Grok Bot parity; reference comparison status is tracked separately.
