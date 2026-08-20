# Plugins and MCP

The server registry owns plugin identity, transport, safe configuration, authentication status, health, enablement, discovered tools, per-Bot assignment, and lifecycle. Local MCP servers are the v1 starting point. OAuth and hosted-service connectors implement the same adapter boundary later; neither clients nor Bots receive transport-specific authority.

## Local MCP lifecycle

HomeBot launches a configured absolute executable directly—never through a shell—with a cleared environment, piped stdin/stdout, bounded redacted stderr, a response deadline, and kill-on-drop cleanup. It performs the MCP initialize/initialized lifecycle and paginated `tools/list` discovery over newline-delimited stdio. Tool names and schemas are validated and bounded before they enter the registry.

The authoritative states are `connect`, `waiting`, `reopen`, `connected`, and `error`. Desktop and Android render these server values and submit authenticated, idempotent mutations; they do not infer health locally. Connect and health probes publish waiting followed by connected or error. Disable preserves discovery metadata but changes the recovery action to Reopen. Removal cascades assignments and tool metadata.

Registry endpoints under `/api/v1/plugins` cover list/create, connect/reopen/health, enable/disable, per-Bot assignment, and removal. Local configuration stores only an executable path and structured arguments. A future adapter that needs credentials must use opaque HomeBot secret references.

## Authorization and trust

Plugin output is untrusted data. Installation or authentication does not grant filesystem, process, browser, secret, or external-mutation capabilities. A dead plugin is isolated, reported with a provider-neutral state, restarted only under bounded policy, and cannot crash the HomeBot server.

MCP results use an explicit `UntrustedMcpOutput` boundary and cannot convert themselves into prompts, approvals, permission grants, or privileged instructions. Per-Bot assignment narrows availability but never expands capability grants. Invocation still passes the server policy engine's `PluginRead`/`PluginWrite` checks.
