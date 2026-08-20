# Providers

`ProviderAdapter` normalises discovery, health, authentication state, model/capability discovery, conversation start/resume, streamed content, activities, approvals, cancellation, usage, compaction, and failures. Provider-native payloads must terminate inside the adapter crate.

## Runtime contract

Every adapter has a stable lowercase ID and returns provider-neutral descriptors, availability/authentication state, models, and a capability set. Starting or resuming work uses a HomeBot-owned operation ID and returns a bounded receiver of normalized events: conversation identity, content deltas, activities, approval requests, usage, compaction, and exactly one terminal completed/cancelled/failed state.

`ProviderRuntime` rejects duplicate adapter and active-operation IDs, verifies that adapters preserve the requested operation ID, and remembers which adapter owns cancellation. The server calls `finish` only after the terminal provider event is durable. Recovery asks every adapter for interrupted operation IDs; provider-native recovery tokens remain inside the adapter and provider-conversation mapping layers.

## Process supervision

Structured CLI adapters launch through `SupervisedProcess`. It clears the inherited environment, passes only adapter-selected variables, pipes stdin/stdout/stderr, and kills the child if the supervisor is dropped. Adapters consume structured stdout inside this crate. Stderr is kept as a byte-bounded tail, explicit secret values are redacted before retention, and reports use a diagnostic ID plus normalized exit classification.

Shutdown first closes stdin so a well-behaved provider can exit cleanly. If it exceeds the configured grace period, HomeBot kills and reaps it. Nonzero or signal exits become crash reports rather than panics or provider-native client payloads.

Initial adapters are Codex App Server over its structured protocol, Claude Code over a supported structured CLI/SDK surface, an OpenAI-compatible HTTP adapter using an OS-backed secret reference, and a constrained generic process adapter. A Bot references a provider profile; provider conversation mappings belong to a chat/profile pair and never define Bot identity.

Codex integration follows the official [App Server contract](https://learn.chatgpt.com/docs/app-server). WebSocket app-server transport is currently documented as experimental, so HomeBot should prefer the stable local stdio JSONL transport until implementation evidence changes that decision.
