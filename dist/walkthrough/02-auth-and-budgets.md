# 02 - Auth and budgets

## Goal

Auth / quota / rate-limit paths return the right status codes and
`x-marg-reason` values. Revoke and budget set both invalidate the auth
cache within the TTL window so subsequent requests respect the new state.

## Setup

Marg is running from scenario 01. Stub provider returns OK for every
request.

## Steps

```walkthrough
# 1. No auth -> 401
PROBE POST /v1/chat/completions 401 jq '.error.code == "missing_auth_header"'

# 2. Bad token -> 401
PROBE POST /v1/chat/completions 401 header 'Authorization: Bearer not-a-real-token'

# 3. Create an active key, plain budget (no caps)
ADMIN POST /admin/keys '{"principal_id":"alice","kind":"user","team":"eng"}' 201 capture .token=KEY
ADMIN GET /admin/keys 200 jq 'length > 0'

# 4. Active key + active budget -> 200
PROBE POST /v1/chat/completions 200 bearer $KEY body @scenario02-prompt.json

# 5. Daily-cap key
ADMIN POST /admin/keys '{"principal_id":"bob","kind":"service"}' 201 capture .token=KEY_LIMITED capture .key_id=BOB_ID
ADMIN PUT /admin/budgets '{"key_id":"$BOB_ID","daily_usd":0.001,"rpm":60}' 200
# First request settles ~$0.0008, fine. Second exceeds the cap.
PROBE POST /v1/chat/completions 200 bearer $KEY_LIMITED body @scenario02-prompt.json
PROBE POST /v1/chat/completions 429 bearer $KEY_LIMITED body @scenario02-prompt.json header 'expect x-marg-reason: budget_exceeded'

# 6. RPM-bound key
ADMIN POST /admin/keys '{"principal_id":"carol","kind":"agent"}' 201 capture .token=KEY_RPM capture .key_id=CAROL_ID
ADMIN PUT /admin/budgets '{"key_id":"$CAROL_ID","daily_usd":100,"rpm":2}' 200
# Two grants succeed, third is rate_limited.
PROBE POST /v1/chat/completions 200 bearer $KEY_RPM body @scenario02-prompt.json
PROBE POST /v1/chat/completions 200 bearer $KEY_RPM body @scenario02-prompt.json
PROBE POST /v1/chat/completions 429 bearer $KEY_RPM body @scenario02-prompt.json header 'expect x-marg-reason: rate_limited'

# 7. Strict mode clamps burst to 1
EDIT marg.toml set [rate_limits].strict_mode = true
PROBE POST /admin/policy/reload 200
PROBE POST /v1/chat/completions 200 bearer $KEY_RPM body @scenario02-prompt.json
PROBE POST /v1/chat/completions 429 bearer $KEY_RPM body @scenario02-prompt.json header 'expect x-marg-reason: rate_limited'
EDIT marg.toml set [rate_limits].strict_mode = false
PROBE POST /admin/policy/reload 200

# 8. Revoke produces 401 within the auth-cache TTL
ADMIN DELETE /admin/keys/$BOB_ID 200
SLEEP 6
PROBE POST /v1/chat/completions 401 bearer $KEY_LIMITED
```

## Expected

Every PROBE line returns the status it claims; `x-marg-reason` is set on
the documented refusals; the revoke step turns subsequent same-token
requests into 401 within the cache TTL.

## Cleanup

`run.sh` deletes the three keys + budgets at scenario end (unless
`MARG_WT_KEEP_ARTIFACTS=1`).
