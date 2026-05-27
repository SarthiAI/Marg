-- Denormalise `team` into request_log so per-team cost reports and per-team
-- request listings can be served by a single index lookup, not a JOIN against
-- the keys table. Added in P11 after the business-use-case suite surfaced
-- that compliance needs `GET /admin/requests?team=...` as a first-class
-- filter, and the request_log table is hot enough that the JOIN was the
-- wrong shape.
ALTER TABLE request_log ADD COLUMN team TEXT;
CREATE INDEX IF NOT EXISTS idx_request_log_team ON request_log(team);
