# Marg

Self-hosted AI gateway written in Rust. Applications point their LLM client at Marg instead of directly at OpenAI, Anthropic, Google, or Bedrock. Marg enforces budgets, routes between providers, gives one observability surface, and (in v2.0 with Kavach enabled) becomes a default-deny, cryptographically auditable governance gateway.

This is the v0.1 scaffold. The build is being assembled phase by phase. See `../build-state/INDEX.md` for the full roadmap.

## What works today (P00 to P03)

- A single static binary called `marg`.
- OpenAI-compatible Chat Completions endpoint (`POST /v1/chat/completions`), streaming and non-streaming.
- Provider adapters for OpenAI, Anthropic, Google Gemini, and AWS Bedrock. Apps speak the OpenAI shape; Marg translates to and from the upstream protocol.
- Config-driven routing with per-model glob match, per-team match, weighted A/B split, and one-shot failover on retriable upstream errors (5xx, timeout, network).
- Per-key budgets and per-key requests-per-minute rate limits, enforced on the hot path.
- Pluggable storage:
  - **SQLite** (default) for single-node development and small production.
  - **Postgres** for production single-node and small clusters.
  - **Redis** as an optional hot store paired with either backend for cluster-shared budget reservations and rate counters.
- Health, readiness, version endpoints. `/ready` reports the backend status for both storage and hot store.
- Graceful shutdown on SIGTERM / SIGINT.

Observability (Prometheus metrics, structured JSON logs), the admin HTTP API, and the console UI land in P04 through P06. Kavach governance lands in P08 and P09. Roadmap in `../build-state/INDEX.md`.

## Throughput tiers

Numbers below are the design targets for each deployment shape. Full benchmark scenarios live in `bench/scenarios/` and the acceptance gates are tracked in `../build-state/architecture/testing-strategy.md`. Per-release measured numbers will be published in `BENCHMARKS.md` from P04 onward.

| Tier | Backend | Hot store | Marg instances | Design target |
|---|---|---|---|---|
| Dev / small prod | SQLite | local (in-process) | 1 | ~1k req/s, single file, zero external services |
| Single-node prod | Postgres | local (in-process) or Redis | 1 | ~50k req/s on a 16-core box |
| Clustered prod | Postgres | Redis (required) | many | millions req/s aggregate (P07 cluster-10 acceptance gate) |

Hot-store invariant: if Redis is configured and unreachable, Marg refuses requests with `503 hot_store_unreachable` rather than silently degrading. Same fail-closed rule applies to the durable backend.

## Build

```bash
cd marg
cargo build --release
```

The release binary lands at `./target/release/marg`.

## Run

```bash
./target/release/marg start --config ./marg.toml
```

In another terminal:

```bash
curl http://localhost:8080/health
curl http://localhost:8080/version
```

Stop with `Ctrl-C` or `kill -TERM <pid>`.

## Configuration

See `marg.toml.example` for the documented config shape. Copy it to `marg.toml` and edit. Secrets in any `api_key` or `dsn` field accept three reference forms: `plain:<value>` (default), `env:<NAME>`, and `file:<path>`. Use the env or file forms when you do not want the secret to sit in the config file.

Switching the durable backend is one config change plus a migration:

```toml
[storage]
backend = "postgres"
dsn = "env:MARG_PG_DSN"

[storage.hot]
backend = "redis"
url = "env:MARG_REDIS_URL"
```

```bash
marg db migrate --config ./marg.toml
marg start --config ./marg.toml
```

`/ready` will report `{ "storage": {..., "ok": true}, "hot": {..., "ok": true} }` once both backends are reachable.

## Workspace layout

```
marg/
├── Cargo.toml                    workspace root
├── marg-cli/                     binary entry point (the `marg` command)
├── marg-core/                    core types, config loader, error definitions
├── marg-server/                  axum server, routes, graceful shutdown
├── marg-storage/                 storage trait + backends (sqlite, postgres, redis) - P01, P03
├── marg-providers/               provider adapter trait + clients - P01, P02
└── bench/
    ├── provider-stub/            deterministic fake provider for benchmarks - P01
    ├── data/                     synthetic prompt corpus and key fixtures
    ├── scenarios/                benchmark scenario scripts
    ├── rigs/                     hardware tier configs and run scripts
    └── results/                  benchmark results checked in per release
```

## License

[Elastic License 2.0](LICENSE).

## Documentation

The full project documentation lives in the parent folder under `../build-state/`. Start at `../CLAUDE.md` for the project overview and `../build-state/INDEX.md` for the phase roadmap.
