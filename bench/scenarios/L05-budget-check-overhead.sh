#!/usr/bin/env bash
# L05 budget-check-overhead
#
# Sends paired runs: one against a key with unlimited budget (daily_budget = 0)
# and one against a key with a high but enforced budget. Reports the delta in
# p99 latency between them. Acceptance gate (single-node-prod): < 100us added
# at p99.
#
# Requires marg and provider-stub running. Provide two valid marg tokens.
#
# Inputs:
#   MARG_URL          default http://127.0.0.1:8080
#   MARG_TOKEN_FREE   token for the unlimited-budget key
#   MARG_TOKEN_LIMIT  token for the budgeted key
#   MODEL             default gpt-4o-mini
#   ITER              samples per group (default 5000)
set -euo pipefail

MARG_URL=${MARG_URL:-http://127.0.0.1:8080}
MARG_TOKEN_FREE=${MARG_TOKEN_FREE:?"set MARG_TOKEN_FREE"}
MARG_TOKEN_LIMIT=${MARG_TOKEN_LIMIT:?"set MARG_TOKEN_LIMIT"}
MODEL=${MODEL:-gpt-4o-mini}
ITER=${ITER:-5000}

run() {
    local token=$1
    local label=$2
    local out=$(mktemp)
    for ((i=1; i<=ITER; i++)); do
        curl -s -o /dev/null -w "%{time_total}\n" \
            -H "Authorization: Bearer ${token}" \
            -H "Content-Type: application/json" \
            --data-binary '{"model":"'"$MODEL"'","messages":[{"role":"user","content":"ping"}],"max_tokens":1}' \
            "${MARG_URL}/v1/chat/completions" | awk '{ printf "%.0f\n", $1 * 1000000 }' >> "$out"
    done
    sort -n "$out" | awk -v label="$label" '
    { a[NR]=$1; sum+=$1 }
    END {
        n=NR;
        printf "%s p50=%d us  p95=%d us  p99=%d us  avg=%.0f us\n", label, a[int(n*0.5)+1], a[int(n*0.95)+1], a[int(n*0.99)+1], sum/n;
    }'
    rm -f "$out"
}

echo "L05 budget-check-overhead (n=${ITER} per group)"
run "$MARG_TOKEN_FREE"  "free  "
run "$MARG_TOKEN_LIMIT" "limit "
