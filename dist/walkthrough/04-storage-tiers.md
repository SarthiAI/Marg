# 04 - Storage tiers

## Goal

SQLite default works end-to-end. Postgres + Redis tier works end-to-end.
Fail-closed semantics hold when Postgres or Redis are stopped.

## Prereqs

Docker available. `run.sh` skips this scenario otherwise and marks it
`SKIPPED`.

## Steps

```walkthrough
# Phase A: SQLite default
USE config single-node-dev
PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=false
PROBE GET /ready 200 jq '.storage.backend == "sqlite"'

# Phase B: Postgres + Redis in Docker
DOCKER UP postgres redis
USE config single-node-prod  # storage.dsn = pg, hot.backend = redis
PROBE GET /ready 200 jq '.storage.backend == "postgres" and .hot.backend == "redis"'
PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=false
ASSERT spend visible in Postgres `key_budget` table

# Phase C: restart marg with Redis flushed; budget persists via Postgres
DOCKER exec redis-cli FLUSHALL
RESTART marg
PROBE GET /admin/budgets/$KEY_ID 200 jq '.daily_used_usd > 0'

# Phase D: fail-closed when Postgres stops
DOCKER stop postgres
SLEEP 6  # auth-cache TTL
PROBE GET /ready 503 jq '.storage.healthy == false'
PROBE POST /v1/chat/completions 503 bearer $KEY body @stream=false header 'expect x-marg-reason: hot_store_unreachable'
DOCKER start postgres

# Phase E: fail-closed when Redis stops
DOCKER stop redis
PROBE POST /v1/chat/completions 503 bearer $KEY body @stream=false header 'expect x-marg-reason: hot_store_unreachable'
DOCKER start redis
```

## Expected

- Phase A: `marg_storage_query_duration_seconds{backend="sqlite",...}` is
  the only backend label observed.
- Phase B: per-request spend rows appear in Postgres
  `request_log` + `key_budget` tables; Redis carries rate-limit + reserve
  counters.
- Phase C: budget gauge restored on boot from Postgres; no Redis-only data
  required.
- Phase D and E: 503 responses with the documented `x-marg-reason`.

## Cleanup

Docker compose down + prune per the global Docker cleanup rule.
