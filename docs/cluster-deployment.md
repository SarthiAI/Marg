# Cluster deployment

Marg is stateless. Any instance can serve any request. Horizontal
scale is shared Redis (hot counters) and shared Postgres (durable
state). This page is the recipe for running 3 to 100 Marg instances
behind a single endpoint.

## Components

```
                 +-----------+
clients -------> | LB or AWS | --(public)--> N x Marg instances
                 |  ALB / NLB|                  |
                 +-----------+                  |
                                                | (private)
                                                v
                              +-------------------+  +-------------+
                              | RDS Postgres (HA) |  | ElastiCache |
                              |  durable state    |  | Redis (HA)  |
                              +-------------------+  +-------------+
```

Marg instances:
- Stateless. Restart any time, scale by adjusting the LB target
  group.
- Identical `marg.toml` on every node. The only per-node difference is
  the listen address (`0.0.0.0:8080`).
- Bind admin port to the private subnet (or behind a separate LB with
  strict ingress). Never publish admin to the open Internet.

Redis:
- Must be reachable from every Marg node.
- Single-shard ElastiCache is fine up to ~10 Marg nodes. Beyond
  that, switch to clustered mode (`redis_cluster_mode = true` in
  `[storage.hot]`).
- Enable transit encryption and at-rest encryption.

Postgres:
- Multi-AZ is recommended for production. Marg writes during the
  cold path (key insert, budget settle, audit flush in v2.0) and
  reads on first-use of a key. RDS r6g.2xlarge handles cluster-3
  traffic with headroom. r6g.4xlarge or larger for cluster-10.
- **`max_connections` on the Postgres side must cover the sum of
  every Marg instance's `[storage].max_connections` value plus
  enough headroom for `psql` sessions, migrations, and monitoring.**
  Default Marg pool is 200; default Postgres `max_connections` is
  100. For a 3-node cluster running default pools, set Postgres
  `max_connections` to at least 650 (200 x 3 + 50). For RDS, edit
  the parameter group; for self-hosted Postgres, set it in
  `postgresql.conf` and restart. `/ready` returns 503 the moment a
  Marg instance cannot reach Postgres, so misconfigured pools
  surface via the load balancer health check rather than as silent
  request failures.

Host-level tuning per Marg node:
- File descriptors: soft and hard limit at 1,048,576. The
  `dist/systemd/marg.service` shipped with the release pins
  `LimitNOFILE=1048576`; manual installations need
  `/etc/security/limits.d/marg.conf` per the install guide. Marg
  itself logs a `tracing::warn` line at boot if the soft limit is
  below 65,536.
- Kernel TCP defaults: most distributions are fine out of the box
  for the cluster-3 traffic shape. Document any host-specific
  tuning in your runbook.

## marg.toml for cluster mode

```toml
[server]
bind = "0.0.0.0:8080"

[storage]
backend = "postgres"
dsn     = "env:MARG_PG_DSN"

[storage.hot]
backend = "redis"
url     = "env:MARG_REDIS_URL"
key_prefix = "marg"
redis_cluster_mode = true     # only when using cluster-mode Redis

[admin]
bind = "0.0.0.0:8081"
bootstrap_token_path = "/var/lib/marg/admin-bootstrap.token"

[security]
log_prompts   = false
log_responses = false
```

Per node, expose env vars:

```
MARG_PG_DSN="postgres://marg:<pw>@<rds-endpoint>:5432/marg"
MARG_REDIS_URL="redis://<elasticache-endpoint>:6379"
OPENAI_API_KEY="..."
```

## First boot

On exactly one node, run:

```bash
marg db migrate --config /etc/marg/marg.toml
```

This applies the schema. Subsequent starts on every node skip the
migration (idempotent).

`marg admin bootstrap` mints the first admin token. Either run it
explicitly, or let `marg start` auto-mint on first boot via the
`[admin].bootstrap_token_path` path. The token is what you use to
register routes, keys, and budgets through the admin API.

## Load balancer

Use a TCP/HTTP load balancer with sticky-session disabled (Marg is
stateless; sticky-session is pure waste). Health check
`/ready` returns 200 when the node is fully up and 503 when either
the durable backend or the hot store is unreachable; the LB drops
the node from rotation in that case.

AWS recommendations:
- ALB if the clients speak HTTP/1.1 or HTTP/2.
- NLB if you need the lowest possible added latency and your clients
  do their own TLS termination.

## Capacity sizing

Two columns: measured on a 16-core single-node rig, and projected
(linear extrapolation for the cluster shapes; cluster validation
is pending the P10 acceptance run).

| Tier | Marg nodes | Redis | Postgres | Sustained RPS (per cluster) | p99 | Source |
|------|------------|-------|----------|-----------------------------|-----|--------|
| single-node-prod | 1 x 16-core box | local Redis | local Postgres | **~6,000 req/s** (5,998 sustained 5 min, p95 14 ms) | < 50 ms measured | single-instance measured |
| cluster-3 | 3 x 16-core nodes | r6g.xlarge | r6g.2xlarge | ~18,000 req/s projected | < 75 ms target | linear extrapolation, P10 acceptance run pending |
| cluster-10 | 10 x 16-core nodes | r6g.2xlarge cluster | r6g.4xlarge HA | ~60,000 req/s projected | < 150 ms target | linear extrapolation, P10 acceptance run pending |

