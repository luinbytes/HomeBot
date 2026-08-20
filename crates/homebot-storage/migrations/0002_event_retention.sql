CREATE TABLE event_retention_cursors (
    owner_id TEXT PRIMARY KEY NOT NULL,
    minimum_resume_sequence INTEGER NOT NULL CHECK (minimum_resume_sequence >= 0),
    updated_at_ms INTEGER NOT NULL
);
