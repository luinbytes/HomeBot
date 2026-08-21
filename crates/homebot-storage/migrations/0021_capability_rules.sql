CREATE TABLE capability_rules (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    capability TEXT NOT NULL,
    effect TEXT NOT NULL CHECK (effect IN ('allow', 'require_approval', 'deny')),
    device_id TEXT,
    bot_id TEXT,
    chat_id TEXT,
    workspace_id TEXT,
    action_prefix TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX capability_rules_owner ON capability_rules(owner_id, capability, effect, created_at_ms);

CREATE TABLE capability_rule_audit (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('created', 'updated', 'deleted')),
    snapshot_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX capability_rule_audit_owner ON capability_rule_audit(owner_id, created_at_ms, id);
