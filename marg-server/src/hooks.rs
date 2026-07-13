//! Generic content hooks for embedding Marg's gateway in a host process.
//!
//! A host (for example Niyam wrapping Drishti's prompt-injection / PII /
//! output-safety checks) registers a pre-request and/or post-response hook via
//! [`crate::GatewayBuilder`]. Marg invokes the hook inside the chat pipeline
//! and applies the returned [`ContentDecision`]. Marg stays content-agnostic:
//! it never inspects a score, model, or threshold, it only enforces the
//! decision the host returns.
//!
//! Hooks are optional. When none are registered (the standalone `run()` path
//! and any library use that does not set them), the pipeline behaves exactly
//! as it did before this API existed. See ADR-031.

use async_trait::async_trait;
use bytes::Bytes;

/// Pre-request content check. Invoked after the request is parsed and the
/// Kavach action context is built, before the Kavach gate and before the
/// request is forwarded to a provider. The host wraps its prompt-side checks
/// (prompt-injection, PII redaction) here.
#[async_trait]
pub trait RequestContentHook: Send + Sync {
    async fn on_request(&self, ctx: &RequestHookCtx) -> ContentDecision;
}

/// Post-response content check. Invoked after a non-streaming provider
/// response is received, before it is returned to the caller and before the
/// final audit entry. For streaming responses see the
/// `buffer_streaming_for_post_hook` config flag (off by default): with it off
/// the post-hook is skipped for streams; with it on Marg buffers the streamed
/// text and runs the post-hook once at stream close. The host wraps its
/// output-safety check here.
#[async_trait]
pub trait ResponseContentHook: Send + Sync {
    async fn on_response(&self, ctx: &ResponseHookCtx) -> ContentDecision;
}

/// Context handed to a [`RequestContentHook`].
pub struct RequestHookCtx {
    /// The model named in the request body (before routing resolves it).
    pub model: String,
    /// The resolved caller principal id (from Marg auth).
    pub principal_id: String,
    /// The per-request correlation id. Identical to the id Marg writes on this
    /// request's audit-chain entry, so a host can tie a pre-check, a
    /// post-check, and the audit record together.
    pub request_id: String,
    /// The parsed `messages` array from the request body, or `Null` when the
    /// body carried no `messages` field.
    pub messages: serde_json::Value,
    /// The original, unmodified request body.
    pub raw_body: Bytes,
    /// Whether the caller requested a streamed response.
    pub stream: bool,
}

/// Context handed to a [`ResponseContentHook`].
pub struct ResponseHookCtx {
    /// The model that actually served the response (post-routing).
    pub model: String,
    /// The resolved caller principal id (from Marg auth).
    pub principal_id: String,
    /// The per-request correlation id. Matches [`RequestHookCtx::request_id`]
    /// and the audit-chain entry for this request.
    pub request_id: String,
    /// The provider response status code.
    pub status: u16,
    /// The response body. Full body for non-streaming responses; for a
    /// buffered stream this is the assembled streamed content.
    pub body: Bytes,
    /// Whether this response was produced by the streaming path.
    pub streamed: bool,
}

/// What a hook decides. Marg applies it verbatim and does not interpret why.
pub enum ContentDecision {
    /// Proceed unchanged.
    Allow,
    /// Proceed, but replace the body with this one. For a pre-hook this is the
    /// body forwarded to the provider (for example a PII-redacted prompt); the
    /// modified body is re-parsed and drives routing, quota, and the gate. For
    /// a post-hook this is the body returned to the caller (for example a
    /// replacement safe response).
    AllowModified { body: Bytes },
    /// Stop here. Marg returns this status and body to the caller and does not
    /// forward (pre-hook) or does not return the provider body (post-hook).
    /// Marg writes a `content_hook_rejected` audit entry describing the stop.
    Reject { status: u16, body: Bytes },
}
