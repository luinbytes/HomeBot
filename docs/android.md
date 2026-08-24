# Android architecture

HomeBot Android is a native Kotlin and Jetpack Compose application. It is a client of the authoritative HomeBot server, never a second runtime. The application module lives in `android/app`; Rust-owned generated protocol bindings remain in `android/protocol` and are compiled directly into the app.

Compose uses native roles, selected-state semantics, headings and live regions; informational cards are not exposed as no-op controls, and typography uses scalable `sp` units. Cross-platform budgets and the TalkBack release checklist are defined in [performance-accessibility.md](performance-accessibility.md).

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

## Bot, chat and coding surfaces

The native product shell adapts the desktop hierarchy to a phone-sized conversation index. Pinned Bots appear as large contacts above a recency-sorted conversation list, while unpinned Bots and groups remain in the list below. The shell uses the desktop semantic palette and identity colors with Android-native touch targets, system light/dark appearance, back behavior and notification settings. Bot create/edit/pin/hide/review-hidden/duplicate/archive/restore and exact-name-confirmed delete, direct-chat creation, timeline reads, messages, queued follow-ups, steering, stop, retry, approvals, unread clearing, group mentions and ownership handoff all call authenticated versioned server endpoints. Less common Bot actions live in a compact overflow menu so the roster remains usable at phone widths. Incremental stream events advance the cursor and trigger a timeline refresh; Android does not manufacture assistant messages or mutate durable product state locally.

The Android document picker feeds the three-stage authenticated attachment contract: create metadata, upload bounded bytes, then finalize by SHA-256 before attaching the server identifier to a message. Coding chats expose repository registration and isolated attachment, normalized VCS status, working and staged diffs, commits, branch creation, approval-gated pushes, pull-request inspection and creation, detach, exact checkpoint comparison and server-owned safe restore. Remote source paths never become Android-local filesystem authority.

The settings and automations surface fetches routine definitions, run history and triggers. It supports authoritative create/edit/duplicate/delete, dry run, Run now, enable/disable and explicit one-shot scheduling. A mobile demonstration recorder starts a durable server recording, appends structured Bot-prompt actions and explicitly finishes to an editable disabled draft or cancels without creating a routine. Every successful mutation refreshes the server-owned routine projection and selected history in the same coroutine instead of manufacturing local routine state. Skills assignment and plugin/MCP health and enablement use the same authenticated mutation contract. Secret rows contain only label and availability status—values are never returned or rendered. A paired device can inspect and revoke only its own session; owner-wide session listing and revocation remain forbidden to paired-device credentials.

Assistant Packs appear in the same surface. Users choose a pack, Bot, IANA timezone, and local time; Android sends one authenticated install mutation and then refreshes the server-owned Skills and routines. The client does not assemble or persist marketplace instructions itself.

## Notifications and background behavior

While the app process is alive, the authenticated WebSocket remains the only live notification source. Terminal Bot messages, pending approvals, terminal routine runs and attention-required activities are mapped from sequenced events into Android notification channels. Notification intents use exact `homebot://chat/<id>?activity=<id>` or `homebot://routine/<id>?run=<id>` targets; reopening hydrates the authoritative timeline before highlighting the target.

Android connectivity callbacks nudge the existing bounded reconnect state machine when a usable network returns. HomeBot does not permanently poll, schedule periodic background work, or claim an always-on foreground service. If Android stops the process, live delivery pauses until the user reopens HomeBot; a future optional push relay can extend public-internet delivery without weakening the self-hosted server contract.

## Credentials and endpoints

The persistent device-session credential is AES-256-GCM encrypted with a non-exportable Android Keystore key before storage. Logs and `toString` output redact it. DataStore contains only non-secret preferences such as the selected endpoint and device name. Room is intentionally absent in v1 groundwork because there is no offline-editing contract yet; adding a second cache now would create an unjustified source of truth.

All non-loopback endpoints require HTTPS, including LAN and Tailscale connections. Plain HTTP is limited by both client validation and Android's network-security policy to localhost and the Android emulator's host bridge. Pairing deep links carry a short-lived one-time credential, not the persistent device session.

## Verification

Deterministic MockWebServer tests cover pairing, credential redaction, version skew, revocation, snapshot hydration, cursor resume, stale-cursor snapshot fallback, event replay, duplicate-sequence suppression, authenticated product mutations, the complete routine editor/recorder mutation route set, typed queued steering, and attachment create/upload/finalize. GitHub Actions runs Android lint and JVM unit tests, assembles both debug and minified release variants, and exercises the release packaging contract with an ephemeral CI-only signing key. The resulting debug and `ci-ephemeral` artifacts prove the build and verification pipeline; neither is a public release candidate or the production signing identity.

Feature work builds on this transport to render the server-owned Bot, chat, group, activity, approval, routine, plugin, device, provider, workspace, diff, and Git models. Background reconnect must respect Android execution limits and avoid permanent polling.
