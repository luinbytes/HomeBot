# Plugins and MCP

The server registry owns plugin identity, transport, safe configuration, authentication status, health, enablement, discovered tools, per-Bot assignment, and lifecycle. Local MCP, remote MCP, first-party memory presets, and Composio Tool Router sessions use the same adapter boundary; neither clients nor Bots receive transport-specific authority.

## Local MCP lifecycle

HomeBot launches a configured absolute executable directly—never through a shell—with a cleared environment, piped stdin/stdout, bounded redacted stderr, a response deadline, and kill-on-drop cleanup. It performs the MCP initialize/initialized lifecycle and paginated `tools/list` discovery over newline-delimited stdio. Tool names and schemas are validated and bounded before they enter the registry.

The authoritative states are `connect`, `waiting`, `reopen`, `connected`, and `error`. Desktop and Android render these server values and submit authenticated, idempotent mutations; they do not infer health locally. Connect and health probes publish waiting followed by connected or error. Disable preserves discovery metadata but changes the recovery action to Reopen. Removal cascades assignments and tool metadata.

Registry endpoints under `/api/v1/plugins` cover list/create, connect/reopen/health, enable/disable, per-Bot assignment, and removal. Local configuration stores only an executable path and structured arguments. Remote adapters store only endpoint metadata and opaque HomeBot secret references.

## Remote MCP OAuth

Desktop and Android can create public or bearer-authenticated remote MCP connections directly. When an unauthenticated discovery request returns `401`, HomeBot exposes Sign in only for remote records without a conflicting static authorization header. `POST /api/v1/plugins/{plugin_id}/authorize` performs the MCP 2025-11-25 authorization sequence on the Mac server: protected-resource metadata discovery, authorization-server metadata discovery, mandatory PKCE S256, RFC 8707 `resource` binding, and dynamic client registration. Redirects are disabled during discovery, metadata and token bodies are bounded, authorization-server endpoints require HTTPS, and callback URIs require HTTPS or loopback HTTP.

The unauthenticated callback is protected by a random single-use state value and a ten-minute in-memory flow deadline. Access, refresh, and dynamically issued client credentials are stored as one opaque OS-keychain value; SQLite and both native clients see only the derived reference. Expiring access tokens refresh before MCP use, rotated refresh tokens replace the old bundle, and failed refresh returns the plugin to `required`. Deleting the plugin deletes its OAuth bundle first. A HomeBot restart intentionally invalidates unfinished browser authorization flows, which can be safely restarted from Sign in.

Dynamic registration is the active generic registration method. Authorization servers that accept only a pre-registered client or an HTTPS Client ID Metadata Document remain unsupported rather than receiving a partial or insecure fallback. Android connections advertised over private plaintext HTTP also cannot be OAuth callback targets; pair through an HTTPS HomeBot endpoint for that flow.

## Composio and Google Workspace

`POST /api/v1/connectors/composio` creates an owner-scoped Composio Tool Router session from an allowlist of 1 to 16 toolkit slugs. The Composio project key is resolved from the server credential store only while calling Composio and is reused as an opaque `x-api-key` header reference for the session MCP endpoint. The endpoint must pass HomeBot's HTTPS and credential-embedding checks.

HomeBot sends `workbench.enable=false` and `enable_proxy_execution=false`; a Composio session therefore does not introduce the hosted VM excluded from the v1 product contract. Search and bounded multi-tool execution remain available through Composio's MCP meta-tools, while each actual tool call still crosses HomeBot's assignment and approval policy.

Desktop and Android expose native Google Workspace and generic Composio setup. Google uses Composio's `googlesuper` toolkit so Gmail, Drive, Calendar, Docs, Sheets, Slides, Meet, and Tasks share one provider consent flow. The server creates the OAuth link and validates the returned HTTPS URL; Android never stores the project key. A connector remains `required` or `waiting` until Composio reports an active owner/toolkit account. MCP discovery alone cannot make the UI claim that account authentication succeeded.

Connected toolkit names are projected from the server record into both native clients. `POST /api/v1/connectors/composio/{plugin_id}/revoke` resolves the project key from the server vault, lists only the current HomeBot owner and requested toolkit, revokes every matching account through Composio, clears discovered tools, and returns the connector to `required`. “Switch” performs that explicit revoke before requesting a fresh authorization link; “Revoke” stops there.

`POST /api/v1/connectors/composio/{plugin_id}/events` reconciles Composio's one project webhook subscription against a public HTTPS HomeBot base URL. Only V3 trigger and connected-account-expiry events are enabled. The returned signing secret goes directly to the OS credential vault; SQLite and native clients receive only a derived opaque reference and the truthful `not_configured`, `ready`, or `error` state. The public ingress route has a 1 MiB body limit, requires the Composio HMAC headers, rejects timestamps outside five minutes, verifies owner and connector scope, and accepts only supported event types. Stable webhook IDs become unique durable outbox IDs, so duplicate delivery cannot create a second routine job even after restart. Raw provider data is discarded before persistence.

Composio permits one webhook subscription per project. HomeBot therefore rejects configuring the same project key for a second connector while the first connector owns event ingress. Both native clients expose event setup, but a LAN, loopback, `.local`, or private-address endpoint cannot be used as the callback; the paired server must have a publicly reachable HTTPS origin.

Live Composio and Google account acceptance, subscription creation, and delivery still require a real project key, public HTTPS endpoint, completed provider consent, and a provider-generated event. Direct Google Workspace developer-preview MCP OAuth may use the generic remote path only when its registration contract is compatible; it is not represented as accepted by the managed Composio path.

## Authorization and trust

Plugin output is untrusted data. Installation or authentication does not grant filesystem, process, browser, secret, or external-mutation capabilities. A dead plugin is isolated, reported with a provider-neutral state, restarted only under bounded policy, and cannot crash the HomeBot server.

MCP results use an explicit `UntrustedMcpOutput` boundary and cannot convert themselves into prompts, approvals, permission grants, or privileged instructions. Per-Bot assignment narrows availability but never expands capability grants. Invocation still passes the server policy engine's `PluginRead`/`PluginWrite` checks.
