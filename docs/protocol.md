# Client/server protocol v1

Status: M0 contract. Wire schemas begin in `protocol/schema/` and are Rust-owned.

## Transport

- HTTP handles health/version, pairing exchange, snapshots, CRUD queries/commands, and resumable binary attachment upload.
- WebSocket carries authenticated live events, command lifecycle updates, streaming message parts, activities, approvals, and heartbeats.
- TLS/WSS is required outside loopback or an explicitly accepted private-network exception. Public-interface plaintext is rejected by default.

## Negotiation

The client opens with `hello(protocol_version, client_version, resume_after)`. The server accepts only versions inside its declared inclusive compatibility range. Unsupported clients receive the structured `protocol_version_unsupported` error before closure. A server may support multiple protocol versions, but never reinterpret an existing field incompatibly.

## Snapshot and resume

1. Authenticate the device session.
2. Negotiate protocol compatibility.
3. If `resume_after` remains inside the retained outbox window, replay every event with a greater sequence in order.
4. Otherwise return a complete authorised snapshot and its boundary sequence.
5. Continue with incremental events whose sequences are strictly greater than the boundary.

Sequence numbers are server-monotonic for the authenticated owner stream. Clients ignore a repeated event ID, reject an unexplained gap, and request a fresh snapshot. Reconnect never relies on message text comparison.

## Mutations and idempotency

Every mutation has a client-generated request ID and idempotency key. Repeating the same key and same canonical command returns the prior operation/result. Reusing a key for a different command is a conflict. Asynchronous commands emit accepted, then exactly one terminal completed, failed, or cancelled state. Cancellation is itself idempotent and race-safe with normal completion.

## Errors

Errors contain a stable machine code, safe user-facing message, retryable flag, and request ID where relevant. Debug details remain in redacted local diagnostics and never expose provider secrets, pairing tokens, raw environment variables, or filesystem data outside the requesting capability scope.

Required common codes include `unauthenticated`, `forbidden`, `approval_required`, `not_found`, `conflict`, `validation_failed`, `rate_limited`, `provider_unavailable`, `operation_cancelled`, `resume_unavailable`, `protocol_version_unsupported`, and `internal`.

## Attachments

Attachments are uploaded over authenticated HTTP with size/type limits, streaming digest verification, an idempotency key, and an explicit finalise step. WebSocket messages refer to completed attachment IDs. Partial uploads expire and cannot be consumed.

## Heartbeats and backpressure

The server sends nonce-bearing pings. A client responds with pong before the advertised timeout. Slow clients have a bounded queue; the server closes them with a resumable cursor instead of allowing unbounded memory growth. Streamed content can be coalesced only when doing so preserves the final canonical message parts.

## Compatibility rules

- Additive optional fields are permitted within a protocol version.
- Changing meaning, type, required status, ordering guarantees, or security semantics requires a new version.
- Unknown event kinds are ignored only if the negotiated version marks them ignorable; otherwise the client resnapshots or fails closed.
- Android models must be generated from or mechanically checked against the committed schema in CI.
- Golden fixtures cover every envelope, malformed input, unknown fields, skew, reconnect, duplicate mutation, cancellation race, and slow-client path.
