# Kavach in Marg

Marg ships with Kavach baked in. Every binary, every deployment, every install
includes the post-quantum signed audit chain and the default-deny gate. There
is no "Marg without Kavach" build (ADR-011).

At runtime, operators pick a **mode**:

- **observe (first-boot default)**: every request is evaluated, refused
  decisions are logged and audited, but the request still goes through. Use
  this while you tune your policy file.
- **enforce**: refused decisions return `403` to the caller, with
  `x-marg-reason: kavach_refuse` and an `x-kavach-refuse-code` header.

## Two files on disk

```
/etc/marg/marg.toml      operations config         (DevOps)
/etc/marg/policy.toml    policy + invariants       (Security / Compliance)
/etc/marg/marg.key       Kavach signing keypair    (root:marg 0640)
/var/lib/marg/audit/     signed audit chain JSONL  (one file per Marg process)
```

This split lets the two teams hold the lock on different files, with
filesystem ACLs. No Marg-side RBAC. A bad policy edit rolls back with
`git revert` on one file; provider rotations are unaffected. See ADR-014.

For dev, you can put `[[policy]]` / `[[invariant]]` blocks at the top of
`marg.toml` and skip `[kavach].policy_path` entirely.

## What gets recorded

Marg writes **one signed chain entry per request** with the full lifecycle
nested inside `context_snapshot` under `schema = "marg.request.v1"`:

```json
{
  "schema": "marg.request.v1",
  "mode": "observe",
  "principal_id": "alice",
  "principal_kind": "user",
  "action_name": "marg.chat.gpt-4o",
  "model": "gpt-4o",
  "input_token_count": 412,
  "max_tokens": 1024,
  "estimated_cost_usd": 0.0061,
  "streaming": false,
  "verdict": {
    "real_kind": "refuse",
    "effective_kind": "permit",
    "evaluator": "policy",
    "reason_code": "NO_POLICY_MATCH",
    "reason_text": "no policy permits 'marg.chat.gpt-4o' for principal 'alice'"
  },
  "provider_call": { "provider": "openai", "model": "gpt-4o", "status": 200, "failovers": 0, "attempts": [...] },
  "response":     { "status": 200, "input_tokens": 412, "output_tokens": 318, "actual_cost_usd": 0.0093, "latency_ms": 1421, "client_disconnect": false },
  "error": null
}
```

`real_kind` is what Kavach actually decided. `effective_kind` is what Marg
applied. The two diverge in observe mode (the "would have refused" signal
`marg policy audit` surfaces); they match in enforce mode.

Non-request events get their own dedicated chain entries with
`schema = "marg.policy_reload.v1"` (policy reload, success/failure, hash
before/after) and `schema = "marg.key_event.v1"` (create / revoke /
invalidate / expired).

Every entry is signed ML-DSA-65 + Ed25519 (hybrid) by default. Each entry
hash-chains to the previous one, so tampering, reordering, or splicing
breaks verification at the offending index.

## Operator workflow

```bash
# First boot in observe mode (default).
./marg start --config marg.toml

# Watch what would have been refused.
./marg policy audit --since 24h

# Tune /etc/marg/policy.toml until that list is empty (or only what you
# expect). Then promote.
sed -i 's/mode = "observe"/mode = "enforce"/' marg.toml

# Reload without restart.
curl -X POST -H "Authorization: Bearer $TOKEN" http://localhost:8081/admin/policy/reload
# or:
kill -HUP $(pidof marg)
```

## Admin API surface

| Endpoint | Purpose |
|---|---|
| `GET /admin/audit/status` | Kavach mode, chain head hash, policy hash, permit knobs |
| `GET /admin/audit/entries?since=<index>&limit=<n>` | Paginated chain view |
| `GET /admin/audit/export?since=<index>` | JSONL bytes for offline verification or SIEM ingest |
| `POST /admin/audit/verify` | Verify the live chain or a file path |
| `GET /admin/policy` | Effective policy (Marg routing + Kavach side) |
| `POST /admin/policy/reload` | Re-read `marg.toml` + Kavach policy file, transactional swap |
| `POST /admin/keys/{id}/invalidate` | Invalidate one key (local in v1.0, cluster broadcast in v1.0+P10) |

Every response from `/v1/chat/completions` carries:
- `x-kavach-mode`: `observe` or `enforce`
- `x-kavach-verdict`: `permit` / `refuse` / `invalidate` (the real verdict)
- `x-kavach-version`: kavach-core version embedded at build time
- `x-kavach-refuse-code`: only on `403 kavach_refuse`
- `x-kavach-permit`: base64-url-encoded **signed** permit envelope, only
  when `[kavach].expose_permit_to_caller = true`. See "Permit signing"
  below for the verification recipe.

## Permit signing

Every `Verdict::Permit` produced by the gate is signed with ML-DSA-65
(plus Ed25519 in hybrid mode) over the token's canonical bytes
(`token_id + evaluation_id + issued_at + expires_at + action_name`).
The signature is wrapped in a `SignedTokenEnvelope`:

```json
{
  "key_id": "<kp-id>",
  "algorithm": "ml-dsa-65+ed25519",
  "ml_dsa_signature": "<bytes>",
  "ed25519_signature": "<bytes>"
}
```

The envelope rides in `PermitToken.signature`, which Marg also packs into
the base64-url-encoded `x-kavach-permit` response header when
`expose_permit_to_caller = true`. Downstream services that receive the
permit verify it offline against Marg's published public key bundle:

