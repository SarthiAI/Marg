// T01 single-instance-passthrough
//
// Sustained non-streaming throughput against marg pointed at the provider
// stub (latency 5 ms). Acceptance gate (single-node-prod): >= 50k req/s,
// p99 < 50 ms, sustained for 10 minutes.
//
// Inputs (env):
//   MARG_URL    default http://127.0.0.1:8080
//   MARG_TOKEN  required
//   MODEL       default gpt-4o-mini
//   RATE        constant arrival rate (default 50000)
//   DURATION    test duration (default "10m")

import http from "k6/http";
import { check } from "k6";

const marg = (__ENV.MARG_URL || "http://127.0.0.1:8080") + "/v1/chat/completions";
const token = __ENV.MARG_TOKEN || "REPLACE_ME";
const model = __ENV.MODEL || "gpt-4o-mini";
const rate = parseInt(__ENV.RATE || "50000");
const duration = __ENV.DURATION || "10m";

export const options = {
    scenarios: {
        sustained: {
            executor: "constant-arrival-rate",
            rate,
            timeUnit: "1s",
            duration,
            preAllocatedVUs: 500,
            maxVUs: 5000,
        },
    },
    thresholds: {
        "http_req_duration": ["p(99)<50"],
        "http_req_failed":   ["rate<0.001"],
    },
};

const payload = JSON.stringify({
    model,
    messages: [{ role: "user", content: "ping" }],
    max_tokens: 1,
});

const headers = {
    "Authorization": `Bearer ${token}`,
    "Content-Type": "application/json",
};

export default function () {
    const res = http.post(marg, payload, { headers });
    check(res, { "200": (r) => r.status === 200 });
}
