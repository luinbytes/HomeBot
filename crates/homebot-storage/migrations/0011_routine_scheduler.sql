ALTER TABLE routine_triggers ADD COLUMN owner_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
ALTER TABLE routine_triggers ADD COLUMN last_evaluated_at_ms INTEGER;
ALTER TABLE routine_triggers ADD COLUMN next_fire_at_ms INTEGER;
ALTER TABLE routine_triggers ADD COLUMN created_at_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE routine_triggers ADD COLUMN updated_at_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE routine_triggers ADD COLUMN last_event_sequence INTEGER NOT NULL DEFAULT 0 CHECK(last_event_sequence >= 0);
CREATE INDEX routine_triggers_due ON routine_triggers(owner_id, enabled, next_fire_at_ms);

CREATE TABLE routine_trigger_deliveries (
  trigger_id TEXT NOT NULL REFERENCES routine_triggers(id) ON DELETE CASCADE,
  delivery_key TEXT NOT NULL,
  received_at_ms INTEGER NOT NULL,
  PRIMARY KEY(trigger_id, delivery_key)
);

CREATE TABLE routine_jobs (
  id TEXT PRIMARY KEY NOT NULL,
  owner_id TEXT NOT NULL,
  trigger_id TEXT NOT NULL REFERENCES routine_triggers(id) ON DELETE CASCADE,
  routine_id TEXT NOT NULL REFERENCES routines(id) ON DELETE CASCADE,
  routine_version_id TEXT NOT NULL REFERENCES routine_versions(id),
  delivery_key TEXT NOT NULL,
  trigger_json TEXT NOT NULL CHECK(json_valid(trigger_json)),
  input_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(input_json)),
  status TEXT NOT NULL CHECK(status IN ('queued','running','retry_wait','succeeded','failed','cancelled','skipped')),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  scheduled_for_ms INTEGER NOT NULL,
  next_attempt_at_ms INTEGER NOT NULL,
  cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)),
  error_message TEXT,
  created_at_ms INTEGER NOT NULL,
  started_at_ms INTEGER,
  finished_at_ms INTEGER,
  UNIQUE(trigger_id, delivery_key)
);
CREATE INDEX routine_jobs_due ON routine_jobs(owner_id, status, next_attempt_at_ms, scheduled_for_ms);
CREATE INDEX routine_jobs_active ON routine_jobs(owner_id, routine_id, status);

ALTER TABLE routine_runs ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 1 CHECK(attempt_count > 0);
ALTER TABLE routine_runs ADD COLUMN scheduled_for_ms INTEGER;
ALTER TABLE routine_runs ADD COLUMN bot_id TEXT REFERENCES bots(id) ON DELETE SET NULL;
UPDATE routine_runs SET bot_id = (SELECT bot_id FROM routines WHERE routines.id = routine_runs.routine_id) WHERE bot_id IS NULL;
