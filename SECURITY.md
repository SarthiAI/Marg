# Marg security notes

This file is the v1.0 self-led security review. It is the artefact
that satisfies the P07 exit criterion for a security audit on the
Marg-only paths. Kavach-specific paths are reviewed in P09.

## Threat model

Marg is a self-hosted proxy between client applications and large
language model providers. The interesting trust boundaries are:

1. **Client app to Marg.** Apps authenticate with a Marg-issued API
   key. Marg enforces budgets, rate limits, and route policy.
2. **Marg to provider.** Marg holds the real provider credentials.
   The client app must never see them.
3. **Operator to Marg.** Operators authenticate with an admin Bearer
   token. The admin token can mint API keys, change budgets, and
   reload routing policy.
4. **Marg to its backends.** Postgres and Redis hold durable and
   hot state. Compromise of either means trust in everything
   downstream is compromised. Same with the local request log on
   disk.
5. **Operator workstation to Marg Console.** The console is a static
   single-page app served by the admin port. It uses the same
   admin Bearer token as the JSON API.

Out of scope: TLS termination (assumed handled by a reverse proxy or
ALB in front of Marg), host-level kernel exploits, supply-chain
compromise of crates.io.

## Authentication

### Client API keys

- Format: `marg_live_<32-byte-base32>`. Generated with a CSPRNG.
- Surfaced once at creation; stored as a SHA-256 hash.
- Lookup is `hash(token) -> key_id`. No timing oracle on the hash
  itself (the lookup is a single indexed query).
- Cache TTL on the auth path is 60 seconds. A revoke explicitly
  invalidates the cache; the revoke endpoint also clears the
  budget gauge for the key so stale Prometheus series do not
  accumulate.

### Admin tokens

- Same format and storage as client keys.
- 5-second cache TTL on the admin port (admin operations are
  infrequent and need fast revoke).
- A 0600-mode bootstrap token is written on first boot to a
  configurable path. Idempotent: subsequent boots do not mint
  another token if any active admin token exists.
- `marg admin bootstrap` mints an explicit token when the
  bootstrap path is empty (or set to `""`).

### Privilege separation

There is exactly one privilege level inside Marg: admin. Either you
have an admin token or you do not. v1.0 intentionally does not
ship per-route ACLs or RBAC; that complexity belongs in the
operator's IAM, not in Marg.

## Provider credentials

- Provider API keys live only in `marg.toml` (or in env vars / files
  referenced by the `env:` / `file:` secret shapes).
- They never appear in:
  - logs (every error path scrubs the `Authorization` header before
    logging),
  - response bodies,
  - the admin API,
  - the request log on disk,
  - any Prometheus metric label.
- Provider responses can echo prompt content but never our provider
  key (we strip the `Authorization` header on the outbound side
  too, in case a provider ever decided to reflect headers).
- `marg admin keys` only deals with Marg-issued keys. There is no
  endpoint that returns a provider key.

## Body and connection limits

- Configurable `[server].max_body_bytes` cap on chat request bodies.
  Default 1 MiB. Requests larger than the limit get a 413 with
  `x-marg-reason: body_too_large`.
- HTTP/1.1 keep-alive and HTTP/2 supported, with axum's default
  per-connection request limit. Backpressure on streaming is
  end-to-end; Marg never buffers a full response.
- Connect timeout and read timeout per provider, with sensible
  defaults (120 s read timeout).
- File descriptors raised to 1,048,576 in the shipped systemd
  unit; documented as the production minimum.

## Streaming safety

- SSE upstream chunks pass through unchanged. The only operation
  Marg performs is extracting usage from the final chunk (or
  Anthropic's stop event, or Google's UsageMetadata, or Bedrock's
  event-stream metadata) for budget settlement.
- The streaming pipeline is a token-by-token tokio channel; there
  is no buffering of the response body.
- A dropped client connection cancels the upstream call. When the
  inner `mpsc` send to the client fails, the streaming task drops
  the reqwest byte-stream, which aborts the underlying HTTP
  request to the provider so further tokens are never generated.
  `marg_provider_errors_total{kind="client_disconnect"}` increments
  and the request is logged with status 499 (`client closed
  request`). This closes the cost-amplification path that v1.0
  pre-P08 carried as a known issue.

