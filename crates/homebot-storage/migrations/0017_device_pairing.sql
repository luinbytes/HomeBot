CREATE TABLE pairing_credentials (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    token_digest BLOB NOT NULL UNIQUE CHECK(length(token_digest) = 32),
    endpoint TEXT NOT NULL,
    expected_origin TEXT NOT NULL,
    endpoint_kind TEXT NOT NULL CHECK(endpoint_kind IN ('loopback', 'lan', 'tailscale', 'custom_https')),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    consumed_at_ms INTEGER,
    failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK(failed_attempts >= 0),
    CHECK(expires_at_ms > created_at_ms)
);
CREATE INDEX pairing_credentials_expiry ON pairing_credentials(expires_at_ms);

CREATE TABLE device_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80),
    token_digest BLOB NOT NULL UNIQUE CHECK(length(token_digest) = 32),
    endpoint_kind TEXT NOT NULL CHECK(endpoint_kind IN ('loopback', 'lan', 'tailscale', 'custom_https')),
    created_at_ms INTEGER NOT NULL,
    last_seen_at_ms INTEGER,
    revoked_at_ms INTEGER
);
CREATE INDEX device_sessions_owner_created ON device_sessions(owner_id, created_at_ms, id);

CREATE TABLE pairing_exchange_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_digest BLOB NOT NULL CHECK(length(token_digest) = 32),
    attempted_at_ms INTEGER NOT NULL
);
CREATE INDEX pairing_exchange_attempts_time ON pairing_exchange_attempts(attempted_at_ms);
