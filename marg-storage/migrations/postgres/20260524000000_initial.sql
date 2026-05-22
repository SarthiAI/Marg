-- P03 initial schema for Postgres. Consolidated (keys + budgets + counters +
-- request_log with all P01/P02 columns). Operators starting on Postgres get
-- this in one migration.

CREATE TABLE IF NOT EXISTS keys (
    id              TEXT PRIMARY KEY,
    token_hash      TEXT NOT NULL UNIQUE,
    token_prefix    TEXT NOT NULL,
    principal_id    TEXT NOT NULL,
    principal_kind  TEXT NOT NULL CHECK (principal_kind IN ('user', 'service', 'agent')),
    status          TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    created_at      TIMESTAMPTZ NOT NULL,
    revoked_at      TIMESTAMPTZ,
    team            TEXT
);
CREATE INDEX IF NOT EXISTS idx_keys_token_hash ON keys(token_hash);
CREATE INDEX IF NOT EXISTS idx_keys_principal ON keys(principal_id);
CREATE INDEX IF NOT EXISTS idx_keys_team ON keys(team);

CREATE TABLE IF NOT EXISTS budgets (
    key_id      TEXT PRIMARY KEY REFERENCES keys(id) ON DELETE CASCADE,
    daily_usd   DOUBLE PRECISION NOT NULL DEFAULT 0,
    rpm         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS budget_counters (
    key_id      TEXT NOT NULL REFERENCES keys(id) ON DELETE CASCADE,
    day         DATE NOT NULL,
    spent_usd   DOUBLE PRECISION NOT NULL DEFAULT 0,
    PRIMARY KEY (key_id, day)
);
CREATE INDEX IF NOT EXISTS idx_budget_counters_key_day ON budget_counters(key_id, day);

CREATE TABLE IF NOT EXISTS request_log (
    id              TEXT PRIMARY KEY,
    timestamp       TIMESTAMPTZ NOT NULL,
    key_id          TEXT NOT NULL,
    principal_id    TEXT NOT NULL,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    input_tokens    BIGINT NOT NULL DEFAULT 0,
    output_tokens   BIGINT NOT NULL DEFAULT 0,
    cost_usd        DOUBLE PRECISION NOT NULL DEFAULT 0,
    latency_ms      BIGINT NOT NULL,
    status          INTEGER NOT NULL,
    stream          BOOLEAN NOT NULL DEFAULT FALSE,
    error           TEXT,
    attempts        JSONB
);
CREATE INDEX IF NOT EXISTS idx_request_log_timestamp ON request_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_request_log_key ON request_log(key_id);
