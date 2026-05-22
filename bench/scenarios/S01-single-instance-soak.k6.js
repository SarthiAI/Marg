// S01 single-instance-24h-soak
//
// 24 hour sustained run on single-node-prod at 80% of the T01 throughput.
// Acceptance gates: RSS growth < 5% across the run, p99 latency drift < 10%
// across the run, zero panics, zero open file descriptor leaks, audit chain
// flushes successfully (when Kavach lands in P08).
//
// This scenario is driven the same way as T01. The DIFFERENCE is duration
// and the additional observability sampling done by the surrounding runner
// (see `bench/rigs/single-node-prod/run-soak.sh` once it lands).
//
// Inputs (env):
//   MARG_URL     default http://127.0.0.1:8080
//   MARG_TOKEN   required
//   MODEL        default gpt-4o-mini
//   RATE         constant arrival rate (default 40000, 80% of 50000 T01 cap)
//   DURATION     soak duration (default "24h", smoke override "30m")
//   SAMPLE_EVERY Prometheus scrape spacing for the runner (default "60s")

import http from "k6/http";
import { check } from "k6";

const marg = (__ENV.MARG_URL || "http://127.0.0.1:8080") + "/v1/chat/completions";
const token = __ENV.MARG_TOKEN || "REPLACE_ME";
const model = __ENV.MODEL || "gpt-4o-mini";
const rate = parseInt(__ENV.RATE || "40000");
const duration = __ENV.DURATION || "24h";

export const options = {
    scenarios: {
        soak: {
            executor: "constant-arrival-rate",
            rate,
            timeUnit: "1s",
            duration,
            preAllocatedVUs: 400,
            maxVUs: 4000,
        },
    },
    thresholds: {
        // Soak runs allow a wider p99 ceiling than T01 to absorb GC and
        // OS-level jitter over 24h. Drift is what we care about, measured
        // by the runner against early-window vs late-window samples.
        "http_req_duration": ["p(99)<75"],
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
