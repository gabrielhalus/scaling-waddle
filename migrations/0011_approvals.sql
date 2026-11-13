CREATE TABLE approvals (
    id           TEXT PRIMARY KEY,
    run_id       TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    step_id      TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'rejected', 'expired', 'cancelled')),
    message      TEXT,
    requested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    decided_by   TEXT,
    decided_at   TEXT,
    comment      TEXT,
    UNIQUE (run_id, step_id)
) STRICT;

CREATE INDEX approvals_status ON approvals (status, requested_at);
