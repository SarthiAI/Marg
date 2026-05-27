# 15 - Mode flip via reload

## Goal

Flipping `[kavach].mode` in `marg.toml` and reloading propagates
end-to-end: the response header, the audit status endpoint, and the
behavior of refused requests all switch together.

## Steps

```walkthrough
# Start in observe
EDIT marg.toml set [kavach].mode = "observe"
ADMIN POST /admin/policy/reload 200 jq '.kavach.mode == "observe"'
PROBE POST /v1/chat/completions 200 bearer $KEY_REFUSED header 'expect x-kavach-mode: observe' header 'expect x-kavach-verdict: refuse'

# Flip to enforce
EDIT marg.toml set [kavach].mode = "enforce"
ADMIN POST /admin/policy/reload 200 jq '.kavach.mode == "enforce"'
ADMIN GET /admin/audit/status 200 jq '.mode == "enforce"'
PROBE POST /v1/chat/completions 403 bearer $KEY_REFUSED header 'expect x-marg-reason: kavach_refuse'

# Flip back to observe
EDIT marg.toml set [kavach].mode = "observe"
ADMIN POST /admin/policy/reload 200 jq '.kavach.mode == "observe"'
PROBE POST /v1/chat/completions 200 bearer $KEY_REFUSED header 'expect x-kavach-mode: observe' header 'expect x-kavach-verdict: refuse'
```

## Expected

Three reload cycles, three distinct response shapes for the same refused
key. `/admin/audit/status` always reflects the current mode.

## Cleanup

Leave config in observe (the walkthrough default).
