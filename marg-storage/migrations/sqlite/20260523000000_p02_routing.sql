-- P02 schema: team on keys, attempts JSON chain on request_log.

ALTER TABLE keys ADD COLUMN team TEXT;
CREATE INDEX IF NOT EXISTS idx_keys_team ON keys(team);

ALTER TABLE request_log ADD COLUMN attempts TEXT;
