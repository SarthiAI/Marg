# 09 - Kavach observe -> enforce flow

## Goal

First boot in observe with empty policy logs every request as a
"would-refuse". Adding a permit rule + reloading keeps observe semantics
correct. Flipping to enforce and reloading actually blocks refused
traffic.

## Setup

`[kavach].mode = "observe"`, no `policy_path`, no inline `[[policy]]`. Two
Marg keys: one whose principal kind is `agent` (acts as engineer in this
test), one whose principal kind is `user` (acts as support_agent).

## Steps

```walkthrough
# 1. Observe + empty policy: every request is a would-refuse
PROBE POST /v1/chat/completions 200 bearer $KEY_ENG body @model=gpt-4o-mini
ADMIN GET /admin/audit/entries?since=0&limit=5 200 \
  jq '.entries[].data.verdict.real_kind' | grep -c refuse
PROBE GET /admin/policy 200 jq '.kavach.policy_rule_count == 0'
CLI marg policy audit --since 5m | grep would_refuse

# 2. Add a permit rule for identity_role = "engineer" (agent kind)
EDIT policy.toml +
[[policy]]
id = "allow-eng-chat"
match = { action = "marg.chat.gpt-4*" }
when = { identity_role = "engineer" }
effect = "permit"
ADMIN POST /admin/policy/reload 200 jq '.kavach.policy_rule_count == 1'

# 3. engineer request now permits (real_kind = permit)
PROBE POST /v1/chat/completions 200 bearer $KEY_ENG body @model=gpt-4o-mini
ADMIN GET /admin/audit/entries 200 jq 'last(.entries[]) | .data.verdict.real_kind == "permit"'

# 4. support_agent request still real-refuses but effective-permits (observe)
PROBE POST /v1/chat/completions 200 bearer $KEY_SUPPORT body @model=gpt-4o-mini header 'expect x-kavach-mode: observe' header 'expect x-kavach-verdict: refuse'
ADMIN GET /admin/audit/entries 200 jq 'last(.entries[]) | .data.verdict.real_kind == "refuse" and .data.verdict.effective_kind == "permit"'

# 5. Flip to enforce and reload
EDIT marg.toml set [kavach].mode = "enforce"
ADMIN POST /admin/policy/reload 200 jq '.kavach.mode == "enforce"'
PROBE GET /admin/audit/status 200 jq '.mode == "enforce"'

# 6. support_agent request now hard-refuses
PROBE POST /v1/chat/completions 403 bearer $KEY_SUPPORT body @model=gpt-4o-mini header 'expect x-marg-reason: kavach_refuse' header 'expect x-kavach-refuse-code: NO_POLICY_MATCH'

# 7. engineer request still works
PROBE POST /v1/chat/completions 200 bearer $KEY_ENG body @model=gpt-4o-mini
```

## Expected

Observe-mode request flow is unchanged (200) but the audit chain records
the would-refuse intent. Enforce-mode short-circuits refused requests with
403 + `x-marg-reason: kavach_refuse` + the appropriate refuse code.

## Cleanup

Reset `[kavach].mode = "observe"` and remove the test policy file at
scenario end.
