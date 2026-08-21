ALTER TABLE bots ADD COLUMN pinned_at_ms INTEGER;
ALTER TABLE bots ADD COLUMN hidden_at_ms INTEGER;

CREATE INDEX bots_owner_roster_order
ON bots(owner_id, archived_at_ms, hidden_at_ms, pinned_at_ms, name COLLATE NOCASE, id);
