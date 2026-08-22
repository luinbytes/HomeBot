# Providers

`ProviderAdapter` normalises discovery, health, authentication state, model/capability discovery, conversation start/resume, streamed content, activities, approvals, cancellation, usage, compaction, and failures. Provider-native payloads must terminate inside the adapter crate.

## Runtime contract

Every adapter has a stable lowercase ID and returns provider-neutral descriptors, availability/authentication state, models, and a capability set. Starting or resuming work uses a HomeBot-owned operation ID and returns a bounded receiver of normalized events: conversation identity, content deltas, activities, approval requests, usage, compaction, and exactly one terminal completed/cancelled/failed state.

The server persists each chat's `default`/`plan` choice independently from the provider conversation and sends it on later turns only when supported. Native compaction keeps the mapping; reset removes only that mapping. In both cases the HomeBot transcript remains app-owned, visible and excluded from a fresh provider start unless the user explicitly supplies context again. See [working-context.md](working-context.md).

`ProviderRuntime` rejects duplicate adapter and active-operation IDs, verifies that adapters preserve the requested operation ID, and remembers which adapter owns cancellation. Approval decisions are routed back through the adapter as `allow_once`, `allow_for_session`, `deny`, or `cancel`; provider-native decision payloads never enter the server API. The server calls `finish` only after the terminal provider event is durable. Recovery asks every adapter for interrupted operation IDs; provider-native recovery tokens remain inside the adapter and provider-conversation mapping layers.

## Production configuration

The production server—not either client—constructs the provider registry. With no explicit configuration it creates stable Codex CLI and Claude Code profiles, searches the server's allowlisted `PATH`, persists only safe profile metadata, and reports `not_installed` or `authentication_required` instead of substituting a fixture. Desktop local mode invokes this same composition root when it supervises the bundled server; remote desktop and Android clients consume the registry owned by their configured server. All clients receive only safe profile IDs, capabilities, and health in the authenticated snapshot; executable paths, environment values, and secret references remain server-only.

Set `HOMEBOT_PROVIDER_CONFIG` to a JSON file to replace those defaults with multiple named profiles. The file is bounded to 1 MiB, rejects unknown fields, and never accepts credential values. Example:

```json
{
  "profiles": [
    {
      "id": "ecb670a1-ad4b-46b3-827a-60f2ed88cb92",
      "adapter_id": "codex-work",
      "kind": "codex",
      "display_name": "Codex · Work",
      "binary": "codex",
      "model": null
    },
    {
      "id": "c151dc0e-d57c-47f9-ae6f-64490258a79a",
      "adapter_id": "private-api",
      "kind": "open-ai-compatible",
      "display_name": "Private API",
      "model": "model-name",
      "base_url": "https://models.example.com/v1",
      "api_style": "responses",
      "secret_reference_id": "09f43a83-a11b-49c7-b97f-013615af2bd2"
    }
  ]
}
```

Kinds are `codex`, `claude-code`, `open-ai-compatible`, and the opt-in `generic-process` JSONL bridge. OpenAI-compatible profiles require `api_style` (`responses` or `chat-completions`) and an opaque secret reference previously created through HomeBot's secret API. Remote URLs must use HTTPS; loopback HTTP remains available for local model servers. A malformed profile aborts startup with a specific configuration error. It is never silently ignored.

## Process supervision

Structured CLI adapters launch through `SupervisedProcess`. It clears the inherited environment, passes only adapter-selected variables, pipes stdin/stdout/stderr, and kills the child if the supervisor is dropped. Adapters consume structured stdout inside this crate. Stderr is kept as a byte-bounded tail, explicit secret values are redacted before retention, and reports use a diagnostic ID plus normalized exit classification.

Shutdown first closes stdin so a well-behaved provider can exit cleanly. If it exceeds the configured grace period, HomeBot kills and reaps it. Nonzero or signal exits become crash reports rather than panics or provider-native client payloads.

Initial adapters are Codex App Server over its structured protocol, Claude Code over a supported structured CLI/SDK surface, an OpenAI-compatible HTTP adapter using an OS-backed secret reference, and a constrained generic process adapter. A Bot references a provider profile; provider conversation mappings belong to a chat/profile pair and never define Bot identity.

## Codex CLI

