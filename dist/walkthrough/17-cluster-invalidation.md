# 17 - Cluster signed key invalidation

## Goal

A key killed on one Marg node is dropped on every other node within a second,
and the cross-node message is post-quantum signed so a forged "kill key"
message published straight to Redis is rejected (ADR-027).

## Setup

Two Marg processes on one dev laptop, sharing one Redis and one Postgres, with
the **same** Kavach keypair so each can verify the other's signatures.

```bash
# Shared backends (Docker)
docker run -d --name marg-redis -p 6379:6379 redis:7
docker run -d --name marg-pg -e POSTGRES_PASSWORD=marg -p 5432:5432 postgres:16

# One config, two nodes. Both point at the same Redis + Postgres and the same
# keypair file. node_id differs only for nicer logs.
#   [storage].backend = "postgres", dsn -> the pg above
#   [storage.hot].backend = "redis", url -> the redis above
#   [kavach].mode = "enforce", keypair_path = "/tmp/marg-cluster.key" (same file)
#   [kavach.cluster].invalidation_channel = "marg:kavach:invalidation"

MARG_NODE=A ./marg start --config node-a.toml   # binds :8080 / :8081
MARG_NODE=B ./marg start --config node-b.toml   # binds :9080 / :9081
```

A test key with the engineer permit rule, plus a drift knob that trips easily
(for example `session_age_max = "1s"` or a geo jump), so an Invalidate is easy
to provoke on node A.

## Steps

```walkthrough
# A. Both nodes serve the same key while it is healthy.
PROBE :8080 POST /v1/chat/completions 200 bearer $KEY body @model=gpt-4o-mini
PROBE :9080 POST /v1/chat/completions 200 bearer $KEY body @model=gpt-4o-mini

# B. Trip an invalidation on node A (geo drift jump, or admin invalidate).
PROBE :8080 POST /v1/chat/completions 403 bearer $KEY body @model=gpt-4o-mini \
  header 'x-forwarded-geo: IN;city=Mumbai;lat=19.07;lon=72.87' \
  header 'expect x-marg-reason: kavach_invalidate'

# C. Within ~1s node B refuses the same key WITHOUT any drift trigger of its
#    own: it received and applied node A's signed invalidation.
SLEEP 1
PROBE :9080 POST /v1/chat/completions 401 bearer $KEY body @model=gpt-4o-mini

# D. Both nodes' audit chains carry a marg.key_event.v1 (kind=invalidated).
#    On node B the principal is "cluster" (applied from a peer).
ADMIN :9081 GET /admin/audit/entries 200 jq 'any(.entries[]; .data.principal == "cluster" and .data.kind == "invalidated")'

# D2. Operator kill also propagates. Invalidate a DIFFERENT key on node A via
#     the admin API; node B drops it cluster-wide too (works in any mode).
ADMIN :8081 POST /admin/keys/$KEY3_ID/invalidate 200
SLEEP 1
CHECK :9081 /metrics 'marg_cluster_invalidations_total{direction="received",result="ok"}' increased

# E. Forged message is rejected. Publish a plaintext (unsigned) payload
#    straight onto the channel; neither node acts on it.
RUN redis-cli -h localhost PUBLISH marg:kavach:invalidation '{"target":{"session":"00000000-0000-0000-0000-000000000000"},"reason":"forged","evaluator":"attacker"}'
SLEEP 1
# A previously-healthy second key still works on both nodes (the forged drop was ignored).
PROBE :8080 POST /v1/chat/completions 200 bearer $KEY2 body @model=gpt-4o-mini
PROBE :9080 POST /v1/chat/completions 200 bearer $KEY2 body @model=gpt-4o-mini
# The reject is counted.
CHECK :9081 /metrics contains 'marg_cluster_invalidations_total{direction="received",result="rejected"}'
```

## Expected

- A key killed on node A is refused on node B within a second, with no
  independent trigger on B.
- Node B's audit chain shows the invalidation applied from a peer
  (`principal = "cluster"`).
- An unsigned message published directly to the Redis channel is dropped by
  both nodes and counted under `result="rejected"`; unrelated keys keep
  working, proving the forged "kill" had no effect.
- `marg_cluster_invalidations_total` shows `published` on node A and
  `received` on node B.

## Observe-mode check (optional)

Set both nodes to `[kavach].mode = "observe"` and reload. Trip the same drift
on node A: node A logs the would-invalidate but does NOT broadcast, node B is
unaffected, and `marg_cluster_invalidations_total{result="suppressed"}`
increments on node A. This confirms observe mode never fans out a cluster-wide
key drop.

## Cleanup

Stop both nodes. `docker rm -f marg-redis marg-pg`. Remove the shared keypair
and the two node configs.
