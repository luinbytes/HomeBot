# Provider runtime research

Updated: 25 August 2026. This is a source-backed contract and gap report, not
evidence that HomeBot has passed live-provider acceptance.

## Bottom line

HomeBot has real provider adapters, not only a mock abstraction. The checked-in
tests prove normalization, supervision, and fake-process round trips. They do
not prove a supported account can complete a real turn, perform a real edit,
survive a client/server restart, or cancel provider-side work. The current
production path also has no HomeBot-owned provider login flow: it discovers a
CLI, calls its health command, and assumes the local CLI is already signed in.

The safe product interpretation is:

* Codex: use the locally installed `codex` App Server with its own ChatGPT
  account authentication; HomeBot must add a guided sign-in/status flow and a
  live acceptance lane.
* Claude Code: use the locally installed `claude` CLI with the user’s existing
  Claude.ai/Claude Code account. Do not capture or proxy Claude.ai credentials in
  HomeBot. Anthropic’s current product terms say third-party products should
  use API-key or supported cloud-provider authentication instead, unless
  Anthropic has approved the Claude.ai-login use case.
* OpenAI-compatible: treat Responses and Chat Completions as two explicit
  wire profiles. “Compatible” is not a provider-wide guarantee; each endpoint
  and event shape needs a live smoke test.
* Grok Bot is a useful acceptance bar for behavior (persistent named teammates,
  background work, visible handoffs, and group collaboration), not an API or
  implementation dependency.

## Evidence classes

“Implemented” below means the current source contains the behavior. “Fixture
proven” means only a test double or recorded payload exercised it. “Live
unproven” means no checked-in evidence demonstrates an installed,
authenticated provider. A green fixture test must not be reported as live
provider support.

## Codex CLI / App Server

### First-party contract

