# FAQ

Short answers to questions that keep coming up. For depth see the
config, routing, cluster, and troubleshooting docs.

## Why an OpenAI-compatible API on the proxy side?

Almost every LLM SDK in every language already speaks the OpenAI
Chat Completions shape. By matching it, Marg works with all of them
out of the box. Switching apps to Marg is a one-line change in the
client (point at Marg's URL, use Marg's token).

## Do I need an SDK to use Marg?

No. The OpenAI SDK in any language works. There is no Marg-specific
SDK and there will never be one. Admin work uses the HTTP API (or
the bundled `marg` CLI / Marg Console UI).

## SQLite, Postgres, or Redis?

| You are running | Backend | Hot store |
|-----------------|---------|-----------|
| Local dev or a tiny single-user side project | SQLite | local (default) |
| One production node, up to ~6,000 req/s | Postgres | local or Redis |
| Multiple Marg nodes behind a load balancer | Postgres | Redis (required) |

The ~6,000 req/s ceiling is the single-instance measured number
on a 16-core box. Scale beyond that with a multi-node cluster;
Marg is stateless and adds nodes linearly.

Switch by editing two TOML sections and running `marg db migrate`.
No code change.

## How is Marg different from LiteLLM, OpenRouter, or Helicone?

- **Self-hosted only.** No Marg cloud, no telemetry, no phone-home.
  Marg sits inside your VPC or laptop.
- **Single static binary.** Around 22 MB. No mandatory database.
- **Production-grade from day one.** All-Rust hot path, sub-1 ms
  per-decision latency (mean 0.73 ms measured), sub-second
  cluster invalidation with the bundled Kavach layer.
- **Algorithm in the hot path, no LLM judging requests.** Routing,
  budgets, rate limits are all deterministic code.
- **Governance is mandatory.** Every Marg deployment carries the
  Kavach policy engine and the post-quantum signed audit chain.
  Operators choose mode at runtime (observe vs enforce, see
  ADR-010 / ADR-011); they do not choose whether the layer
  exists.

## When is Kavach ready?

Kavach is integrated into Marg as a mandatory dependency starting
in P09 of the build plan and ships with v1.0 at the end of P10.
There is no separate v2.0; the original v2.0 plan was folded into
v1.0 by ADR-010 and ADR-011 once the product position made the
governance layer non-negotiable.

## Why no automated tests?

Correctness is validated by end-to-end manual walkthroughs of the
user-visible capabilities at the end of every phase. Performance
and behaviour under load are validated by a load + chaos
benchmark suite with absolute acceptance gates (kept outside this
repo). Unit tests on internal modules tend to lock in
implementation details rather than behaviour; the bench suite
catches the regressions that actually matter.

## Can I run Marg in observe-only mode?

Yes, and it is the default on the first boot of any fresh Marg
deployment. Kavach policy evaluates every request but does not
refuse anything; would-have-refused events are captured in the
audit chain for review. Operators tune their policy file against
real traffic, then flip `[kavach].mode = "enforce"` once they are
confident. See ADR-003 and ADR-010.

## Is Marg HIPAA / SOC2 / PCI compliant?

Marg gives you the technical controls (provider-key isolation,
post-quantum signed audit chain via Kavach, no telemetry, no SaaS
dependency). Compliance itself is a property of how you operate
the deployment, not the software. The audit chain is designed to
satisfy auditor "non-repudiable log" requirements.

## How do I rotate provider API keys?

Edit `marg.toml`, restart. Marg holds no separate cache of
provider keys; they are read on every cold path and used as the
configured value. Rolling restart across a cluster keeps the
service up.

## How do I rotate admin tokens?

Two-step:
1. Mint a fresh one: `POST /admin/auth/tokens`.
2. Revoke the old one: `POST /admin/auth/tokens/<id>/revoke`.

The 5-second auth cache TTL means the old token will stop working
within 5 seconds of revoke.

## Does Marg log my prompts?

Off by default. `marg_request_log` stores model, token counts, cost,
status, provider, attempts, but never the request or response body.
Flip `[security].log_prompts = true` only in a private debugging
environment.

## How big is the binary?

Around 22 MB on linux-x64 release builds. Static, no runtime
dependencies. The Kavach governance layer is mandatory and folded
into that number; there is no smaller "pure proxy" variant by
design (see ADR-011).

## Can I run Marg in Kubernetes?

Yes. The container image is `FROM scratch` plus the binary plus the
console bundle. Use the standard StatefulSet-with-Postgres pattern
or the Deployment-behind-Service pattern depending on whether you
want sticky log files per pod. Marg itself is stateless; only the
disk-backed request log accumulates locally.

## What about retries?

Marg never retries a 4xx. For 5xx / timeout / network, Marg tries
each fallback once. There is intentionally no retry storm.
Application-side retry libraries should treat Marg's response code
as authoritative; do not layer retries on top of Marg's retries.

## How do I cancel an in-flight request?

Drop the HTTP connection. Marg cancels the upstream provider call
immediately (the byte-stream from reqwest is dropped, which aborts
the underlying HTTP request to the provider). The request log
captures the partial state with `status = 499` ("client closed
request"). The `marg_provider_errors_total{kind="client_disconnect"}`
metric increments once per cancelled stream.

## Will I get the published throughput on my own deployment?

Yes, with one operator step. The single-instance numbers were
measured with Marg running the same way the shipped
`marg.service` runs it. The single deployment step that is not
shipped is raising Postgres `max_connections` above its default
100 (Marg's pool is 200), because Postgres is not part of Marg.
The seven-step recipe is in `docs/install.md` under "Production
checklist". If you skip the Postgres step, Marg's `/ready`
endpoint returns 503 under load and the load balancer drops the
node; nothing fails silently.

## Does "6,000 req/s on one node" mean my app will see 6,000 req/s end-to-end?

Only if your apps put enough concurrent client connections in
front of Marg to keep that pipeline full. The 6,000 number is
Marg's own capacity (auth, budget, routing, hot-path checks, async
write of the spend / log row). End-to-end wall-clock per chat is
dominated by the upstream LLM, not by Marg.

Concretely: if the LLM takes 800 ms to answer, each client
connection is tied up for ~810 ms, so you need about 4,800
concurrent client connections to actually hit 6,000 req/s
end-to-end. Most apps do not have that much concurrency in front
of one node, and Marg's overhead disappears into the LLM latency
they were already paying.

Marg adds about 1 ms of work per request regardless of how fast or
slow the upstream provider is.
