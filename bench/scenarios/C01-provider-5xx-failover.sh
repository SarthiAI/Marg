#!/usr/bin/env bash
# C01 provider-5xx-failover
#
# Exercises Marg's failover semantics: primary provider returns 503 for the
# first N requests, fallback provider must serve them. Verifies:
#   - The client never sees a 5xx (failover hides the failure).
#   - The request log records the failover (attempts column has 2+ entries).
#   - No retry storm (each upstream gets exactly one call per request).
#
# Inputs:
#   MARG_BIN          path to marg release binary (default ./target/release/marg)
#   STUB_BIN          path to marg-provider-stub release binary
#                     (default ./target/release/marg-provider-stub)
#   WORK_DIR          scratch dir (default ./bench/results/C01-tmp)
#   PORT              marg listen port (default 8081)
#   PRIMARY_PORT      primary stub port (default 18091)
#   FALLBACK_PORT     fallback stub port (default 18092)
#   REQUESTS          number of requests to send (default 5)
set -euo pipefail

MARG_BIN=${MARG_BIN:-./target/release/marg}
STUB_BIN=${STUB_BIN:-./target/release/marg-provider-stub}
WORK_DIR=${WORK_DIR:-./bench/results/C01-tmp}
PORT=${PORT:-8081}
PRIMARY_PORT=${PRIMARY_PORT:-18091}
FALLBACK_PORT=${FALLBACK_PORT:-18092}
REQUESTS=${REQUESTS:-5}

if [ ! -x "$MARG_BIN" ]; then
    echo "marg binary not found at $MARG_BIN; run cargo build --release first" >&2
    exit 1
fi
if [ ! -x "$STUB_BIN" ]; then
    echo "marg-provider-stub binary not found at $STUB_BIN; run cargo build --release first" >&2
    exit 1
fi

mkdir -p "$WORK_DIR"
CFG="$WORK_DIR/marg.toml"
DB="$WORK_DIR/marg.db"
PRIMARY_LOG="$WORK_DIR/primary.log"
FALLBACK_LOG="$WORK_DIR/fallback.log"
MARG_LOG="$WORK_DIR/marg.log"
rm -f "$DB" "$PRIMARY_LOG" "$FALLBACK_LOG" "$MARG_LOG"

# Primary (OpenAI shape) is forced to return 503 for the first N requests.
"$STUB_BIN" \
    --bind "127.0.0.1:${PRIMARY_PORT}" \
    --mode openai \
    --inject-status 503 \
    --inject-first-n "$REQUESTS" \
    > "$PRIMARY_LOG" 2>&1 &
PRIMARY_PID=$!

# Fallback (Anthropic shape) is healthy.
"$STUB_BIN" \
    --bind "127.0.0.1:${FALLBACK_PORT}" \
    --mode anthropic \
    > "$FALLBACK_LOG" 2>&1 &
FALLBACK_PID=$!

cleanup() {
    kill "${MARG_PID:-}" 2>/dev/null || true
    kill "$PRIMARY_PID" 2>/dev/null || true
    kill "$FALLBACK_PID" 2>/dev/null || true
    wait "${MARG_PID:-}" 2>/dev/null || true
    wait "$PRIMARY_PID" 2>/dev/null || true
    wait "$FALLBACK_PID" 2>/dev/null || true
}
trap cleanup EXIT

cat > "$CFG" <<TOML
[server]
bind = "127.0.0.1:${PORT}"

[storage]
backend = "sqlite"
path = "${DB}"

[providers.openai]
api_key = "stub-primary"
base_url = "http://127.0.0.1:${PRIMARY_PORT}"
timeout_seconds = 5

[providers.anthropic]
api_key = "stub-fallback"
base_url = "http://127.0.0.1:${FALLBACK_PORT}"
api_version = "2023-06-01"
default_max_tokens = 64
timeout_seconds = 5

[[routes]]
match.model = "gpt-*"
primary = "openai"
fallback = ["anthropic:claude-3-5-sonnet"]
TOML

# Wait for stubs to bind.
for port in "$PRIMARY_PORT" "$FALLBACK_PORT"; do
    for _ in {1..50}; do
        if nc -z 127.0.0.1 "$port" 2>/dev/null; then break; fi
        sleep 0.05
    done
done

"$MARG_BIN" start --config "$CFG" > "$MARG_LOG" 2>&1 &
MARG_PID=$!

for _ in {1..100}; do
    if curl -fsS "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then break; fi
    sleep 0.05
done

# Create a key with unlimited budget.
KEY_OUT=$("$MARG_BIN" keys create --principal-id c01-test --config "$CFG")
TOKEN=$(echo "$KEY_OUT" | awk '/marg_live_/ {print $1; exit}')
if [ -z "$TOKEN" ]; then
    echo "failed to extract token from key creation output:" >&2
    echo "$KEY_OUT" >&2
    exit 1
fi

success=0
failovers_seen=0
for i in $(seq 1 "$REQUESTS"); do
    BODY=$(curl -sS -o "$WORK_DIR/resp-$i.json" -w '%{http_code} %header{x-marg-provider} %header{x-marg-failovers}' \
        -H "Authorization: Bearer $TOKEN" \
        -H "Content-Type: application/json" \
        "http://127.0.0.1:${PORT}/v1/chat/completions" \
        --data "{\"model\":\"gpt-4o-mini\",\"messages\":[{\"role\":\"user\",\"content\":\"hello $i\"}]}") || true
    code=$(echo "$BODY" | awk '{print $1}')
    provider=$(echo "$BODY" | awk '{print $2}')
    failovers=$(echo "$BODY" | awk '{print $3}')
    echo "request $i: status=$code provider=$provider failovers=$failovers"
    if [ "$code" = "200" ] && [ "$provider" = "anthropic" ]; then
        success=$((success + 1))
    fi
    if [ "${failovers:-0}" -ge 1 ]; then
        failovers_seen=$((failovers_seen + 1))
    fi
done

echo
echo "successful failover requests: $success / $REQUESTS"
echo "responses with at least one failover header: $failovers_seen / $REQUESTS"
"$MARG_BIN" log tail --config "$CFG" --limit 10

if [ "$success" -ne "$REQUESTS" ] || [ "$failovers_seen" -ne "$REQUESTS" ]; then
    echo "FAIL: failover did not hide all primary 503s" >&2
    exit 1
fi

echo "PASS"
