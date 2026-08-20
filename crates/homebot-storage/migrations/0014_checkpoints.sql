CREATE TABLE turn_checkpoints (
  id TEXT PRIMARY KEY NOT NULL,
  owner_id TEXT NOT NULL,
  chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL REFERENCES repository_workspaces(id) ON DELETE RESTRICT,
  message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
  phase TEXT NOT NULL CHECK(phase IN ('before_turn', 'after_turn', 'restore_safety')),
  git_ref TEXT NOT NULL,
  commit_oid TEXT NOT NULL,
  provider_profile_id TEXT,
  provider_conversation_id TEXT,
  created_at_ms INTEGER NOT NULL,
  UNIQUE(owner_id, git_ref)
);
CREATE INDEX turn_checkpoints_chat ON turn_checkpoints(owner_id, chat_id, created_at_ms, id);
CREATE INDEX turn_checkpoints_message ON turn_checkpoints(owner_id, message_id, phase);

CREATE TABLE checkpoint_restores (
  id TEXT PRIMARY KEY NOT NULL,
  owner_id TEXT NOT NULL,
  chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  checkpoint_id TEXT NOT NULL REFERENCES turn_checkpoints(id) ON DELETE RESTRICT,
  safety_checkpoint_id TEXT NOT NULL REFERENCES turn_checkpoints(id) ON DELETE RESTRICT,
  reconciliation TEXT NOT NULL CHECK(reconciliation IN ('unchanged', 'forked')),
  previous_provider_conversation_id TEXT,
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX checkpoint_restores_chat ON checkpoint_restores(owner_id, chat_id, created_at_ms, id);
