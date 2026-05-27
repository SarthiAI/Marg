# 14 - Hot reload paths

## Goal

`POST /admin/policy/reload` and SIGHUP apply the same changes. In-flight
streaming requests are unaffected by a reload (no half-loaded state).
Both reload paths emit a `marg.policy_reload.v1` chain entry.

## Steps

```walkthrough
# 1. Reload via admin endpoint
EDIT policy.toml +new policy rule
ADMIN POST /admin/policy/reload 200 jq '.kavach.policy_rule_count' was=N became=N+1
ADMIN GET /admin/audit/entries 200 jq 'last(.entries[]) | .data.schema == "marg.policy_reload.v1" and .data.principal == "admin"'

# 2. Same change via SIGHUP
EDIT policy.toml -that rule
PROBE SIGHUP marg
SLEEP 1
ADMIN GET /admin/policy 200 jq '.kavach.policy_rule_count' became=N
ADMIN GET /admin/audit/entries 200 jq 'last(.entries[]) | .data.schema == "marg.policy_reload.v1" and .data.principal == "system"'

# 3. Concurrent reload + in-flight streams: no dropped requests
STUB inject openai delay 8s
PARALLEL
  PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=true   # in-flight
  SLEEP 2; ADMIN POST /admin/policy/reload 200
WAIT
ASSERT stream completed with full SSE body
ASSERT no marg_provider_errors_total{kind="client_disconnect"} increment
```

## Expected

Reload applies atomically; no request sees a half-loaded policy. Both
paths leave an entry in the chain. In-flight streaming requests complete
unaffected.

## Cleanup

Revert policy edits.
