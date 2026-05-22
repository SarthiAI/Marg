# Marg

Self-hosted AI gateway written in Rust. Applications point their LLM client at Marg instead of directly at OpenAI, Anthropic, Google, or Bedrock. Marg enforces budgets, routes between providers, gives one observability surface, and ships with Kavach baked in for default-deny governance, post-quantum signed audit, and cluster-wide key invalidation.

## What ships in v1.0

- A single static binary called `marg`.
- OpenAI-compatible Chat Completions endpoint (`POST /v1/chat/completions`), streaming and non-streaming.
- Provider adapters for OpenAI, Anthropic, Google Gemini, and AWS Bedrock. Apps speak the OpenAI shape; Marg translates to and from the upstream protocol.
- Config-driven routing with per-model glob match, per-team match, weighted A/B split, and one-shot failover on retriable upstream errors (5xx, timeout, network).
- Per-key budgets and per-key requests-per-minute rate limits, enforced on the hot path with a token-bucket window.
- Pluggable storage:
  - **SQLite** (default) for single-node development and small production.
  - **Postgres** for production single-node and small clusters.
  - **Redis** as the cluster-shared hot store for budgets and rate counters.
- Async write batcher behind the request log so a single instance does not stall on Postgres write tail latency.
- Health, readiness, version endpoints. `/ready` reports the backend status for both storage and hot store.
- Graceful shutdown on SIGTERM / SIGINT.

Prometheus metrics and structured JSON logs, the admin HTTP API on a separate port, and the Marg Console operator UI all ship in the same single binary.

## Marg Console (operator UI)

Small TypeScript single-page app embedded in the binary. After `marg start`, open the admin URL in a browser (default `http://127.0.0.1:8081/`) and the root path lands on `/console/`. Sign in with the admin Bearer token written to `./marg-admin.token` on first boot. The console covers keys, budgets, routes, policy, providers, request log, and admin tokens.

## Throughput

Single-instance numbers, measured on a 16 vCPU / 32 GB cloud box with Postgres + Redis on the same host:

| Workload | Sustained |
|---|---|
| Non-streaming chat (`/v1/chat/completions`) | ~6,000 req/s, p95 under 20 ms |
| Streaming chat (SSE) | ~7,000 streams/s, zero failures |
| Decision time (auth + budget + rate-limit + route) | mean under 1 ms |

Horizontal scale is supported: Postgres + Redis are shared, marg instances are stateless. Cluster numbers and the v1.0 acceptance gates are tracked in `build-state/architecture/testing-strategy.md`. Hot-store invariant: if Redis or the durable backend is unreachable, Marg refuses with `503 hot_store_unreachable` rather than silently degrading.

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

See `marg.toml.example` for the documented config shape. Copy it to `marg.toml` and edit. Secrets in any `api_key`, `dsn`, or `url` field accept three reference forms: `plain:<value>` (default), `env:<NAME>`, and `file:<path>`. Use the env or file forms when you do not want the secret in the config file.

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

`/ready` reports `{ "storage": {..., "ok": true}, "hot": {..., "ok": true} }` once both backends are reachable.

## Workspace layout

```
marg/
├── Cargo.toml             workspace root
├── marg-cli/              binary entry point (the `marg` command)
├── marg-core/             core types, config loader, error definitions
├── marg-server/           axum server, routes, graceful shutdown, write batcher
├── marg-storage/          storage trait + backends (sqlite, postgres, redis)
├── marg-providers/        provider adapter trait + clients
└── console/               Marg Console UI sources (TypeScript + Vite)
```

## License

[Elastic License 2.0](LICENSE).

## Documentation

User-facing docs under `docs/`:

- `docs/install.md` install from a release archive, container, or source
- `docs/config-reference.md` every TOML section and key
- `docs/routing-policy.md` match, primary plus fallback, weighted split, hot reload
- `docs/cluster-deployment.md` multi-node behind a load balancer with Redis and Postgres
- `docs/troubleshooting.md` 4xx / 5xx symptoms, streaming hangs, recovery
- `docs/faq.md` short answers

The release security review lives at `SECURITY.md` and the changelog at `CHANGELOG.md`.
