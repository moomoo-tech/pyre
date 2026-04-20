#!/usr/bin/env bash
# Compression perf test — baseline (disabled) vs enabled (br / gzip)
# Usage: bash benchmarks/run_compression_bench.sh
set -e

if ! command -v wrk &>/dev/null; then
    echo "wrk not found"; exit 1
fi

PORT=8001
URL="http://127.0.0.1:${PORT}/json-fortunes"
DURATION="10s"
THREADS=4
CONNECTIONS=100

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -z "${PYTHON:-}" ]]; then
    if [[ -x .venv/bin/python ]]; then
        PYTHON=".venv/bin/python"
    else
        PYTHON="python3"
    fi
fi

start_server() {
    local compression="$1"
    PYRE_COMPRESSION=$compression PYRE_PORT=$PORT \
        $PYTHON benchmarks/bench_compression.py >/tmp/bench_compression.log 2>&1 &
    local pid=$!
    for _ in $(seq 1 50); do
        sleep 0.1
        if curl -sf http://127.0.0.1:${PORT}/ >/dev/null 2>&1; then
            echo "$pid"
            return
        fi
    done
    echo "server failed to start" >&2; exit 1
}

stop_server() {
    kill "$1" 2>/dev/null || true
    wait "$1" 2>/dev/null || true
    # Port may linger briefly — wait until free
    for _ in $(seq 1 20); do
        if ! ss -ltn 2>/dev/null | grep -q ":${PORT} "; then return; fi
        sleep 0.1
    done
}

run_wrk() {
    local label="$1"; shift
    echo ""
    echo "── ${label} ──"
    wrk -t${THREADS} -c${CONNECTIONS} -d${DURATION} "$@" "$URL" \
        | awk '/Requests\/sec|Transfer\/sec/ { print "  " $0 }'
}

echo "╔══════════════════════════════════════════════════════════╗"
echo "║  Pyre compression benchmark — /json-fortunes (~3.5 KB)  ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo "Config: ${THREADS} threads, ${CONNECTIONS} connections, ${DURATION}"

# Baseline: compression compiled in but disabled. Client sends
# Accept-Encoding to match the "enabled" test for apples-to-apples.
echo ""
echo "▸ Baseline (compression DISABLED, client sends Accept-Encoding)"
PID=$(start_server 0)
run_wrk "baseline-br-gzip" -H "Accept-Encoding: br, gzip"
stop_server "$PID"

# Enabled: same client. Expect transfer drop + comparable RPS.
echo ""
echo "▸ Compression ENABLED — Accept-Encoding: br, gzip (brotli selected)"
PID=$(start_server 1)
run_wrk "enabled-br-gzip" -H "Accept-Encoding: br, gzip"
stop_server "$PID"

echo ""
echo "▸ Compression ENABLED — Accept-Encoding: gzip only"
PID=$(start_server 1)
run_wrk "enabled-gzip" -H "Accept-Encoding: gzip"
stop_server "$PID"

echo ""
echo "▸ Compression ENABLED — client sends NO Accept-Encoding (fast path no-op)"
PID=$(start_server 1)
run_wrk "enabled-noaccept"
stop_server "$PID"

echo ""
echo "Key observations:"
echo "  • Baseline vs enabled-noaccept → no-op fast path cost"
echo "  • Baseline(br,gzip) vs enabled(br,gzip) → compression cost vs wire saving"
echo "  • Transfer/sec drop = bandwidth saved per second"
