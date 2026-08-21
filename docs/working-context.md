# Queued work and provider context

HomeBot owns the durable chat transcript, queued prompts and user-selected interaction mode. A provider conversation is only replaceable working context; it is not the Bot, chat, or archive.

## Queued turns

Messages submitted while a direct-chat Bot is running become ordered SQLite records with their exact attachment and Skill-version bindings. Ordinary follow-ups retain FIFO order. Explicit steering prompts are marked separately and retain FIFO priority ahead of ordinary follow-ups, without unsafely interrupting the in-flight provider operation. After a successful turn, the server atomically promotes the oldest queued prompt into visible transcript history, reserves the chat, and starts the next provider turn. Queue reordering, removal, promoted message and chat state are sequenced events, so reconnecting clients neither duplicate nor lose a turn. Stop or provider failure leaves remaining queued work visible in the same order instead of silently executing it.

## Interaction mode

`default` and `plan` are provider-neutral server state. Plan mode is accepted only when the configured adapter advertises `plan_mode`; the selected mode is sent on every later start/resume request. Unsupported modes fail with a structured validation response. Desktop and Android consume the same Rust-owned protocol model.

## Compact and reset

Native `compact` is available only for adapters advertising `compaction` and only while the Bot is idle. `reset` removes the provider conversation mapping and is available for every configured provider. Both operations are authenticated and idempotent, emit running/completed/failed context state, advance a durable generation on success, and clear stale usage.

SQLite atomically admits only one context operation per chat. Concurrent requests receive a structured conflict, and an operation interrupted by server restart is recovered as failed instead of remaining permanently busy.

Neither operation deletes HomeBot messages, Bot identity, attachments, Skills, checkpoints or app-owned memory. After reset, the next provider start receives only the new prompt plus its explicitly selected Skill context; archived messages remain visible but are not silently re-injected.

Usage and model context-window values are exposed when an adapter reports them. Missing values remain `null` rather than being guessed. Provider errors are normalized and never include native credential output.

## Verification

The server integration fixture runs three ordered queued turns in plan mode, checks usage projection, compacts once under duplicate retry, resets, starts a fresh provider conversation with only the new prompt, and reopens SQLite while verifying all transcript messages and context generation remain durable. Separate fixtures prove unsupported plan/native compaction fail closed and reset remains available.
