# Providers

`ProviderAdapter` normalises discovery, health, authentication state, model/capability discovery, conversation start/resume, streamed content, activities, approvals, cancellation, usage, compaction, and failures. Provider-native payloads must terminate inside the adapter crate.

Initial adapters are Codex App Server over its structured protocol, Claude Code over a supported structured CLI/SDK surface, an OpenAI-compatible HTTP adapter using an OS-backed secret reference, and a constrained generic process adapter. A Bot references a provider profile; provider conversation mappings belong to a chat/profile pair and never define Bot identity.

Codex integration follows the official [App Server contract](https://learn.chatgpt.com/docs/app-server). WebSocket app-server transport is currently documented as experimental, so HomeBot should prefer the stable local stdio JSONL transport until implementation evidence changes that decision.
