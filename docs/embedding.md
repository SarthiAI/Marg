# Embedding Marg in your own Rust binary

Most people run Marg as a standalone process: `./marg start` and point their app's OpenAI client at it. You do not need this document for that.

This document is for the narrower case where you are building your own Rust service and want Marg's gateway to run **inside** your process as one component, rather than as a separate daemon next to it. You get one binary, one process, one shared audit trail, and you can run your own content checks around every model call. Marg's standalone behavior does not change; this is an additive library API on top of the same gateway.

If you just want cost control, routing, and observability in front of your LLM calls, run the daemon. Reach for embedding only when you are assembling a larger governance binary that needs the gateway as an in-process plane.

## What you get

Instead of calling `marg_server::run(...)` (which binds sockets and takes over the process), you assemble the gateway with a builder and mount it yourself:

- **`GatewayBuilder`** builds the whole gateway from a config, but binds no socket and installs no signal handler. You own the runtime, the listeners, and shutdown.
- **`Gateway`** hands you a mountable `axum::Router` (`gateway.router()`), an optional admin router (`gateway.admin_router()`), access to the Kavach runtime (`gateway.kavach()`), and a `gateway.reload()` you wire to whatever triggers a config reload.
- **An injectable audit chain** so Marg writes its records into a chain you own and share with the rest of your process.
- **Two content hooks** that run inside the request pipeline, so you can inspect or replace a prompt before it is sent and a response before it is returned.

## Minimal example

```rust,ignore
use std::sync::Arc;
use marg_server::{GatewayBuilder, RequestContentHook, ResponseContentHook};
use kavach_pq::SignedAuditChain;

// Your process owns one shared chain, built from your keypair. Point the
// config's [kavach].keypair_path at that same key file (see "One trust root").
let chain: Arc<SignedAuditChain> = my_shared_chain();

let gateway = GatewayBuilder::from_config_path("marg.toml").await?
    .with_audit_chain(chain.clone())
    .with_pre_hook(my_pre_hook())    // Arc<dyn RequestContentHook>
    .with_post_hook(my_post_hook())  // Arc<dyn ResponseContentHook>
    .build().await?;

// Mount Marg as one plane of your own app, on your own listener.
let app = my_router().merge(gateway.router());
let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
axum::serve(listener, app).await?;
```

`GatewayBuilder::from_config(config)` takes an already-parsed `marg_core::Config` if you build your config in memory instead of from a file. A gateway built that way cannot `reload()` from disk (there is no source path), which is the only difference.

Register neither a chain nor hooks and you get the exact standalone pipeline. Everything below is opt-in.

## The content hooks

Hooks are generic trait objects. Marg never sees a score, a model, or a threshold; it only applies the decision you return. That keeps Marg independent of whatever content system you plug in.

```rust,ignore
#[async_trait]
pub trait RequestContentHook: Send + Sync {
    async fn on_request(&self, ctx: &RequestHookCtx) -> ContentDecision;
}

#[async_trait]
pub trait ResponseContentHook: Send + Sync {
    async fn on_response(&self, ctx: &ResponseHookCtx) -> ContentDecision;
}

pub enum ContentDecision {
    Allow,                                  // proceed unchanged
    AllowModified { body: Bytes },          // proceed with this body instead
    Reject { status: u16, body: Bytes },    // stop, return this to the caller
}
```

**Where the pre-hook runs.** Right after the request is parsed, before Marg's Kavach gate and before the request is forwarded to a provider. So content is checked first, policy and quota second. A malicious prompt is caught regardless of budget. One consequence to know: an authenticated but abusive caller can make your hook do work on a request that the gate or quota would later reject.

- `Allow`: the request continues unchanged.
- `AllowModified { body }`: Marg re-parses your body and uses it for everything downstream, so a redacted prompt is what routing, quota, the gate, and the provider all see.
- `Reject { status, body }`: Marg returns your status and body to the caller, does not call the gate or the provider, and writes an audit entry marked `content_hook_rejected`.

**Where the post-hook runs.** After a non-streaming provider response, before it is returned to the caller and before the final audit entry.

- `Allow`: return the provider response.
- `AllowModified { body }`: return your replacement body (the provider was still called, so cost and usage are still recorded).
- `Reject { status, body }`: return your rejection instead of the provider body, audited the same way.

**Context.** The pre-hook context carries the model, the resolved caller id, the parsed `messages`, the original request body, the stream flag, and a `request_id`. The post-hook context carries the model, caller id, status, response body, the `streamed` flag, and the same `request_id`. That `request_id` is the id Marg writes on the audit entry, so a pre-check, a post-check, and the audit record for one request all share it.

## Streaming responses

- The **pre-hook always runs** for streaming requests too (it only looks at the prompt).
- The **post-hook on a streamed response is off by default**, because running it would mean buffering the whole stream and losing token-by-token delivery. With it off, streamed responses flow through untouched and skip the post-hook.
- Turn it on per deployment with `buffer_streaming_for_post_hook = true` under `[kavach]` in the config. Marg then accumulates the streamed text, runs the post-hook once at stream close, and releases the buffered content (or your replacement). This trades streaming latency for output coverage; you choose. Note that the HTTP status is already sent when a stream starts, so a buffered-stream `Reject` can replace the body content but not the status code.

Non-streaming responses always run the post-hook.

## One shared trust root

The point of injecting a chain is that Marg's records land in the same chain the rest of your process appends to, so you export and verify one chain.

Marg's permit signer and verifier still take their key from the config's `[kavach].keypair_path`. So build your shared chain from a keypair, and point `keypair_path` at that same key file. Then every signature (audit entries and permits) verifies under one bundle. There is no separate keypair-injection API; the shared file path plus the injected chain is all you need.

When you inject a chain, Marg does not flush it to its own export file. Your process owns persistence and export of the shared chain.

## The `kavach-pq` version requirement

`with_audit_chain` takes `Arc<kavach_pq::SignedAuditChain>`, so `kavach-pq` is part of `marg-server`'s public API. Your binary and `marg-server` must resolve the **same** `kavach-pq` version, or the two `SignedAuditChain` types are considered different and the code will not compile. Keep both on compatible caret ranges (for example `kavach-pq = "0.1"`) so Cargo unifies them to one version. A `kavach-pq` major bump becomes a coordinated release across Kavach, Marg, and your host.

## What does not change

- The standalone daemon (`marg_server::run`) behaves exactly as before. It registers no hooks and injects no chain.
- The five gateway routes, the middleware, the audit format, and the Kavach pins are unchanged.
- Marg takes no dependency on your content system. Hooks are generic trait objects, so Marg never learns what a score is.
