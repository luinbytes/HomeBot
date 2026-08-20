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
- POST /api/v1/approvals/{approval_id}/decision

Every mutation is authenticated, owner-scoped and idempotent. A normal message submitted while the chat is idle becomes a durable user message. A normal message submitted while the Bot is working becomes an ordered queued prompt. Steering explicitly appends an immediate user message only while work is active. Stop clears authoritative running state. Approval decisions are accepted exactly once and the server rejects attempts to change an already decided approval.

The timeline response is an authorized snapshot of messages, rich parts, activities, approvals and queued prompts at an outbox boundary. Incremental WebSocket events use normalized Chat-changed, Message-changed, Message-delta, Activity-changed, Approval-changed and Queued-prompt-changed bodies.

## Desktop reconciliation

The desktop timeline model hydrates from the HTTP timeline and then applies strictly sequenced events. It deduplicates event IDs and old sequences, requests a new snapshot on gaps and never appends the same streamed delta twice. Full Message-changed events replace canonical state.

The composer carries attachments, replies and Bot mentions. It chooses send, queue or steer based on server running state and exposes stop, retry and approval decisions as commands for the transport client. Scroll anchoring follows new content only when the user is already at the bottom; otherwise it counts unseen updates and offers a jump-to-latest action.

Provider-specific tool names and payloads do not enter this contract. Activities use a safe user-facing title, detail, normalized status and attention flag.
