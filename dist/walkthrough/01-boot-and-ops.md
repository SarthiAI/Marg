# 01 - Boot and ops surface

## Goal

Marg boots cleanly from a fresh checkout, exposes every documented ops
endpoint, and shuts down gracefully on SIGTERM. Policy reload on SIGHUP
produces a `marg.policy_reload.v1` chain entry.

## Setup

1. Working directory is the `marg/` repo root.
2. `target/release/marg` and `target/release/marg-provider-stub` are built.
3. `dist/walkthrough/run.sh` writes a temporary `marg.toml` with the stub
   provider as the only registered provider, observe mode, SQLite default,
   and `[admin].bootstrap_token_path = "$TMP/admin.token"`.

## Steps

```walkthrough
PROBE GET /health 200
PROBE GET /ready 200
PROBE GET /version 200 jq '.marg' '.kavach_core' '.kavach_pq' '.kavach_redis'
PROBE GET /metrics 200 grep 'marg_requests_total'
PROBE SIGHUP marg
PROBE GET /admin/audit/entries?since=0&limit=5 200 jq '.entries[] | select(.data.schema == "marg.policy_reload.v1")'
PROBE SIGTERM marg
EXPECT exit 0
```

## Expected

- `/health` returns `{"status":"ok"}`.
- `/ready` returns `{"status":"ready","storage":{...},"hot":{...}}`.
- `/version` returns `{"marg":"...","kavach_core":"0.1.2","kavach_pq":"0.1.2","kavach_redis":"0.1.2",...}`.
- `/metrics` exposes the Prometheus text format with at least one
  `marg_requests_total{...}` line after the first chat completion.
- After `SIGHUP`, the audit chain has one new entry with
  `data.schema == "marg.policy_reload.v1"`.
- `SIGTERM` triggers the documented drain log line and exits 0 within 5
  seconds.

## Cleanup

`run.sh` removes the temporary marg.toml + sqlite + admin.token unless
`MARG_WT_KEEP_ARTIFACTS=1` is set.
