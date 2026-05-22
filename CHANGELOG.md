# Changelog

All notable changes to Marg are documented in this file. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - unreleased

The first shippable release of Marg as a standalone AI gateway. No
Kavach governance yet; that lands in v2.0.

### Added

- OpenAI-compatible `POST /v1/chat/completions` on the proxy port,
  streaming and non-streaming. Apps point their existing OpenAI SDK
  at Marg and change one URL.
- Provider adapters for OpenAI, Anthropic, Google Gemini, and AWS
  Bedrock. Apps speak OpenAI on the proxy side; Marg translates to
  and from each upstream wire format.
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
  - Keys: create, list, detail, revoke.
  - Budgets: list, set cap and rpm.
  - Routes: persisted CRUD with policy reload, side-by-side with
    config-file routes.
  - Policy: view live routes plus pricing, reload-now.
  - Providers: health derived from in-process Prometheus counters.
  - Requests: filtered request log query.
  - Admin tokens: create, list, revoke. Bootstrap token written
    0600 on first boot.
  - OpenAPI 3.1 spec at `/admin/openapi.json`.
- Marg Console: an embedded TypeScript single-page app served by the
  admin port. Eight pages covering every admin operation. Sign in
  with the bootstrap admin token. Bundle is built and committed at
  compile time, so `cargo build` succeeds without Node installed.
- CLI subcommands: `marg start`, `marg db migrate`, `marg admin
  bootstrap`, `marg admin tokens {list, revoke}`, `marg keys {create,
  list, revoke}`, `marg budget {show, set}`, `marg log tail`.
- Graceful shutdown on `SIGTERM` and `SIGINT`. Both ports drain
  in-flight requests, streaming connections close cleanly with a
  final SSE event.
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

- No multi-account / named provider instances (e.g.
  `openai-eu`). Routes reference canonical provider names only.
  Deferred until a real customer asks for it.
- No usage extraction for Bedrock providers that do not return
  token counts in their response (a small subset). Falls back to
  cost = 0 for those calls.
- The web console is unauthenticated by default to the admin
  Bearer token. There is no SSO integration in v1.0.

### Compatibility

- OpenAI Chat Completions API: requests using the documented
  shape forward cleanly. Vendor extensions outside the
  documented schema are not interpreted.
- Anthropic Messages API: full support for `messages`, `system`,
  `tools`, `tool_choice`, streaming, and the usage block.
- Google Gemini: `generateContent` and `streamGenerateContent`
  (alt=sse), tool calling, system instructions.
- AWS Bedrock: SigV4 + event-stream for `invoke` and
  `invoke-with-response-stream`, Anthropic-on-Bedrock body shape.

### Migration from any earlier prototype

There is no earlier release. v1.0 is the first published version.
The build was assembled phase by phase (P00 through P07 of the
internal plan) without intermediate public releases.
