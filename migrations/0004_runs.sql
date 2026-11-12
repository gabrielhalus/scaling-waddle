CREATE TABLE events (
    id              TEXT PRIMARY KEY,
    workflow_id     TEXT NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    source          TEXT NOT NULL,
    type            TEXT NOT NULL,
    payload         TEXT NOT NULL,
    correlation_id  TEXT NOT NULL,
    idempotency_key TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE UNIQUE INDEX events_idempotency ON events (workflow_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE TABLE runs (
    id               TEXT PRIMARY KEY,
    workflow_id      TEXT NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    version          INTEGER NOT NULL,
    event_id         TEXT NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    status           TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'waiting', 'succeeded', 'failed', 'cancelled')),
    error            TEXT,
    lease_owner      TEXT,
    lease_expires_at INTEGER,
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finished_at      TEXT
) STRICT;

CREATE INDEX runs_queue ON runs (status, created_at);

CREATE TABLE step_runs (
    run_id      TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    step_id     TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'waiting', 'succeeded', 'failed', 'skipped', 'cancelled')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    output      TEXT,
    error       TEXT,
    started_at  TEXT,
    finished_at TEXT,
    PRIMARY KEY (run_id, step_id)
) STRICT;
