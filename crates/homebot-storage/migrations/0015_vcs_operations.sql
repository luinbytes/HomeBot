CREATE TABLE vcs_operation_results (
    idempotency_key TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK(length(action) BETWEEN 1 AND 80),
    response_json TEXT NOT NULL CHECK(json_valid(response_json)),
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE
);

CREATE INDEX idx_vcs_operation_results_owner_chat
    ON vcs_operation_results(owner_id, chat_id, created_at_ms);
