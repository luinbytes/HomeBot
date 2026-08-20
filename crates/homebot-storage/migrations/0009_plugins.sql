ALTER TABLE plugins ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE plugins ADD COLUMN description TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX plugins_owner_name ON plugins(owner_id, name COLLATE NOCASE);

ALTER TABLE mcp_connections ADD COLUMN auth_status TEXT NOT NULL DEFAULT 'not_required'
  CHECK(auth_status IN ('not_required','required','waiting','connected','error'));
ALTER TABLE mcp_connections ADD COLUMN error_message TEXT;

CREATE TABLE plugin_bot_assignments (
  plugin_id TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
  owner_id TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
  PRIMARY KEY(plugin_id, bot_id)
);

CREATE TABLE mcp_tools (
  plugin_id TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  title TEXT,
  description TEXT,
  input_schema_json TEXT NOT NULL CHECK(json_valid(input_schema_json)),
  PRIMARY KEY(plugin_id, name)
);
