# Marg benchmark suite

Every production-scale number Marg claims is backed by a runnable benchmark
that lives here. The full plan, scenario catalogue, rig definitions, and
acceptance gates are documented in `../../build-state/architecture/testing-strategy.md`.

## Tree

```
bench/
├── provider-stub/      deterministic fake LLM provider (built as part of the workspace)
├── data/               synthetic prompts, key fixtures, tool-call corpus (P01 onward)
├── scenarios/          k6 and shell scenario scripts (P01 onward)
├── rigs/               hardware-tier bring-up scripts (dev-laptop in P00, others later)
├── results/            committed benchmark output per release
└── report.py           generate REPORT.md per run and refresh BENCHMARKS.md
```

## Status

P00 ships the directory tree and a stub `provider-stub` crate that compiles as
part of the workspace. Real scenarios and data corpus start landing in P01 per
`testing-strategy.md`.

## Running benchmarks

Per-phase exit criteria list the scenarios that must pass before the phase is
`DONE`. Each scenario has a run command documented in its own file under
`scenarios/`. The standing acceptance discipline is locked by ADR-008.

## Per-run reporting

Each rig's runner script writes scenario output under
`results/<YYYY-MM-DD>-<git-sha>/<scenario-id>-<name>/` along with a
`summary.json` describing the headline number and whether the acceptance gate
passed. After a run completes:

```
bench/report.py bench/results/<run-id>
```

writes `REPORT.md` inside the run directory and refreshes the matching rows
in the repo's `BENCHMARKS.md` so the published numbers always reflect the
most recent passing run. `--no-update` prints the report without touching
`BENCHMARKS.md`.

## Notes for operators of this folder

- Do not delete `results/` directories. They are the historical record.
- Re-run stale results (older than 90 days) before any release work.
- Real-provider scenarios (`R01` to `R05`) require live API keys and are run
  out of band, not in CI.
