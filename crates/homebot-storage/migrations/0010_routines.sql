ALTER TABLE routines ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE routines ADD COLUMN bot_id TEXT REFERENCES bots(id) ON DELETE CASCADE;
ALTER TABLE routines ADD COLUMN description TEXT NOT NULL DEFAULT '';
ALTER TABLE routines ADD COLUMN draft INTEGER NOT NULL DEFAULT 1 CHECK(draft IN (0,1));
CREATE UNIQUE INDEX routines_owner_name ON routines(owner_id, name COLLATE NOCASE);

ALTER TABLE routine_runs ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE routine_runs ADD COLUMN dry_run INTEGER NOT NULL DEFAULT 0 CHECK(dry_run IN (0,1));
ALTER TABLE routine_runs ADD COLUMN input_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(input_json));
ALTER TABLE routine_runs ADD COLUMN result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json));
ALTER TABLE routine_runs ADD COLUMN error_message TEXT;

CREATE TABLE routine_recordings (
  id TEXT PRIMARY KEY NOT NULL,
  owner_id TEXT NOT NULL,
  bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  actions_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(actions_json)),
  status TEXT NOT NULL CHECK(status IN ('recording','finished','cancelled')),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
