# 11 - Kavach permit signing

## Goal

With `[kavach].expose_permit_to_caller = true`, every permitted request
carries an `x-kavach-permit` header that is a base64url-no-pad encoding of
the signed `PermitToken`. Verifying the envelope succeeds when valid and
fails after a single-byte flip.

## Setup

`[kavach].mode = "enforce"`, `expose_permit_to_caller = true`,
`permit_signer_hybrid = true` (default inherits `audit_hybrid = true`).
The engineer permit rule from scenario 09 is loaded.

## Steps

```walkthrough
# 1. Permitted request carries the signed permit envelope in the header
PROBE POST /v1/chat/completions 200 bearer $KEY_ENG body @model=gpt-4o-mini capture header x-kavach-permit=PERMIT_B64
ASSERT $PERMIT_B64 is non-empty

# 2. Decode + verify: signature is good
CLI marg policy verify-permit --permit-b64 $PERMIT_B64 prints {"verified": true, "algorithm": "ml-dsa-65+ed25519", "key_id": "..."}

# 3. Byte-flip: signature should fail
PERMIT_BAD = byte-flip(PERMIT_B64, offset=20)
CLI marg policy verify-permit --permit-b64 $PERMIT_BAD prints {"verified": false, ...}

# 4. PQ-only signer: same flow with the hybrid knob off
EDIT marg.toml set [kavach].permit_signer_hybrid = false
ADMIN POST /admin/policy/reload 200
PROBE POST /v1/chat/completions 200 bearer $KEY_ENG body @model=gpt-4o-mini capture header x-kavach-permit=PERMIT_PQ
CLI marg policy verify-permit --permit-b64 $PERMIT_PQ prints {"verified": true, "algorithm": "ml-dsa-65", "key_id": "..."}
EDIT marg.toml unset [kavach].permit_signer_hybrid
ADMIN POST /admin/policy/reload 200
```

## Expected

- Permit envelope is present on every 200 response in enforce mode when
  `expose_permit_to_caller` is on.
- Verify recipe in `docs/kavach.md` works against the captured permit.
- Byte-flip produces `verified: false`.
- Flipping `permit_signer_hybrid` to false switches the envelope algorithm
  to `ml-dsa-65` only without disrupting traffic.

## Cleanup

Restore default config (hybrid permit signing on).
