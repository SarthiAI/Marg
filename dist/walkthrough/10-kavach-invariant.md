# 10 - Kavach invariants

## Goal

A `[[invariant]]` block (`param_max` on `max_tokens`) refuses oversize
requests in enforce mode with `INVARIANT_VIOLATION`.

## Setup

`[kavach].mode = "enforce"`, the engineer permit rule from scenario 09 is
still loaded. Add:

```toml
[[invariant]]
kind = "param_max"
name = "max_tokens_under_4k"
field = "max_tokens"
max = 4000.0
```

## Steps

```walkthrough
ADMIN POST /admin/policy/reload 200 jq '.kavach.invariant_count == 1'

# Below the cap: passes
PROBE POST /v1/chat/completions 200 bearer $KEY_ENG body @model=gpt-4o-mini max_tokens=1000

# Above the cap: refused
PROBE POST /v1/chat/completions 403 bearer $KEY_ENG body @model=gpt-4o-mini max_tokens=16000 \
  header 'expect x-marg-reason: kavach_refuse' \
  header 'expect x-kavach-refuse-code: INVARIANT_VIOLATION'

# The chain captures the invariant violation
ADMIN GET /admin/audit/entries 200 jq 'last(.entries[]) | .data.verdict.reason_code == "INVARIANT_VIOLATION"'
```

## Expected

Below-cap request returns 200, above-cap returns 403 with the documented
refuse code. Chain entry carries `verdict.reason_code` =
`INVARIANT_VIOLATION` and a human-readable `verdict.reason_text`.

## Cleanup

Remove the invariant block at end (reload).
