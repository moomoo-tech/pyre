#!/usr/bin/env bash
# ╔══════════════════════════════════════════════════════════════════╗
# ║        Pyre vs Robyn — Head-to-Head Benchmark                   ║
# ╚══════════════════════════════════════════════════════════════════╝
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

source .venv/bin/activate 2>/dev/null || true

THREADS=4
CONNECTIONS=100
DURATION="10s"
PYRE_PORT=18888
ROBYN_PORT=18889

cleanup() {
    lsof -ti:$PYRE_PORT | xargs kill -9 2>/dev/null || true
    lsof -ti:$ROBYN_PORT | xargs kill -9 2>/dev/null || true
    sleep 0.5
}
trap cleanup EXIT
cleanup 2>/dev/null

# ─── Check wrk ───────────────────────────────────────────────
if ! command -v wrk &>/dev/null; then
    echo "❌ wrk not found."
    exit 1
fi

NUM_CPUS=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)

# ─── Create Pyre bench server ────────────────────────────────
PYRE_SERVER=$(mktemp /tmp/pyre_vs_XXXXXX.py)
cat > "$PYRE_SERVER" << PYEOF
from pyreframework import Pyre
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

@app.get("/compute")
def compute(req):
    total = 0
    for i in range(100):
        total += i * i
    return {"result": total}

if __name__ == "__main__":
    app.run(host="127.0.0.1", port=$PYRE_PORT, mode="subinterp")
PYEOF

# POST lua script
POST_LUA=$(mktemp /tmp/wrk_post_XXXXXX.lua)
cat > "$POST_LUA" << 'LUAEOF'
wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"
wrk.body = '{"symbol":"AAPL","quantity":100,"price":185.50,"side":"buy"}'
LUAEOF

# ─── Helpers ─────────────────────────────────────────────────
run_wrk() {
    local url="$1"
    local duration="$2"
    local lua="$3"
    local args="-t$THREADS -c$CONNECTIONS -d$duration --latency"
    [[ -n "$lua" ]] && args="$args -s $lua"
    wrk $args "$url" 2>&1
}

parse_rps() {
    echo "$1" | grep "Requests/sec" | awk '{print $2}'
}

parse_latency() {
    local p=$1 data="$2"
    echo "$data" | grep "${p}%" | awk '{print $2}'
}

# Collect results into arrays
declare -a ROUTE_NAMES PYRE_RPS ROBYN_RPS PYRE_P50 ROBYN_P50 PYRE_P99 ROBYN_P99

bench_route() {
    local name="$1"
    local path="$2"
    local method="${3:-GET}"

    echo "  Testing: $name ..."

    local lua_arg=""
    [[ "$method" == "POST" ]] && lua_arg="$POST_LUA"

    local pyre_result robyn_result
    pyre_result=$(run_wrk "http://127.0.0.1:$PYRE_PORT$path" "$DURATION" "$lua_arg")
    robyn_result=$(run_wrk "http://127.0.0.1:$ROBYN_PORT$path" "$DURATION" "$lua_arg")

    ROUTE_NAMES+=("$name")
    PYRE_RPS+=("$(parse_rps "$pyre_result")")
    ROBYN_RPS+=("$(parse_rps "$robyn_result")")
    PYRE_P50+=("$(parse_latency 50 "$pyre_result")")
    ROBYN_P50+=("$(parse_latency 50 "$robyn_result")")
    PYRE_P99+=("$(parse_latency 99 "$pyre_result")")
    ROBYN_P99+=("$(parse_latency 99 "$robyn_result")")
}

# ─── Start servers ────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║         Pyre v1.4.0 vs Robyn — Head-to-Head             ║"
echo "╠══════════════════════════════════════════════════════════╣"
printf "║  CPUs: %-47s║\n" "$NUM_CPUS"
printf "║  wrk: t=%s c=%s d=%s%-33s║\n" "$THREADS" "$CONNECTIONS" "$DURATION" ""
echo "╚══════════════════════════════════════════════════════════╝"

echo ""
echo "Starting Pyre on :$PYRE_PORT ..."
python "$PYRE_SERVER" &
PYRE_PID=$!
sleep 2

echo "Starting Robyn on :$ROBYN_PORT ..."
ROBYN_PORT=$ROBYN_PORT python benchmarks/robyn_app.py 2>&1 &
ROBYN_PID=$!
sleep 3

# Verify
curl -sf "http://127.0.0.1:$PYRE_PORT/" > /dev/null || { echo "❌ Pyre not running"; exit 1; }
curl -sf "http://127.0.0.1:$ROBYN_PORT/" > /dev/null || { echo "❌ Robyn not running"; exit 1; }
echo "✅ Both servers ready"
echo ""

# ─── Run benchmarks ──────────────────────────────────────────
bench_route "GET /"                    "/"
bench_route "GET /json"                "/json"
bench_route "GET /user/42"             "/user/42"
bench_route "GET /user/7/post/99"      "/user/7/post/99"
bench_route "POST /echo"               "/echo" "POST"
bench_route "GET /headers"             "/headers"
bench_route "GET /compute"             "/compute"

# ─── Memory ──────────────────────────────────────────────────
PYRE_MEM=$(ps -o rss= -p $PYRE_PID 2>/dev/null | awk '{printf "%.1f", $1/1024}')
ROBYN_MEM=$(ps -o rss= -p $ROBYN_PID 2>/dev/null | awk '{printf "%.1f", $1/1024}')

# ─── Results table ───────────────────────────────────────────
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Results: Pyre v1.4.0 vs Robyn"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
printf "  %-25s │ %12s │ %12s │ %8s\n" "Route" "Pyre (req/s)" "Robyn (req/s)" "Ratio"
printf "  %-25s─┼─%12s─┼─%12s─┼─%8s\n" "─────────────────────────" "────────────" "────────────" "────────"

for i in "${!ROUTE_NAMES[@]}"; do
    p=${PYRE_RPS[$i]}
    r=${ROBYN_RPS[$i]}
    if [[ -n "$p" && -n "$r" && "$r" != "0" ]]; then
        ratio=$(awk "BEGIN {printf \"%.1fx\", $p / $r}")
    else
        ratio="N/A"
    fi
    printf "  %-25s │ %12s │ %12s │ %8s\n" "${ROUTE_NAMES[$i]}" "$p" "$r" "$ratio"
done

echo ""
printf "  %-25s │ %12s │ %12s │\n" "Memory (MB)" "$PYRE_MEM" "$ROBYN_MEM"

echo ""
echo "  Latency P50 / P99:"
printf "  %-25s │ %8s / %-8s │ %8s / %-8s\n" "Route" "Pyre P50" "P99" "Robyn P50" "P99"
printf "  %-25s─┼─%8s───%-8s─┼─%8s───%-8s\n" "─────────────────────────" "────────" "────────" "────────" "────────"
for i in "${!ROUTE_NAMES[@]}"; do
    printf "  %-25s │ %8s / %-8s │ %8s / %-8s\n" \
        "${ROUTE_NAMES[$i]}" "${PYRE_P50[$i]}" "${PYRE_P99[$i]}" "${ROBYN_P50[$i]}" "${ROBYN_P99[$i]}"
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Done."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Cleanup
rm -f "$PYRE_SERVER" "$POST_LUA"
