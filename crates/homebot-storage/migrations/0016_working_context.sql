CREATE TABLE chat_working_contexts (
  owner_id TEXT NOT NULL,
  chat_id TEXT PRIMARY KEY NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  provider_profile_id TEXT NOT NULL REFERENCES provider_profiles(id) ON DELETE CASCADE,
  interaction_mode TEXT NOT NULL DEFAULT 'default' CHECK(interaction_mode IN ('default','plan')),
  used_tokens INTEGER CHECK(used_tokens IS NULL OR used_tokens >= 0),
  context_window_tokens INTEGER CHECK(context_window_tokens IS NULL OR context_window_tokens > 0),
  compaction_status TEXT NOT NULL DEFAULT 'idle' CHECK(compaction_status IN ('idle','running','completed','failed')),
  generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0),
  compacted_at_ms INTEGER,
  last_error TEXT,
  updated_at_ms INTEGER NOT NULL
);

CREATE INDEX chat_working_contexts_owner ON chat_working_contexts(owner_id, chat_id);

ALTER TABLE queued_prompts ADD COLUMN prompt_kind TEXT NOT NULL DEFAULT 'follow_up'
  CHECK(prompt_kind IN ('follow_up','steering'));
