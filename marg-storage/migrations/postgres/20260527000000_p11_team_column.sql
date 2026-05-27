-- See the matching sqlite migration. Denormalised `team` on request_log,
-- indexed for filter queries. Production fleets that already have data will
-- have NULL for historical rows, which the team filter treats as "team not
-- known" rather than a match.
ALTER TABLE request_log ADD COLUMN IF NOT EXISTS team TEXT;
CREATE INDEX IF NOT EXISTS idx_request_log_team ON request_log(team);
