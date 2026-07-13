# marg-server

The HTTP server library behind [Marg](https://github.com/SarthiAI/Marg), the self-hosted AI gateway.

This crate is the request pipeline: the OpenAI-compatible proxy endpoints, the admin API, budget and rate-limit enforcement, the async write batcher, Prometheus metrics, graceful shutdown, and the Kavach governance integration (default-deny gating, the signed post-quantum audit chain, and signed cross-node key invalidation for clustered deployments).

## Two ways to use it

**As the standalone daemon.** `run("marg.toml")` loads the config, binds the gateway and admin listeners, installs the reload signal handler, and serves until shutdown. This is what the `marg` binary does. To run the gateway that way, see the [main repository](https://github.com/SarthiAI/Marg) (one-line installer or Docker image).

**Embedded in your own binary.** `GatewayBuilder` assembles the same gateway without binding any socket and returns a `Gateway` whose `router()` you mount into your own axum app. You own the listeners, signals, and shutdown. Two additive extras make it useful as one plane of a larger governance binary:

- **Injectable audit chain.** `with_audit_chain(chain)` makes Marg append its verdicts and request records to a shared `kavach_pq::SignedAuditChain` you own, so your whole process exports and verifies one chain instead of several.
- **Content hooks.** `with_pre_hook(...)` and `with_post_hook(...)` register generic checks that run inside the request pipeline (before the gate and forwarding, and on the response) and return `Allow`, `AllowModified { body }`, or `Reject { status, body }`. Marg applies the decision; it never inspects a score or a model, so it stays agnostic to whatever content system you wrap.

```rust,ignore
use std::sync::Arc;
use marg_server::{GatewayBuilder, RequestContentHook, ResponseContentHook};

let gateway = GatewayBuilder::from_config_path("marg.toml").await?
    .with_audit_chain(shared_chain)   // append to your one shared chain
    .with_pre_hook(pre_hook)          // e.g. prompt-injection / PII checks
    .with_post_hook(post_hook)        // e.g. output-safety checks
    .build().await?;

let app: axum::Router = my_router().merge(gateway.router());
// serve `app` on your own listener; call gateway.reload() on config change.
```

Registering neither hooks nor a chain gives the exact standalone behavior. See the [embedding guide](https://github.com/SarthiAI/Marg/blob/main/docs/embedding.md) for the full hook contract, streaming behavior, the shared trust root, and the `kavach-pq` version requirement.

## License

Elastic License 2.0. See [LICENSE](https://github.com/SarthiAI/Marg/blob/main/LICENSE).
