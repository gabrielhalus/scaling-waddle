CREATE TABLE secrets (
    id         TEXT PRIMARY KEY,
    project    TEXT NOT NULL,
    name       TEXT NOT NULL,
    key_id     TEXT NOT NULL,
    nonce      BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (project, name)
) STRICT;
