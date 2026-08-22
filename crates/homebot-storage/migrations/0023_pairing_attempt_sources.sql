ALTER TABLE pairing_exchange_attempts ADD COLUMN source_digest BLOB;

CREATE INDEX pairing_exchange_attempts_source_time
    ON pairing_exchange_attempts(source_digest, attempted_at_ms);
