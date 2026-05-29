# Configuration reference

Marg loads exactly one TOML file, passed via `--config`. The file is
the source of truth. Live changes go through the admin API; the
config file is reloaded only on process restart (with the exception
of routes and pricing, which also accept hot-reload via the admin
`/admin/policy/reload` endpoint).

This page documents every section. For an annotated working sample,
see `marg.toml.example`.

## Secret reference shapes

Any field whose name ends in `api_key`, `dsn`, `url`, or
`session_token` accepts three forms:

| Form | Meaning |
|------|---------|
| `plain:abc123` (or just `abc123`) | Literal value. Fine for local dev. |
| `env:NAME` | Read from environment variable `NAME` at startup. |
| `file:/path/to/secret` | Read trimmed file contents at startup. |

The `file:` form is what plugs into HashiCorp Vault, AWS Secrets
Manager, and Kubernetes secrets without any Marg-specific glue.

## `[server]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `bind` | string | `"0.0.0.0:8080"` | Proxy port. Bind to `127.0.0.1` if a reverse proxy fronts Marg. |
| `max_body_bytes` | int | `1048576` | Maximum chat request body. 4 MB and above is reasonable for long-context models; budget RAM accordingly. |

## `[storage]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `backend` | string | `"sqlite"` | `"sqlite"` for single-node, `"postgres"` for production. |
| `path` | string | `"./marg.db"` | SQLite file path. Ignored when backend is postgres. |
| `dsn` | string | n/a | Postgres DSN. Required when backend is postgres. |
| `max_connections` | int | `200` | Upper bound on the storage connection pool. Tune up for high-RPS Postgres deployments. |
| `min_connections` | int | `8` | Pool floor. Connections held open even when idle. |

### `[storage.hot]`

Optional. When the block is omitted, Marg uses an in-process budget
reservation and rate-limit window per Marg instance. Cluster deployments
must set this to Redis so that all instances share state. The only
supported backend today is `"redis"`.

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `backend` | string | `"redis"` | Only `"redis"` is accepted. Omit the whole `[storage.hot]` block for in-process state. |
| `url` | string | n/a | Redis URL. Required when the block is present. Accepted schemes are `redis://`, `rediss://`, `redis+unix://`, and `unix://`. The bundled Redis client connects to a single endpoint; clustered Redis must be fronted by a configuration endpoint or proxy that speaks the standard wire protocol over one of those schemes. See `cluster-deployment.md` for the operational pattern. |
| `key_prefix` | string | `"marg"` | Namespace for sharing one Redis across multiple Marg fleets. |

### `[storage.write_batcher]`

Coalesces request-log inserts into batched writes. Defaults are tuned
for production traffic; the knobs are exposed for tuning under unusual
load.

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `channel_depth` | int | `10000` | In-memory queue depth for pending writes. |
| `max_batch_size` | int | `256` | Maximum rows folded into a single INSERT. |
| `max_batch_age_ms` | int | `50` | Oldest pending write before the batch is flushed. |

## `[providers.<name>]`

One block per upstream provider. The native adapters are
`[providers.openai]`, `[providers.anthropic]`, `[providers.google]`,
and `[providers.bedrock]`. Any number of additional OpenAI-compatible
upstreams live under `[providers.openai_compatible.<name>]` (see below).

OpenAI-shape providers (`openai`, `anthropic`, `google`, and entries
under `openai_compatible`):

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `api_key` | string | n/a | Provider API key. Required. |
| `base_url` | string | provider default | Override for compatible endpoints (Azure OpenAI, on-prem proxies, etc.). Required under `openai_compatible.*`. |
| `timeout_seconds` | int | `120` | Per-request upstream timeout. |
| `api_version` | string | provider default | Anthropic and Google only. Pin the API version. |
| `default_max_tokens` | int | `1024` | Anthropic only. Used when the client request omits the field. |

Bedrock:

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `region` | string | n/a | AWS region. Required. |
| `access_key_id` | string | env | Falls back to `AWS_ACCESS_KEY_ID` if empty. |
| `secret_access_key` | string | env | Falls back to `AWS_SECRET_ACCESS_KEY`. |
| `session_token` | string | env | Optional, for STS credentials. |
| `base_url` | string | AWS default | Override the upstream Bedrock URL. Useful for VPC endpoints (PrivateLink), internal proxies, or local test stubs. |
| `default_max_tokens` | int | `1024` | Used when the client request omits the field. |
| `anthropic_version` | string | `"bedrock-2023-05-31"` | Sent in the Anthropic-on-Bedrock body. |
| `timeout_seconds` | int | `120` | Per-request upstream timeout. |

