CREATE TABLE webhook_tokens (
    id          TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    revoked_at  TEXT
) STRICT;

CREATE INDEX webhook_tokens_workflow ON webhook_tokens (workflow_id);
