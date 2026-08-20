# Bots

Bots are durable HomeBot identities. Their name, title, description, color, shape, permissions, unread state and attention state belong to HomeBot and survive server and client restarts. A provider profile is an advanced setting, not the Bot's identity.

## Lifecycle and validation

Names are trimmed, required, limited to 48 Unicode characters and unique per owner without case sensitivity. Titles are optional and limited to 80 characters. Descriptions are optional and limited to 2,000 characters. Identity fields reject control characters.

Archive is reversible and never deletes conversations or provider mappings. The normal roster excludes archived Bots, while the server's lifecycle endpoint and desktop archived view can restore them. Archive and restore reject invalid transitions.

New Bots use a rounded-square violet identity and ask-before-changes permissions unless the client explicitly selects another valid value. Permission enforcement remains server-side.

## Server contract

Authenticated operations are:

- GET /api/v1/bots
- POST /api/v1/bots
- PUT /api/v1/bots/{bot_id}
- POST /api/v1/bots/{bot_id}/archive
- POST /api/v1/bots/{bot_id}/restore
- POST /api/v1/bots/{bot_id}/read

Every mutation carries a request ID and idempotency key. Duplicate names return conflict; invalid fields return validation-failed; owner-scoped misses return not-found. Changes enter the durable event stream as bot-changed, and reconnect snapshots include the complete active and archived roster.

Provider health is normalized as not-configured, ready, or unavailable. Clients never need provider-specific status payloads.

## Desktop model

The desktop roster applies snapshots and incremental Bot-changed events, preserves selection when possible, sorts identities consistently, and hides archived Bots unless requested. The editor validates obvious field and duplicate-name errors before submission, while the server remains authoritative.

Visual fixtures cover the empty roster, editor, disconnected server, unavailable provider, unread indicator and approval attention states. Provider profile and permission controls remain collapsed under Advanced settings.
