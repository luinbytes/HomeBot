# Android architecture

HomeBot Android is a native Kotlin and Jetpack Compose application. It is a client of the authoritative HomeBot server, never a second runtime. The application module lives in `android/app`; Rust-owned generated protocol bindings remain in `android/protocol` and are compiled directly into the app.

## Runtime and state

`HomeBotClient` uses OkHttp for version negotiation, pairing exchange, and the authenticated WebSocket event stream. A single FIFO event processor applies stream messages in arrival order. It rejects sequence gaps, ignores already-applied sequences, retains the last safe projection during a disconnect, and sends the last cursor when reconnecting. The server may replay events or require a fresh snapshot when that cursor has expired.

The connection state exposed as a Kotlin `StateFlow` covers:

- unpaired and one-time pairing exchange;
- connecting and protocol-version negotiation;
- snapshot hydration;
- live sequenced state;
- bounded reconnect with a resume cursor;
- incompatible client/server versions;
- revoked device sessions; and
- structured offline failures.

Compose observes only this projection and submits actions through the client. Durable Bots, chats, groups, messages, activity, approvals, routines, and workspace state remain server-owned.

## Credentials and endpoints

The persistent device-session credential is AES-256-GCM encrypted with a non-exportable Android Keystore key before storage. Logs and `toString` output redact it. DataStore contains only non-secret preferences such as the selected endpoint and device name. Room is intentionally absent in v1 groundwork because there is no offline-editing contract yet; adding a second cache now would create an unjustified source of truth.

All non-loopback endpoints require HTTPS, including LAN and Tailscale connections. Plain HTTP is limited by both client validation and Android's network-security policy to localhost and the Android emulator's host bridge. Pairing deep links carry a short-lived one-time credential, not the persistent device session.

## Verification

Deterministic MockWebServer tests cover pairing, credential redaction, version skew, revocation, snapshot hydration, cursor resume, stale-cursor snapshot fallback, event replay, and duplicate-sequence suppression. GitHub Actions runs Android lint, JVM unit tests, and a debug APK build, then publishes the APK as a CI artifact.

Feature work builds on this transport to render the server-owned Bot, chat, group, activity, approval, routine, plugin, device, provider, workspace, diff, and Git models. Background reconnect must respect Android execution limits and avoid permanent polling.
