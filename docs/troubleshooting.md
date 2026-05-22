# Troubleshooting

Common operator-side symptoms and what to check first. Every error
response carries `x-marg-reason` and `x-request-id`. Use those as the
starting point.

## Marg returns 401

Symptom: `x-marg-reason: unauthorized`.

Cause: the `Authorization: Bearer <token>` header is missing or the
token has been revoked. Auth cache TTL is 60 seconds, so a freshly
revoked token may still authenticate for up to a minute. The console
explicitly invalidates the cache on revoke; if you revoked via the
HTTP API and want immediate effect, follow up with `POST
/admin/auth/cache/invalidate`.

## Marg returns 429

`x-marg-reason: budget_exceeded` means the key hit its USD cap.
Check `GET /admin/budgets/<key_id>`. To raise the cap live, `PATCH
/admin/budgets/<key_id>` with the new cap.

`x-marg-reason: rate_limited` means the key hit its rpm limit. The
window is per-minute, sliding. Either raise `rpm` on the key or wait.

## Marg returns 503

`x-marg-reason: hot_store_unreachable` means Redis is down or the
network path to Redis is severed. `/ready` will also be 503. Check
Redis health and the security group / iptables rules between Marg
and Redis. C03 (redis-partition) is the chaos scenario that
exercises this path.

`x-marg-reason: storage_unreachable` means the durable backend is
down (Postgres or SQLite file inaccessible). Same triage: check the
backend.

503 with no `x-marg-reason` and a body of `disk full` means the
local disk that backs the request log filled. Free space; the node
will return to /ready 200 automatically. C05 (disk-full) is the
chaos scenario.

## Streaming hangs

Symptom: the SSE connection opens but no chunks arrive.

Likely causes:
- The upstream provider is itself slow on the first token. Check
  `marg_request_duration_seconds_bucket{model="..."}` p99.
- A reverse proxy in front of Marg is buffering. Nginx and ALB both
  need explicit "no buffering" for SSE to flow through:
  - Nginx: `proxy_buffering off;` for the route.
  - ALB: idle-timeout >= the longest expected stream.
- The client's HTTP library is buffering. Many SDKs require an
  explicit `stream: true` and `stream_options: include_usage`.

## Failover not happening

Symptom: upstream 503 surfaces directly to the client (`x-marg-failovers: 0`).

Check:
- Was the failure actually retriable? 4xx never failovers.
- Does the matched route have a `fallback` list? Run `GET
  /admin/policy` to see the live engine. Config-file routes show up
  alongside DB routes.
- Did the fallback also fail? Look at `x-marg-attempts` on the
  response.

## "Same OpenAI request via Marg returns different JSON than direct"

It should not. R05 (streaming-correctness-live) compares both paths
byte-for-byte and the gate is zero mismatch. If you see a divergence
in production:
1. Capture the exact request body and a `x-request-id`.
2. Repeat the request directly to OpenAI with the same body.
3. Compare. The two should agree on every chunk except the optional
   reordering of the usage block.

## Console will not load

Symptom: the admin port responds to `/admin/...` but the browser
gets 404 on `/console/`.

The console bundle lives at `console/dist/` and is embedded at
compile time. A `cargo build` from a clean checkout where `console/
dist/` is missing falls back to a "not built" placeholder. Rebuild
the console:

```bash
cd marg/console
npm install
npm run build
cd ..
cargo build --release
```

## Postgres connection storm on startup

Marg opens its connection pool lazily, so the first request after
startup pays the connect cost. If the cluster is large (cluster-10
or beyond), every Marg instance racing for connections at boot can
saturate Postgres briefly. The fix is to:
- Set the `MARG_PG_POOL_WARMUP=true` env var (Marg pre-opens half
  the pool at startup, in 100 ms-spaced batches).
- Or run a `health_check` script on each node that hits Marg with a
  cheap admin call before the LB adds the node to rotation.

## Permission denied writing the admin bootstrap token

Symptom: `marg start` exits with "failed to write
bootstrap_token_path".

The directory must be writable by the user Marg runs as (`marg`
under systemd). On first boot Marg writes the token with mode 0600.
Either run as a user with write access, or pre-create the path with
the right ownership, or set `bootstrap_token_path = ""` and call
`marg admin bootstrap` explicitly.

## High p99, low p50

Tail latency under load almost always points at one of:
- Postgres slow query (look at
  `marg_storage_query_duration_seconds_bucket{backend="postgres"}` p99).
- A single noisy key starving the rest. Check
  `marg_requests_total{key_id}` rate per key.
- Provider tail latency. `marg_request_duration_seconds_bucket{provider}`
  vs the provider's own p99.

## I lost the admin token

If `bootstrap_token_path` still exists on disk, read it. Otherwise
the only recovery is to stop Marg, blow away the `admin_tokens`
table (or set every existing token's `revoked_at`), restart, and let
the bootstrap path mint a fresh one. There is no backdoor.

## Where to ask for help

Open an issue at `https://github.com/chirotpal/marg/issues` with:
- the Marg version (`marg --version`)
- the relevant log lines (the JSON access log has the request id)
- the `x-request-id` from the failing response
- the matched route shape (from `GET /admin/policy`)
