# 12 - Kavach drift detection

## Goal

Each built-in drift detector (geo, session-age, device, behavior) produces
a `Verdict::Invalidate` on the documented trigger. The Marg key drops out
of the local cache, a `marg.key_event.v1` chain entry is appended with
`kind = "invalidated"`, and the subsequent request returns 401 because
the cache miss re-resolves against a now-invalidated session.

## Setup

`[kavach].mode = "enforce"`. Drift knobs:

```toml
[kavach.drift]
geo_max_distance_km = 200
session_age_max = "24h"
device_fingerprint_enabled = true
behavior_rate_warn = 60
behavior_rate_violation = 120
```

A test key with the engineer permit rule.

## Steps

```walkthrough
# A. Geo drift
PROBE POST /v1/chat/completions 200 bearer $KEY body @model=gpt-4o-mini \
  header 'x-forwarded-for: 203.0.113.5' \
  header 'x-forwarded-geo: IN;city=Bangalore;lat=12.97;lon=77.59'
SLEEP 1
PROBE POST /v1/chat/completions 403 bearer $KEY body @model=gpt-4o-mini \
  header 'x-forwarded-for: 198.51.100.10' \
  header 'x-forwarded-geo: IN;city=Mumbai;lat=19.07;lon=72.87' \
  header 'expect x-marg-reason: kavach_invalidate'
ADMIN GET /admin/audit/entries 200 jq 'last(.entries[]) | .data.verdict.evaluator == "drift" and .data.verdict.real_kind == "invalidate"'
# Next request authoritatively 401 since the session is invalidated and the cache was dropped
PROBE POST /v1/chat/completions 401 bearer $KEY body @model=gpt-4o-mini

# B. Session-age drift (use a key issued > 24h ago, or temporarily set session_age_max = "1s")
EDIT policy.toml set session_age_max = "1s"
ADMIN POST /admin/policy/reload 200
SLEEP 2
PROBE POST /v1/chat/completions 403 bearer $FRESH_KEY body @model=gpt-4o-mini header 'expect x-marg-reason: kavach_invalidate'
EDIT policy.toml set session_age_max = "24h"
ADMIN POST /admin/policy/reload 200

# C. Behavior drift: hammer the same key past the violation threshold
LOOP 200 PROBE POST /v1/chat/completions accept-any-of 200,403 bearer $KEY2 body @model=gpt-4o-mini at-least-once 403 with x-marg-reason: kavach_invalidate
```

## Expected

Each drift detector triggers its documented Invalidate verdict. The local
auth cache drops on every invalidation. The chain carries one extra entry
per invalidation (`marg.key_event.v1`, `kind = "invalidated"`,
`principal = "drift"`).

## Cleanup

Reset drift knobs to scenario defaults. Restore the test keys' `status =
active` (or recreate).
