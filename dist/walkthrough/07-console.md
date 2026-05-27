# 07 - Marg Console (web UI)

## Goal

The bundled Console SPA renders every page over the admin API, the verify
button on the audit page round-trips, the policy page surfaces permit
signer state + drift detector summary correctly.

## Setup

Marg running with the console embedded (the workspace build embeds
`console/dist/` at compile time). Operator opens `http://127.0.0.1:8081/`
in a browser and logs in with the bootstrap token.

## Steps

This scenario is *manual*. `run.sh` does not script the browser; it prints
the steps below and records pass/fail based on operator input.

```walkthrough
PROMPT visit http://127.0.0.1:8081/ and log in with bootstrap token
PROMPT dashboard renders: top spenders list, Kavach mode badge visible
PROMPT keys page: create + revoke flow works, confirm-by-prefix on revoke
PROMPT budgets page: inline cap + rpm editors save and reload value
PROMPT routes page: config + stored tables render, create drawer works
PROMPT policy page: Kavach sub-card shows mode, permit signer (enabled + algorithm + key id), drift detectors summary
PROMPT providers page: health rows render, counters non-zero after traffic
PROMPT requests page: filter by key works, expand row shows attempt chain
PROMPT audit page: list renders, verify button reports verified=true
PROMPT auth-tokens page: create + revoke flow works
PROMPT logout returns to login screen
```

## Expected

Every page renders without console errors. Audit verify returns success.
Permit signer line on the policy page reads `enabled (ml-dsa-65+ed25519,
key <kp-id>)` (or `ml-dsa-65` if `permit_signer_hybrid = false`).

## Cleanup

None. (Manual scenario.)
