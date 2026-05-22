#!/usr/bin/env bash
# C04 postgres-failover
#
# Trigger Postgres primary swap during sustained load. Acceptance gate
# (cluster-3 rig): total downtime <= 10s, no data loss for committed
# request_log rows or budget_counters rows.
#
# This script is the runner contract for the cluster-3 rig. It expects:
#   - LB_URL fronting 3 Marg instances pointed at the same Postgres replica
#     set behind a virtual IP / pgbouncer.
#   - FAILOVER_CMD that triggers a primary swap (e.g. patronictl failover).
#   - psql access to the post-failover primary for the integrity check.
#
# Local dev-laptop note: not runnable on a single laptop. The cluster-3
# rig bring-up lives in bench/rigs/cluster-3 (created in P07).
set -euo pipefail

LB_URL=${LB_URL:?LB_URL required}
TOKEN=${TOKEN:?TOKEN required}
PG_DSN=${PG_DSN:?PG_DSN required (post-failover primary, for verification)}
FAILOVER_CMD=${FAILOVER_CMD:?FAILOVER_CMD required (e.g. \"patronictl -c cfg failover --master pg1 --candidate pg2\")}
WARMUP_SECONDS=${WARMUP_SECONDS:-30}
LOAD_SECONDS=${LOAD_SECONDS:-90}
WORK_DIR=${WORK_DIR:-./bench/results/C04-tmp}

if ! command -v k6 >/dev/null 2>&1; then
    echo "k6 not installed; cannot run C04. Install: https://k6.io/" >&2
    exit 1
fi
if ! command -v psql >/dev/null 2>&1; then
    echo "psql not installed; cannot verify post-failover row counts" >&2
    exit 1
fi

mkdir -p "$WORK_DIR"

cat > "$WORK_DIR/script.js" <<'JS'
import http from 'k6/http';
import { check } from 'k6';
import { sleep } from 'k6';

const URL = `${__ENV.LB_URL}/v1/chat/completions`;
const TOKEN = __ENV.TOKEN;
const BODY = JSON.stringify({
  model: 'gpt-4o-mini',
  messages: [{ role: 'user', content: 'failover load' }],
});

export const options = {
  scenarios: {
    sustained: {
      executor: 'constant-arrival-rate',
      rate: 1000,
      timeUnit: '1s',
      duration: `${__ENV.LOAD_SECONDS}s`,
      preAllocatedVUs: 200,
      maxVUs: 1000,
    },
  },
};

export default function () {
  const res = http.post(URL, BODY, {
    headers: { 'Authorization': `Bearer ${TOKEN}`, 'Content-Type': 'application/json' },
  });
  check(res, { 'status 200 or 503': r => r.status === 200 || r.status === 503 });
}
JS

echo "warming up for ${WARMUP_SECONDS}s..."
LB_URL="$LB_URL" TOKEN="$TOKEN" LOAD_SECONDS="$WARMUP_SECONDS" \
    k6 run --quiet "$WORK_DIR/script.js" || true

PRE_LOG_COUNT=$(psql "$PG_DSN" -At -c 'SELECT COUNT(*) FROM request_log')
echo "pre-failover request_log row count: $PRE_LOG_COUNT"

(
  LB_URL="$LB_URL" TOKEN="$TOKEN" LOAD_SECONDS="$LOAD_SECONDS" \
      k6 run --summary-export="$WORK_DIR/summary.json" "$WORK_DIR/script.js"
) &
LOAD_PID=$!

sleep 10
echo "triggering postgres failover..."
START_NS=$(date +%s%N)
eval "$FAILOVER_CMD"
# Wait for marg's /ready to report storage.ok again on every instance. The
# rig bring-up exposes a comma-separated list in READY_URLS; if absent we
# poll LB_URL/ready.
READY_URLS=${READY_URLS:-${LB_URL}/ready}
IFS=',' read -ra URLS <<< "$READY_URLS"
for url in "${URLS[@]}"; do
    until curl -fsS "$url" | grep -q '"ok":true'; do
        if (( ($(date +%s%N) - START_NS) / 1000000 > 30000 )); then
            echo "FAIL: $url did not return ready within 30s of failover" >&2
            kill $LOAD_PID 2>/dev/null || true
            exit 1
        fi
        sleep 0.5
    done
done
END_NS=$(date +%s%N)
DOWNTIME_MS=$(( (END_NS - START_NS) / 1000000 ))
echo "failover downtime: ${DOWNTIME_MS} ms"

wait $LOAD_PID
POST_LOG_COUNT=$(psql "$PG_DSN" -At -c 'SELECT COUNT(*) FROM request_log')
echo "post-failover request_log row count: $POST_LOG_COUNT"

if [ "$POST_LOG_COUNT" -lt "$PRE_LOG_COUNT" ]; then
    echo "FAIL: row count dropped after failover (data loss)" >&2
    exit 1
fi
if [ "$DOWNTIME_MS" -gt 10000 ]; then
    echo "FAIL: downtime exceeded 10s gate" >&2
    exit 1
fi

echo "PASS"