## Failover and fail-closed

- 4xx from upstream surfaces directly to the client. Never
  retried, never failed over. This avoids "user prompt was
  malformed" silently retrying against a different provider that
  responds differently.
- 5xx, connect timeout, read timeout, and network errors trigger
  the fallback chain. Each fallback runs at most once per request.
- Hot store unreachable (Redis down) returns 503 with
  `x-marg-reason: hot_store_unreachable`. No silent permits.
- Durable backend unreachable (Postgres / SQLite) returns 503 with
  `x-marg-reason: storage_unreachable`. Same fail-closed rule.
- Asynchronous write batcher full (queue depth has reached
  `[storage.write_batcher].channel_depth`) returns 503 with
  `x-marg-reason: storage_overloaded`. The request never silently
  drops its spend or audit row. The `marg_write_batcher_overflow_total`
  counter increments per refusal.

## Cross-origin and console

- The admin Bearer token authentication is header-based, not
  cookie-based. CSRF is therefore not applicable to the admin
  port.
- `[admin.cors]` defaults to disabled. Enable only when serving the
  console from a different origin (e.g. the Vite dev server). When
  the console is served same-origin from the admin port (the
  default), CORS is not needed.
- The console's DOM is constructed via a small `h()` helper that
  uses `textContent`, never `innerHTML`, so user-controlled
  strings (key names, request log content) cannot inject HTML.
  XSS via `innerHTML` is structurally unreachable.

## Server-Side Request Forgery (SSRF)

- Provider `base_url` is operator-controlled config. There is no
  client-controlled URL anywhere in the request path. Apps cannot
  redirect Marg to an arbitrary host.
- Provider names referenced in routes are validated at config-load
  against the providers block. An unknown name is a startup error,
  not a runtime ambiguity.

## Secret references

- `plain:`, `env:`, `file:` shapes are resolved at startup, not at
  request time. A `file:` reference returns the trimmed file
  contents.
- A missing env var or unreadable file is a fatal startup error,
  not a silent empty string. We surface the missing reference
  before accepting traffic.

## Audit and request log

- v1.0 request log records: timestamp, request_id, key_id, team,
  model, provider, input_tokens, output_tokens, cost_usd, status,
  failover count, and per-attempt provider plus outcome.
- No prompt or response body is logged by default. The
  `[security].log_prompts` and `log_responses` switches are off by
  default; flip only in private debugging.
- The log lives in the durable backend (SQLite or Postgres). On a
  disk-full event the request log surfaces 503 and never silently
  drops entries.

The full tamper-evident, post-quantum signed audit chain is a
v2.0 feature (Kavach). v1.0 carries the operational request log
only.

## Cryptography in v1.0

- Token hashing uses SHA-256 with no salt because tokens are
  high-entropy CSPRNG output (32 bytes of base32). Salting offers
  no protection here and would prevent the indexed `hash(token)
  -> key_id` lookup.
- TLS termination is delegated. Run Marg behind a reverse proxy or
  ALB that handles TLS. Marg itself binds plain HTTP.
- Bedrock SigV4 signing is implemented locally with `hmac` and
  `sha2`. The signer is feature-by-feature compatible with the
  AWS docs; no rolling-your-own primitives.

Post-quantum signatures (ML-DSA-65 + Ed25519 hybrid via Kavach)
ship in v2.0.

## Dependencies

- All Rust dependencies pinned in `Cargo.lock`. Periodic
  `cargo audit` runs documented under "release process" in
  `CHANGELOG.md`.
- Console dependencies: zero runtime, build-time only (Vite +
  TypeScript). The shipped bundle is hand-written DOM and a
  minimal `h()` helper; no React, no framework, no extra
  attack surface.

## Things v1.0 deliberately does NOT do

- No per-route ACL. Use IAM to control who has an admin token.
- No request-body content filtering. Drishti / content moderation
  is a separate, optional layer that lands later.
- No automatic provider failover on 4xx. 4xx surfaces directly.
- No self-signed TLS. Front Marg with a real terminator.
- No "demo mode" with bundled keys. Every deployment must
  provision its own provider credentials and admin token.

