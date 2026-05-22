#!/usr/bin/env bash
# T03 cluster-3-passthrough
#
# Sustained non-streaming RPS across 3 Marg instances sharing one Postgres
# and one Redis. Acceptance gate (cluster-3 rig): >= 150,000 req/s sustained
# for 10 minutes with p99 < 75ms.
#
# This script is the runner contract for the cluster-3 rig. It expects a
# rig bring-up script to have already provisioned:
#   - 3 marg instances exposing port 8080 (URLs in MARG_URLS, comma separated)
#   - 1 Postgres instance reachable from each marg
#   - 1 Redis instance reachable from each marg
#   - the marg-provider-stub running on STUB_URL
#   - a load-balancer at LB_URL fronting the 3 marg instances
#
# Local dev-laptop note: this scenario does not run on a single laptop. The
# documented rig bring-up lives in bench/rigs/cluster-3 (created in P07).
# Use the smoke variant T03-smoke.sh for a single-instance sanity pass.
set -euo pipefail

LB_URL=${LB_URL:?LB_URL required (load balancer fronting the 3 marg instances)}
TOKEN=${TOKEN:?TOKEN required (marg api token with unlimited budget)}
DURATION=${DURATION:-600}            # 10 minutes
TARGET_RPS=${TARGET_RPS:-150000}
VUS=${VUS:-2000}
WORK_DIR=${WORK_DIR:-./bench/results/T03-tmp}

if ! command -v k6 >/dev/null 2>&1; then
    echo "k6 not installed; cannot run T03. Install: https://k6.io/" >&2
    exit 1
fi

mkdir -p "$WORK_DIR"

cat > "$WORK_DIR/script.js" <<'JS'
import http from 'k6/http';
import { check } from 'k6';

const URL = `${__ENV.LB_URL}/v1/chat/completions`;
const TOKEN = __ENV.TOKEN;
const BODY = JSON.stringify({
  model: 'gpt-4o-mini',
  messages: [{ role: 'user', content: 'hello cluster' }],
});

export const options = {
  scenarios: {
    constant_rps: {
      executor: 'constant-arrival-rate',
      rate: parseInt(__ENV.TARGET_RPS, 10),
      timeUnit: '1s',
      duration: `${__ENV.DURATION}s`,
      preAllocatedVUs: parseInt(__ENV.VUS, 10),
      maxVUs: parseInt(__ENV.VUS, 10) * 4,
    },
  },
  thresholds: {
    http_req_duration: ['p(99)<75'],
    http_req_failed: ['rate<0.001'],
  },
};

export default function () {
  const res = http.post(URL, BODY, {
    headers: { 'Authorization': `Bearer ${TOKEN}`, 'Content-Type': 'application/json' },
  });
  check(res, { 'status 200': r => r.status === 200 });
}
JS

LB_URL="$LB_URL" TOKEN="$TOKEN" TARGET_RPS="$TARGET_RPS" DURATION="$DURATION" VUS="$VUS" \
    k6 run --summary-export="$WORK_DIR/summary.json" "$WORK_DIR/script.js"

# k6 returns non-zero when a threshold fails. We rely on that as the gate.
echo "PASS"
