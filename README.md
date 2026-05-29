<div align="center">

<img src="assets/logo.png" alt="Marg logo" width="140" />

# Marg

**Run every AI call through one place. Get agent security, budgets, audit, policy, and provider failover without changing a single line of application code.**

</div>

Marg is an open-source self-hosted gateway for AI. Your apps point at
Marg instead of any OpenAI-compatible upstream (OpenAI, OpenRouter,
Cerebras, Groq, Together AI, Fireworks AI, vLLM, LM Studio, Ollama,
and the like). Marg routes the call, enforces your rules, records
the result, and forwards to the real upstream. The app does not know
the difference. You get every benefit of a managed AI platform on
infrastructure you control.

One static binary. One config file. No database to install, no
Kubernetes to learn, no cloud account, no vendor lock-in, no data
leaves your network.

## Try it in 60 seconds

```bash
# Linux x64, Linux arm64, macOS Apple Silicon
curl -fsSL https://github.com/SarthiAI/Marg/releases/latest/download/install.sh | sh
```

```bash
# Container platform
docker run -d --name marg -p 8080:8080 -p 8081:8081 \
  -v marg-data:/etc/marg sarthiai/marg:latest
```

When the command returns, the gateway is running. The proxy answers on
`http://127.0.0.1:8080`. The admin console is on
`http://127.0.0.1:8081`. The bootstrap admin token is printed to your
terminal.

## Use it from your app

Open the admin console (sign in with the bootstrap admin token), add at
least one upstream key (OpenAI, OpenRouter, Cerebras, or any other
OpenAI-compatible endpoint), then create an application API key with a
daily budget and a requests-per-minute cap. The key is shown once;
copy it. Now your app talks to Marg with that key, using the OpenAI
SDK in any language.

**Python**

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key="marg_live_...",  # the application key you created in the console
)

resp = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "Hello, Marg"}],
)
print(resp.choices[0].message.content)
```

**Node**

```javascript
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "http://127.0.0.1:8080/v1",
  apiKey: "marg_live_...",
});

const resp = await client.chat.completions.create({
  model: "gpt-4o",
  messages: [{ role: "user", content: "Hello, Marg" }],
});
console.log(resp.choices[0].message.content);
```

**curl**

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer marg_live_..." \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello, Marg"}]
  }'
```

