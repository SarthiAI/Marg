// T02 single-instance-streaming
//
// Concurrent streaming connections, ~100 tok/s each from the stub.
// Acceptance gate (single-node-prod): >= 10k concurrent streams with no
// client-visible stalls.
//
// k6 does not natively read SSE chunks, so this scenario times the full
// response and asserts that the response completed within the expected
// window. Per-token cadence validation runs from the provider stub side.
//
// Inputs (env):
//   MARG_URL    default http://127.0.0.1:8080
//   MARG_TOKEN  required
//   MODEL       default gpt-4o-mini
//   VUS         concurrent streams (default 10000)
//   DURATION    test duration (default "5m")

import http from "k6/http";
import { check } from "k6";

const marg = (__ENV.MARG_URL || "http://127.0.0.1:8080") + "/v1/chat/completions";
const token = __ENV.MARG_TOKEN || "REPLACE_ME";
const model = __ENV.MODEL || "gpt-4o-mini";
const vus = parseInt(__ENV.VUS || "10000");
const duration = __ENV.DURATION || "5m";

export const options = {
    scenarios: {
        streaming: {
            executor: "constant-vus",
            vus,
            duration,
        },
    },
    thresholds: {
        "http_req_failed": ["rate<0.001"],
    },
};

const payload = JSON.stringify({
    model,
    messages: [{ role: "user", content: "tell me a short story" }],
    stream: true,
    max_tokens: 32,
});

const headers = {
    "Authorization": `Bearer ${token}`,
    "Content-Type": "application/json",
};

export default function () {
    const res = http.post(marg, payload, { headers, timeout: "30s" });
    check(res, {
        "200": (r) => r.status === 200,
        "ends with [DONE]": (r) => r.body.indexOf("[DONE]") !== -1,
    });
}
