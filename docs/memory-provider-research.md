# Memory provider integration research

Updated: 27 August 2026. This is a source-backed provider contract and
implementation task note. It is not evidence that HomeBot has passed live
provider acceptance.

## Bottom line

Memory must remain a server-owned capability. The native desktop app and the
Android app should select and display the HomeBot server's memory connection;
neither client should own a second memory runtime or invent its own Bot
state.

The provider contracts are not interchangeable. A generic `recall` and
`retain` abstraction is useful at the HomeBot boundary, but the adapter must
use the provider's exact tool names, argument shape, scoping model, async
completion semantics, and authentication transport. Runtime `tools/list` (or
the provider's generated API schema) is authoritative over a remembered tool
name. In particular, current Supermemory uses `search_memory`, while current
Graphiti source exposes `search_memory_facts` and `add_memory` signatures that
must not be approximated.

Every automatic operation should:

* use an explicit owner and Bot scope, such as a provider project, bank,
  workspace, graph group, dataset, namespace, or archive;
* put recalled material into a bounded, clearly labelled, untrusted context
  block. Memory is data, not system instructions, and must never be allowed to
  rewrite HomeBot's harness policy;
* write only the agreed conversation content after a completed turn. Do not
  persist API keys, OAuth tokens, hidden system prompts, raw tool credentials,
  or unrelated users' content;
* apply timeouts, retry budgets, provider failure isolation, and read-after-
  write polling where the provider writes asynchronously; and
* keep destructive operations such as delete, forget, clear, detach, or bulk
  purge behind an explicit user action.

Cloud and self-hosted connections should be separate connection records. A
hosted OAuth endpoint is not proof that an equivalent local MCP endpoint
exists, and a local REST API is not proof that an MCP client can connect to it.
The UI should show endpoint, transport, auth mode, scope, and capability
health without exposing secrets.

## Contract matrix

| Provider | Hosted availability | Self-host availability | Transport and auth | Canonical recall / retain contract | Automatic status |
|---|---|---|---|---|---|
| Supermemory | Yes. Remote MCP at `https://mcp.supermemory.ai/mcp`. | Yes. Official server runs locally, normally REST at `http://localhost:6767`; local MCP endpoint is not established by the current self-host quickstart. | Remote MCP uses OAuth by default, or `Authorization: Bearer sm_...`; `x-sm-project` can scope a project. Local REST uses the generated `sm_...` bearer key. | `search_memory(query, includeProfile?, containerTag?)`; `add_memory(content, action?, containerTag?)`. The current read tool is `search_memory`, not `recall`. | Safe through a scoped adapter. Hosted MCP can be discovered directly. Local self-host needs a REST adapter or a confirmed local MCP bridge. |
| Honcho | Yes. MCP at `https://mcp.honcho.dev`; API at `https://api.honcho.dev`. | Yes. FastAPI server plus separately deployed MCP Worker; Worker uses `HONCHO_API_URL`, defaulting to hosted API. | MCP bearer key in `Authorization`; optional `X-Honcho-Workspace-ID`. Workspace can also be passed as an argument. | Full lifecycle is session and peer based: create stable session and peers, attach peers, `get_session_context` or `chat` for retrieval, then `add_messages_to_session` with exact user and assistant messages. | Safe through a full lifecycle adapter. A bare generic search is not equivalent to Honcho's peer-aware context. |
| Hindsight | Yes. MCP at `https://api.hindsight.vectorize.io/mcp/{bank_id}/` or `/mcp/`. | Yes. MCP is built into the local API at `http://localhost:8888/mcp/{bank_id}/` or `/mcp/`; stdio `hindsight-local-mcp` is also documented. | Hosted bearer API key. Self-host has no auth by default, or can enable the documented API-key tenant extension and bearer header. | `recall(query, max_tokens?, budget?, types?, tags?, tags_match?, tag_groups?, query_timestamp?)`; `retain(content, context?, timestamp?, tags?, metadata?, document_id?)`; `sync_retain` has the same fields and waits for persistence. | Safe with per-Bot banks or deterministic tags. `sync_retain` is appropriate when the next turn needs read-after-write consistency. |
| Holographic | No hosted service contract. | Yes. It is a local provider shipped inside Hermes Agent and stores facts in SQLite with FTS5, optional HRR retrieval, trust feedback, and entity links. | No network authentication; HomeBot stores it in the server database. | `fact_store` supports add, search, probe, related, reason, contradict, update, remove, and list; `fact_feedback` records helpful/unhelpful ratings. Prefetch recalls facts while writes remain explicit tool calls. | Active as a built-in, owner/Bot-scoped provider with durable facts, FTS, entity links, trust feedback, and deletion. Optional HRR contradiction scoring remains explicitly unavailable. |
| Mem0 / OpenMemory | Yes. Remote MCP at `https://mcp.mem0.ai/mcp`; hosted platform REST is under `https://api.mem0.ai`. | Yes. The current self-host server runs at `http://localhost:8888`, with `POST /search` and `POST /memories`; it is REST, not the hosted MCP contract. | Hosted MCP supports OAuth or bearer `MEM0_API_KEY`. Self-host accepts `X-API-Key`, bearer JWT, a legacy admin key, or explicitly disabled auth for local development. | Hosted MCP exposes `add_memory`, `search_memories`, and lifecycle tools discovered at runtime. Current self-host REST accepts search `{query, filters, top_k?}` and add `{messages, user_id?, agent_id?, run_id?, metadata?}`. | Automatic scoped recall/retain is active for HomeBot's self-host REST adapter using deterministic `user_id` plus `agent_id`; hosted MCP remains runtime-discovered. Destructive routes are user-only. |
| Zep Cloud | Yes. Hosted Memory MCP at `https://api.getzep.com/mcp`; BYOC uses the same path on the customer's host. | Old Zep Community Edition is not a supported current deployment. Graphiti is the open-source self-host path and is listed separately below. | MCP uses Zep's OAuth/IdP flow. Zep Cloud API base is `https://api.getzep.com`. | MCP tools include `search_graph`, `get_user_summary`, `get_subgraph`, `get_node_neighbors`, `list_episodes`, and `add_memory`, plus explicit-graph variants such as `search_graph_in` and `add_memory_to_graph`. | Hosted OAuth connector is feasible, but it is not the same scope model as a generic Bot memory store. Confirm graph identity and privacy scope before automatic writes. |
| Graphiti | No simple public hosted Graphiti endpoint is documented as a separate product contract. | Yes. Official MCP server supports Streamable HTTP at `http://localhost:8000/mcp/`, plus stdio/SSE variants; default Docker setup uses FalkorDB or Neo4j. | Local MCP normally has no external bearer auth unless placed behind one. Graphiti itself needs an LLM and embedder/database configuration. | Source signature is exactly `search_memory_facts(query, group_ids?, max_facts?, center_node_uuid?, edge_types?, valid_at_after?, valid_at_before?, invalid_at_after?, invalid_at_before?)` and `add_memory(name, episode_body, group_id?, source?, source_description?, uuid?, reference_time?, excluded_entity_types?, custom_extraction_instructions?, previous_episode_uuids?, update_communities?, saga?, saga_previous_episode_uuid?)`. | Safe through a dedicated self-host adapter, deterministic `group_id` scopes, and completion tracking. Runtime discovery is mandatory because the README for some versions names these tools `search_facts` and `add_episode`. |
| Cognee | Yes. Cognee Cloud provides managed storage and pipelines, but the current docs do not publish one universal cloud MCP URL. | Yes. Official MCP uses stdio `uv run cognee-mcp`, Streamable HTTP `http://127.0.0.1:8000/mcp`, or SSE `/sse`; the API bridge accepts `--api-url` and `--api-token`. | Local standalone defaults to SQLite, LanceDB, and Kuzu. API bridge forwards `Authorization: Bearer`; cloud connection should use the user-supplied API base/key. | `recall(query, search_type?, datasets?, session_id?, system_prompt?, top_k?)`, where `datasets` is a comma-separated string. `remember(data?, content_base64?, filename?, dataset_name?, session_id?, custom_prompt?, background?)`. | Safe with deterministic `dataset_name` and optional `session_id`. Background ingestion requires `cognify_status` polling when consistency matters. `forget` is destructive and user-only. |
| Letta | Yes. Letta Cloud at `https://api.letta.com`; it provides managed agents, archives, and blocks. | Yes. Letta App Server can run locally, commonly at `http://localhost:8283`. | REST uses bearer API auth. Self-host is a complete Letta runtime rather than a generic memory MCP endpoint. | Archive/passages are the closest standalone memory contract: create an archive, create passages with `{text, created_at?, metadata?, tags?}`, then search passages. Agent archival-memory and core-memory block endpoints are tied to a Letta-managed agent. | Do not route HomeBot turns through Letta agent messages. A dedicated archive adapter can be planned, but generic automatic recall/retain is not yet a first-party HomeBot MCP contract. |

## Provider contracts

### Supermemory

The [official MCP documentation](https://supermemory.ai/docs/supermemory-mcp/mcp)
currently documents the remote Streamable HTTP endpoint
`https://mcp.supermemory.ai/mcp`. OAuth is the default connection flow. The
same documentation permits an API-key alternative with
`Authorization: Bearer sm_...` and supports the `x-sm-project` header for
project scope.

The current tool names and fields are:

```text
search_memory(
  query: string,
  includeProfile?: boolean = true,
  containerTag?: string,
)

add_memory(
  content: string,
  action?: "save" | "forget" = "save",
  containerTag?: string,
)
```

The MCP server also exposes `context` for formatted profile context,
`whoAmI`, document and memory listing, and document retrieval. `context` takes
an optional `containerTag` and `includeRecent` flag. HomeBot should not map
`search_memory` to a provider call named `recall` unless the adapter's
capability record explicitly aliases it.

The [official self-host quickstart](https://github.com/supermemoryai/supermemory/blob/main/apps/docs/self-hosting/quickstart.mdx)
documents a local server started with `supermemory-server`, local API base
`http://localhost:6767`, and REST examples such as:

```http
POST http://localhost:6767/v3/documents
Authorization: Bearer sm_...
Content-Type: application/json

{"content":"...", "containerTag":"homebot-bot-id"}
```

and:

```http
POST http://localhost:6767/v3/search
Authorization: Bearer sm_...
Content-Type: application/json

{"q":"...", "containerTag":"homebot-bot-id"}
```

Local data defaults to `./.supermemory` or `SUPERMEMORY_DATA_DIR`, while
credentials are kept in `~/.supermemory/env`. The local API is described as
compatible with the cloud API, but the quickstart does not establish a local
MCP URL. This should therefore be represented as self-host REST until a live
MCP `tools/list` probe proves otherwise.

### Honcho

The [official Honcho MCP README](https://github.com/plastic-labs/honcho/blob/main/mcp/README.md)
documents `https://mcp.honcho.dev`, bearer authentication, and optional
`X-Honcho-Workspace-ID`. The [official lifecycle instructions](https://github.com/plastic-labs/honcho/blob/main/mcp/instructions.md)
are important for HomeBot because Honcho models memory around stable peers and
sessions rather than a single unqualified text store.

The recommended turn lifecycle is:

```text
create_session({workspace_id, session_id})
create_peer({workspace_id, peer_id})             # user and assistant peers
add_peers_to_session({workspace_id, session_id,
  peers: [{peer_id, observe_me, observe_others}, ...]})

# retrieve context before generation
get_session_context({workspace_id, session_id, ...})
# or peer-aware reasoning
chat({workspace_id, peer_id, query, target_peer_id, session_id, reasoning_level?})

# retain the completed turn exactly
add_messages_to_session({workspace_id, session_id,
  messages: [{peer_id, content}, ...]})
```

Other documented tools include `list_workspaces`, `inspect_workspace`,
`search`, `get_metadata`, `set_metadata`, `get_peer_card`,
`set_peer_card`, `get_peer_context`, `get_representation`,
`list_sessions`, `inspect_session`, `get_session_messages`,
`list_conclusions`, `query_conclusions`, `create_conclusions`,
`schedule_dream`, and `get_queue_status`.

Honcho's [self-host configuration](https://github.com/plastic-labs/honcho/blob/main/mcp/src/config.ts)
shows that the MCP Worker can point at a local FastAPI API with
`HONCHO_API_URL=http://127.0.0.1:28000`; when unset it routes to
`https://api.honcho.dev`. The bearer key remains mandatory, and the workspace
header is a request-scoping convenience, not a substitute for a stable
workspace record. Automatic HomeBot writes should use one stable peer identity
per owner/Bot and one stable session per coherent conversation.

### Hindsight

The [official Hindsight MCP server documentation](https://github.com/vectorize-io/hindsight/blob/main/hindsight-docs/docs/developer/mcp-server.md)
documents both single-bank and multi-bank paths. Hosted deployments use
`https://api.hindsight.vectorize.io/mcp/{bank_id}/` or `/mcp/` with a bearer
API key. The [local MCP documentation](https://github.com/vectorize-io/hindsight/blob/main/hindsight-docs/docs-integrations/local-mcp.md)
uses `http://localhost:8888/mcp/{bank_id}/` or `/mcp/`, and also documents the
stdio `hindsight-local-mcp` client. Local auth is off by default, but the
documented tenant extension can require a bearer key using
`HINDSIGHT_API_TENANT_EXTENSION` and `HINDSIGHT_API_TENANT_API_KEY`.

The core tools are:

```text
retain(
  content: string,
  context?: string = "general",
  timestamp?: ISO-8601 string,
  tags?: list[string],
  metadata?: object,
  document_id?: string,
)

sync_retain(
  content: string,
  context?: string = "general",
  timestamp?: ISO-8601 string,
  tags?: list[string],
  metadata?: object,
  document_id?: string,
)

recall(
  query: string,
  max_tokens?: integer = 4096,
  budget?: "low" | "mid" | "high" = "high",
  types?: list["world" | "experience" | "observation"],
  tags?: list[string],
  tags_match?: "any" | "all" | "any_strict" | "all_strict" | "exact",
  tag_groups?: list[object],
  query_timestamp?: ISO-8601 string,
)
```

`reflect` is a model-generated synthesis tool, not a plain retrieval call. It
should not be silently invoked as automatic recall because its output needs
the same untrusted-context treatment as an LLM response. HomeBot can safely
use `recall` before a turn and `sync_retain` after a turn when the selected bank
and tag scope belong to that Bot.

### Mem0 and OpenMemory

The [official Mem0 MCP documentation](https://github.com/mem0ai/mem0/blob/main/docs/platform/mem0-mcp.mdx)
documents the hosted Streamable HTTP endpoint `https://mcp.mem0.ai/mcp`.
OAuth browser sign-in and bearer `MEM0_API_KEY` are documented. The current
tool names are:

```text
add_memory
search_memories
get_memories
get_memory
update_memory
delete_memory
delete_all_memories
delete_entities
list_entities
list_events
get_event_status
```

The official lifecycle plugin guidance requires explicit `user_id` and
`app_id` scope on writes and reads. Current examples use a filter equivalent
to:

```json
{
  "filters": {
    "AND": [
      {"user_id": "owner-id"},
      {"app_id": "homebot-bot-id"}
    ]
  }
}
```

`query` and `top_k` are present in current search guidance, but the complete
MCP argument schema is version-sensitive. HomeBot must call `tools/list` and
persist the discovered provider version/capabilities before sending a live
request. A known official issue documents signature drift in OpenMemory
around `limit` versus `top_k` and `get_all(user_id=...)` versus filter-based
calls. It is a compatibility warning, not a normative schema:
[Mem0 issue 6078](https://github.com/mem0ai/mem0/issues/6078).

The [open-source setup documentation](https://github.com/mem0ai/mem0/blob/main/docs/open-source/setup.mdx)
describes a local FastAPI server at `http://localhost:8888`, dashboard at
`http://localhost:3000`, and OpenAPI at `/docs`. The current
[official server source](https://github.com/mem0ai/mem0/blob/main/server/main.py)
defines `POST /search` with `{query, filters, top_k?, threshold?, explain?,
show_expired?}` and `POST /memories` with `{messages, user_id?, agent_id?,
run_id?, metadata?, ...}`. Its auth boundary accepts `X-API-Key` or a bearer
JWT. Local development can disable auth with `AUTH_DISABLED=true`, but HomeBot
never enables that mode. No official local MCP URL matches the hosted endpoint,
so HomeBot keeps REST and MCP connections distinct.

HomeBot's self-host adapter maps `search_memories` to `/search` and
`add_memory` to `/memories`, with deterministic owner `user_id` and Bot
`agent_id` values. It preserves the exact user and assistant message roles.
Hosted event status should be polled when a write returns an event ID.
`update_memory`, delete variants, and entity deletion remain explicit user
operations only.

### Zep Cloud and Graphiti

Zep's [official Memory MCP documentation](https://help.getzep.com/memory-mcp-server)
documents the hosted endpoint `https://api.getzep.com/mcp`, OAuth/IdP sign-in,
and BYOC deployments using the same `/mcp` path on the customer's host. The
MCP surface includes `search_graph`, `get_user_summary`, `get_subgraph`,
`get_node_neighbors`, `list_episodes`, and `add_memory`, with explicit graph
variants such as `search_graph_in` and `add_memory_to_graph`. The
[Zep versus Graphiti guide](https://help.getzep.com/zep-vs-graphiti) treats
Zep Cloud/BYOC and Graphiti as different deployment choices. The former Zep
Community Edition is not a current supported self-host assumption.

The [official Graphiti MCP source](https://github.com/getzep/graphiti/blob/main/mcp_server/src/graphiti_mcp_server.py)
contains these exact functions and fields:

```python
async def add_memory(
    name: str,
    episode_body: str,
    group_id: str | None = None,
    source: str = "text",
    source_description: str = "",
    uuid: str | None = None,
    reference_time: str | None = None,
    excluded_entity_types: list[str] | None = None,
    custom_extraction_instructions: str | None = None,
    previous_episode_uuids: list[str] | None = None,
    update_communities: bool = False,
    saga: str | None = None,
    saga_previous_episode_uuid: str | None = None,
) -> SuccessResponse | ErrorResponse

async def search_memory_facts(
    query: str,
    group_ids: str | list[str] | None = None,
    max_facts: int = 10,
    center_node_uuid: str | None = None,
    edge_types: list[str] | None = None,
    valid_at_after: str | None = None,
    valid_at_before: str | None = None,
    invalid_at_after: str | None = None,
    invalid_at_before: str | None = None,
) -> FactSearchResponse | ErrorResponse
```

`source` accepts `text`, `json`, or `message`. For `json`, `episode_body`
must be a properly escaped JSON string. `group_id` scopes the graph and
should be deterministic per owner and Bot. `add_memory` returns before all
background processing has finished and serializes additions per group, so an
adapter that needs immediate read-after-write behavior needs a completion
strategy.

The [official Graphiti MCP README](https://github.com/getzep/graphiti/blob/main/mcp_server/README.md)
documents the local Streamable HTTP endpoint `http://localhost:8000/mcp/`, a
health endpoint at `/health`, and Docker-backed FalkorDB or Neo4j. The README
for some released versions calls the operations `add_episode` and
`search_facts`, while the source function names above are
`add_memory` and `search_memory_facts`. HomeBot must perform MCP capability
discovery and adapt to the discovered names instead of assuming that one
release's names are universal. Graphiti also needs an LLM, embedder, and graph
database configuration, even when the MCP server itself is local.

### Cognee

The [official Cognee MCP tools page](https://docs.cognee.ai/cognee-mcp/mcp-tools)
documents the following current lifecycle fields:

```text
remember(
  data?: string,
  content_base64?: string,
  filename?: string,
  dataset_name?: string,
  session_id?: string,
  custom_prompt?: string,
  background?: boolean = false,
)

recall(
  query: string,
  search_type?: string,
  datasets?: string,          # comma-separated, not a JSON array
  session_id?: string,
  system_prompt?: string,
  top_k?: integer,
)
```

`data` and `content_base64` are mutually exclusive. `dataset_name` defaults to
the agent-scoped dataset and should be replaced with a deterministic HomeBot
owner/Bot dataset. `session_id` keeps session cache separate from permanent
graph memory. Current docs describe `top_k` with a default of 15 and a 1 to 100
range; older versions documented a different default, so runtime schema
discovery remains required. `background=true` requires `cognify_status`
polling if the next turn depends on the newly remembered data.

The [local setup documentation](https://docs.cognee.ai/cognee-mcp/mcp-local-setup)
documents stdio (`uv run cognee-mcp`), Streamable HTTP
`http://127.0.0.1:8000/mcp`, and SSE `/sse`. The API bridge accepts
`--api-url http://localhost:8080 --api-token your_backend_token` and forwards
the token as `Authorization: Bearer`. The local default storage is SQLite,
LanceDB, and Kuzu under `SYSTEM_ROOT_DIRECTORY`. `MCP_ALLOWED_HOSTS` and
origin validation should remain enabled for non-loopback deployments.

Cognee Cloud provides managed storage and pipelines, but the current cloud
overview does not publish one universal MCP URL. A HomeBot cloud connection
should therefore require the user-selected Cognee API base and key, or use a
documented `cognee.serve()` connection, rather than hardcoding an endpoint
that is not in the first-party contract. `forget` and related deletion tools
remain explicit user actions.

### Letta

Letta is a complete agent runtime with memory primitives, not currently a
standalone first-party HomeBot memory MCP service. The hosted API base is
`https://api.letta.com`; self-host tutorials use a local App Server such as
`http://localhost:8283`, with bearer auth for API requests.

The archive/passages API is the cleanest adapter boundary:

```http
POST /v1/archives/
{"name":"homebot-owner-bot", "description":"..."}

POST /v1/archives/{archive_id}/passages
{"text":"...", "created_at":"...", "metadata":{}, "tags":["..."]}
```

Agent archival memory is exposed under
`/v1/agents/{agent_id}/archival-memory`, with search under
`/v1/agents/{agent_id}/archival-memory/search`; the exact search query and
pagination fields are versioned in the generated API reference. Core memory
blocks are attached and updated under
`/v1/agents/{agent_id}/core-memory/blocks/...`.

HomeBot should not call `POST /v1/agents/{agent_id}/messages` for ordinary
HomeBot turns. That would create or invoke a second Letta-owned Bot runtime,
breaking the server-owned harness model. A future dedicated archive adapter
can create/search passages with an explicit HomeBot archive, but until that
adapter exists Letta should be presented as a planned API integration rather
than claiming generic automatic recall and retain.

## Additional mainstream candidate: LangMem and LangGraph Store

[LangMem](https://langchain-ai.github.io/langmem/) is a first-party LangChain
framework for memory tools, not a hosted memory vendor with a universal
remote MCP URL. It provides `create_manage_memory_tool` and
`create_search_memory_tool`, normally backed by a LangGraph Store. The
illustrative store is an `InMemoryStore`, but production deployments can use
`AsyncPostgresStore` or another persistent store.

The [LangGraph Store contract](https://github.com/langchain-ai/docs/blob/main/src/oss/langgraph/stores.mdx)
uses tuple namespaces and methods equivalent to:

```python
aput(namespace, key, value, index=None)
aget(namespace, key)
adelete(namespace, key)
asearch(namespace_prefix, *, query=None, filter=None,
        limit=10, offset=0)
alist_namespaces(*, prefix=None, suffix=None, max_depth=None,
                 limit=100, offset=0)
```

Namespaces such as `("owner-id", "homebot-bot-id", "memories")` provide a
natural HomeBot scope. Persistent backends include Postgres, MongoDB, Redis,
and Upstash. LangGraph Agent Server can be hosted or self-hosted, but this
still requires a HomeBot-deployed bridge or embedded framework runtime. It is
therefore a useful future integration target, not a URL-only provider
connection today. HomeBot should not claim LangMem support until it has a
bridge with a defined MCP or internal RPC contract.

## Name resolution

“Holographic” is the local memory provider shipped inside Hermes Agent, not a
hosted vendor product. Its authoritative implementation exposes a SQLite fact
store, FTS5 search, trust feedback, entity resolution, and optional HRR
retrieval. HomeBot now implements its non-HRR fact lifecycle as a built-in
provider, with explicit writes and automatic scoped prefetch.

“Poncho” is an agent harness with configurable storage and Postgres memory,
not a standalone memory provider contract. “Honcho” is the provider-neutral
memory product with hosted and self-hosted lifecycle APIs, and is already in
the catalog. HomeBot should integrate Poncho as another harness only if a
separate compatibility request defines that scope; it should not mislabel it
as memory.

## Remaining implementation and acceptance tasks

The catalog split, exact Supermemory/OpenMemory adapters, MCP discovery,
server-owned bearer and generic OAuth/PKCE connections, deterministic scopes,
and bounded recall/retain injection are implemented. Hosted and self-hosted
entries still remain truthful about provider-specific gaps.

1. Track async completion for Hindsight retain when strict consistency is
   selected, Mem0 event IDs, Cognee background cognify, Graphiti background
   episode ingestion, and Honcho asynchronous reasoning or dreaming.
2. Add live smoke tests for each declared transport: hosted MCP, loopback MCP,
   loopback REST, and self-host API bridge. Tests must cover connect,
   tools-list, scoped recall, scoped retain, restart/reconnect, timeout, and
   provider outage.
3. Add explicit destructive-operation gates and audit entries for all delete,
   forget, clear, detach, metadata mutation, and bulk purge calls.
4. Add optional Holographic HRR contradiction scoring only if parity requires
   NumPy-compatible vector behavior; the active relational fact path reports
   this capability as unavailable today.
5. Keep Letta and LangMem/LangGraph Store as planned adapters until their
   HomeBot-owned bridge contracts exist. Do not route HomeBot conversations
   through an external agent runtime.
6. Add pre-registered OAuth client and HTTPS Client ID Metadata Document modes
   only when a real target rejects dynamic registration; do not weaken PKCE,
   issuer validation, resource binding, or callback policy to imitate support.

## Acceptance scenarios

* A user can connect a hosted or loopback provider, see endpoint, auth mode,
  scope, and discovered capabilities, and verify health without exposing a
  secret.
* A user assigns one provider scope to one Bot. A turn recalls only that
  scope, injects a bounded untrusted memory block, and retains the completed
  user and assistant content in that scope.
* A second Bot and a second owner cannot retrieve the first scope's memories,
  even when they use the same provider account.
* A provider restart, expired OAuth session, timeout, or async write does not
  corrupt the HomeBot turn. Reconnect resumes with the same scope and reports
  pending or failed operations accurately.
* Delete, forget, clear, detach, and bulk operations require an explicit UI
  action and are recorded with the affected provider scope.

## Primary sources

* [Supermemory MCP](https://supermemory.ai/docs/supermemory-mcp/mcp) and
  [Supermemory self-host quickstart](https://github.com/supermemoryai/supermemory/blob/main/apps/docs/self-hosting/quickstart.mdx)
* [Honcho MCP](https://github.com/plastic-labs/honcho/blob/main/mcp/README.md),
  [Honcho lifecycle instructions](https://github.com/plastic-labs/honcho/blob/main/mcp/instructions.md),
  and [Honcho MCP configuration](https://github.com/plastic-labs/honcho/blob/main/mcp/src/config.ts)
* [Hermes Holographic provider](https://github.com/NousResearch/hermes-agent/tree/main/plugins/memory/holographic)
* [Hindsight MCP server](https://github.com/vectorize-io/hindsight/blob/main/hindsight-docs/docs/developer/mcp-server.md)
  and [Hindsight local MCP](https://github.com/vectorize-io/hindsight/blob/main/hindsight-docs/docs-integrations/local-mcp.md)
* [Mem0 MCP](https://github.com/mem0ai/mem0/blob/main/docs/platform/mem0-mcp.mdx)
  and [Mem0 open-source setup](https://github.com/mem0ai/mem0/blob/main/docs/open-source/setup.mdx)
* [Zep Memory MCP](https://help.getzep.com/memory-mcp-server),
  [Zep versus Graphiti](https://help.getzep.com/zep-vs-graphiti),
  and [Graphiti MCP source](https://github.com/getzep/graphiti/blob/main/mcp_server/src/graphiti_mcp_server.py)
* [Graphiti MCP README](https://github.com/getzep/graphiti/blob/main/mcp_server/README.md)
* [Cognee MCP tools](https://docs.cognee.ai/cognee-mcp/mcp-tools) and
  [Cognee local MCP setup](https://docs.cognee.ai/cognee-mcp/mcp-local-setup)
* [Letta archive creation](https://docs.letta.com/api/typescript/resources/archives/methods/create),
  [Letta passages](https://docs.letta.com/api/typescript/resources/archives/subresources/passages/methods/create),
  and [Letta agent passages](https://docs.letta.com/api/typescript/resources/agents/subresources/passages)
* [LangMem](https://langchain-ai.github.io/langmem/) and
  [LangGraph Store](https://github.com/langchain-ai/docs/blob/main/src/oss/langgraph/stores.mdx)
