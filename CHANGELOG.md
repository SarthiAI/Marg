# Changelog

All notable changes to Marg are documented in this file. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.4] - 2026-07-13

Additive library API so `marg-server` can be embedded in another Rust binary
as an in-process plane. Nothing about the standalone daemon changes; running
`marg start` behaves exactly as before.

### Added

- Embeddable gateway API. `GatewayBuilder` assembles the full gateway from a
  config without binding any socket or installing a signal handler, and
  returns a `Gateway` with a mountable `router()` (plus `admin_router()`,
  `kavach()`, and `reload()`). The host owns the listeners, signals, and
  shutdown. `run()` is re-expressed on top of the builder, so there is one
  assembly path and its behavior is unchanged.
- Injectable shared audit chain. `GatewayBuilder::with_audit_chain(chain)`
  makes Marg append its verdicts and request records to a host-owned
  `kavach_pq::SignedAuditChain` instead of creating its own, so a host process
  exports and verifies one chain. The chain shares Marg's trust root through
  the existing `[kavach].keypair_path` (no keypair-injection API needed).
- Generic content hooks. `with_pre_hook` and `with_post_hook` register
  `RequestContentHook` / `ResponseContentHook` trait objects that run inside
  the request pipeline (pre-hook before the gate and forwarding, post-hook on
  the provider response) and return `Allow`, `AllowModified { body }`, or
  `Reject { status, body }`. Marg applies the decision and stays agnostic to
  any content system; a rejection is audited as `content_hook_rejected`. Hooks
  are optional; with none registered the pipeline is byte-for-byte the same.
- `[kavach].buffer_streaming_for_post_hook` (default `false`). Off keeps token
  streaming and skips the post-hook on streamed responses; on buffers the
  stream and runs the post-hook once at stream close. Embed-only; has no
  effect on the standalone path or on non-streaming responses.
- New embedding guide at [`docs/embedding.md`](docs/embedding.md) and a crate
  README for `marg-server` covering the builder, hooks, shared chain, and the
  `kavach-pq` version requirement.

### Changed

- Kavach dependency resolved uniformly to 0.1.4 across `kavach-core`,
  `kavach-pq`, and `kavach-redis`. Because `with_audit_chain` puts
  `kavach_pq::SignedAuditChain` in the public API, an embedding host must
  resolve the same `kavach-pq` version this crate does (keep both on
  compatible caret ranges so Cargo unifies them).

## [Unreleased] - cluster line

First cluster-capable work, landing ahead of the cluster release (version
chosen at cluster-release time per ADR-020). The 0.1.x line stays
single-server; this is the multi-node groundwork.

### Added

- Cluster mode, enabled automatically when a Redis hot store is configured
  (`[storage.hot].backend = "redis"`). Multiple Marg nodes behind a load
  balancer share rate limits, budgets, and Kavach session state, and a key
  killed on one node is dropped fleet-wide in under a second.
- Signed cross-node key invalidation (ADR-027). Invalidation messages are
  ML-DSA-65 signed (hybrid with Ed25519) by the cluster keypair and verified
  on receipt, so a forged "kill key" message published to Redis is rejected.
  The channel is signed, not encrypted: the payload is a key id, not a secret.
  Both automatic kills (drift / policy Invalidate verdicts) and operator kills
  (admin `invalidate` / `revoke`) propagate cluster-wide. Drift / policy
  broadcasts respect mode (observe never fans out); a deliberate admin kill
  fires in any mode.
- New `[kavach.cluster]` config block (channel, node id, replay window,
  session TTL) and a `marg_cluster_invalidations_total` metric
  (labels `direction`, `result`).
- `docs/cluster-deployment.md` rewritten from a stub into a full operator
  guide; `docs/install.md` gains a Docker cluster recipe.

## [0.1.2] - unreleased

Hotfix release. The 0.1.0 and 0.1.1 Docker image was unstartable on
amd64 (and arm64): `exec /marg: no such file or directory` on every
container run. Root cause was a `FROM scratch` base image that could
not host the dynamic Rust binary the workflow produces. 0.1.2 ships a
container image that actually runs.

### Fixed

- Docker image base switched from `FROM scratch` to
  `debian:bookworm-slim` plus `ca-certificates`, `tini`, and `curl`.
  Final image is around 30 MB. The binary inside is byte-identical to
  the one in the matching release archive. The container runs as the
  non-root `marg` system user (uid 1001), matching the system user the
  curl-pipe-sh installer creates on bare metal so file permissions stay
  consistent across both install paths.
- Release workflow's aarch64-linux build moved from x86_64-with-cross
  to the native GitHub-hosted `ubuntu-22.04-arm` runner. No more
  `gcc-aarch64-linux-gnu` install, no linker env vars. Plain
  `cargo build --release --target ...` for all three targets. Same
  binaries, simpler workflow.
- Release workflow added a post-push smoke step that pulls the
  just-published Docker image, runs it, and verifies `/health` returns
  `200` before the workflow turns green. The release fails red if the
  image cannot boot.
- Docker Hub tag stripped its leading `v` to match the documented
  reference (`sarthiai/marg:0.1.2`, not `:v0.1.2`).
- `Dockerfile` moved from repo root to `docker/Dockerfile` to match
  the bundled binaries staging path used by the workflow.

## [0.1.1] - 2026-05-29

Hotfix release. The 0.1.0 installer dropped the bundled systemd unit
into place but did not create the `marg` system user and group the
unit runs under, so `systemctl start marg` failed with exit code
217/USER on a fresh Linux box. 0.1.1 fixed the installer.

### Fixed

- `marg init --systemd` now creates the `marg` system user and matching
  group via `useradd --system --no-create-home --shell /usr/sbin/nologin
  --user-group marg` when they are not already present, and chowns the
  data directory recursively to `marg:marg` so the service can read its
  config and write its SQLite database, signed audit chain, and admin
  token file. The one-line installer (`curl ... install.sh | sudo sh`)
  picks this up automatically. Re-running is idempotent.

## [0.1.0] - 2026-05-29

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
