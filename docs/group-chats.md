# Group chats and coordination

Group chats are durable, server-owned conversations with at least three Bots. The server persists participants, one explicit current owner, per-Bot execution state, a maximum parallel-Bot limit and a finite coordination-turn budget. These constraints remain authoritative after every client disconnect and server restart.

## Coordination policy

Each group starts with a bounded turn budget between 1 and 64 and a parallel limit between 1 and 8. The server atomically consumes the budget before a Bot-to-Bot coordination turn begins. Once the budget is exhausted or stop is requested, further turns fail closed. Stop also transitions every participant to the visible stopped state and clears active operation IDs.

This budget prevents open-ended Bot ping-pong. Clients may present and lower limits, but cannot bypass server enforcement.

## Context and ownership

Bot-authored group messages carry ordinary mentions plus explicit shared-context message IDs. Storage accepts those references only when every referenced message belongs to the same group. Provider-native conversation IDs do not serve as shared-context identity.

Exactly one participant has the owner role. A handoff validates the current owner, target participant, optional source message and reason in one transaction, then changes participant roles and records immutable handoff history. This makes ownership visible and recoverable rather than an inference from the latest message.

## Parallel state

Every participant has a durable normalized state: idle, running, waiting, completed, failed or stopped. A running state may include an active operation ID. The server rejects transitions that would exceed the group's maximum parallel-Bot count.
