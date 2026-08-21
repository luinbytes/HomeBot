CREATE TABLE message_references (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('bot', 'group', 'routine', 'plugin')),
    target_id TEXT NOT NULL,
    target_version_id TEXT,
    label_snapshot TEXT NOT NULL CHECK (length(label_snapshot) BETWEEN 1 AND 120),
    PRIMARY KEY (message_id, ordinal)
);
CREATE INDEX message_references_target ON message_references(kind, target_id);

CREATE TABLE queued_prompt_references (
    prompt_id TEXT NOT NULL REFERENCES queued_prompts(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('bot', 'group', 'routine', 'plugin')),
    target_id TEXT NOT NULL,
    target_version_id TEXT,
    label_snapshot TEXT NOT NULL CHECK (length(label_snapshot) BETWEEN 1 AND 120),
    PRIMARY KEY (prompt_id, ordinal)
);
