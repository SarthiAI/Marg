# 06 - Admin HTTP API

## Goal

Every documented admin endpoint returns the documented JSON shape. Bad
config on reload keeps the previous good config serving.

## Setup

Marg running. `BOOTSTRAP_TOKEN` is read from
`[admin].bootstrap_token_path`. All probes go to the admin port
(`127.0.0.1:8081` by default), Authorization Bearer `$BOOTSTRAP_TOKEN`.

## Steps

```walkthrough
# Keys CRUD
ADMIN POST /admin/keys '{"principal_id":"wt-user","kind":"user"}' 201 jq '.token | length > 20'
ADMIN GET /admin/keys 200 jq 'length > 0'
ADMIN GET /admin/keys/$KEY_ID 200 jq '.principal_id == "wt-user"'
ADMIN DELETE /admin/keys/$KEY_ID 200

# Budgets get / set
ADMIN PUT /admin/budgets '{"key_id":"$KEY_ID","daily_usd":1,"rpm":60}' 200
ADMIN GET /admin/budgets/$KEY_ID 200 jq '.daily_usd == 1 and .rpm == 60'

# Routes get / post
ADMIN GET /admin/routes 200 jq '.stored_routes | type == "array"'
ADMIN POST /admin/routes '{...}' 201

# Policy view + reload
ADMIN GET /admin/policy 200 jq '.kavach.mode' jq '.kavach.permit_signer.enabled == true' jq '.kavach.drift.enabled | type == "boolean"'
ADMIN POST /admin/policy/reload 200 jq '.reloaded == true'

# Providers health
ADMIN GET /admin/providers/health 200 jq '.providers | type == "array"'

# Requests list (cursor pagination - response shape: {entries:[...], next_cursor:"..."})
ADMIN GET /admin/requests?limit=10 200 jq '.entries | type == "array"' jq '.next_cursor | type == "string" or . == null'

# Audit list / export / verify / status
ADMIN GET /admin/audit/entries?since=0&limit=5 200 jq '.entries | type == "array"'
ADMIN GET /admin/audit/export?since=0 200 header 'expect content-type: application/jsonl; charset=utf-8'
ADMIN POST /admin/audit/verify '{}' 200 jq '.verified == true'
ADMIN GET /admin/audit/status 200 jq '.permits.signer.enabled == true' jq '.drift.enabled | type == "boolean"'

# Admin tokens CRUD
ADMIN POST /admin/auth/tokens '{"name":"wt-rotate"}' 201 capture .token=ROT_TOKEN
ADMIN GET /admin/auth/tokens 200 jq '.tokens | length >= 2'
ADMIN DELETE /admin/auth/tokens/$ROT_TOKEN_ID 200

# Malformed reload keeps previous policy serving
EDIT marg.toml inject syntax error
ADMIN POST /admin/policy/reload 400 jq '.error.message | test("config")'
PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=false
EDIT marg.toml revert
```

## Expected

Every endpoint round-trips with the documented shape. Malformed reload
returns a 400 with the parse error in the body; chat traffic keeps
flowing against the previous good policy.

## Cleanup

`run.sh` removes the rotation token + the test routes added above (unless
`MARG_WT_KEEP_ARTIFACTS=1`).
