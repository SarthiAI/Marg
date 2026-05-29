# Routing policy

Marg's routing engine is deterministic. The same request always
takes the same first hop (or the same weighted-split bucket). Failover
is bounded: each fallback runs at most once per request.

## Mental model

```
request -> match the first [[routes]] entry -> attempt primary
  if 5xx / timeout / network: attempt fallback[0]
    if same: attempt fallback[1]
      ...
  if 4xx (non-retriable): surface as-is
  if all attempts fail: 502 ProviderWithAttempts
```

`split` is an alternative to `primary`. The chosen bucket inherits the
route's `fallback` list (if any):

```
request -> match -> draw bucket from weights -> attempt the chosen provider
  if 5xx / timeout / network: attempt fallback[0]
    ...
```

Most A/B routes omit `fallback` entirely so the cohort assignment
stays clean. Set one only when keeping the experiment alive through
an upstream outage matters more than measurement purity.

## Match clauses

| Clause | Behaviour |
|--------|-----------|
| `match.model` | Glob against the request's `model` field. `gpt-4*` matches `gpt-4`, `gpt-4o`, `gpt-4o-mini`. Empty / missing = no constraint. |
| `match.team` | Exact match against the Marg API key's `team` field. Useful for "experimental" cohorts. |

A route with no `match` block always matches. Place such a route at
the bottom of the list as a catch-all.

## Primary + fallback (failover)

```toml
[[routes]]
match.model = "gpt-4*"
primary = "openai"
fallback = ["anthropic:claude-3-5-sonnet"]
```

What happens:

1. Request `gpt-4o` arrives.
2. Marg attempts OpenAI with model `gpt-4o`.
3. OpenAI returns 503.
4. Marg attempts Anthropic with model `claude-3-5-sonnet` (the
   override after the colon).
5. If Anthropic succeeds, the response carries
   `x-marg-provider: anthropic`, `x-marg-model: claude-3-5-sonnet`,
   `x-marg-failovers: 1`, and `x-marg-attempts: 2` (the integer count
   of attempts, including the successful one).

Failover is triggered by:

- HTTP 502, 503, 504 from upstream
- Connect timeout
- Read timeout
- Network errors

4xx from upstream (400, 401, 403, 404, 422, 429) does NOT trigger
failover; it surfaces directly to the client. This is intentional:
"the user prompt was malformed" should not silently retry against a
different provider that might respond differently.

## Weighted split (A/B)

```toml
[[routes]]
match.team = "experimental"
split = [
  { provider = "openai",    weight = 50 },
  { provider = "anthropic", weight = 50, model = "claude-3-5-sonnet" },
]
```

For requests from team `experimental`, Marg picks a bucket by
weighted random draw. The split is stable per request, not per key,
so two consecutive requests from the same key can land on different
providers. With no `fallback` list (as above), a 5xx from the chosen
bucket surfaces directly and the cohort assignment stays clean. Add a
`fallback = [...]` line to the same route only when keeping the
experiment alive through an upstream outage matters more than the A/B
measurement.

Weights are integers and need not sum to 100. The split is
proportional to the weights.

## Where routes come from

Two sources are merged at startup and on every `/admin/policy/reload`:

1. The `[[routes]]` array in the config file, in declaration order.
2. Routes persisted via the admin `POST /admin/routes` endpoint, in
   their `position` order.

Config routes always come first. Inside each source, order is
preserved.

A route persisted via the admin API survives restarts. A route in
the config file is gone the moment you remove it and restart.

## Hot reload

`POST /admin/policy/reload` rebuilds the routing engine and pricing
table inside an `Arc<ArcSwap<RoutingEngine>>` and atomically swaps
it. In-flight requests finish on the old engine; the next request
sees the new one. No connection is dropped.

A failed reload (config parse error, missing provider reference) is
a no-op: the live engine stays in place and the admin call returns
4xx with the parse error. You will never end up with a half-loaded
policy.

## Headers Marg adds to the response

| Header | Notes |
|--------|-------|
| `x-marg-provider` | Which provider served the response. |
| `x-marg-model` | Which upstream model name was actually called. |
| `x-marg-failovers` | Count of failover attempts (0 if primary succeeded). |
| `x-marg-attempts` | Integer count of attempts made for this request (primary plus any fallbacks tried). The full per-attempt breakdown is in the request log, queryable via `GET /admin/requests`. |
| `x-marg-reason` | On error responses, the structured reason code (e.g. `budget_exceeded`, `hot_store_unreachable`, `rate_limited`). |
| `x-request-id` | Operator-supplied id or fresh UUID per request. |

## Things route specs intentionally do not do

- No regex (globs only).
- No per-route quotas (use budgets on the key).
- No provider weights inside `primary + fallback` (use `split` for
  that workload).
- No retry policies beyond "try each fallback once". Retry storms are
  a noisy-neighbour problem.