Hot-path decision time (key lookup, budget check, rate-limit window)
sits at mean 0.73 ms over 1.96 M samples. The upstream LLM call is
the latency floor; Marg's own work is below the noise on a single
chat.

These are Marg's per-instance capacity numbers, not end-to-end
throughput. Real apps' wall-clock per chat is dominated by the LLM,
not by Marg. See `docs/install.md` "What out of the box performance
means" for the explanation.

Production-grade single-node performance requires two operator
steps that the documentation lists in one place at
`docs/install.md` "Production checklist". They are:

1. Raise Postgres `max_connections` to cover Marg's pool plus
   headroom (`ALTER SYSTEM SET max_connections = 300;` for a
   single-node deploy with the default pool).
2. Install the shipped `dist/systemd/marg.service` so
   `LimitNOFILE=1048576` is pinned and process supervision is
   correct.

If either is skipped, Marg surfaces the symptom in the boot log
(`RLIMIT_NOFILE` warn line) or under load (`/ready` 503 + `too
many clients already` in `marg.log`). Nothing fails silently.

## Scaling out

```
+1 Marg node:
  - Provision the host (same marg.toml, same env vars)
  - Start systemd unit
  - Health check passes -> LB adds it to rotation
```

That is the whole procedure. No rebalance, no rolling restart, no
quorum negotiation. Marg discovers nothing about its peers. All
shared state is in Redis and Postgres.

## Scaling down

```
-1 Marg node:
  - LB drains the target group (waits for in-flight requests)
  - systemctl stop marg  (graceful shutdown drains keep-alive)
  - terminate the host
```

`SIGTERM` flips both ports into draining mode and waits up to 30s
for in-flight requests to finish. Streaming connections that exceed
the drain window are closed cleanly with a final SSE event so the
client can reconnect.

## Disaster recovery

| Failure | Behaviour |
|---------|-----------|
| One Marg node dies | LB routes around it. No data loss for committed writes; in-flight requests on the dead node fail with 502. |
| Redis partition | Every node returns 503 with `x-marg-reason: hot_store_unreachable`. No request is silently permitted. Recovery within 5s of restore. |
| Postgres failover | Marg holds the connection pool and resumes when the new primary is reachable. Cold-cache lookups during the swap return 503. |
| Disk fills on one node | Marg surfaces 503 immediately on that node. No silent log loss. Other nodes unaffected. |

Each of these has a documented chaos scenario in the internal
benchmark suite (kept outside this repo). The cluster-3 and
cluster-10 rigs drive them as part of the P10 acceptance gate.

## Observability checklist for the LB and dashboards

- `marg_requests_total{provider, model, status}` rate and p99
- `marg_request_duration_seconds_bucket` histogram (use for
  Apdex-style SLOs)
- `marg_provider_errors_total{provider, kind}` (the `client_disconnect`
  kind shows how often a streaming client closed early after P08)
- `marg_failover_total{from_provider, to_provider}`
- `marg_budget_remaining_usd{key_id}` (alert when below threshold
  for any key flagged "critical")
- `marg_storage_query_duration_seconds{operation, backend}` and
  `marg_hot_store_query_duration_seconds{operation}` for backend
  health
- `marg_decision_duration_seconds` histogram for the in-process
  decision path (auth + parse + route + quota). The Marg overhead
  on top of the upstream call is approximately p99 of this
  histogram, so dashboards should plot it directly next to
  `marg_request_duration_seconds` and the upstream latency
  signal from the provider client.
- `marg_write_batcher_queue_depth` gauge and
  `marg_write_batcher_flushes_total{outcome, kind}` /
  `marg_write_batcher_rows_total{kind}` counters. A persistently
  rising queue depth signals durable-storage saturation; the
  `marg_write_batcher_overflow_total` counter increments every time
  Marg refused a request with 503 `storage_overloaded` because
  the queue was full.

Scrape `/metrics` from every Marg node into Prometheus. The
admin port also exposes `/metrics` so a same-origin console can
show health without CORS.

## Async write batcher and eventual consistency

After P08 the per-request `add_spend` and `append_request_log`
writes do not happen synchronously on the request path. They are
enqueued onto a bounded channel and flushed in batches by a
background task. Tuning lives at `[storage.write_batcher]`:

| Field | Default | Meaning |
|-------|---------|---------|
| `channel_depth` | `10000` | Maximum in-flight jobs across all instances. Set higher when bursty traffic outruns Postgres briefly. |
| `max_batch_size` | `256` | Maximum rows flushed in one multi-row INSERT. Postgres latency scales sub-linearly with batch size. |
| `max_batch_age_ms` | `50` | Maximum time a job waits before its batch is flushed even when not full. |

Two semantics to know:

1. **Eventual consistency.** The `spent` counter in durable storage
   lags the live request stream by up to `max_batch_age_ms`. The
   hot-store budget reservation is still synchronous, so a key
   over its daily cap still gets refused immediately; the durable
   counter is the reporting view, not the gate.
2. **Fail-closed back-pressure.** A full queue is refused with 503
   and `x-marg-reason: storage_overloaded`. Marg never silently
   drops a write. Alert on
   `marg_write_batcher_overflow_total` being non-zero; scale out
   Postgres or raise `channel_depth` to clear it.
