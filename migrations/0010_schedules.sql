CREATE TABLE schedules (
    workflow_id TEXT PRIMARY KEY REFERENCES workflows (id) ON DELETE CASCADE,
    last_fire   INTEGER NOT NULL
) STRICT;
