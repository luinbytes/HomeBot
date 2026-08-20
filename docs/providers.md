# Providers

`ProviderAdapter` normalises discovery, health, authentication state, model/capability discovery, conversation start/resume, streamed content, activities, approvals, cancellation, usage, compaction, and failures. Provider-native payloads must terminate inside the adapter crate.

## Runtime contract

Every adapter has a stable lowercase ID and returns provider-neutral descriptors, availability/authentication state, models, and a capability set. Starting or resuming work uses a HomeBot-owned operation ID and returns a bounded receiver of normalized events: conversation identity, content deltas, activities, approval requests, usage, compaction, and exactly one terminal completed/cancelled/failed state.

`ProviderRuntime` rejects duplicate adapter and active-operation IDs, verifies that adapters preserve the requested operation ID, and remembers which adapter owns cancellation. Approval decisions are routed back through the adapter as `allow_once`, `allow_for_session`, `deny`, or `cancel`; provider-native decision payloads never enter the server API. The server calls `finish` only after the terminal provider event is durable. Recovery asks every adapter for interrupted operation IDs; provider-native recovery tokens remain inside the adapter and provider-conversation mapping layers.

## Process supervision

Structured CLI adapters launch through `SupervisedProcess`. It clears the inherited environment, passes only adapter-selected variables, pipes stdin/stdout/stderr, and kills the child if the supervisor is dropped. Adapters consume structured stdout inside this crate. Stderr is kept as a byte-bounded tail, explicit secret values are redacted before retention, and reports use a diagnostic ID plus normalized exit classification.

Shutdown first closes stdin so a well-behaved provider can exit cleanly. If it exceeds the configured grace period, HomeBot kills and reaps it. Nonzero or signal exits become crash reports rather than panics or provider-native client payloads.

Initial adapters are Codex App Server over its structured protocol, Claude Code over a supported structured CLI/SDK surface, an OpenAI-compatible HTTP adapter using an OS-backed secret reference, and a constrained generic process adapter. A Bot references a provider profile; provider conversation mappings belong to a chat/profile pair and never define Bot identity.

## Codex CLI

Codex integration follows the official [App Server contract](https://learn.chatgpt.com/docs/app-server). HomeBot launches `codex app-server --listen stdio://`, performs the required `initialize`/`initialized` handshake, and uses structured requests for account health, model discovery, thread start/resume, turns, interruption, and compaction. WebSocket app-server transport is currently documented as experimental, so HomeBot uses local stdio JSONL.

App Server notifications become provider-neutral content deltas, activities, usage, compaction, and terminal events. Command and file-change server requests become HomeBot approvals, then map back to App Server decisions only after the server resolves them. Native error details are classified and redacted at the adapter boundary.

Each `CodexAdapter` owns one `CodexProfile` with a stable adapter ID, explicit binary path, selected safe environment, and optional working directory. Registering more than one instance supports independent Codex accounts without binding Bot identity to a provider profile. The default environment allowlist includes discovery and Codex configuration paths but deliberately excludes API-key variables.

Fixture tests exercise JSONL normalization and a complete fake App Server round trip including approval. A real-binary smoke test reports an explicit skip when `codex` is absent; it never pretends that provider messaging was verified without an installed and authenticated CLI.
