CREATE TABLE browser_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 80),
    directory_ref TEXT NOT NULL CHECK(length(directory_ref) BETWEEN 1 AND 160),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(owner_id, display_name)
);

CREATE TABLE browser_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    profile_id TEXT NOT NULL REFERENCES browser_profiles(id) ON DELETE RESTRICT,
    runtime_session_id TEXT,
    current_url TEXT,
    controller TEXT NOT NULL CHECK(controller IN ('bot', 'user')),
    status TEXT NOT NULL CHECK(status IN ('active', 'awaiting_approval', 'closed', 'failed')),
    pending_approval_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX browser_sessions_owner_chat ON browser_sessions(owner_id, chat_id, updated_at_ms, id);
