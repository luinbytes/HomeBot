# Security and capability threat model

Status: M0 security contract. HomeBot assumes model output, repository content, websites, attachments, provider events, plugins, and MCP servers are untrusted.

## Dependency audit exceptions

`RUSTSEC-2023-0071` is ignored only in the GitHub cargo-audit lockfile scan because Cargo locks SQLx's optional MySQL/RSA graph even though HomeBot builds SQLx with default features disabled and SQLite alone. CI separately fails if `rsa` appears in the selected dependency tree. The exception must be removed if SQLx changes its lock graph or HomeBot selects a feature that makes RSA reachable.

## Protected assets

User files and repositories, browser sessions, credentials, provider accounts, device sessions, chat history, routine authority, source-control remotes, local processes, and availability of the host.

## Trust boundaries

The authenticated server boundary is the only authority boundary. Desktop and Android are untrusted requesters after authentication and cannot grant themselves a capability. Provider adapters, MCP servers, browser pages, repositories, and tool output cannot turn content into approval. OS credential stores protect secret values; SQLite contains opaque references.

## Capability policy

Capabilities are evaluated server-side against authenticated owner/device, Bot, chat, workspace, operation class, canonical target, and current policy version. Minimum classes are filesystem read/write, process execution, browser observe/act, Git read/write/remote, plugin read/write, external communication/mutation, secret use, and device administration.

Policy modes are deny, require approval, and allow. Deny wins over allow. High-risk operations such as deletion, credential access, external sending/publishing, payments, permission changes, remote Git mutation, arbitrary execution, and public-network exposure require a structured target-and-effect approval unless an equally specific persistent rule authorises them. Approval IDs are single-use, scoped, expiring, and bound to the canonical operation digest.

Custom rules are durable SQLite metadata (never secret material), reload into the server policy engine after restart, and can be narrowed by paired device, Bot, chat, workspace, capability, and action prefix. Only the authenticated owner may create, update, delete, or read the immutable rule audit log; paired devices receive the current safe projection for monitoring but cannot administer policy. Every mutation appends a safe before-deletion/current-state snapshot to the audit history, and idempotent replay does not duplicate that history.

The implemented `PolicyEngine` defaults unmatched requests to approval and evaluates authenticated owner/device, Bot, chat, workspace, capability and action scopes. Its private authorization proof cannot be supplied by a client. Approval records bind the full canonical request digest, expire, are consumed once, and become invalid after any policy revision. Filesystem write digests include the proposed content; terminal digests include executable, arguments, working directory and filtered environment; browser digests include the complete action. This prevents payload substitution after approval.

## Abuse cases and required mitigations

| Threat | Required control | Negative verification |
| --- | --- | --- |
| Credential exfiltration | OS-backed secret store, explicit secret-aware tool, redaction, no generic model/log access | Ordinary tool/provider context cannot read a stored secret |
| Malicious prompt or tool output | Treat content as data, capability checks after model decision, structured approval | Text saying “approved” never satisfies an approval |
| Exposed LAN/public listener | Loopback default, explicit remote config, TLS requirement, startup warning, auth and rate limits | Fresh config refuses non-loopback bind |
| Stolen pairing link | Short expiry, single use, hashed token, endpoint binding, rate limit, revocation | Second exchange and post-expiry exchange fail |
| Path traversal | Canonicalise beneath approved roots, reject `..`, absolute escape, device paths | Traversal cannot read outside scope |
| Symlink escape or race | Descriptor-relative operations where possible, revalidate target, no-follow policy | Symlink swap cannot escape root |
| Command injection | Structured argv/cwd/env, no implicit shell, filtered environment | Metacharacters remain literal arguments |
| Untrusted MCP/plugin | Per-plugin capabilities, bounded schemas/output, process/network policy, no instruction privilege | Plugin cannot invoke an ungranted capability |
| Malicious repository | Treat hooks/config/content as untrusted, disable implicit hooks, safe Git flags, dirty-tree preservation | Opening a repo executes no hook or config command |
| Device-session theft | Hashed at rest, scoped/revocable sessions, constant-time verify, audit | Revoked session cannot reconnect or mutate |
| Replay/duplicate mutation | Idempotency key bound to canonical command and owner | Same key with changed payload conflicts |
| Sensitive log leakage | Field-level redaction, bounded logs, secret scanning fixtures | Canary secrets absent from logs/crash reports/history |
| Denial of service | Size/time/concurrency limits, backpressure, rate limits, child supervision | Oversized/slow input is bounded and server remains healthy |

