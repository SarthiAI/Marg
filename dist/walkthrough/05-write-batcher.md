# 05 - Async write batcher

## Goal

The async write batcher (ADR-012) absorbs steady-state load with zero
overflow, fills batches by both size and age, and fail-closes with 503
`storage_overloaded` when the queue saturates.

## Setup

Use the single-node-prod profile (Postgres + Redis), since the SQLite path
serialises writes and obscures the batcher's behaviour.

## Steps

```walkthrough
# 1. Steady load: batcher flushes by size + age, overflow stays at 0
LOOP 5000 PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=false
ASSERT metric marg_write_batcher_overflow_total == 0
ASSERT metric marg_write_batcher_flushes_total{outcome="ok"} > 0
ASSERT metric marg_write_batcher_flushes_total{kind="request_log"} > 0
ASSERT metric marg_write_batcher_flushes_total{kind="spend"} > 0

# 2. Synthetic overload: pause Postgres so the batcher backs up
DOCKER pause postgres
LOOP 200 PROBE POST /v1/chat/completions accept-any-of 200,503 bearer $KEY body @stream=false
ASSERT metric marg_write_batcher_overflow_total > 0
ASSERT at least one PROBE returned 503 with x-marg-reason: storage_overloaded

# 3. Resume Postgres: queue drains, no data loss for in-memory entries that hadn't overflowed
DOCKER unpause postgres
SLEEP 2
ASSERT metric marg_write_batcher_queue_depth approaches 0
```

## Expected

- Steady state: `marg_write_batcher_overflow_total` does not increment.
- Pause: queue depth climbs, overflow counter increments, fresh requests
  fail closed with `storage_overloaded`. Previously-enqueued entries are
  not silently dropped (they flush on unpause).

## Cleanup

`docker unpause` postgres, full Docker teardown per the global rule.
