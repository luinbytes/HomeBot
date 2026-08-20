# Direct chats

Direct chats are server-owned conversations between the user and one durable Bot. A Bot has at most one direct chat per owner. Chat identity and HomeBot transcript history do not depend on a provider conversation ID.

## Durable timeline

Migration 0005 adds owner-scoped direct chat state, rich message parts, reply and mention metadata, normalized error data, execution activity fields, approval presentation fields and ordered queued prompts. Text and attachment parts retain stable IDs and ordinals across restart.

The server exposes:

- POST /api/v1/chats/direct
- GET /api/v1/chats/{chat_id}/timeline
- POST /api/v1/chats/{chat_id}/messages
- POST /api/v1/chats/{chat_id}/steer
- POST /api/v1/chats/{chat_id}/stop
- POST /api/v1/chats/{chat_id}/read
- POST /api/v1/chats/{chat_id}/messages/{message_id}/retry
- POST /api/v1/approvals/{approval_id}/decision

Every mutation is authenticated, owner-scoped and idempotent. A normal message submitted while the chat is idle becomes a durable user message. A normal message submitted while the Bot is working becomes an ordered queued prompt. Steering explicitly appends an immediate user message only while work is active. Stop clears authoritative running state. Approval decisions are accepted exactly once and the server rejects attempts to change an already decided approval.

The timeline response is an authorized snapshot of messages, rich parts, activities, approvals and queued prompts at an outbox boundary. Incremental WebSocket events use normalized Chat-changed, Message-changed, Message-delta, Activity-changed, Approval-changed and Queued-prompt-changed bodies.

## Desktop reconciliation

The desktop timeline model hydrates from the HTTP timeline and then applies strictly sequenced events. It deduplicates event IDs and old sequences, requests a new snapshot on gaps and never appends the same streamed delta twice. Full Message-changed events replace canonical state.

The composer carries attachments, replies and Bot mentions. It chooses send, queue or steer based on server running state and exposes stop, retry and approval decisions as commands for the transport client. Scroll anchoring follows new content only when the user is already at the bottom; otherwise it counts unseen updates and offers a jump-to-latest action.

Provider-specific tool names and payloads do not enter this contract. Activities use a safe user-facing title, detail, normalized status and attention flag.

## Provider turns

When a Bot has a provider profile, the server resolves its normalized adapter and starts or resumes the provider conversation after persisting the user message. One stable streaming Bot message receives durable text deltas. Normalized provider activities and approvals are persisted before their WebSocket events are published. Completion, cancellation and normalized failures produce exactly one terminal message state and clear the authoritative chat-running flag.

Provider conversation IDs remain a mapping of Bot, chat and provider profile. They never replace HomeBot Bot, chat or message identity. Stop is routed by the server to the adapter that owns the active operation. Approval decisions are routed to that adapter before the durable approval is marked decided.
