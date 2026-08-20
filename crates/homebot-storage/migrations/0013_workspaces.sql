CREATE TABLE repository_workspaces (
  id TEXT PRIMARY KEY NOT NULL,
  owner_id TEXT NOT NULL,
  name TEXT NOT NULL,
  root_path TEXT NOT NULL,
  root_path_normalized TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  UNIQUE(owner_id, root_path_normalized)
);

CREATE TABLE chat_workspaces (
  owner_id TEXT NOT NULL,
  chat_id TEXT PRIMARY KEY NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL REFERENCES repository_workspaces(id) ON DELETE RESTRICT,
  mode TEXT NOT NULL CHECK(mode IN ('primary', 'isolated')),
  worktree_path TEXT,
  branch_name TEXT,
  base_ref TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  CHECK((mode = 'primary' AND worktree_path IS NULL) OR (mode = 'isolated' AND worktree_path IS NOT NULL))
);
CREATE INDEX chat_workspaces_workspace ON chat_workspaces(owner_id, workspace_id, chat_id);
