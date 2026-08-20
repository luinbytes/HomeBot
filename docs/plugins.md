# Plugins and MCP

The server registry owns plugin identity, transport, configuration references, authentication status, health, enablement, per-Bot assignment, capabilities, and lifecycle. Local MCP servers are the first implementation target. OAuth/service connectors remain behind the same registry boundary.

Plugin output is untrusted data. Installation or authentication does not grant filesystem, process, browser, secret, or external-mutation capabilities. A dead plugin is isolated, reported with a provider-neutral state, restarted only under bounded policy, and cannot crash the HomeBot server.