OpenAI’s [Codex App Server README](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
defines a bidirectional JSON-RPC protocol over newline-delimited JSON on stdio;
WebSocket is explicitly experimental/unsupported. A client must send exactly
one `initialize`, then `initialized`, before other methods. A normal turn is
`thread/start` or `thread/resume`, followed by `turn/start`, streamed item
notifications, and `turn/completed`; cancellation is `turn/interrupt`.

The same first-party contract documents ChatGPT-managed auth as the recommended
mode. `account/login/start` supports browser `chatgpt` and device-code
`chatgptDeviceCode`; Codex persists and refreshes those tokens. `account/read`
returns the auth mode/account and `account/updated` reports changes. The
[Codex CLI guide](https://learn.chatgpt.com/docs/codex/cli) says the normal path
is install Codex, run it in a project, and choose “Sign in with ChatGPT” on
first use.

The App Server contract also says:

* `thread/resume` reopens a stored thread and appends later turns;
* `thread/interrupt` completes with `status: "interrupted"`, but does not
  terminate background terminals;
* `item/*` notifications are the canonical incremental activity stream;
* experimental methods/fields require `capabilities.experimentalApi: true`;
* `collaborationMode` is currently supported for `turn/start`, while the older
  `multiAgentMode` is deprecated/ignored.

### Current HomeBot mapping

| Contract | Current source evidence | Classification / question |
|---|---|---|
| stdio App Server, initialization, request routing | `crates/homebot-providers/src/codex.rs:386-456` starts `codex app-server --listen stdio://`, performs `initialize`, then `initialized`; `:461-527` routes JSON-RPC | Implemented; only fake-process tests prove the round trip. Verify against the installed Codex version because this is a versioned, evolving protocol. |
| account health | `codex.rs:186-233` calls `account/read` and maps `requiresOpenaiAuth`/`account` to HomeBot health | Implemented; not a login flow. There is no HomeBot route/UI calling `account/login/start`, polling `account/login/completed`, or presenting browser/device-code instructions. |
| new/resumed turn, model discovery, streaming, approvals, interruption, compaction | `codex.rs:235-361`; `codex/protocol.rs:11-159` | Implemented and fixture-proven. Real provider, real approval, real edit, and real cancellation remain unproven. |
| capability declaration | `codex.rs:163-184` advertises resume, stream, activities, approvals, cancellation, usage, compaction, and plan mode | The declared surface is broader than the evidence. Gate it with a real Codex smoke test, especially approval and file-change behavior. |
| local auth state | `codex.rs:804-827` passes `HOME`, `CODEX_HOME`, XDG paths, and TLS paths while excluding API-key variables | Good secret boundary and compatible with Codex-managed local auth. Verify that the supervised process resolves the same `CODEX_HOME`/keyring as the user’s interactive CLI. |
| restart/recovery | `codex.rs:357-360` returns an empty `recover()` result | Partial/missing. HomeBot can resume a stored provider thread when it has a mapping, but it does not recover an interrupted in-flight operation after the server or App Server dies. |
| attachments/background terminals | `codex.rs:270-298` rejects metadata-only attachments; cancellation only calls `turn/interrupt` | Partial. Attachments need resolved local content. A cancellation acceptance test must also check for orphaned background terminal work and file mutations. |

The checked-in Codex test named `smoke_test_skips_explicitly_when_codex_is_not_installed`
(`crates/homebot-providers/src/codex/tests.rs:84-100`) only checks executable
resolution and health when a binary exists. The App Server tests around
`:102-258` and `homebot-server/src/provider_bootstrap.rs:367-470` execute shell
fixtures, not Codex. They are useful deterministic protocol tests and cannot
substitute for an authenticated ChatGPT Plus/Pro run.

### Concrete Codex verification

On a machine where the user has already completed the supported Codex login:

1. Record `account/read` showing `account.type = chatgpt` and a Plus/Pro plan
   (without logging or persisting tokens).
2. Start HomeBot’s real server composition root and send a Bot message. Capture
   `thread/started`, `turn/started`, content deltas, activities, usage, and
   `turn/completed` from the authenticated provider.
3. In a disposable repository, approve a write/command, verify the real diff,
   resume the same thread, then interrupt a long turn. Check that interrupted
   work has no late approval or unexpected file mutation.
4. Disconnect the desktop/Android client, reconnect and replay from durable
   HomeBot state; then restart the server and explicitly record the current
   limitation if the active operation cannot be recovered.
5. Run two independent Bots (and, if supported by the server, two profiles) in
   parallel and verify that operation IDs, streams, approvals, and transcripts
   do not cross.

## Claude Code

### First-party contract

The [Claude Code authentication guide](https://code.claude.com/docs/en/authentication)
supports Claude.ai Pro/Max/Team/Enterprise account login, Console login, and
cloud-provider credentials. It says credentials are stored in the macOS
Keychain or Linux `~/.claude/.credentials.json` (mode `0600`), and that
`claude auth status` returns JSON with exit code 0 when authenticated and 1
when not. The CLI reference documents `claude auth login`, `auth status`, and
`auth logout`.

The [CLI reference](https://code.claude.com/docs/en/cli-usage) documents the
non-interactive bridge HomeBot is attempting: `-p`,
`--input-format stream-json`, `--output-format stream-json`, `--verbose`,
`--include-partial-messages`, and `--replay-user-messages`. The stream begins
with a system `init`, emits incremental messages, and ends with a system
`result`; `--resume <session-id>` continues a saved session. For non-interactive
permission prompts, Anthropic documents `--permission-prompt-tool`; without a
configured prompt tool, a headless process cannot hand a permission question
back to HomeBot.

Anthropic’s [Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)
also says the SDK is Python/TypeScript and that another language should spawn
the CLI with `-p`. Its explicit legal boundary is important for a sellable
product: [Anthropic’s legal/compliance page](https://code.claude.com/docs/en/legal-and-compliance)
says OAuth is intended for purchasers’ ordinary use of Claude Code/native
Anthropic apps and that third-party developers should use API-key or supported
cloud-provider auth rather than offer Claude.ai login or route subscription
credentials on users’ behalf. HomeBot can therefore support a user’s local
CLI/account installation, but must not silently become a Claude.ai OAuth proxy.

### Current HomeBot mapping

| Contract | Current source evidence | Classification / question |
|---|---|---|
| headless stream | `crates/homebot-providers/src/claude.rs:95-188` launches `claude -p` with stream-json input/output, verbose partial messages, user replay, optional `--resume`, model, cwd, and plan mode | Implemented and fixture-proven. Verify flags against the installed CLI release and capture real `init`/result/session IDs. |
| auth health | `claude.rs:197-248` runs `claude auth status`, parses `loggedIn`/`authenticated`, and maps exit failure to authentication required | Implemented; live account status unproven. `docs/providers.md:68` is directionally correct but should not imply a completed account flow. |
| resume | `claude.rs:265-287` passes `--resume <conversation_id>` | Implemented; no live follow-up proof in checked-in tests. Session storage/search behavior should be tested from the same supervised `HOME`/`CLAUDE_CONFIG_DIR`. |
| approvals | `claude.rs:299-307` always returns an error from `resolve_approval`; `discover()` does not advertise `Approvals` | Explicitly missing for this adapter. A coding Bot that needs a permission prompt cannot currently expose a HomeBot approval and continue. Add a supported MCP permission-prompt bridge or keep the limitation visible. |
| compaction/recovery/attachments | `claude.rs:309-317` rejects manual compaction and returns no recovery; `:483-490` rejects metadata-only attachments | Partial/missing, not a complete Grok-style work surface. |
| credentials | `claude.rs:458-480` passes `HOME`, `CLAUDE_CONFIG_DIR`, XDG paths, and TLS paths but no API-key environment values | Compatible with local CLI account files and avoids accidental secret inheritance. Verify Keychain/Linux file access under the actual supervised process. Do not add a HomeBot Claude.ai token store without Anthropic approval. |

The Claude fixture tests (`crates/homebot-providers/src/claude/tests.rs:25-133`)
prove only an executable shell emitting expected JSONL, including cancellation.
They do not prove Claude account auth, a real tool call, an approval, a real
edit, or provider-side cancellation.

### Concrete Claude verification

1. On macOS and Linux, sign in through the installed CLI using the user’s
   supported account method; record only the redacted `auth status` result.
2. Send a normal question through HomeBot and verify live stream frames,
   durable final message, usage, session ID, and a follow-up using `--resume`.
3. Run a disposable-repository edit. Confirm whether the current noninteractive
   permission mode can complete it. If it pauses/fails, classify Claude coding
   execution as partial until HomeBot supplies a supported permission-prompt
   bridge or an explicit user-configured permission mode.
4. Test disconnect/reconnect and process failure separately. The adapter’s
   `recover()` is empty, so a server restart must not be reported as recovered
   merely because a later fresh session works.

## OpenAI-compatible APIs

### First-party wire expectations

OpenAI’s [Responses streaming reference](https://platform.openai.com/docs/api-reference/responses-streaming)
defines Server-Sent Events when `stream: true`, including
`response.created`, `response.output_text.delta`, and `response.completed`.
The Responses API can use `previous_response_id`, but the same documentation
notes that clients manually managing context may need to include prior items in
subsequent input. The [models endpoint](https://platform.openai.com/docs/api-reference/models/object)
is `GET /v1/models` with Bearer auth and a `{data:[{id:...}]}` response.

For Chat Completions, the [API reference](https://platform.openai.com/docs/api-reference/chat/object)
documents SSE streaming and `stream_options.include_usage`; the final usage
chunk is emitted before `[DONE]` but may be absent if the stream is interrupted.
These are OpenAI’s contracts, not a promise that every “OpenAI-compatible”
server implements the same endpoints, event names, continuation, tools, or
usage semantics.

### Current HomeBot mapping

| Contract | Current source evidence | Classification / question |
|---|---|---|
| secret and endpoint boundary | `crates/homebot-providers/src/openai_compatible.rs:44-87, 141-150` requires HTTPS except loopback, rejects URL credentials/query/fragment, resolves an opaque secret reference at request time, and sends Bearer auth | Implemented and unit-tested for the boundary. Live secret-store and endpoint verification remain unproven. |
| model discovery | `openai_compatible.rs:261-301` calls `models` and requires `data[].id` | Implemented for OpenAI-shaped servers. Treat a nonconforming model list as an explicit health/protocol failure, not compatibility. |
| Responses and Chat Completions | `openai_compatible.rs:153-232`; `openai_compatible/protocol.rs:11-113` | Implemented and fixture-proven for a narrow event subset. It does not prove any third-party gateway. |
| continuation | Responses sends `previous_response_id`; Chat Completions `resume()` returns `ConversationUnavailable` (`openai_compatible.rs:166-175`) | Partial by design. The docs claim transcript replay, but this adapter does not assemble/send HomeBot message history for Chat Completions. Keep that capability off until replay is real. |
| cancellation | `openai_compatible.rs:335-343, 366-443` stops consuming the SSE stream and emits `Cancelled` | Client-side cancellation only. There is no generic OpenAI-compatible server cancellation request; verify whether the provider stops generation or continues billable work. |
| activities, approvals, plan, compaction, recovery, attachments | `discover()` advertises only streaming/cancellation/usage (plus Responses resume/activities); `:303-363` rejects attachments/plan and has no approvals/compaction/recovery | Correctly exposed as unsupported, but still far from a full coding/team provider. |

The fixture tests (`openai_compatible/tests.rs:32-210`) verify a local Axum
server, SSE normalization, bearer injection, secret redaction, and stream
cancel. They do not establish compatibility with OpenAI, Ollama, LM Studio, or
any other live endpoint. Each configured profile needs a provider-specific
smoke check for `/models`, the selected generation path, event termination,
continuation, usage, and cancellation.

## Grok Bot acceptance bar

The current official [Grok Bot overview](https://docs.x.ai/grok-bot/overview)
defines a Bot as a persistent, named teammate with durable memory/files/
preferences. It is message-first, available across desktop and mobile, works
in the background on a persistent computer, and can independently collaborate
with other Bots. The official [message and collaboration guide](https://docs.x.ai/grok-bot/chat-and-collaboration)
says the transcript shows normal messages alongside tool activity, created
files, questions, and approval requests; a Bot can asynchronously hand work to
another Bot, which wakes and replies later; group chats make those handoffs
visible. The official [FAQ](https://docs.x.ai/grok-bot/faq) says closing the app
does not stop background work and multiple Bots run in parallel. The official
[Automations announcement](https://x.ai/news/grok-automations) says each run
opens a real conversation, saves a full run history, and can report by app
notification/email.

These are acceptance behaviors, not claims about HomeBot’s current state:

| Intended teammate behavior | HomeBot question to prove |
|---|---|
| Open Bot, type, send; same thread later on another client | Does the server persist the user message before dispatch and replay the same Bot/chat after reconnect? |
| Work continues with clients closed | Does server-owned execution remain alive after desktop/Android disconnect, and is the outcome durable and notification-addressable? |
| Multiple Bots work in parallel | Can independent operations run concurrently without shared provider/process state, cross-stream events, or serializing on one Bot? |
| Bot-to-Bot handoff is visible and asynchronous | Is each delegation a persisted HomeBot message/event with sender/recipient/ownership, depth/budget/cycle enforcement, and a receiving Bot execution? |
| Group coordination | Can a group chat show the handoff and final result without invisible prompt stuffing? |
| Human judgment boundary | Do approval/question events survive disconnect and resume exactly once, with a clear stop/cancel path? |
| Routine/background history | Does a routine create a real durable conversation/run that can be reopened and continued, rather than only a scheduler record? |

The official Grok material also warns that a Bot’s shared computer is not a
security boundary. HomeBot should preserve its stronger server-enforced
Bot/workspace/permission boundaries rather than copy that model, while still
matching the visible teammate behavior.

## Required live evidence before provider claims

The minimum useful acceptance record is three separate lanes:

1. **Fixture:** current deterministic tests for protocol normalization,
   supervision, limits, cancellation races, and secret redaction.
2. **Local integration:** HomeBot’s real server composition root, SQLite, HTTP/
   WebSocket clients, and a disposable repository, with a fake provider only
   where a deterministic failure/restart race is needed.
3. **Real provider:** installed/authenticated Codex and/or Claude CLI, or a
   real OpenAI-compatible endpoint, recording redacted health, streamed events,
   durable transcript, activity/approval state, follow-up resume, cancellation,
   client disconnect/reconnect, server restart, and at least two concurrent
   Bots.

Until lane 3 is captured, describe these adapters as “implemented, fixture-
verified, live-provider acceptance pending.” Do not use the existing fixture
tests or provider docs as proof of everyday usability.