## Reporting a vulnerability

Email `security@<your-domain>` with the request_id, the affected
version, and the smallest reproducing case you can share. We
acknowledge within 48 hours and patch within 14 days for any
issue that is exploitable against a default-configuration
deployment.

## Self-audit log

Each row is one explicit check performed during the v1.0 review.
This list is part of the P07 exit criteria; subsequent releases
add rows as the surface grows.

| Path | Verified | Evidence and notes |
|------|----------|--------------------|
| Authorization header never reaches logs | yes | `marg-server::observability::record_outcome` (`marg-server/src/observability.rs:77-87`) writes only method, path, status, latency, request_id, key_id, model, provider. tower-http's `TraceLayer` defaults at `marg-server/src/lib.rs:85` do not log request headers. |
| Provider key not in `/admin/keys` response | yes | `admin/handlers/keys.rs:79-108` returns only `MargKey` plus `BudgetSpec`. Create at the same file returns only the freshly minted Marg token. No upstream provider field anywhere. |
| Auth cache invalidated on revoke | yes | `admin/handlers/keys.rs:110-121` calls `state.metrics.clear_budget_remaining(&id)` then `state.key_cache.invalidate_all()`. Coarse but correct: every key's cache entry drops, so the revoke takes effect inside the next request. |
| Admin token file mode 0600 | yes | `admin/server.rs:106-120` `write_bootstrap_file` uses `OpenOptions::mode(0o600)` under `#[cfg(unix)]` at line 113. |
| Console uses `textContent`, never `innerHTML` | yes | `console/src/dom.ts:10-58` `h()` helper wraps strings via `document.createTextNode` at line 52. Zero `innerHTML` occurrences anywhere in `console/src/`. |
| 4xx upstream not failed over | yes | `marg-core/src/request_log.rs:47-52` `is_retriable` matches only `Timeout | Network | Upstream5xx`. `proxy.rs:67,102,169,197` honour the function. |
| Body size enforced before allocation | yes | `marg-server/src/lib.rs:86` installs `tower_http::limit::RequestBodyLimitLayer::new(cfg.server.max_body_bytes)`. A secondary hardcoded 8 MiB ceiling lives in `chat.rs:23` as a defence-in-depth bound. |
| `file:` secret missing is fatal at startup | yes | `marg-core/src/secret.rs:39-49` returns `Err(ConfigError::Validation)` on a read failure; the server start path propagates. |
| `env:` secret missing is fatal at startup | yes | Same file, lines 25-37, returns `Err` on `std::env::var` failure. |
| Bootstrap idempotency | yes | `admin/server.rs:59-67` `count_active_admin_tokens` first; mints only when the count is zero. |
| Admin auth middleware uniform | yes | `admin/router.rs:13-42` mounts every `/admin/*` JSON route inside the `protected` group with `require_admin_token`. The only public paths on the admin port are `/`, `/console*`, `/admin/openapi.json`, and `/metrics`. |
| Streaming: client drop cancels upstream | yes | `chat.rs::stream_response` breaks the loop on the first failed `tx.send` and drops `byte_stream`, which aborts the reqwest streaming request to the upstream provider. `marg_provider_errors_total{kind="client_disconnect"}` increments. The request is logged with status 499. |
| Write batcher overflow refuses (never silently drops) | yes | `chat.rs::non_stream_response` and `chat.rs::stream_response` route every `add_spend` and `append_request_log` through `state.write_batcher.enqueue(WriteJob::...)`. On `Err(Overflow)` the non-stream path returns `ChatError::StorageOverloaded` (503 + `x-marg-reason: storage_overloaded`); the stream path logs the overflow at warn. The `marg_write_batcher_overflow_total` counter increments per refusal. |
| Strict-mode rate limit is opt-in only | yes | `quota::check` passes `state.rate_limits.strict_mode` into `hot.allow_request`. The default in `marg-core::config::RateLimitsConfig::default` is `strict_mode = false`, i.e. the documented token-bucket convention. Enabling it requires an explicit `[rate_limits].strict_mode = true` line in marg.toml. |
