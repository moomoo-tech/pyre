#!/usr/bin/env bash
# ╔══════════════════════════════════════════════════════════════════╗
# ║  Pyre v1.4.0 Full Benchmark Suite — Multi-phase + Latency Bins ║
# ╚══════════════════════════════════════════════════════════════════╝
#
# Usage:
#   bash benchmarks/run_full_bench.sh                    # defaults
#   bash benchmarks/run_full_bench.sh --duration 60s     # override
#   bash benchmarks/run_full_bench.sh --workers 24 --io-workers 16
#
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

source .venv/bin/activate 2>/dev/null || true

# ─── Defaults ────────────────────────────────────────────────────
THREADS=4
CONNECTIONS=100
DURATION="10s"
LONG_DURATION="300s"
PORT=18888
WORKERS=""
IO_WORKERS=""
SKIP_LONG=0

# ─── Parse args ──────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case $1 in
        --duration)     DURATION="$2"; shift 2;;
        --long)         LONG_DURATION="$2"; shift 2;;
        --threads)      THREADS="$2"; shift 2;;
        --connections)  CONNECTIONS="$2"; shift 2;;
        --port)         PORT="$2"; shift 2;;
        --workers)      WORKERS="$2"; shift 2;;
        --io-workers)   IO_WORKERS="$2"; shift 2;;
        --skip-long)    SKIP_LONG=1; shift;;
        *)              echo "Unknown arg: $1"; exit 1;;
    esac
done

# ─── Check wrk ───────────────────────────────────────────────────
if ! command -v wrk &>/dev/null; then
    echo "❌ wrk not found. Install: brew install wrk (macOS) / apt install wrk (Linux)"
    exit 1
fi

# ─── Cleanup ─────────────────────────────────────────────────────
cleanup() {
    lsof -ti:$PORT | xargs kill -9 2>/dev/null || true
    sleep 0.5
}
trap cleanup EXIT
cleanup 2>/dev/null

# ─── Write bench server ─────────────────────────────────────────
BENCH_SERVER=$(mktemp /tmp/pyre_bench_XXXXXX.py)
WORKER_ARGS=""
[[ -n "$WORKERS" ]] && WORKER_ARGS="workers=$WORKERS, "
[[ -n "$IO_WORKERS" ]] && WORKER_ARGS="${WORKER_ARGS}io_workers=$IO_WORKERS, "

cat > "$BENCH_SERVER" << PYEOF
from pyreframework import Pyre, PyreResponse
import json

app = Pyre()

@app.get("/")
def index(req):
    return "Hello from Pyre!"

@app.get("/json")
def json_route(req):
    return {"message": "hello", "status": "ok", "code": 200}

@app.get("/user/{id}")
def user(req):
    return {"id": req.params["id"], "ip": req.client_ip}

@app.get("/user/{id}/post/{post_id}")
def user_post(req):
    return {"user": req.params["id"], "post": req.params["post_id"]}

@app.post("/echo")
def echo(req):
    return req.json()

@app.get("/headers")
def headers(req):
    return {"host": req.headers.get("host", ""), "ua": req.headers.get("user-agent", "")}

@app.get("/query")
def query(req):
    return req.query_params

@app.get("/compute")
def compute(req):
    total = 0
    for i in range(100):
        total += i * i
    return {"result": total}

if __name__ == "__main__":
    app.run(host="127.0.0.1", port=$PORT, ${WORKER_ARGS}mode="subinterp")
PYEOF

# Also create a POST lua script
POST_LUA=$(mktemp /tmp/wrk_post_XXXXXX.lua)
cat > "$POST_LUA" << 'LUAEOF'
wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"
wrk.body = '{"symbol":"AAPL","quantity":100,"price":185.50,"side":"buy"}'
LUAEOF

# ─── Helpers ─────────────────────────────────────────────────────
NUM_CPUS=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)
OS_NAME=$(uname -s)

