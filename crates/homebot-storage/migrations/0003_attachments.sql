CREATE TABLE attachments (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    filename TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256 TEXT NOT NULL,
    storage_path TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'ready', 'expired')),
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    finalized_at_ms INTEGER
);
CREATE INDEX attachments_owner_status ON attachments(owner_id, status);

CREATE TABLE attachment_create_requests (
    idempotency_key TEXT PRIMARY KEY NOT NULL,
    request_hash TEXT NOT NULL,
    attachment_id TEXT NOT NULL REFERENCES attachments(id) ON DELETE CASCADE
);
