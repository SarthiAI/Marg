# Changelog

All notable changes to Marg are documented in this file. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - unreleased

The first published release of Marg. OpenAI-compatible proxy with
provider failover, budgets, rate limits, pluggable storage,
observability, an embedded operator console, and Kavach governance
(default-deny gate, drift detection, post-quantum signed audit chain)
baked in from the first request. Marg 0.1.0 is single-server only;
cluster deployment is a future release.

### Added

- OpenAI-compatible `POST /v1/chat/completions` on the proxy port,
  streaming and non-streaming. Apps point their existing OpenAI SDK
  at Marg and change one URL. Marg forwards each request to any
  OpenAI-compatible upstream you configure (OpenAI, OpenRouter,
  Cerebras, Groq, Together AI, Fireworks AI, vLLM, LM Studio, Ollama,
  ...). See `docs/providers.md` for the canonical configurations and
  the v0.1.0 validation set.
- Named OpenAI-compatible provider instances via the
  `[providers.openai_compatible.<name>]` sub-namespace. Any number of
  entries, each keyed by an operator-chosen name; each entry uses the
  same shape as `[providers.openai]` and routes through the OpenAI
  adapter with a `base_url` override. Route tables reference the
  instances by their key name. Reserved names (`openai`, `anthropic`,
  `google`, `bedrock`) cannot collide. (See ADR-017.)
- OpenAI adapter accepts both base_url conventions: pasting the
  canonical OpenAI SDK form (`https://openrouter.ai/api/v1`,
  `https://api.cerebras.ai/v1`) and pasting the bare host
  (`https://api.openai.com`) both work. Marg appends the right path
  suffix in either case.
- Cost tracking falls back to the caller-supplied model name when the
  upstream returns a date-aliased or otherwise distinct resolved name
  (e.g. OpenAI resolving `gpt-4o-mini` to `gpt-4o-mini-2024-07-18`).
  `[[pricing]]` entries authored against the stable name continue to
  match, so `request_log.cost_usd` is non-zero out of the box.
- Config-driven routing engine:
  - Glob match on `model`, exact match on `team`.
  - `primary` + ordered `fallback` for failover on retriable 5xx,
    timeout, and network errors.
  - Weighted `split` for A/B comparisons.
  - Hot reload via `POST /admin/policy/reload`, atomic swap, in-flight
    requests finish on the old engine.
- Per-key USD budgets and per-key rpm rate limits, enforced in the
  hot path. Sub-millisecond decision time validated by L02.
- Pluggable storage:
  - SQLite default (single file, zero external services).
  - Postgres for production single-node and clusters.
  - Redis hot store for cluster-shared budget reservations and
    rpm counters.
- Pluggable secret references: `plain:`, `env:`, `file:` shapes on
  every `api_key` / `dsn` / `url`. HashiCorp Vault, AWS Secrets
  Manager, and Kubernetes secrets plug in via `file:` without any
  Marg-specific glue.
- Observability:
  - Prometheus metrics: requests total, request duration histogram,
    tokens total, budget remaining gauge, provider errors,
    failover counter, storage and hot-store query durations,
    active streams.
  - Structured JSON access log on `target=marg.access` with
    request_id, key_id, principal_id, provider, model, status,
    latency_ms.
  - `x-request-id` echoed back to clients (operator-supplied id
    honoured, fresh UUID otherwise).
- Admin HTTP API on a separate port (default `127.0.0.1:8081`):
  - Keys: create, list, detail, revoke, invalidate.
  - Budgets: set cap and rpm (upsert), fetch per key.
  - Routes: persisted CRUD with policy reload, side-by-side with
    config-file routes.
  - Policy: view live routes plus pricing, reload-now.
  - Providers: health derived from in-process Prometheus counters.
  - Requests: filtered request log query.
  - Audit: list and export Kavach signed audit entries, verify a
    chain, fetch chain status.
  - Admin tokens: create, list, revoke. Bootstrap token written
    0600 on first boot.
  - OpenAPI 3.1 spec at `/admin/openapi.json`.
- Marg Console: an embedded TypeScript single-page app served by the
  admin port. Ten pages (dashboard, keys, budgets, routes, policy,
  providers, requests, audit, admin tokens, API reference) plus a
  sign-in screen, covering every admin operation. Sign in with the
  bootstrap admin token. Bundle is built and committed at compile
  time, so `cargo build` succeeds without Node installed.