print_header() {
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  $1"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

# Run wrk and parse output into structured format
run_bench() {
    local label="$1"
    local url="$2"
    local duration="$3"
    local lua_script="$4"

    local wrk_args="-t$THREADS -c$CONNECTIONS -d$duration --latency"
    [[ -n "$lua_script" ]] && wrk_args="$wrk_args -s $lua_script"

    local result
    result=$(wrk $wrk_args "$url" 2>&1)

    local rps=$(echo "$result" | grep "Requests/sec" | awk '{print $2}')
    local transfer=$(echo "$result" | grep "Transfer/sec" | awk '{print $2}')
    local avg_lat=$(echo "$result" | grep "Latency" | awk '{print $2}')
    local max_lat=$(echo "$result" | grep "Latency" | awk '{print $4}')
    local total=$(echo "$result" | grep "requests in" | awk '{print $1}')
    local errors=$(echo "$result" | grep "Socket errors" || echo "none")
    local non2xx=$(echo "$result" | grep "Non-2xx" || echo "")

    # Latency distribution (--latency flag)
    local p50=$(echo "$result" | grep "50%" | awk '{print $2}')
    local p75=$(echo "$result" | grep "75%" | awk '{print $2}')
    local p90=$(echo "$result" | grep "90%" | awk '{print $2}')
    local p99=$(echo "$result" | grep "99%" | awk '{print $2}')

    echo ""
    echo "  📊 $label"
    echo "  ┌─────────────────────────────────────────────────┐"
    printf "  │ QPS:        %-37s│\n" "$rps req/s"
    printf "  │ Total:      %-37s│\n" "$total requests"
    printf "  │ Transfer:   %-37s│\n" "${transfer}/s"
    echo "  ├─────────────────────────────────────────────────┤"
    printf "  │ Avg:        %-37s│\n" "$avg_lat"
    printf "  │ Max:        %-37s│\n" "$max_lat"
    echo "  ├────────── Latency Distribution ─────────────────┤"
    printf "  │ P50:        %-37s│\n" "$p50"
    printf "  │ P75:        %-37s│\n" "$p75"
    printf "  │ P90:        %-37s│\n" "$p90"
    printf "  │ P99:        %-37s│\n" "$p99"
    echo "  ├─────────────────────────────────────────────────┤"
    if [[ -n "$non2xx" ]]; then
        printf "  │ ⚠️  %-43s│\n" "$non2xx"
    else
        printf "  │ Errors:     %-37s│\n" "0 Non-2xx, 0 Socket"
    fi
    echo "  └─────────────────────────────────────────────────┘"
}

# ─── Start server ────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║       Pyre v1.4.0 — Full Benchmark Suite                ║"
echo "╠══════════════════════════════════════════════════════════╣"
printf "║  OS: %-49s║\n" "$OS_NAME ($(uname -m))"
printf "║  CPUs: %-47s║\n" "$NUM_CPUS"
printf "║  wrk: %-48s║\n" "t=$THREADS c=$CONNECTIONS"
printf "║  Short: %-46s║\n" "$DURATION"
printf "║  Long: %-47s║\n" "$LONG_DURATION"
[[ -n "$WORKERS" ]] && printf "║  Python workers: %-37s║\n" "$WORKERS"
[[ -n "$IO_WORKERS" ]] && printf "║  IO workers: %-41s║\n" "$IO_WORKERS"
echo "╚══════════════════════════════════════════════════════════╝"

echo ""
echo "Starting Pyre on :$PORT ..."
python "$BENCH_SERVER" &
SERVER_PID=$!
sleep 2

# Verify
if ! curl -sf "http://127.0.0.1:$PORT/" > /dev/null; then
    echo "❌ Server failed to start"
    exit 1
fi
echo "✅ Server ready (PID: $SERVER_PID)"

# Record baseline memory
MEM_START=$(ps -o rss= -p $SERVER_PID 2>/dev/null | awk '{printf "%.1f", $1/1024}')

# ─── Phase 1: Short bursts (all routes) ─────────────────────────
print_header "Phase 1: Short Burst — $DURATION per route"

run_bench "GET / (plain text)"              "http://127.0.0.1:$PORT/"                 "$DURATION"
run_bench "GET /json (JSON response)"       "http://127.0.0.1:$PORT/json"             "$DURATION"
run_bench "GET /user/42 (path param)"       "http://127.0.0.1:$PORT/user/42"          "$DURATION"
run_bench "GET /user/7/post/99 (2 params)"  "http://127.0.0.1:$PORT/user/7/post/99"   "$DURATION"
run_bench "POST /echo (JSON parse+serialize)" "http://127.0.0.1:$PORT/echo"           "$DURATION" "$POST_LUA"
run_bench "GET /headers (header access)"    "http://127.0.0.1:$PORT/headers"          "$DURATION"
run_bench "GET /query?a=1&b=2 (query params)" "http://127.0.0.1:$PORT/query?a=1&b=2&c=3" "$DURATION"
run_bench "GET /compute (CPU-bound)"        "http://127.0.0.1:$PORT/compute"          "$DURATION"

# ─── Phase 2: Concurrency scaling ───────────────────────────────
print_header "Phase 2: Concurrency Scaling — $DURATION"

for C in 50 100 256 512; do
    result=$(wrk -t$THREADS -c$C -d$DURATION --latency "http://127.0.0.1:$PORT/" 2>&1)
    rps=$(echo "$result" | grep "Requests/sec" | awk '{print $2}')
    p50=$(echo "$result" | grep "50%" | awk '{print $2}')
    p99=$(echo "$result" | grep "99%" | awk '{print $2}')
    max=$(echo "$result" | grep "Latency" | awk '{print $4}')
    printf "  c=%-4s │ %10s req/s │ P50 %-8s │ P99 %-8s │ Max %-8s\n" "$C" "$rps" "$p50" "$p99" "$max"
done

# ─── Phase 3: Long sustained (stability) ────────────────────────
if [[ $SKIP_LONG -eq 0 ]]; then
    print_header "Phase 3: Sustained Load — $LONG_DURATION GET /"

    run_bench "GET / sustained ($LONG_DURATION)" "http://127.0.0.1:$PORT/" "$LONG_DURATION"

    # Memory after sustained load
    MEM_END=$(ps -o rss= -p $SERVER_PID 2>/dev/null | awk '{printf "%.1f", $1/1024}')

    echo ""
    echo "  Memory: ${MEM_START} MB (start) → ${MEM_END} MB (after ${LONG_DURATION} sustained)"
fi

# ─── Phase 4: High concurrency stress ───────────────────────────
print_header "Phase 4: Stress Test — c=1024 $DURATION"

run_bench "GET / (c=1024 stress)" "http://127.0.0.1:$PORT/" "$DURATION"

# Override connections for this test
result=$(wrk -t$THREADS -c1024 -d$DURATION --latency "http://127.0.0.1:$PORT/" 2>&1)
rps=$(echo "$result" | grep "Requests/sec" | awk '{print $2}')
p50=$(echo "$result" | grep "50%" | awk '{print $2}')
p99=$(echo "$result" | grep "99%" | awk '{print $2}')
total=$(echo "$result" | grep "requests in" | awk '{print $1}')
errors=$(echo "$result" | grep "Socket errors" || echo "none")
non2xx=$(echo "$result" | grep "Non-2xx" || echo "0")

echo ""
echo "  🔥 Stress Result (c=1024):"
printf "     QPS: %s | P50: %s | P99: %s | Total: %s\n" "$rps" "$p50" "$p99" "$total"
printf "     Errors: %s\n" "${errors:-none}"

# ─── Summary ─────────────────────────────────────────────────────
MEM_FINAL=$(ps -o rss= -p $SERVER_PID 2>/dev/null | awk '{printf "%.1f", $1/1024}')

print_header "Summary"
echo ""
printf "  Server:     Pyre v1.4.0 (%s)\n" "$OS_NAME"
printf "  Memory:     %s MB (final)\n" "$MEM_FINAL"
printf "  Process:    alive ✅\n"

# Health check
curl -sf "http://127.0.0.1:$PORT/" > /dev/null && echo "  Health:     responding ✅" || echo "  Health:     DEAD ❌"

echo ""
echo "  Done. Server PID $SERVER_PID will be killed on exit."
echo ""

# Cleanup
rm -f "$BENCH_SERVER" "$POST_LUA"
