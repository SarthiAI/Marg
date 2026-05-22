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

### `[storage.hot]`

Optional. When omitted, Marg uses an in-process budget reservation and
rate-limit window per Marg instance. Cluster deployments must set
this to Redis so that all instances share state.

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `backend` | string | `"local"` | `"local"` or `"redis"`. |
| `url` | string | n/a | Redis URL. Required when backend is redis. |
| `key_prefix` | string | `"marg"` | Namespace for sharing one Redis across multiple Marg fleets. |
| `redis_cluster_mode` | bool | `false` | Set true when targeting a Redis cluster (clustered ElastiCache, etc.). |

## `[providers.<name>]`

One block per upstream provider. The provider name is what you
reference in route specs. `[providers.openai]`, `[providers.anthropic]`,
`[providers.google]`, `[providers.bedrock]` are the supported names.

OpenAI-shape providers (`openai`, `anthropic`, `google`):

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `api_key` | string | n/a | Provider API key. Required. |
| `base_url` | string | provider default | Override for compatible endpoints (Azure OpenAI, on-prem proxies, etc.). |
| `timeout_seconds` | int | `120` | Per-request upstream timeout. |
| `api_version` | string | n/a | Anthropic and Google only. Pin the API version. |
| `default_max_tokens` | int | n/a | Anthropic / Google. Used when the client request omits the field. |

Bedrock:

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `region` | string | n/a | AWS region. Required. |
| `access_key_id` | string | env | Falls back to `AWS_ACCESS_KEY_ID` if empty. |
| `secret_access_key` | string | env | Falls back to `AWS_SECRET_ACCESS_KEY`. |
| `session_token` | string | env | Optional, for STS credentials. |
| `anthropic_version` | string | `"bedrock-2023-05-31"` | Sent in the Anthropic-on-Bedrock body. |

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

## Hot reload vs restart

| Change | Apply by |
|--------|----------|
| Routes (config file) | restart, or persist via `/admin/routes` and call `/admin/policy/reload` |
| Pricing (config file) | restart, or `/admin/policy/reload` |
| Provider credentials | restart |
| `[server]`, `[admin]`, `[storage]`, `[storage.hot]` | restart |
| API keys, budgets, rpm caps | live via `/admin/keys` and `/admin/budgets` (no restart) |