### `[providers.openai_compatible.<name>]`

Any number of additional OpenAI-compatible upstreams (OpenRouter,
Cerebras, Groq, Together AI, vLLM, LM Studio, Ollama, etc.). Each entry
uses the same key shape as `[providers.openai]` with a required
`base_url`. The `<name>` must be ASCII alphanumeric plus `_` or `-`,
and cannot collide with the reserved names `openai`, `anthropic`,
`google`, or `bedrock`. Route specs reference the entry by `<name>`.

### `[providers]` (no name)

```toml
[providers]
default = "openai"
```

`default` is the provider used when no route matches.

## `[[routes]]`

Routes are evaluated top to bottom. The first block whose `match`
conditions are satisfied wins.

| Key | Type | Notes |
|-----|------|-------|
| `match.model` | glob | Match by model name. Supports `*` (zero or more characters). |
| `match.team` | exact | Match by the `team` field on the Marg API key. |
| `primary` | string | First provider tried. Format `"provider"` or `"provider:model_override"`. Mutually exclusive with `split`. |
| `fallback` | list | Ordered list of providers to try when `primary` returns a retriable error (5xx, timeout, network). Each entry tried at most once per request. |
| `split` | list | Weighted A/B distribution. Each entry is `{provider, weight, model}`. Mutually exclusive with `primary`. |

See `routing-policy.md` for the full match / split / failover
semantics with examples.

## `[security]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `log_prompts` | bool | `false` | Logs the entire request body. Off by default. Production should leave this off. |
| `log_responses` | bool | `false` | Same, for response bodies. |

## `[admin]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | bool | `true` | Master switch for the admin port. |
| `bind` | string | `"127.0.0.1:8081"` | Admin port. Always bind to localhost or a private network. Never publish to the open Internet. |
| `bootstrap_token_path` | string | `"./marg-admin.token"` | First boot writes a fresh bootstrap admin token here, mode 0600. Subsequent boots leave existing tokens alone. Set to `""` to disable bootstrap. |

### `[admin.cors]`

CORS for the admin HTTP API. Off by default. Enable only when the
console is served from a different origin (Vite dev server or a
remote dashboard).

## `[cors]`

Top-level CORS, applied to the proxy port. Most deployments leave
this off because LLM clients talk to Marg directly over server-side
HTTPS.

## `[rate_limits]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `default_rpm` | int | `0` | Default requests-per-minute applied to a key when its `rpm` is unset. `0` disables enforcement. |
| `strict_mode` | bool | `false` | Shape of the token bucket. `false` sets capacity equal to `rpm`, so a fresh key can burst up to `rpm` immediately and then sustains `rpm` over a rolling 60 s (standard token-bucket convention). `true` forces capacity = 1 and refill = `rpm / 60` per second: zero burst tolerance, requests spaced evenly. Use `true` when downstream provider quotas require strictly even pacing. |

## `[[pricing]]`

Override the bundled pricing table.

```toml
[[pricing]]
model = "gpt-4o"
input_per_1k_usd  = 0.0025
output_per_1k_usd = 0.01
```

The bundled table covers all common OpenAI, Anthropic, Google, and
Bedrock models at their official list prices. Override only when
your account has a custom rate.

## Environment variables

A small number of runtime knobs are read directly from the process
environment rather than the TOML file, because they govern how the
process logs and are typically set by the supervisor (systemd, Docker,
Kubernetes) rather than the config file.

| Variable | Values | Default | Notes |
|----------|--------|---------|-------|
| `MARG_LOG_FORMAT` | `json`, `text`, `compact` | JSON when running `marg start`, compact otherwise | Forces the log format regardless of subcommand. `text` and `compact` are equivalent. The bundled `dist/systemd/marg.service` sets this to `json` so logs land in `journald` in structured form. |

`RUST_LOG` is also honoured for level filtering (e.g.
`RUST_LOG=info,marg_server=debug`). Default is `info`.

## Hot reload vs restart

| Change | Apply by |
|--------|----------|
| Routes (config file) | restart, or persist via `/admin/routes` and call `/admin/policy/reload` |
| Pricing (config file) | restart, or `/admin/policy/reload` |
| Provider credentials | restart |
| `[server]`, `[admin]`, `[storage]`, `[storage.hot]` | restart |
| API keys, budgets, rpm caps | live via `/admin/keys` and `/admin/budgets` (no restart) |
