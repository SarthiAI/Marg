# Cluster deployment

Run several Marg nodes behind a load balancer, all sharing one Redis and one
Postgres, so the gateway scales horizontally and keeps its security
guarantees fleet-wide. This page covers what turns on in cluster mode, how to
configure it, and the threat model for the cross-node traffic.

For a single-node install, see [`install.md`](install.md). Single-node needs
none of this; omit the Redis hot store and every node-to-node feature stays
off.

## What cluster mode is

Marg's proxy is stateless: any node can serve any request. The shared state
lives in Postgres (durable: keys, budgets, request log, routes) and Redis
(hot: budget counters, rate-limit buckets, Kavach session state). Point every
node at the same Postgres and the same Redis and you have a cluster.

Cluster mode is detected automatically: when `[storage.hot].backend = "redis"`
is set, the node switches its Kavach session store and key-invalidation path
to their cluster-aware variants. With no Redis hot store, a node runs exactly
as it did single-node.

## What turns on when Redis is configured

1. **Shared rate limits and budgets.** Per-key rpm buckets and budget
   reservations move from per-process memory to the shared Redis, so a key's
   limit is enforced across the whole fleet, not per node.

2. **Shared Kavach session state.** Origin facts for drift detection (origin
   geo, first-seen device, session age, recent behaviour) are written to Redis
   on first sighting, so drift is evaluated consistently no matter which node
   a request lands on.

3. **Signed cross-node key invalidation.** When a key is killed on one node,
   that node broadcasts a signed "drop this key" message to every other node.
   Each node drops the key from its local auth cache (and, for a session-scoped
   kill, flips the session), so the key is refused fleet-wide in well under a
   second. Both paths propagate: automatic kills (a drift detector or policy
   evaluator returning `Invalidate`) and operator kills (the admin
   `invalidate` / `revoke` endpoints). Automatic kills respect mode (observe
   never broadcasts); a deliberate admin kill fires in any mode.

## The invalidation channel is signed (ADR-027)

The invalidation messages travel on a Redis Pub/Sub channel, but they are
**not** plaintext. Each message is signed with the cluster's ML-DSA-65 (hybrid
with Ed25519) key, the same post-quantum signer Marg uses for the audit chain,
and every node verifies the signature before acting. Anything unsigned,
tampered, or older than the replay window is dropped and counted in the
`marg_cluster_invalidations_total{result="rejected"}` metric.

Why this matters: without signing, anyone who could reach your Redis could
publish forged "kill key X" messages and knock valid keys offline across the
whole cluster (a denial-of-service). They could not do the reverse: there is
no "this key is fine" message, so a forged message can only deny, never
permit. Signing closes that denial vector.

The payload is just a key identifier, not a secret, so the channel is signed
but not encrypted; confidentiality is not the concern here. Run Redis with
TLS, AUTH, and network isolation anyway, as defence in depth for the rest of
the hot-store data.

Observe mode never broadcasts automatic kills. A drift- or policy-driven
`Invalidate` is a policy decision, and in observe mode policy decisions are
logged, not enforced, so the broadcast is suppressed (counted under
`result="suppressed"`). A policy reload that flips a node to enforce starts
those broadcasts; flipping back to observe stops them. A deliberate admin kill
(`invalidate` / `revoke`) is different: it is an explicit operator command, not
a policy decision, so it broadcasts in any mode.

## Key distribution

All nodes in a cluster share the same Kavach signing keypair (the file at
`[kavach].keypair_path`). The operator distributes that keypair to every node
as part of bring-up, exactly as the audit-chain signer is keyed today. The
shared key is what lets any node verify an invalidation signed by any other.
Rotating the cluster key is an operator action across all nodes at once.

## Minimal cluster configuration

On every node, the same `marg.toml` (only `node_id` may differ):

```toml
[storage]
backend = "postgres"
dsn = "env:MARG_PG_DSN"          # all nodes -> same Postgres

[storage.hot]
backend = "redis"
url = "env:MARG_REDIS_URL"        # all nodes -> same Redis
key_prefix = "marg"

[kavach]
mode = "enforce"
keypair_path = "/etc/marg/marg.key"   # SAME keypair file on every node

[kavach.cluster]
invalidation_channel = "marg:kavach:invalidation"   # same on every node
# node_id = "marg-node-1"          # optional; auto-generated if unset
# max_message_age_seconds = 30     # raise if node clocks drift
```

Put a load balancer in front of the nodes' `:8080` proxy ports. Keep the
`:8081` admin ports on an internal network or behind an allowlist.

## Docker cluster mode

The container reads the same config. Point it at an external Redis and
Postgres and mount the shared keypair:

```bash
docker run -d --name marg-node-1 \
  -p 8080:8080 -p 8081:8081 \
  -e MARG_PG_DSN="postgres://..." \
  -e MARG_REDIS_URL="redis://..." \
  -v /etc/marg:/etc/marg \
  sarthiai/marg:latest
```

Mount the same `/etc/marg` keypair material on each node (or distribute the
key file by your own secret mechanism). Everything else is identical across
nodes.

## Operational notes

- **Redis partition handling.** If a node loses Redis, the invalidation bridge
  logs and reconnects with backoff. Hot-store reads fail closed (a budget or
  rate-limit check that cannot reach Redis refuses rather than over-permits).
  Local invalidation on the issuing node still stands even if the broadcast
  could not be published.
- **Clock skew.** Received invalidations older than `max_message_age_seconds`
  are rejected as stale. If nodes reject each other's fresh messages, your
  clocks are too far apart; widen the window or fix NTP.
- **Observability.** Watch `marg_cluster_invalidations_total` (labels
  `direction` = published/received, `result` = ok/suppressed/rejected). A
  rising `rejected` count means forged, malformed, or stale messages are
  reaching the channel; investigate Redis access.
- **FD limits.** The per-node file-descriptor guidance in
  [`install.md`](install.md) applies per node under cluster load.

## What cluster mode does not change

- The single binary is identical to the single-node one; cluster behaviour is
  config-driven, not a separate build.
- The OpenAI-compatible API surface is unchanged; clients point at the load
  balancer instead of one node.
- Postgres remains the durable source of truth; Redis is hot state only and is
  always reconstructable.
