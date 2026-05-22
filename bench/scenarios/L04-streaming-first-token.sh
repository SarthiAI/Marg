#!/usr/bin/env bash
# L04 streaming-first-token
#
# Measures the time from sending a streaming chat request to receiving the
# first SSE byte. Acceptance gate (single-node-prod): p99 < 10ms with stub at
# 0 latency.
#
# Requires the stub to run on $STUB_PORT and marg pointed at it.
#
# Inputs:
#   MARG_URL    default http://127.0.0.1:8080
#   MARG_TOKEN  required
#   MODEL       default gpt-4o-mini
#   ITER        samples to take (default 100)
set -euo pipefail

MARG_URL=${MARG_URL:-http://127.0.0.1:8080}
MARG_TOKEN=${MARG_TOKEN:?"set MARG_TOKEN to a marg api key"}
MODEL=${MODEL:-gpt-4o-mini}
ITER=${ITER:-100}

samples=()
for ((i=1; i<=ITER; i++)); do
    body='{"model":"'"$MODEL"'","stream":true,"messages":[{"role":"user","content":"ping"}],"max_tokens":1}'
    ttfb_us=$(curl -s -o /dev/null -w "%{time_starttransfer}\n" \
        -H "Authorization: Bearer ${MARG_TOKEN}" \
        -H "Content-Type: application/json" \
        --data-binary "$body" \
        "${MARG_URL}/v1/chat/completions" | awk '{ printf "%.0f", $1 * 1000000 }')
    samples+=($ttfb_us)
done

printf "%s\n" "${samples[@]}" | sort -n | awk '
NR==1 { min=$1 }
{ a[NR]=$1; sum+=$1 }
END {
    n=NR;
    p50=a[int(n*0.5)+1];
    p95=a[int(n*0.95)+1];
    p99=a[int(n*0.99)+1];
    printf "L04 streaming-first-token: n=%d  min=%d us  p50=%d us  p95=%d us  p99=%d us  avg=%.0f us\n", n, min, p50, p95, p99, sum/n;
}'
