// L02 hot-path-decision-time
//
// Sends a small chat request against marg (pointed at marg-provider-stub
// with 0 latency) and reads the latency observed. Reports the p50, p95,
// p99 contribution from the marg internal pipeline (everything outside the
// upstream call).
//
// Acceptance gate: p99 < 1ms on single-node-prod with Kavach OFF over 1M
// samples. dev-laptop is informational.
//
// Inputs (env):
//   MARG_URL       http base URL (default http://127.0.0.1:8080)
//   MARG_TOKEN     marg api token from `marg keys create`
//   MODEL          requested model (default gpt-4o-mini)

import http from "k6/http";
import { check } from "k6";
import { Trend } from "k6/metrics";

const marg = (__ENV.MARG_URL || "http://127.0.0.1:8080") + "/v1/chat/completions";
const token = __ENV.MARG_TOKEN || "REPLACE_ME";
const model = __ENV.MODEL || "gpt-4o-mini";

const decisionMs = new Trend("marg_decision_ms");

export const options = {
    scenarios: {
        steady: {
            executor: "constant-arrival-rate",
            rate: 1000,
            timeUnit: "1s",
            duration: "60s",
            preAllocatedVUs: 50,
            maxVUs: 200,
        },
    },
    thresholds: {
        "marg_decision_ms": ["p(99)<10"], // dev-laptop band; the production gate is 1ms.
    },
};

export default function () {
    const payload = JSON.stringify({
        model,
        messages: [{ role: "user", content: "ping" }],
        max_tokens: 1,
    });

    const headers = {
        "Authorization": `Bearer ${token}`,
        "Content-Type": "application/json",
    };

    const start = Date.now();
    const res = http.post(marg, payload, { headers });
    const totalMs = Date.now() - start;
    decisionMs.add(totalMs);

    check(res, {
        "status was 200": (r) => r.status === 200,
    });
}
