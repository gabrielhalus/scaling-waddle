CREATE TABLE integrations (
    id         TEXT PRIMARY KEY,
    project    TEXT NOT NULL,
    name       TEXT NOT NULL,
    base_url   TEXT NOT NULL,
    headers    TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project, name)
) STRICT;
