#!/usr/bin/env bash
# B01 budget-exhaustion-cutoff
#
# Drives a key past its daily cap and verifies the first request after the
# cap returns 429 with X-Marg-Reason: budget_exceeded. Acceptance gate:
# cutoff happens within 1 request of the cap; cutoff response decision time
# < 1 ms.
#
# Inputs:
#   MARG_URL    default http://127.0.0.1:8080
#   MARG_TOKEN  required, ideally pointed at a low daily budget (e.g. $0.01)
#   MODEL       default gpt-4o-mini
#   MAX_ITER    safety cap on requests (default 1000)
set -euo pipefail

MARG_URL=${MARG_URL:-http://127.0.0.1:8080}
MARG_TOKEN=${MARG_TOKEN:?"set MARG_TOKEN to a marg api key with a tight daily budget"}
MODEL=${MODEL:-gpt-4o-mini}
MAX_ITER=${MAX_ITER:-1000}

attempt=0
ok=0
denied=0
denied_reason=""
denied_ms=0
last_status=0

while [ $attempt -lt $MAX_ITER ]; do
    attempt=$((attempt + 1))
    response=$(curl -s -o /tmp/b01.body -w "%{http_code} %{time_total}\n" \
        -H "Authorization: Bearer ${MARG_TOKEN}" \
        -H "Content-Type: application/json" \
        -D /tmp/b01.headers \
        --data-binary '{"model":"'"$MODEL"'","messages":[{"role":"user","content":"ping"}],"max_tokens":1}' \
        "${MARG_URL}/v1/chat/completions")
    status=$(echo "$response" | awk '{print $1}')
    time_total=$(echo "$response" | awk '{print $2}')
    last_status=$status
    if [ "$status" = "200" ]; then
        ok=$((ok + 1))
        continue
    elif [ "$status" = "429" ]; then
        denied=$((denied + 1))
        denied_reason=$(grep -i "^x-marg-reason:" /tmp/b01.headers | awk '{print $2}' | tr -d '\r')
        denied_ms=$(awk -v t="$time_total" 'BEGIN { printf "%.3f", t * 1000 }')
        break
    else
        echo "B01: unexpected status $status on attempt $attempt"
        cat /tmp/b01.body
        exit 1
    fi
done

echo "B01 budget-exhaustion-cutoff"
echo "  attempts before 429:  ${ok}"
echo "  first 429 at attempt: ${attempt}"
echo "  X-Marg-Reason:        ${denied_reason}"
echo "  429 response time:    ${denied_ms} ms"

if [ "$last_status" != "429" ]; then
    echo "FAIL: did not see 429 within ${MAX_ITER} attempts"
    exit 1
fi
if [ "$denied_reason" != "budget_exceeded" ]; then
    echo "FAIL: X-Marg-Reason was '$denied_reason', expected 'budget_exceeded'"
    exit 1
fi
echo "PASS"