Streaming works the same way: add `stream: True` (Python) or `stream:
true` (Node) and the response arrives as Server-Sent Events on the same
endpoint. Every request is enforced against your budget, rate limit,
and [Kavach](https://github.com/SarthiAI/Kavach) policy, then forwarded
to the configured provider.

## Where your provider API keys live

Real provider keys live in Marg's config file under the
`[providers.<name>]` blocks. One place. Applications never see them.
Apps hold short-lived Marg keys; Marg holds the real provider keys and
attaches the upstream auth header server-side, just before the outbound
call.

For any OpenAI-compatible endpoint (OpenRouter, Cerebras, Groq, Together
AI, Fireworks, self-hosted vLLM, LM Studio, Ollama, anything else that
speaks `/v1/chat/completions`), the block carries an extra `base_url`
pointing at the upstream:

```toml
# OpenAI direct (native adapter)
[providers.openai]
api_key = "env:OPENAI_API_KEY"
base_url = "https://api.openai.com"

# OpenRouter: OpenAI-compatible meta-provider, many models behind one key
[providers.openai_compatible.openrouter]
api_key = "env:OPENROUTER_API_KEY"
base_url = "https://openrouter.ai/api/v1"

# Cerebras: OpenAI-compatible, LPU inference
[providers.openai_compatible.cerebras]
api_key = "env:CEREBRAS_API_KEY"
base_url = "https://api.cerebras.ai/v1"

# Groq: OpenAI-compatible, sub-second responses
[providers.openai_compatible.groq]
api_key = "env:GROQ_API_KEY"
base_url = "https://api.groq.com/openai/v1"

# Self-hosted vLLM / LM Studio / Ollama, no real key needed
[providers.openai_compatible.local_llama]
api_key = "plain:not-needed"
base_url = "http://10.0.0.5:8000/v1"

# Native adapter (Marg translates the OpenAI-shape request into the
# Anthropic Messages API on the fly). base_url defaults to
# https://api.anthropic.com; override only for VPC endpoints or test
# stubs. Same shape applies for [providers.google] and [providers.bedrock].
[providers.anthropic]
api_key = "file:/var/run/secrets/marg/anthropic-api-key"
```

Three input forms work on every credential or URL field, not just
`api_key`:

| Form | Example | When to use |
|---|---|---|
| `plain:<value>` | `"plain:sk-abc"` | Laptops and dev boxes only. The literal string goes into the file. |
| `env:<NAME>` | `"env:OPENAI_API_KEY"` | Most containers, systemd units, 12-factor deployments. The secret stays in the environment, never lands in the file. |
| `file:<path>` | `"file:/var/run/secrets/marg/openai"` | Drops in behind HashiCorp Vault Agent, AWS Secrets Manager (CLI sync), GCP Secret Manager, Kubernetes secret mounts, sealed-secrets, SOPS, any sidecar that materialises a secret to disk. Marg reads and trims the file contents at boot. |

### How the key is protected

**At rest.** The production pattern keeps the key out of `marg.toml`
entirely. Use `env:` or `file:` references and the secret lives in
whatever store you already trust to encrypt secrets at rest: HashiCorp
Vault, AWS Secrets Manager, GCP Secret Manager, Kubernetes secrets
backed by KMS-encrypted etcd, sealed-secrets, SOPS, anything you
already operate. Marg reads the secret through the reference; the file
on disk never has it. This is the same delegation pattern your Postgres
connection strings and S3 credentials already follow.

**On disk.** The `marg.toml` file itself is mode 0600, owned by the
marg user (the installer puts it there). Only the Marg process can read
it.

**In memory.** Every secret is held in a self-zeroing `SecretString`.
When Marg releases the secret, the bytes are overwritten before the
memory is returned to the allocator.

**What can see a provider key.** The Marg process, on the hot path of a
single request, for the microseconds it takes to attach the upstream
auth header. That is the entire blast radius.

**What never sees a provider key:**

- Standard Marg logs at any level. Prompts and responses are never
  written to logs in v0.1.0; the `[security].log_prompts` and
  `log_responses` config keys are parsed for forward compatibility
  but have no effect on the running binary today.
- The request log. It records principal, model, tokens, cost, outcome.
  Never the upstream auth.
- The signed audit chain. Same rule.
- The admin API. `GET /admin/providers/health` returns derived health
  counters per configured provider; the response shape never carries
  credentials or the configured base URL.
- The Marg Console. Same surface as the admin API.
- Application traffic. Apps hold `marg_live_...` keys, validated by
  Marg against its own hashed store; the real provider auth header is
  attached by Marg server-side and never crosses back to the client.

### Rotation

Update the env var or the file, send Marg `SIGHUP` (or call `POST
/admin/policy/reload`), and the new key takes effect on the next
request. Applications keep running. No restart, no redeploy.

## The Marg Console

A small operator UI ships embedded in the binary. After `marg start`,
visit the admin port in a browser:

```
http://127.0.0.1:8081/
```

The bootstrap admin token was printed during `marg init` and saved to
`marg-admin.token` in your config directory (mode 0600). Paste it into
the sign-in screen. Subsequent admin tokens can be created and revoked
from inside the console, so the bootstrap token is only needed once.

The console has ten pages plus a sign-in screen, all served from the
same admin port over the same Bearer-token auth as the HTTP admin API.
No separate deployment, no external SPA host, no Node runtime at the
edge.

| Page | What it does |
|---|---|
| **Dashboard** | Live KPIs: requests today, spend today, providers up, top spenders, recent failovers. Polls every 5 seconds. |
| **Keys** | List, filter, and search application keys. Create a key with budget and rate-limit caps in a side drawer; the new token is shown once. Per-key detail view with recent traffic. Revoke or invalidate. |
| **Budgets** | Per-key daily USD caps and requests-per-minute, with inline editors and a set-by-id drawer. |
| **Routes** | Persisted routes (editable in the UI) split from config-file routes (read-only). Create a route with a single primary plus ordered fallbacks, or a weighted A/B split, in one drawer. |
| **Policy** | Live view of the routing engine plus the pricing table. One-click reload reads config and policy files from disk and atomically swaps the live engine. Kavach mode (observe / enforce) and signer status visible at the top. |
| **Providers** | Per-provider health derived from in-process Prometheus counters (successes, 5xx and 4xx error counts, timeouts, network errors). Polls every 10 seconds. |
| **Requests** | Filtered request log: filter by since-time, key, model, provider, errors-only. Per-row detail panel shows the full attempt chain (which provider tried first, what failed, which fallback served). |
| **Audit** | Kavach signed audit chain: every gate verdict, every key event, every policy reload. Filterable by time window and verdict. Useful in observe mode to see what would be refused before flipping to enforce. |
| **Admin tokens** | Mint, list, and revoke admin Bearer tokens. New tokens shown once. Revoke is confirm-by-prefix to avoid revoking the wrong one. |
| **API** | Live OpenAPI reference rendered from the running binary. Every admin endpoint, its parameters, request body, and response shape. Useful for scripting against the same surface the console uses. |

The console is a 13 KB gzipped TypeScript single-page app (no framework
runtime) and shares the live process state with the admin API. The
exact same JSON the console renders is available at `/admin/*` for
scripting and integration with your existing tooling.

## Why teams use Marg

### Stop runaway AI spend before it shows up on a bill

Every API key gets a hard daily USD budget. The moment the cap is hit,
the next call returns a clean 429 with the reason. The same goes for
requests per minute. No surprise $50,000 bill from a misconfigured loop,
an experimental agent that went sideways, or a developer who left
auto-retry on overnight.

The budget check happens in microseconds, in memory. It does not slow
the application path. It does not need a database call. It just works.

### Survive a provider outage without paging anyone

You configure a primary upstream and a list of fallbacks for each model.
If OpenAI returns a 503, or times out, or the network blinks, Marg tries
OpenRouter. If OpenRouter fails too, Marg tries Cerebras. The
application code does not know there was a problem. The user sees a
slightly slower response and the work gets done.

There are no retry storms. Each request gets exactly one attempt per
fallback. The whole thing happens inside the same client connection.

### Switch providers without touching application code

Your apps speak the OpenAI Chat Completions API. Marg speaks it back.
Behind Marg, the upstream can be any OpenAI-compatible endpoint:
OpenAI direct, OpenRouter, Cerebras, Groq, Together AI, Fireworks AI,
vLLM, LM Studio, Ollama, or anything else that speaks
`/v1/chat/completions`. You change upstreams by editing one block in
a config file, not by chasing down every API call in the codebase.

This is also how you run A/B tests. Send 50% of traffic to GPT-4o on
OpenAI direct, 50% to the same model through OpenRouter, measure the
difference. One config change, no application work, no redeploy.

### See every AI call your system has ever made

Marg gives you one log. Every request, every response, every failover,
every refusal, every cost in USD, filterable by user, team, model,
status, time window. Exportable. Searchable. The same data feeds
Prometheus, so AI spend can live on the same dashboard as the rest of
your infrastructure.

For audit and compliance, every entry is cryptographically signed and
chained, so the log is tamper-evident. Anyone can verify, years later,
that a specific call happened, in a specific order, with a specific
outcome.

### Hide your real provider keys behind short-lived Marg keys

Today, every application that calls OpenAI carries an OpenAI key. If
one leaks, you panic-rotate everywhere. With Marg, applications carry a
Marg key. The real OpenAI key lives only inside Marg, in one place. You
rotate it once, no application changes.

When a Marg key is compromised or an employee leaves, you revoke it in
the console. The next request from that key returns 401 within a
second, on every Marg instance.

### Run the same gateway from your laptop to a production cluster

The same single binary that runs on your laptop with SQLite handles a
production cluster behind a load balancer with shared Postgres and
Redis. You configure backends; you do not change codebases. You scale
horizontally by adding more Marg instances; they are stateless by
design.

## [Kavach](https://github.com/SarthiAI/Kavach): the governance layer baked into Marg

[Kavach](https://github.com/SarthiAI/Kavach) is the part of Marg that
says "yes, this AI call is allowed" or "no, refuse". It runs inside the
same binary, on the hot path, on every single request. It is on by
default. Every Marg install ships with it. Kavach is itself an
open-source project; the source, threat model, and cryptography notes
live at [github.com/SarthiAI/Kavach](https://github.com/SarthiAI/Kavach).

### What Kavach gives you

**Default deny.** In enforce mode, no request reaches the provider
unless a policy explicitly permits it. Same posture as your firewall.
The strongest possible default: a misconfiguration fails closed, not
open.

**Policy in one file.** You write rules in a small declarative
language. "Engineers can use GPT-4 with up to 4,000 tokens per call."
"Support agents can use Claude Haiku, capped at 10 cents per call."
"Block the deprecated model entirely." Compliance owns the policy
file. Engineering owns the application. Neither has to coordinate with
the other to ship a change. Reload happens via one HTTP call. No
restart.

**Drift detection.** Kavach can watch each API key for behavior that
does not match its history. A key suddenly calling from a different
country, or sending 50 times the usual volume, or running on an
unknown device. When a drift signal trips, Kavach refuses the call and
writes a signed audit event before the request reaches the provider.
Same idea as fraud detection on a credit card. Drift detection is
opt-in: four detectors (geo, session-age, device, behavior) ship in
the binary, each inert until you enable it under
`[kavach.drift]` in your config. Start with whichever signal matches
the risk you actually care about; you do not have to turn them all on.

**Tamper-evident, post-quantum signed audit log.** Every gate verdict,
every key event, every policy reload becomes a chain entry. Each entry
is signed with two algorithms together (ML-DSA-65 plus Ed25519): one
that secures today's traffic, one designed to survive future quantum
computers. The chain cannot be edited without invalidating every
signature after the edit. You can verify the whole chain offline, at
any time, with no internet access and no Marg server running.

**Observe before enforce.** On first boot, Kavach runs in observe
mode. Every gate verdict is recorded but nothing is blocked. You see
exactly what would have been refused. You tune the policy on real
traffic. You flip to enforce when you are confident. This is how you
avoid the "we turned on the security thing and now nothing works"
rollout.

**Cryptographic key invalidation.** Revoking a key emits a signed audit
event. Every Marg instance that sees the chain refuses the key. No
race window where one instance has revoked the key and another has not.

### Why this matters

Most AI platforms show you a spend report and call it governance. That
tells you what happened after the money is spent. Kavach decides what
is allowed to happen before the call goes out, signs the result, and
makes the entire record provable to a third party.

If you are in finance, legal, healthcare, or any environment where
someone will eventually ask "how can I prove your AI did not do X",
Kavach is the part that answers them.

## Supported platforms

| Platform | One-line installer | Container image |
|---|:-:|:-:|
| Linux x86_64 (glibc) | yes | `linux/amd64` |
| Linux aarch64 (glibc) | yes | `linux/arm64` |
| macOS Apple Silicon (M-series) | yes | n/a |
| Linux musl / Alpine | no, use container | yes (both archs) |
| macOS Intel | not shipped | n/a |
| Windows | not shipped | n/a |

Both the binary and the container image are around 14 to 16 MB.

## Features at a glance

| Area | What ships |
|---|---|
| API surface | OpenAI Chat Completions, streaming and non-streaming |
| Upstreams | Any OpenAI-compatible endpoint. v0.1.0 validated against OpenAI direct, OpenRouter, Cerebras. Also works with Groq, Together AI, Fireworks AI, vLLM, LM Studio, Ollama, and any other `/v1/chat/completions` server via `base_url` override. |
| Routing | Per-model glob match, per-team match, weighted A/B split, one-shot failover |
| Spend control | Per-key daily USD budget, per-key requests-per-minute, token-bucket |
| Durable storage | SQLite by default, Postgres for production, pluggable |
| Hot store | In-process by default, Redis for multi-instance deployments |
| Async writes | Bounded-channel write batcher so writes never block the request path |
| Governance | [Kavach](https://github.com/SarthiAI/Kavach) gate, drift detection, post-quantum signed audit, observe and enforce modes |
| Hot reload | Routing and policy reload via HTTP or SIGHUP, no dropped connections |
| Operator UI | Embedded Marg Console (TypeScript SPA), about 13 KB gzipped |
| Admin API | Keys, budgets, routes, policy, providers, audit, request log, admin tokens |
| Observability | Prometheus `/metrics`, structured JSON logs, propagated `x-request-id` |
| Secrets | `plain:`, `env:`, `file:` references for every API key and DSN |
| Install | One-line installer, Docker single-liner, idempotent `marg init` |

## How it performs

Single-instance numbers on a 16 vCPU cloud box, Postgres plus Redis on
the same network, full Kavach in path (default-deny gate, signed audit
chain, drift detection all active):

| Workload | Sustained |
|---|---|
| Chat passthrough, p95 under 15 ms | ~6,000 requests per second (5,998 over a 5-minute sustained run, p95 14 ms) |
| Hot-path decision time (auth, budget, rate limit, route, gate, audit append) | mean 0.73 ms over 1.96 M samples |
| Streaming chat (SSE) | fully streaming, token-by-token backpressure |

Marg is stateless. Postgres and Redis are shared, so adding instances
behind a load balancer multiplies throughput linearly.

## Configuration

Everything is in one TOML file (default `marg.toml`). Secrets in any
`api_key`, `dsn`, or `url` field accept three forms:

- `plain:<value>` (default if no scheme prefix)
- `env:<NAME>` reads from an environment variable
- `file:<path>` reads trimmed file contents (Vault and SSM friendly)

Switching from SQLite to Postgres plus Redis is one block plus a
migration:

```toml
[storage]
backend = "postgres"
dsn = "env:MARG_PG_DSN"

[storage.hot]
backend = "redis"
url = "env:MARG_REDIS_URL"
```

```bash
marg db migrate --config /etc/marg/marg.toml
sudo systemctl restart marg
```

See `marg.toml.example` for the documented shape and
[`docs/config-reference.md`](docs/config-reference.md) for every key.

## Documentation

- [`docs/install.md`](docs/install.md) install paths, upgrade, uninstall
- [`docs/config-reference.md`](docs/config-reference.md) every TOML section and key
- [`docs/routing-policy.md`](docs/routing-policy.md) match, primary plus fallback, weighted split, hot reload
- [`docs/kavach.md`](docs/kavach.md) policy file shape, drift detectors, audit chain
- [`docs/cluster-deployment.md`](docs/cluster-deployment.md) multi-node behind a load balancer
- [`docs/troubleshooting.md`](docs/troubleshooting.md) 4xx and 5xx symptoms, streaming hangs, recovery
- [`docs/faq.md`](docs/faq.md) short answers

Security review at [`SECURITY.md`](SECURITY.md). Changelog at
[`CHANGELOG.md`](CHANGELOG.md).

## Build from source

```bash
git clone https://github.com/SarthiAI/Marg
cd marg
cargo build --release
```

The binary lands at `target/release/marg`. Stable Rust 1.75 or newer.
The build is self-contained: no system OpenSSL, no Node.js at runtime
(the console bundle is pre-built and embedded into the binary).

## Workspace layout

```
marg/
├── Cargo.toml                workspace root
├── Dockerfile                container image
├── marg-cli/                 binary entry point, marg command
├── marg-core/                core types, config loader, error definitions
├── marg-server/              axum server, routes, write batcher, Kavach runtime
├── marg-storage/             storage trait, sqlite, postgres, redis backends
├── marg-providers/           provider adapter trait, clients
├── console/                  Marg Console UI sources (TypeScript + Vite)
├── installer/                one-line installer script
├── dist/                     systemd unit, policy.toml.example
└── docs/                     user-facing documentation
```

## What Marg is not

- Not a model. No inference, no weights.
- Not a vector database, not RAG, not an agent framework.
- Not your identity provider. Marg consumes identity, it does not produce it for humans.
- Not a content moderator on its own.
- Not a hosted service. Self-hosted only, by design.

## License

[Elastic License 2.0](LICENSE). Free to run on your own infrastructure
for any purpose, commercial use included. The license stops one thing
only: hosting Marg as a paid managed service in competition with the
project itself.

---

<div align="center">

Designed, developed, and maintained by <a href="https://www.linkedin.com/in/chirotpal/" target="_blank">Chirotpal</a>

</div>