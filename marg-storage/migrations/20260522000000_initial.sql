-- P01 initial schema for SQLite.

CREATE TABLE IF NOT EXISTS keys (
    id              TEXT PRIMARY KEY,
    token_hash      TEXT NOT NULL UNIQUE,
    token_prefix    TEXT NOT NULL,
    principal_id    TEXT NOT NULL,
    principal_kind  TEXT NOT NULL CHECK (principal_kind IN ('user', 'service', 'agent')),
    status          TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    created_at      TEXT NOT NULL,
    revoked_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_keys_token_hash ON keys(token_hash);
CREATE INDEX IF NOT EXISTS idx_keys_principal ON keys(principal_id);

CREATE TABLE IF NOT EXISTS budgets (
    key_id      TEXT PRIMARY KEY,
    daily_usd   REAL NOT NULL DEFAULT 0,
    rpm         INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (key_id) REFERENCES keys(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS budget_counters (
    key_id      TEXT NOT NULL,
    day         TEXT NOT NULL,
    spent_usd   REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (key_id, day),
    FOREIGN KEY (key_id) REFERENCES keys(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_budget_counters_key_day ON budget_counters(key_id, day);

CREATE TABLE IF NOT EXISTS request_log (
    id              TEXT PRIMARY KEY,
    timestamp       TEXT NOT NULL,
    key_id          TEXT NOT NULL,
    principal_id    TEXT NOT NULL,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    cost_usd        REAL NOT NULL DEFAULT 0,
    latency_ms      INTEGER NOT NULL,
    status          INTEGER NOT NULL,
    stream          INTEGER NOT NULL DEFAULT 0,
    error           TEXT
);

CREATE INDEX IF NOT EXISTS idx_request_log_timestamp ON request_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_request_log_key ON request_log(key_id);
