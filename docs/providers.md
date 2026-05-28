# Providers

Marg is an OpenAI-compatible AI gateway. Your application keeps the
OpenAI SDK it already has and points the base URL at Marg. Marg
forwards each request to whichever OpenAI-compatible upstream you
configure: OpenAI itself, an aggregator like OpenRouter, an
LPU-backed runtime like Cerebras, a self-hosted runtime like vLLM,
or anything else that speaks the `/v1/chat/completions` shape.

You do not need a Marg SDK in any language. The OpenAI SDK is the SDK.

## What "OpenAI-compatible" means here

The contract Marg enforces is:

- Requests POST JSON to `/v1/chat/completions` with the OpenAI body
  shape (`model`, `messages`, `max_tokens`, `stream`, ...).
- Responses are either a single JSON object (`{"choices":[...],"usage":{...}}`)
  or, when `stream: true`, a sequence of Server-Sent Events terminated
  by `data: [DONE]`.
- Authentication is `Authorization: Bearer <api_key>`.

Any upstream that honours that contract works through Marg's `openai`
adapter with a `base_url` override. We validate the contract against
three independent backends as part of every release (see "Validated
backends" below). If your upstream of choice is not on the list, the
adapter will still work as long as it follows the wire shape.

## Validated backends (v0.1.0)

| Backend | `base_url` | Auth env var the SDK expects | Notes |
|---|---|---|---|
| OpenAI direct | `https://api.openai.com` | `OPENAI_API_KEY` | Canonical implementation. |
| OpenRouter | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | Routes to many providers behind one API. Model names are provider-prefixed, e.g. `openai/gpt-4o`. |
| Cerebras | `https://api.cerebras.ai/v1` | `CEREBRAS_API_KEY` | LPU-backed runtime. Bare model names, e.g. `llama3.1-8b`. Cheapest per token of the three. |

A successful v0.1.0 release tag means each of these three was driven
through Marg end-to-end and produced 200 responses with correct usage
and cost attribution on both non-stream and stream paths.

## Other OpenAI-compatible endpoints

These all work through the same `openai` adapter with a `base_url`
override. They are not gated on by every release, but the wire shape
is the same one we validate against the three above.

- Groq (`https://api.groq.com/openai/v1`)
- Together AI (`https://api.together.xyz/v1`)
- Fireworks AI (`https://api.fireworks.ai/inference/v1`)
- vLLM, self-hosted (`http://<your-vllm-host>:8000/v1`)
- LM Studio, local (`http://localhost:1234/v1`)
- Ollama, local (`http://localhost:11434/v1`)
- DeepInfra, Anyscale, Perplexity, and most other "OpenAI-compatible"
  managed runtimes.

If you find an OpenAI-compatible endpoint that does not work through
Marg, that is a bug. Open an issue with the failing wire trace.

## Sample configuration

The three validated backends in a single `marg.toml`. The canonical
OpenAI upstream goes under `[providers.openai]`. Every other
OpenAI-compatible upstream goes under `[providers.openai_compatible.<name>]`,
where `<name>` is your choice (used by the route table to point at it):

```toml
[providers.openai]
api_key = "env:OPENAI_API_KEY"
base_url = "https://api.openai.com"

[providers.openai_compatible.openrouter]
api_key = "env:OPENROUTER_API_KEY"
base_url = "https://openrouter.ai/api/v1"

[providers.openai_compatible.cerebras]
api_key = "env:CEREBRAS_API_KEY"
base_url = "https://api.cerebras.ai/v1"
```

Reserved names you cannot use under `openai_compatible`: `openai`,
`anthropic`, `google`, `bedrock`. The name must be ASCII alphanumeric
plus `_` or `-`.

### base_url shape

Marg accepts the canonical OpenAI SDK form (with `/v1` at the end)
**or** the bare host form. Both work, so you can paste whichever your
upstream's docs use:

- `https://api.openai.com` (bare host) → Marg appends `/v1/chat/completions`
- `https://openrouter.ai/api/v1` (OpenAI SDK form) → Marg appends `/chat/completions`

Plus routing so each upstream takes the model names it knows:

```toml
[[routes]]
match.model = "openai/*"
primary = "openrouter"

[[routes]]
match.model = "llama3.1-*"
primary = "cerebras"

[[routes]]
match.model = "gpt-*"
primary = "openai"
```

For full routing semantics see [`routing-policy.md`](routing-policy.md).

## Pricing and cost tracking

Marg's `[[pricing]]` table tells the cost-tracker how many USD a
1K-token bucket of input or output costs for a given model name.
This is what fills `request_log.cost_usd` and what budget enforcement
keys off.

```toml
[[pricing]]
model = "gpt-4o-mini"
input_per_1k_usd = 0.00015
output_per_1k_usd = 0.00060

[[pricing]]
model = "openai/gpt-4o-mini"
input_per_1k_usd = 0.00016
output_per_1k_usd = 0.00063

[[pricing]]
model = "llama3.1-8b"
input_per_1k_usd = 0.00010
output_per_1k_usd = 0.00010
```

Update the table when upstream prices change. Marg does not auto-fetch
prices from any provider.

### When the upstream does not return token counts

A few OpenAI-compatible endpoints omit the `usage` block on streamed
responses. Marg in that case falls back to estimating output tokens
from the response text length divided by an average bytes-per-token
ratio. The estimate is good to within 5% for English-shaped text.
If you need exact counts, set `max_tokens` so the caller-supplied
ceiling is the ground truth.

## Azure OpenAI

Azure OpenAI uses an `api-key:` header instead of `Authorization: Bearer`.
That is the only difference. Marg v0.1.0 hard-codes `Authorization: Bearer`,
which means Azure OpenAI is not supported by v0.1.0 out of the box.
Support lands in v0.2.0 once the configurable-auth-header feature
ships. Until then, run a Cloudflare Worker or NGINX shim that
translates the header.

## What Marg does NOT do per provider

- No provider-specific request body translation. Marg forwards the
  OpenAI-shape body unchanged. If your upstream expects a different
  body shape (e.g. native Anthropic, native Google Generative
  Language API, AWS Bedrock InvokeModel), use that upstream's
  OpenAI-compatible endpoint instead.
- No automatic provider discovery. Each provider you want Marg to
  reach must be in `marg.toml`.
- No SDK in any language. Use the OpenAI SDK with `base_url` pointed
  at Marg.
