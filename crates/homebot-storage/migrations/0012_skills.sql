ALTER TABLE skills ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE skills ADD COLUMN name_normalized TEXT NOT NULL DEFAULT '';
ALTER TABLE skills ADD COLUMN description TEXT NOT NULL DEFAULT '';
ALTER TABLE skills ADD COLUMN version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0);
ALTER TABLE skills ADD COLUMN updated_at_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE skills ADD COLUMN deleted_at_ms INTEGER;
UPDATE skills SET name_normalized = lower(trim(name)) WHERE name_normalized = '';
UPDATE skills
SET name_normalized = name_normalized || '#' || id
WHERE EXISTS (
  SELECT 1 FROM skills AS earlier
  WHERE earlier.owner_id = skills.owner_id
    AND earlier.name_normalized = skills.name_normalized
    AND earlier.id < skills.id
);
CREATE UNIQUE INDEX skills_owner_name ON skills(owner_id, name_normalized);
ALTER TABLE skill_versions ADD COLUMN name TEXT NOT NULL DEFAULT '';
ALTER TABLE skill_versions ADD COLUMN description TEXT NOT NULL DEFAULT '';
UPDATE skill_versions SET name = (SELECT name FROM skills WHERE skills.id = skill_versions.skill_id), description = (SELECT description FROM skills WHERE skills.id = skill_versions.skill_id);

CREATE TABLE skill_bot_assignments (
  owner_id TEXT NOT NULL,
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
  assigned_at_ms INTEGER NOT NULL,
  PRIMARY KEY(skill_id, bot_id)
);
CREATE INDEX skill_bot_assignments_bot ON skill_bot_assignments(owner_id, bot_id, skill_id);

CREATE TABLE message_skill_versions (
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE RESTRICT,
  skill_version_id TEXT NOT NULL REFERENCES skill_versions(id) ON DELETE RESTRICT,
  ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
  PRIMARY KEY(message_id, skill_id),
  UNIQUE(message_id, ordinal)
);
CREATE INDEX message_skill_versions_version ON message_skill_versions(skill_version_id);
ALTER TABLE queued_prompts ADD COLUMN skill_ids_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(skill_ids_json));
ALTER TABLE queued_prompts ADD COLUMN skill_version_ids_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(skill_version_ids_json));
