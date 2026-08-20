PRAGMA foreign_keys = ON;

CREATE TABLE bots (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    provider_profile_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE chats (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('direct', 'group')),
    title TEXT NOT NULL DEFAULT '',
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE chat_participants (
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member',
    PRIMARY KEY (chat_id, bot_id)
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY NOT NULL,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    author_bot_id TEXT REFERENCES bots(id) ON DELETE SET NULL,
    author_kind TEXT NOT NULL CHECK (author_kind IN ('user', 'bot', 'system')),
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER
);
CREATE INDEX messages_chat_created ON messages(chat_id, created_at_ms);

CREATE TABLE message_parts (
    id TEXT PRIMARY KEY NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL,
    content_json TEXT NOT NULL CHECK (json_valid(content_json)),
    UNIQUE (message_id, ordinal)
);

CREATE TABLE execution_activities (
    id TEXT PRIMARY KEY NOT NULL,
    message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
    started_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER
);

CREATE TABLE provider_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    adapter_kind TEXT NOT NULL,
    display_name TEXT NOT NULL,
    configuration_json TEXT NOT NULL CHECK (json_valid(configuration_json)),
    secret_reference_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE provider_conversations (
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    provider_profile_id TEXT NOT NULL REFERENCES provider_profiles(id) ON DELETE CASCADE,
    external_conversation_id TEXT NOT NULL,
    PRIMARY KEY (bot_id, chat_id, provider_profile_id)
);

CREATE TABLE approvals (
    id TEXT PRIMARY KEY NOT NULL,
    operation_id TEXT NOT NULL,
    capability TEXT NOT NULL,
    status TEXT NOT NULL,
    request_json TEXT NOT NULL CHECK (json_valid(request_json)),
    created_at_ms INTEGER NOT NULL,
    decided_at_ms INTEGER
);

CREATE TABLE skills (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, active_version_id TEXT, created_at_ms INTEGER NOT NULL);
CREATE TABLE skill_versions (id TEXT PRIMARY KEY NOT NULL, skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE, version INTEGER NOT NULL, definition_json TEXT NOT NULL CHECK (json_valid(definition_json)), created_at_ms INTEGER NOT NULL, UNIQUE(skill_id, version));
CREATE TABLE plugins (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL, configuration_json TEXT NOT NULL CHECK (json_valid(configuration_json)), enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)), created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL);
CREATE TABLE mcp_connections (id TEXT PRIMARY KEY NOT NULL, plugin_id TEXT REFERENCES plugins(id) ON DELETE CASCADE, transport TEXT NOT NULL, configuration_json TEXT NOT NULL CHECK (json_valid(configuration_json)), status TEXT NOT NULL, updated_at_ms INTEGER NOT NULL);

CREATE TABLE routines (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, active_version_id TEXT, enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)), created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL);
CREATE TABLE routine_versions (id TEXT PRIMARY KEY NOT NULL, routine_id TEXT NOT NULL REFERENCES routines(id) ON DELETE CASCADE, version INTEGER NOT NULL, definition_json TEXT NOT NULL CHECK (json_valid(definition_json)), created_at_ms INTEGER NOT NULL, UNIQUE(routine_id, version));
CREATE TABLE routine_triggers (id TEXT PRIMARY KEY NOT NULL, routine_id TEXT NOT NULL REFERENCES routines(id) ON DELETE CASCADE, kind TEXT NOT NULL, configuration_json TEXT NOT NULL CHECK (json_valid(configuration_json)), enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)));
CREATE TABLE routine_runs (id TEXT PRIMARY KEY NOT NULL, routine_id TEXT NOT NULL REFERENCES routines(id) ON DELETE CASCADE, routine_version_id TEXT NOT NULL REFERENCES routine_versions(id), status TEXT NOT NULL, trigger_json TEXT NOT NULL CHECK (json_valid(trigger_json)), started_at_ms INTEGER NOT NULL, finished_at_ms INTEGER);

CREATE TABLE secret_references (id TEXT PRIMARY KEY NOT NULL, provider TEXT NOT NULL, locator TEXT NOT NULL, label TEXT NOT NULL, created_at_ms INTEGER NOT NULL, UNIQUE(provider, locator));
CREATE TABLE paired_devices (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, credential_reference_id TEXT NOT NULL REFERENCES secret_references(id), created_at_ms INTEGER NOT NULL, last_seen_at_ms INTEGER, revoked_at_ms INTEGER);

CREATE TABLE workspaces (id TEXT PRIMARY KEY NOT NULL, canonical_path TEXT NOT NULL UNIQUE, repository_identity TEXT, created_at_ms INTEGER NOT NULL);
CREATE TABLE checkpoints (id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, chat_id TEXT REFERENCES chats(id) ON DELETE SET NULL, turn_id TEXT NOT NULL, git_ref TEXT NOT NULL, metadata_json TEXT NOT NULL CHECK (json_valid(metadata_json)), created_at_ms INTEGER NOT NULL);
CREATE TABLE vcs_metadata (workspace_id TEXT PRIMARY KEY NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, branch TEXT, head_commit TEXT, dirty INTEGER NOT NULL DEFAULT 0 CHECK(dirty IN (0,1)), updated_at_ms INTEGER NOT NULL);

CREATE TABLE idempotency_records (
    key TEXT PRIMARY KEY NOT NULL,
    request_hash TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    response_json TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE event_outbox (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    owner_id TEXT NOT NULL,
    event_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX event_outbox_owner_sequence ON event_outbox(owner_id, sequence);