Local MCP executables are direct absolute-path child processes with no shell and a cleared environment. MCP stdout is bounded JSON-RPC data; stderr is bounded diagnostic data. Initialization, discovery, and shutdown have deadlines. Discovered annotations, descriptions, schemas, and results remain untrusted and cannot create approvals or alter permission policy. Per-Bot assignment narrows availability but never grants a capability.

## Networking

Default bind is `127.0.0.1`. LAN and Tailscale require explicit configuration. Plain HTTP/WebSocket is allowed only on loopback by default. Private-network plaintext may be enabled only with a visible warning and threat acknowledgement; public endpoints require HTTPS/WSS through a supported TLS configuration or reverse proxy. Origin and CSRF checks protect browser-capable surfaces.

The headless binary refuses a non-loopback bind unless `HOMEBOT_ALLOW_REMOTE=1` is explicitly set and warns whenever it is enabled. Pairing offers expire after five minutes and are single-use; SQLite stores only SHA-256 token digests. Exchange has durable global and per-token failure limits, exact browser-origin validation, and no-store responses. Revoked device sessions fail HTTP authentication and are disconnected from WebSocket streaming on heartbeat. Details and operator commands are in [remote-access.md](remote-access.md).

## Secrets

Secret values are created and resolved through `homebot-secrets`. Production builds use macOS Keychain or a Linux Secret Service-compatible store; every blocking OS call runs outside async executor threads. SQLite stores only owner, opaque locator, label, and timestamps. Values never appear in SQLite, normal chat, activity details, routine history, outbox events, crash reports, CLI arguments, process listings, or ordinary environment inheritance. Secret-bearing Rust request/input types have redacted or absent `Debug` implementations, and short-lived resolved allocations zero memory on drop.

Provider profiles store `SecretReference` identifiers, not credentials. The provider runtime resolves a short-lived redacted value only while constructing an authorized request and zeroes that allocation on drop. Remote BYOK endpoints require HTTPS; cleartext HTTP is accepted only for explicit loopback addresses, and HTTP redirects are disabled for credential-bearing requests.

Credential-store `locked`, `unavailable`, and `missing` conditions remain distinct and fail closed. HomeBot never falls back to plaintext files, SQLite, generic provider context, or environment variables. Metadata migration from schema v7 assigns existing opaque references to the local owner without fabricating, copying, or attempting to recover a value. See [secrets.md](secrets.md).

`SecretToolService` constructs a server-owned `SecretUse` capability request bound to authenticated operation context, opaque secret UUID, and a validated purpose identifier. Default policy requires approval; approvals are digest-bound, expiring, and single-use. A general tool cannot obtain an authorization proof or call the vault through the tool surface.

The local computer layer opens workspace roots as `cap-std` capability directories, rejects absolute/parent paths and symlink components, and bounds reads, writes and listings. PTY commands use an explicit executable and structured arguments, clear inherited environment state, admit only configured keys, and enforce input, output and runtime limits with kill-and-reap cancellation. Browser control and target WebSockets must be loopback, profile directories stay beneath the server-owned profile root, redirects are disabled, protocol messages are bounded, and navigation rejects non-HTTP schemes and embedded credentials.

## Audit

Security-relevant events include authentication failure, pairing creation/exchange, device creation/revocation, policy change, approval request/decision/use, secret reference mutation/use, denied capability, remote bind change, plugin installation, external mutation, and destructive VCS operation. Audit records store identities, scopes, safe metadata, and outcomes, never secret values.

The M6 re-execution of this threat model, resolved findings and concrete negative-test coverage are recorded in [security-review-6c7-69.md](security-review-6c7-69.md). CI also runs a tracked-secret/SQLite-column gate; this complements OS-backed storage and runtime redaction rather than replacing either control.

## Dependency exceptions

The egui 0.32.3 graph selected for HomeBot's Rust 1.85 minimum version transitively includes `ttf-parser` 0.25.1, covered by unmaintained advisory RUSTSEC-2026-0192. The advisory reports maintenance status, not a known vulnerability, and publishes no safe upgrade. CI ignores only this exact advisory while continuing to fail all vulnerability advisories. The exception must be reviewed during 6C7-69 and removed before v1 if egui offers a compatible maintained font stack.
