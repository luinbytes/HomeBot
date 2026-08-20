# Secret storage

Status: implemented server foundation for 6C7-51, 20 August 2026.

HomeBot keeps secret values in the host operating system's credential store. macOS uses Keychain. Linux uses a Secret Service-compatible implementation such as GNOME Keyring or KWallet. The Rust integration is `keyring` 3.6.3, retained because its Rust 1.75 minimum remains compatible with HomeBot's Rust 1.85 baseline; Linux uses the synchronous Secret Service backend with vendored DBus support, and calls run through `spawn_blocking`.

## Boundaries

- SQLite `secret_references` rows contain only owner ID, reference UUID, opaque `homebot:<uuid>` locator, label, and timestamps.
- Authenticated HTTP responses and durable events contain only safe metadata and `ready`, `locked`, `unavailable`, or `missing` state.
- Create/update values are deserialized directly into request objects that cannot be formatted with Rust `Debug` or serialized into generic event/idempotency payloads.
- `SecretInput` and `ResolvedSecret` redact diagnostics and zero their string allocation on drop.
- `VaultProviderSecretResolver` is the explicit bridge into a provider request. `SecretToolService` is the only general-tool bridge: it requires the server policy engine to authorize the exact `SecretUse` capability, opaque reference, operation context, and bounded purpose, with a single-use approval when policy requires it. Chat transcripts, routine context, ordinary tools, and provider configuration receive only a `SecretReference`.

## Lifecycle

`POST /api/v1/secrets` writes the value to the OS store before committing safe metadata. A metadata failure triggers best-effort credential deletion. Repeating the create idempotency UUID returns the existing reference without copying the submitted value into SQLite or an event.

`PUT /api/v1/secrets/{id}` can replace the value, rename the safe label, or do both. `DELETE` removes the OS credential before its metadata; an already-missing OS entry is treated as recoverable stale metadata and is cleaned up. All routes are authenticated and owner-scoped.

Schema migration 0008 adds ownership and update timestamps to the v7 opaque-reference table. Existing references remain associated with the local owner and report `missing`, `locked`, or `unavailable` until the OS store can resolve them. Migration never invents or imports a value.

## Headless Linux

A headless process may not have an unlocked Secret Service session. HomeBot reports `secret_store_locked` or `secret_store_unavailable` and continues serving non-secret capabilities. Configure and unlock a Secret Service-compatible daemon in the user session; HomeBot intentionally has no plaintext fallback.

## Verification

The secret crate tests redacted formatting, create/replace/delete, strict opaque locators, and locked/unavailable failure. Tool tests prove unmatched calls require exact single-use approval and only matching `SecretUse` rules bypass it. Storage tests cover owner isolation, duplicate labels, migration from the v7 table, and absence of value columns. Authenticated server tests place a canary through create/list/update/delete, prove it is absent from response bytes and message/activity/outbox JSON, and verify locked-store errors and post-delete removal.
