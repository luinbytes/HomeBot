ALTER TABLE chats ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE chats ADD COLUMN direct_bot_id TEXT REFERENCES bots(id) ON DELETE CASCADE;
ALTER TABLE chats ADD COLUMN unread_count INTEGER NOT NULL DEFAULT 0 CHECK (unread_count >= 0);
ALTER TABLE chats ADD COLUMN running INTEGER NOT NULL DEFAULT 0 CHECK (running IN (0, 1));
ALTER TABLE chats ADD COLUMN queued_count INTEGER NOT NULL DEFAULT 0 CHECK (queued_count >= 0);
ALTER TABLE chats ADD COLUMN last_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_sequence >= 0);
CREATE INDEX chats_owner_updated ON chats(owner_id, updated_at_ms DESC);
CREATE UNIQUE INDEX chats_owner_direct_bot
    ON chats(owner_id, direct_bot_id)
    WHERE kind = 'direct' AND direct_bot_id IS NOT NULL;

ALTER TABLE messages ADD COLUMN reply_to_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL;
ALTER TABLE messages ADD COLUMN mentioned_bot_ids_json TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(mentioned_bot_ids_json));
ALTER TABLE messages ADD COLUMN error_json TEXT CHECK (error_json IS NULL OR json_valid(error_json));

ALTER TABLE execution_activities ADD COLUMN chat_id TEXT REFERENCES chats(id) ON DELETE CASCADE;
ALTER TABLE execution_activities ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE execution_activities ADD COLUMN detail TEXT NOT NULL DEFAULT '';
ALTER TABLE execution_activities ADD COLUMN requires_attention INTEGER NOT NULL DEFAULT 0
    CHECK (requires_attention IN (0, 1));
CREATE INDEX execution_activities_chat_started
    ON execution_activities(chat_id, started_at_ms);

ALTER TABLE approvals ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE approvals ADD COLUMN chat_id TEXT REFERENCES chats(id) ON DELETE CASCADE;
ALTER TABLE approvals ADD COLUMN message_id TEXT REFERENCES messages(id) ON DELETE CASCADE;
ALTER TABLE approvals ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE approvals ADD COLUMN detail TEXT NOT NULL DEFAULT '';
CREATE INDEX approvals_chat_created ON approvals(chat_id, created_at_ms);

CREATE TABLE queued_prompts (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    attachment_ids_json TEXT NOT NULL CHECK (json_valid(attachment_ids_json)),
    position INTEGER NOT NULL CHECK (position >= 0),
    created_at_ms INTEGER NOT NULL,
    UNIQUE (chat_id, position)
);
CREATE INDEX queued_prompts_owner_chat ON queued_prompts(owner_id, chat_id, position);
