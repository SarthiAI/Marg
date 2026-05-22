#!/usr/bin/env bash
# L01 cold-start
#
# Measures the time from `marg start` invocation to the first successful
# /health response. Acceptance gate (single-node-prod): < 1.5s including TLS
# handshake and first storage hit.
#
# Inputs:
#   MARG_BIN   path to the marg release binary (default ./target/release/marg)
#   MARG_CFG   path to marg config (default ./marg.toml)
#   PORT       port the bind config uses (default 8080)
set -euo pipefail

MARG_BIN=${MARG_BIN:-./target/release/marg}
MARG_CFG=${MARG_CFG:-./marg.toml}
PORT=${PORT:-8080}

if [ ! -x "$MARG_BIN" ]; then
    echo "marg binary not found at $MARG_BIN; run cargo build --release first" >&2
    exit 1
fi

start_ns=$(date +%s%N)
"$MARG_BIN" start --config "$MARG_CFG" > /tmp/marg-l01.log 2>&1 &
MARG_PID=$!
trap "kill $MARG_PID 2>/dev/null || true" EXIT

while ! curl -fsS "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; do
    if ! kill -0 $MARG_PID 2>/dev/null; then
        echo "marg exited before /health responded; log:"
        cat /tmp/marg-l01.log
        exit 1
    fi
    sleep 0.05
done
end_ns=$(date +%s%N)

elapsed_ms=$(( (end_ns - start_ns) / 1000000 ))
echo "L01 cold-start: ${elapsed_ms} ms"

kill $MARG_PID 2>/dev/null || true
wait $MARG_PID 2>/dev/null || true

# Acceptance gate: 1500 ms on single-node-prod. dev-laptop is informational.
if [ "$elapsed_ms" -gt 1500 ]; then
    echo "FAIL: cold-start exceeded 1.5s gate"
    exit 1
fi
echo "PASS"
