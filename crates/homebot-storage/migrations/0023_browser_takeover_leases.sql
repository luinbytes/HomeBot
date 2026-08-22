ALTER TABLE browser_sessions ADD COLUMN controlling_device_id TEXT;
ALTER TABLE browser_sessions ADD COLUMN takeover_expires_at_ms INTEGER;

CREATE INDEX browser_sessions_takeover_lease
    ON browser_sessions(owner_id, controlling_device_id, takeover_expires_at_ms)
    WHERE controller = 'user';
