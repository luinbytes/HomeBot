-- Secret values remain in the operating-system credential store. SQLite owns only
-- owner-scoped opaque locators and non-sensitive display metadata.
ALTER TABLE secret_references ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE secret_references ADD COLUMN updated_at_ms INTEGER NOT NULL DEFAULT 0;

CREATE INDEX secret_references_owner_label
ON secret_references(owner_id, label COLLATE NOCASE);
