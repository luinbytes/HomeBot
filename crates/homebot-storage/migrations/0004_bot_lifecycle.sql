ALTER TABLE bots ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE bots ADD COLUMN shape TEXT NOT NULL DEFAULT 'rounded_square'
    CHECK (shape IN ('circle', 'rounded_square', 'hexagon'));
ALTER TABLE bots ADD COLUMN color TEXT NOT NULL DEFAULT 'violet'
    CHECK (color IN ('violet', 'blue', 'green', 'orange', 'rose', 'slate'));
ALTER TABLE bots ADD COLUMN permission_profile TEXT NOT NULL DEFAULT 'ask_before_changes'
    CHECK (permission_profile IN ('read_only', 'ask_before_changes', 'trusted'));
ALTER TABLE bots ADD COLUMN archived_at_ms INTEGER;
ALTER TABLE bots ADD COLUMN unread_count INTEGER NOT NULL DEFAULT 0 CHECK (unread_count >= 0);
ALTER TABLE bots ADD COLUMN attention TEXT NOT NULL DEFAULT 'none'
    CHECK (attention IN ('none', 'working', 'needs_approval', 'failed'));

CREATE INDEX bots_owner_lifecycle_name
    ON bots(owner_id, archived_at_ms, name COLLATE NOCASE);
