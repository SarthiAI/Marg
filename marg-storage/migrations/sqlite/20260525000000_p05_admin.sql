-- P05 schema: admin auth tokens and runtime-managed routing rules.

CREATE TABLE IF NOT EXISTS admin_tokens (
    id              TEXT PRIMARY KEY,
    token_hash      TEXT NOT NULL UNIQUE,
    token_prefix    TEXT NOT NULL,
    label           TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL,
    revoked_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_admin_tokens_token_hash ON admin_tokens(token_hash);

CREATE TABLE IF NOT EXISTS routes (
    id                      TEXT PRIMARY KEY,
    position                INTEGER NOT NULL,
    match_model             TEXT,
    match_team              TEXT,
    primary_provider        TEXT,
    primary_model           TEXT,
    fallbacks_json          TEXT,
    split_json              TEXT,
    created_at              TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_routes_position ON routes(position);