- CLI subcommands: `marg start`, `marg version`, `marg init`,
  `marg db migrate`, `marg admin bootstrap`,
  `marg admin tokens {list, revoke}`, `marg keys {create, list, revoke}`,
  `marg budget {show, set}`, `marg log tail`, `marg policy audit`.
- Graceful shutdown on `SIGTERM` and `SIGINT`. Both ports stop
  accepting new connections and let in-flight requests (streaming
  included) finish on the existing axum tasks. Process supervision
  (the shipped systemd unit) caps the total drain window at
  `TimeoutStopSec=45`; anything still running when systemd's
  KillSignal fires is terminated by the supervisor.
- CORS layers on both ports, off by default. `[cors]` controls the
  proxy port (`/v1/chat/completions`, `/metrics`) for browser-side
  applications calling Marg directly from a different origin.
  `[admin.cors]` controls the admin port when the Marg Console is
  served from a different origin (for example, the Vite dev server).
  Both accept an explicit `allowed_origins` list.
- Health and readiness:
  - `GET /health` always 200 while the process is up.
  - `GET /ready` returns 503 with per-backend diagnostic when
    either the durable backend or the hot store is unreachable.
  - `GET /version` returns binary version + build commit.

### Security

- Provider API keys never appear in logs or in any response body.
- Marg API keys are surfaced once at creation time, stored hashed.
- Admin tokens stored hashed, 0600 file mode for the bootstrap
  token, 5-second cache TTL.
- Fail-closed on every backend failure. No silent permits.
- 4xx upstream responses surface as-is, never trigger failover.
- No telemetry, no phone-home, no SaaS dependency. Marg is fully
  self-hosted.
- Full security review notes in `SECURITY.md`.

### Performance acceptance gates

Validated by the internal benchmark suite. Single-instance measured numbers and the design targets per deployment tier are documented in the README.

- Cold-start to ready < 1.5 s
- Decision time (auth + budget + rate-limit + route) mean < 1 ms
- Streaming first-token p99 < 10 ms
- Single-instance non-streaming sustained >= 5,000 req/s
- Single-instance streaming >= 5,000 concurrent streams
- Cluster gates (cluster-3 and cluster-10) tracked in the v1.0 release acceptance run
- Chaos (provider 5xx failover, Redis partition, Postgres failover, disk full, clock skew), budget consistency, rate-limit fairness, soak runs, and live-provider correctness all part of the release gate

### Known limitations

- Azure OpenAI is not supported in v0.1.0 because it uses an
  `api-key` header instead of `Authorization: Bearer`. The
  configurable-auth-header surface lands in v0.2.0.
- Upstream response headers (including `x-ratelimit-*`) are not
  forwarded to the caller in v0.1.0. Callers that want to observe
  upstream rate-limit budgets must read them from the upstream's
  documented Prometheus / dashboard surface; Marg's per-key budget
  and rpm controls are independent. Forwarding lands in v0.2.0.
- For OpenAI-compatible upstreams that elide the `usage` block on
  streamed responses, Marg records `output_tokens = 0` and `cost_usd`
  reflects only the input side. Send `stream_options.include_usage = true`
  if the upstream honours it, or set `max_tokens` so the caller-supplied
  ceiling is the ground truth.
- The web console is unauthenticated by default to the admin
  Bearer token. There is no SSO integration in v1.0.

### Compatibility

- OpenAI Chat Completions API: requests using the documented
  shape forward cleanly. Vendor extensions outside the
  documented schema are not interpreted.
- Validated against three independent OpenAI-compatible backends:
  OpenAI direct (`api.openai.com`), OpenRouter
  (`openrouter.ai/api/v1`), and Cerebras (`api.cerebras.ai/v1`).
  Per-backend `summary.json` for each release is captured under
  `bench/results/<timestamp>-r-live/` operator-side.
- By transitivity, any other OpenAI-compatible endpoint that honours
  the same wire shape works through the same `openai` adapter with a
  `base_url` override.

### Migration from any earlier prototype

There is no earlier release. v0.1.0 is the first published version.
The build was assembled phase by phase under the internal plan without
intermediate public releases.