```rust
use kavach_pq::{PqTokenSigner, KavachKeyPair};
use kavach_core::TokenSigner;

// Caller fetches the operator's public key bundle out of band (operator-
// published artefact, e.g. a signed directory entry). v1.0 ships no built-in
// endpoint or CLI for this; the bundle is distributed by the operator.
let verifier = PqTokenSigner::hybrid(
    /* dummy signing keys, only the verifying keys are used in verify */
    Vec::new(), public_bundle.ml_dsa_verifying_key,
    Vec::new(), public_bundle.ed25519_verifying_key,
    public_bundle.key_id,
);

let token: PermitToken = serde_json::from_slice(&decoded_header)?;
verifier.verify(&token, token.signature.as_ref().unwrap())?;
```

Operators who want to keep the audit chain hybrid but ship pq-only
permits (smaller header for cost-sensitive deployments) set
`[kavach].permit_signer_hybrid = false` explicitly. By default the permit
signer inherits `audit_hybrid`, so a hybrid audit chain produces hybrid
permits.

Rotating `marg.key` rotates both the audit-chain signer and the permit
signer atomically. The v1.0 operator workflow is: stop Marg, replace the
file, restart. Hot rotation lands in a follow-up cluster phase.

## Drift detection

Marg ships with the four built-in Kavach drift detectors. Each is gated by
its `[kavach.drift]` config knob (default-off when the knob is unset):

| Knob | Detector | Effect |
|---|---|---|
| `geo_max_distance_km` (float, km) | `geo_drift` | Tolerant geo: IP change within the threshold is a warning, beyond is a violation. Needs `x-forwarded-geo` from the load balancer. |
| `session_age_max` (`"24h"`, `"30m"`, etc.) | `session_age_drift` | Session older than the threshold triggers a violation. Warning at 75% of the cap. |
| `device_fingerprint_enabled` (bool) | `device_drift` | Compares `x-marg-device-fingerprint` against the origin value persisted on the session. |
| `behavior_rate_warn` + `behavior_rate_violation` (u64, per minute) | `behavior_drift` | Action rate past `warn` is a warning, past `violation` is a violation. |

Geo header shape: `<country>[;region=<r>][;city=<c>][;lat=<f>][;lon=<f>]`,
e.g. `IN;city=Mumbai;lat=19.07;lon=72.87`. Marg parses the header; it does
not embed a GeoIP database in v1.0. Configure your load balancer (Cloudflare,
Fastly, GCLB, ALB) to attach the header on ingress.

A drift violation produces `Verdict::Invalidate`. Marg responds 403 with
`x-marg-reason: kavach_invalidate`, drops the offending key from the local
auth cache, marks the session row `invalidated = true` so subsequent same-key
gate evaluations refuse even on a clean drift signal, and appends a
`marg.key_event.v1` chain entry with `kind = "invalidated"` and the drift
detector's reason text.

Drift detector tuning is hot-reloadable: editing `[kavach.drift]` and
calling `POST /admin/policy/reload` swaps the live evaluator without
rebuilding the gate. Detector activation is idempotent.

## Cross-restart audit chain (parked: ADR-016)

Kavach 0.1.2's `SignedAuditChain` is in-memory; each Marg process starts a
fresh genesis chain and writes to its own `audit-<timestamp>.jsonl` file. Each
file is self-contained and self-verifying. For unified-history verification
across restarts, merge the files chronologically offline (each file's
`chain_mode` field tells the verifier which mode to expect).

ADR-016 formalises this as v1.0 behaviour, contingent on a future Kavach
release exposing a `SignedAuditChain::resume_from(prev_head_hash, prev_index,
signer)` ctor. When that API lands, Marg adds a boot-time "load most recent
on-disk file, extract head, resume" step and the operator workflow becomes a
single continuous walk over the unified history. Until then,
`marg audit verify --range all` is documented as a per-lifetime walk.

## Cluster broadcasting (P11)

v1.0 single-node ships with `NoopInvalidationBroadcaster`. `Verdict::Invalidate`
updates the local key cache, marks the session row `invalidated`, and emits a
`marg.key_event.v1` chain entry. P11 plugs `kavach-redis` into the workspace
and the gate's `InvalidationBroadcaster` swaps from `Noop` to
`RedisInvalidationBroadcaster`, so an invalidation on one node propagates
sub-second to every other node.

## Cryptographic posture

- ML-DSA-65 (FIPS 204) post-quantum signatures, ML-KEM-768 (FIPS 203) post-
  quantum key encapsulation, Ed25519 + X25519 classical companions, audited
  RustCrypto implementations under the hood.
- Hybrid by default: every signature is both ML-DSA-65 AND Ed25519. An
  attacker has to break both to forge an audit entry.
- `audit_hybrid = false` switches to ML-DSA-65 only. Smaller signatures
  (~30% by volume), slightly faster, but you lose the classical companion.
- Chain mode (PqOnly / Hybrid) is fixed at chain construction and enforced
  uniformly by the verifier; the chain cannot be downgraded mid-stream.

## Common questions

**Q: How big does the chain get?**
A: Each entry is roughly 4 KB on disk (ML-DSA-65 signature is ~3 KB,
Ed25519 is 64 bytes, plus the JSON payload). At 5,000 req/s sustained that
is ~70 GB/day. For most deployments rotate per day and ship to your SIEM /
cold storage; the in-memory working set is bounded by the flush interval.

**Q: Can I drop a refused request without it landing in the chain?**
A: No. The chain is the security contract: every gate decision is
recorded. Pre-gate refusals (rate limit, budget, auth failure) do not
land in the chain because they happen before the gate runs; they still
land in metrics and the request log.

**Q: What if the keypair file is lost?**
A: The historic chain becomes unverifiable. The running process keeps
serving (the in-memory chain is still valid), but the file-on-disk record
is now an orphan. Back up `marg.key` like you would any production
signing key.
