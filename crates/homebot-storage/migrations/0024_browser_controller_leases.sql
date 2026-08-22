ALTER TABLE browser_sessions ADD COLUMN controller_device_id TEXT;
ALTER TABLE browser_sessions ADD COLUMN controller_lease_expires_at_ms INTEGER;

CREATE INDEX browser_sessions_controller_device
    ON browser_sessions(owner_id, controller_device_id, controller_lease_expires_at_ms);
