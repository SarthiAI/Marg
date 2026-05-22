# Marg benchmarks

This file is the single source of truth for every performance number Marg
claims. It is replaced (not appended) per release, so it always reflects the
most recent passing run on each documented rig.

Numbers below are not yet populated. P04 ships the metrics surface, the bench
report generator, and the `S01` soak scenario. The first full publication of
this file lands at the end of P07 when the v1.0 release runs the cluster-10
acceptance gates.

For now, run any single scenario locally and re-render this file with
`bench/report.py marg/bench/results/<run-id>` (see
`marg/bench/README.md`).

## Run manifest

| Field | Value |
|---|---|
| Marg version | `unreleased` |
| Build commit | `unreleased` |
| Kavach version | n/a (P08+) |
| Report generated | (re-rendered per run) |

## Rigs

See `marg/bench/rigs/` for bring-up scripts.

| Rig | Spec | Used by |
|---|---|---|
| dev-laptop | 8 core, 16 GB | local smoke runs |
| single-node-prod | 16 core, 32 GB, NVMe | L01, L02, L04, L05, T01, T02, T06, S01, K01, K03 to K06, B01, B03 |
| cluster-3 | 3 nodes + Redis + Postgres | T03, C02, C03, C04, C06, K02, S02, B02 |
| cluster-10 | 10 nodes + Redis cluster + Postgres HA | T04, T05, T07 |

## Latency

| ID | Name | Rig | Target | Gate | Result |
|---|---|---|---|---|---|
| L01 | cold-start | single-node-prod | first request after boot | < 1.5s | pending |
| L02 | hot-path-decision-time | single-node-prod | decision time, p99 over 1M | < 1ms | pending |
| L04 | streaming-first-token | single-node-prod | first token byte to client | p99 < 10ms | pending |
| L05 | budget-check-overhead | single-node-prod | added latency from quota middleware | < 100us p99 | pending |

## Throughput

| ID | Name | Rig | Target | Gate | Result |
|---|---|---|---|---|---|
| T01 | single-instance-passthrough | single-node-prod | sustained req/s | >= 50 000, p99 < 50ms | pending |
| T02 | single-instance-streaming | single-node-prod | concurrent streams | >= 10 000 | pending |
| T03 | cluster-3-passthrough | cluster-3 | sustained req/s | >= 150 000, p99 < 75ms | pending |
| T04 | cluster-10-passthrough | cluster-10 | sustained req/s | >= 500 000, p99 < 100ms | pending |
| T05 | cluster-10-million-target | cluster-10 | 1M+ req/s sustained 1h | >= 1 000 000, p99 < 150ms | pending |

## Soak

| ID | Name | Rig | Duration | Gate | Result |
|---|---|---|---|---|---|
| S01 | single-instance-24h | single-node-prod | 24h at 80% of T01 | RSS growth < 5%, p99 drift < 10%, zero panics | pending |
| S02 | cluster-3-24h | cluster-3 | 24h at 80% of T03 | S01 gates + zero broadcast losses | pending |
| S03 | kavach-audit-growth | single-node-prod | 7 days | audit flush latency documented | pending |

## Failover and chaos

| ID | Name | Rig | Target | Gate | Result |
|---|---|---|---|---|---|
| C01 | provider-5xx-failover | single-node-prod | primary 503 for 1 min | < 100ms added latency, zero drops | pending |
| C02 | random-instance-kill | cluster-3 | kill 1 marg every 2 min | recovery within 30s | pending |
| C03 | redis-partition | cluster-3 | sever Redis 30s | fail closed, full recovery 5s | pending |
| C04 | postgres-failover | cluster-3 | primary swap during load | <= 10s downtime | pending |
| C05 | disk-full | single-node-prod | fill audit disk | 503 immediately, no panic | pending |
| C06 | clock-skew | cluster-3 | 30s skew on one node | policy windows hold | pending |

## Cost control

| ID | Name | Rig | Target | Gate | Result |
|---|---|---|---|---|---|
| B01 | budget-exhaustion-cutoff | single-node-prod | drive key to cap | cutoff within 1 req, decision < 1ms | pending |
| B02 | budget-counter-consistency-cluster | cluster-3 | concurrent traffic to one key | final counter drift < 0.5% | pending |
| B03 | rate-limit-fairness | single-node-prod | 100 keys at limit | proportional 429s, no starvation | pending |

## How to regenerate this file

1. Run the relevant scenarios:
   ```
   bench/scenarios/<id>-*.sh
   ```
   Each scenario writes its output to
   `bench/results/<YYYY-MM-DD>-<git-sha>/<id>-<name>/`.
2. Run the report generator:
   ```
   bench/report.py bench/results/<YYYY-MM-DD>-<git-sha>
   ```
   It updates `BENCHMARKS.md` in place with the pass / fail / result column
   for every scenario whose results directory exists under the run id.
3. Commit `BENCHMARKS.md` and the result directory.
