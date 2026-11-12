CREATE TABLE workflows (
    id             TEXT PRIMARY KEY,
    project        TEXT NOT NULL,
    name           TEXT NOT NULL,
    active_version INTEGER,
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project, name)
) STRICT;

CREATE TABLE workflow_versions (
    workflow_id TEXT NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    hash        TEXT NOT NULL,
    definition  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (workflow_id, version)
) STRICT;
