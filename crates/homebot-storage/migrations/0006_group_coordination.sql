ALTER TABLE chats ADD COLUMN ownership_bot_id TEXT REFERENCES bots(id) ON DELETE SET NULL;
ALTER TABLE chats ADD COLUMN coordination_max_turns INTEGER NOT NULL DEFAULT 8
    CHECK (coordination_max_turns BETWEEN 1 AND 64);
ALTER TABLE chats ADD COLUMN coordination_turns_used INTEGER NOT NULL DEFAULT 0
    CHECK (coordination_turns_used >= 0);
ALTER TABLE chats ADD COLUMN max_parallel_bots INTEGER NOT NULL DEFAULT 3
    CHECK (max_parallel_bots BETWEEN 1 AND 8);
ALTER TABLE chats ADD COLUMN stop_requested INTEGER NOT NULL DEFAULT 0
    CHECK (stop_requested IN (0, 1));

ALTER TABLE messages ADD COLUMN shared_context_message_ids_json TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(shared_context_message_ids_json));

CREATE TABLE group_bot_states (
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('idle', 'running', 'waiting', 'completed', 'failed', 'stopped')),
    active_operation_id TEXT,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, bot_id),
    FOREIGN KEY (chat_id, bot_id) REFERENCES chat_participants(chat_id, bot_id) ON DELETE CASCADE
);

CREATE TABLE group_handoffs (
    id TEXT PRIMARY KEY NOT NULL,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    from_bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    to_bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    reason TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX group_handoffs_chat_created ON group_handoffs(chat_id, created_at_ms, id);
