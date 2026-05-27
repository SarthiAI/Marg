# Marg end-to-end walkthrough

This folder contains the v1.0 end-to-end walkthrough that exercises every
Marg + Kavach feature from a fresh checkout. The walkthrough is **manual +
scripted**: there is no automated test harness, each scenario is a Markdown
file with the exact commands to run and the expected observable outcome, and
`run.sh` drives the scenarios in order so an operator can reproduce them
end-to-end on a fresh box.

The walkthrough is the canonical "did this build actually work?" answer. A
release ships when every scenario in this folder is green on the dev laptop
*and* on Civo Round D (single-node-prod with Postgres + Redis in Docker).

## How to run

```bash
cd marg
./dist/walkthrough/run.sh                         # runs every scenario
./dist/walkthrough/run.sh 03-providers            # runs only the named scenario
MARG_WT_KEEP_ARTIFACTS=1 ./dist/walkthrough/run.sh # keep tmp dir on exit
```

`run.sh` requires:

- A built `marg` binary at `target/release/marg` (or `MARG_BIN` set).
- A built provider stub at `target/release/marg-provider-stub` (or
  `MARG_STUB_BIN` set).
- `curl`, `jq`, and a POSIX shell.

Postgres + Redis-backed scenarios require Docker. The script auto-detects
`docker` availability and skips those scenarios if Docker is missing, with a
visible `[skip]` line in the output.

## Layout

| File | What it covers |
|---|---|
| [01-boot-and-ops.md](01-boot-and-ops.md) | Boot, `/health`, `/ready`, `/version`, `/metrics`, SIGTERM, SIGHUP. |
| [02-auth-and-budgets.md](02-auth-and-budgets.md) | Unauthenticated, bad token, active key, budget cap, RPM cap, strict mode, revoke. |
| [03-providers.md](03-providers.md) | OpenAI / Anthropic / Google / Bedrock stubs, routing, failover, streaming, streaming cancel. |
| [04-storage-tiers.md](04-storage-tiers.md) | SQLite default, Postgres + Redis tier, fail-closed when Postgres / Redis stop. |
| [05-write-batcher.md](05-write-batcher.md) | Batcher fills by size + age, overflow returns 503 `storage_overloaded`. |
| [06-admin-api.md](06-admin-api.md) | Every admin endpoint via the bootstrap token. Malformed reload keeps previous policy serving. |
| [07-console.md](07-console.md) | Console pages render, login round-trip works, audit verify button works. |
| [08-cli.md](08-cli.md) | Every `marg` subcommand prints the documented shape. |
| [09-kavach-observe-enforce.md](09-kavach-observe-enforce.md) | Observe -> enforce flow, would-refuse audit entries. |
| [10-kavach-invariant.md](10-kavach-invariant.md) | `[[invariant]]` `param_max` refuses oversize `max_tokens`. |
| [11-kavach-permit-signing.md](11-kavach-permit-signing.md) | `x-kavach-permit` header carries a signed envelope, verify succeeds, byte-flip fails. |
| [12-kavach-drift.md](12-kavach-drift.md) | Geo, session-age, behavior drift each trigger `Invalidate`, key drops from local cache. |
| [13-kavach-audit-chain.md](13-kavach-audit-chain.md) | `audit verify` succeeds on the live chain, fails on a byte-flipped file. |
| [14-hot-reload.md](14-hot-reload.md) | SIGHUP and `POST /admin/policy/reload` apply changes without dropping in-flight traffic. |
| [15-mode-flip.md](15-mode-flip.md) | Observe -> enforce -> observe via reload, drift / refuse semantics flip accordingly. |
| [16-bench-repass.md](16-bench-repass.md) | Bench scenarios re-run on Round D: T01, T02, L02, L05, B01, B03 (default + strict), C01, L03, T06. |

Each scenario file is self-contained: `Goal`, `Setup`, `Steps`, `Expected`,
`Cleanup`. `run.sh` reads only the steps and expected lines from the
machine-readable fenced `walkthrough` blocks, but the prose around them is
the documentation an operator reads.

## Cluster scenarios

Cluster-mode coverage (cluster-3 / cluster-10) is a P11 concern. The
walkthrough here is single-node-dev + single-node-prod only.

## What a green run looks like

`run.sh` exits 0 only when every scenario reports `PASS`. A `FAIL` or a
`PARTIAL` (some steps skipped due to missing prereqs) exits non-zero. The
output appendix is written to `dist/walkthrough/results/<rfc3339>/` so a
team member can grep one place to answer "did 13-kavach-audit-chain pass on
the last build?".
