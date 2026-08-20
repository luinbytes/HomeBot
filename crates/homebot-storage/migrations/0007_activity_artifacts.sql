CREATE TABLE artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    activity_id TEXT REFERENCES execution_activities(id) ON DELETE SET NULL,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 255),
    kind TEXT NOT NULL CHECK (length(kind) BETWEEN 1 AND 64),
    media_type TEXT NOT NULL CHECK (length(media_type) BETWEEN 3 AND 127),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    storage_path TEXT NOT NULL CHECK (length(storage_path) > 0),
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX artifacts_owner_chat_created ON artifacts(owner_id, chat_id, created_at_ms);