Codex integration follows the official [App Server contract](https://learn.chatgpt.com/docs/app-server). HomeBot launches `codex app-server --listen stdio://`, performs the required `initialize`/`initialized` handshake, and uses structured requests for account health, model discovery, thread start/resume, turns, interruption, and compaction. WebSocket app-server transport is currently documented as experimental, so HomeBot uses local stdio JSONL.

App Server notifications become provider-neutral content deltas, activities, usage, compaction, and terminal events. Command and file-change server requests become HomeBot approvals, then map back to App Server decisions only after the server resolves them. Native error details are classified and redacted at the adapter boundary.

Each `CodexAdapter` owns one `CodexProfile` with a stable adapter ID, explicit binary path, selected safe environment, and optional working directory. Registering more than one instance supports independent Codex accounts without binding Bot identity to a provider profile. The default environment allowlist includes discovery and Codex configuration paths but deliberately excludes API-key variables.

Fixture tests exercise JSONL normalization and a complete fake App Server round trip including approval. A real-binary smoke test reports an explicit skip when `codex` is absent; it never pretends that provider messaging was verified without an installed and authenticated CLI.

## Claude

The Claude adapter follows Anthropic's documented non-interactive Agent SDK/CLI bridge. It launches `claude -p` with `stream-json` input and output, verbose partial messages, and user-message replay. Prompts enter through stdin rather than process arguments. `system/init`, partial message events, tool blocks, API retry notices, usage and result records are normalized into HomeBot conversation, content, activity, usage and terminal events. Resume uses the documented `--resume` session identifier, plan execution uses `--permission-mode plan`, and cancellation closes stdin before the supervisor's bounded shutdown.

Profiles select an explicit executable, working directory and allowlisted environment. Health uses the structured `claude auth status` command. The built-in picker aliases are `sonnet`, `opus`, `haiku` and `fable`; callers may still request a full model ID. A real CLI smoke test skips explicitly when Claude is absent, while executable fixtures verify streaming and cancellation.

## OpenAI-compatible BYOK

An OpenAI-compatible profile contains an endpoint, API style, model and opaque `SecretReference`. It never contains the credential value. `VaultProviderSecretResolver` resolves that reference from macOS Keychain or Linux Secret Service only immediately before an HTTP request; `ResolvedSecret` redacts debug output and zeroes its allocation on drop. SQLite and provider configuration therefore persist only the opaque reference. Locked or unavailable credential stores fail closed as authentication unavailable and never fall back to a file or environment value.

Responses API profiles support streamed text, reasoning/tool activity, usage, cancellation and `previous_response_id` continuation. Chat Completions profiles support streamed text, usage and cancellation; they intentionally reject provider-native resume until the server transcript-replay layer supplies full message history. Model discovery uses `GET /models`. Remote endpoints require HTTPS, while explicit loopback endpoints may use HTTP for local Ollama, LM Studio and similar servers. Redirects are disabled so bearer credentials cannot be forwarded unexpectedly.

## Community process contract

`GenericProcessAdapter` is an opt-in JSONL bridge, not a shell command template. HomeBot resolves a simple command name only against its selected, allowlisted executable search path, then launches the executable directly with a cleared environment, explicit arguments and selected environment variables. Absolute executable paths remain supported. It never evaluates a shell or login profile. The adapter writes exactly one request object to stdin:

```json
{"kind":"start","operation_id":"...","bot_id":"...","chat_id":"...","prompt":"...","model":null,"mode":"normal","attachments":[]}
```

Resume uses `kind: "resume"` and includes `conversation_id`. The child writes one serialized `ProviderEvent` per stdout line and must finish with exactly one `completed`, `cancelled` or `failed` event. Lines and event queues are bounded; malformed output fails closed; cancellation closes stdin and then enforces the supervisor deadline. Arguments and environment values are omitted from debug output. Community adapters that need a richer native protocol should implement `ProviderAdapter` directly instead of adding provider-specific fields to this contract.
## Skill assembly

HomeBot resolves Bot-assigned and turn-selected Skills before invoking an adapter. The server deterministically assembles the immutable applied versions into a provider-neutral delimited instruction block; adapters receive only the resulting prompt. Skill tool references do not alter adapter capabilities, plugin assignments, or approval decisions. Historical retry loads the original message's exact Skill versions rather than the current library version.
