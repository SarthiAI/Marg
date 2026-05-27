# 16 - Bench scenarios re-pass

## Goal

Run the v1.0 benchmark scenarios on the single-node-prod rig (Civo Round
D) so we know the Kavach integration did not regress the Round C numbers
from P08, and so the two new Kavach-specific scenarios (L03 and T06) hit
their gates.

## Setup

Single-node-prod rig (Civo `g4c.medium` with Postgres + Redis in Docker,
`LimitNOFILE=1048576` via systemd-run). Marg built from the P10 tree,
Kavach `0.1.2`, in enforce mode with a synthetic 100-rule policy seeded
under `marg/bench/data/policy-100.toml`.

## Scenarios

| Scenario | Gate | Notes |
|---|---|---|
| T01 | >= 5,000 req/s, p95 < 25 ms | Round C floor: 5,996 req/s. |
| T02 | >= 5,000 streams/s, 0 dropped | Round C floor: 7,291 streams/s. |
| L02 | mean < 1 ms over >= 1 M samples | ADR-013. |
| L05 | budget-bound key adds <= 5% overhead | Round C floor: parity. |
| B01 | first_429 at index 2 (budget cutoff) | |
| B03 default | ~360 grants/key in 60s | token-bucket fairness. |
| B03 strict | <= cap+1 grants/key in 60s | strict-mode contract. |
| C01 | 100 OK + 100 502 (50/50 split with primary 5xx) | |
| L03 | mean < 1 ms, p99 < 5 ms with 100-rule policy + drift + invariants | new in P10 |
| T06 | >= 3,000 req/s in enforce with 100-rule policy | new in P10 |

## Steps

```walkthrough
RUN civo provision g4c.medium
RUN rsync marg/ to box
RUN cargo build --release on box (one build only, per project rule)
RUN bench/rigs/civo-roundD/bring-up.sh
RUN bench/rigs/civo-roundD/run.sh T01 T02 L02 L05 B01 B03-default B03-strict C01 L03 T06
RUN bench/report.py results/<ts> -> writes REPORT.md + updates BENCHMARKS.md rows
RUN bench/rigs/civo-roundD/teardown.sh
RUN civo destroy
```

## Expected

Every scenario passes its documented gate. The `BENCHMARKS.md` rows
update in place from each scenario's `summary.json`. `target/` directory
is `cargo clean`-ed on the box before teardown.

## Cleanup

Per the global Civo + Docker rules: stack down, `cargo clean`,
`docker system prune -a -f --volumes`, destroy the Civo instance.

## Sign-off

The walkthrough is complete only after this scenario reports `PASS` and
`bench/report.py` has refreshed `BENCHMARKS.md` with Round D numbers.
