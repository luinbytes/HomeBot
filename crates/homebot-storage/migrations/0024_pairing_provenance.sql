ALTER TABLE pairing_credentials
    ADD COLUMN native_proof_digest BLOB CHECK(native_proof_digest IS NULL OR length(native_proof_digest) = 32);

ALTER TABLE pairing_exchange_attempts
    ADD COLUMN source_digest BLOB NOT NULL
    DEFAULT X'0000000000000000000000000000000000000000000000000000000000000000'
    CHECK(length(source_digest) = 32);

CREATE INDEX pairing_exchange_attempts_source_time
    ON pairing_exchange_attempts(source_digest, attempted_at_ms);
