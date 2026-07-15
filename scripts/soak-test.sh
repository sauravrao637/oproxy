#!/usr/bin/env bash
#
# Load and soak test harness for baseline latency, sustained traffic, large
# response streaming, and process memory growth.
#
# What this does:
#   1. Starts a tiny local HTTP origin (python3 http.server-based echo).
#   2. Starts an isolated oproxy instance (its own port, storage dir, CA dir)
#      pointed at nothing in particular - the origin is reached directly by
#      the client requests being proxied *through* oproxy.
#   3. Baseline: fires REQUESTS requests at CONCURRENCY concurrency through
#      the proxy, using `hey` or `wrk` if either is installed, falling back
#      to a small built-in concurrent-curl runner otherwise. Reports
#      p50/p95/p99 latency and failure count.
#   4. Soak: sustained load for SOAK_SECONDS, mixing small and large
#      (LARGE_BODY_BYTES) bodies, sampling the oproxy process's RSS every
#      SAMPLE_INTERVAL seconds. Fails if RSS grows past MAX_RSS_GROWTH_RATIO
#      of its first post-warmup sample, since unbounded growth under
#      sustained load (not just a single large request) is the regression
#      this is meant to catch.
#
# Tooling choice: shell + curl, with opportunistic use of `hey`/`wrk` when
# present, rather than k6 or a Rust criterion/bench harness. Reasoning:
#   - No extra runtime (Node/Go) or crate to vendor - curl and bash are
#     already assumed available everywhere this project documents (Docker,
#     CI, local dev).
#   - A Rust bench harness would need to live in the `tests/` or a `benches/`
#     crate and compile with the project - fine long-term, but it means
#     perf tooling can never run unless the whole workspace builds, which
#     defeats the point of a quick smoke/regression check operators or CI
#     can run against a released binary.
#   - hey/wrk give better latency histograms when available; the built-in
#     fallback keeps this script self-contained when they aren't.
#
# Usage:
#   scripts/soak-test.sh [--bin PATH] [--requests N] [--concurrency N] \
#       [--soak-seconds N] [--large-body-bytes N]
#
# Exit code is nonzero if the baseline had any failures, the soak phase had
# any failures, or RSS grew past the configured ratio.
#
# NOTE: the specific latency/throughput numbers this script prints are a
# measurement of *this machine, this run* - they are not a portable SLA.
# Record a baseline on your own reference hardware/CI runner and compare
# future runs against that, rather than against numbers quoted in a doc.

set -uo pipefail

OPROXY_BIN="${OPROXY_BIN:-target/release/oproxy}"
REQUESTS="${REQUESTS:-500}"
CONCURRENCY="${CONCURRENCY:-50}"
SOAK_SECONDS="${SOAK_SECONDS:-300}"
LARGE_BODY_BYTES="${LARGE_BODY_BYTES:-5242880}" # 5 MiB
SAMPLE_INTERVAL="${SAMPLE_INTERVAL:-5}"
MAX_RSS_GROWTH_RATIO="${MAX_RSS_GROWTH_RATIO:-1.5}" # fail if RSS > 1.5x the first sample

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) OPROXY_BIN="$2"; shift 2 ;;
    --requests) REQUESTS="$2"; shift 2 ;;
    --concurrency) CONCURRENCY="$2"; shift 2 ;;
    --soak-seconds) SOAK_SECONDS="$2"; shift 2 ;;
    --large-body-bytes) LARGE_BODY_BYTES="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,40p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ ! -x "$OPROXY_BIN" ]]; then
  echo "error: oproxy binary not found/executable at '$OPROXY_BIN'" >&2
  echo "Build it first: cargo build --release" >&2
  exit 2
fi

WORKDIR="$(mktemp -d)"
ORIGIN_PORT=19190
PROXY_PORT=19191
STORAGE_DIR="$WORKDIR/storage"
CA_DIR="$WORKDIR/certs"
mkdir -p "$STORAGE_DIR" "$CA_DIR"

ORIGIN_PID=""
PROXY_PID=""

cleanup() {
  local code=$?
  [[ -n "$PROXY_PID" ]] && kill "$PROXY_PID" 2>/dev/null
  [[ -n "$ORIGIN_PID" ]] && kill "$ORIGIN_PID" 2>/dev/null
  wait 2>/dev/null
  rm -rf "$WORKDIR"
  exit "$code"
}
trap cleanup EXIT INT TERM

echo "== oproxy soak test =="
echo "bin=$OPROXY_BIN requests=$REQUESTS concurrency=$CONCURRENCY soak_seconds=$SOAK_SECONDS large_body_bytes=$LARGE_BODY_BYTES"
echo "workdir=$WORKDIR"

# --- 1. tiny origin server -------------------------------------------------
# Echoes method/path/body-length; also serves a large deterministic body at
# /large so the soak phase can exercise the streaming path
# (stream_threshold_bytes, see docs/configuration.md).
cat > "$WORKDIR/origin.py" <<'PYEOF'
import http.server
import sys

LARGE_BYTES = int(sys.argv[2]) if len(sys.argv) > 2 else 5 * 1024 * 1024

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass  # keep soak output readable

    def _respond(self):
        if self.path == "/large":
            body = b"x" * LARGE_BYTES
        else:
            length = int(self.headers.get("Content-Length", 0))
            _ = self.rfile.read(length) if length else b""
            body = b"ok"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self._respond()

    def do_POST(self):
        self._respond()

if __name__ == "__main__":
    port = int(sys.argv[1])
    http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PYEOF

python3 "$WORKDIR/origin.py" "$ORIGIN_PORT" "$LARGE_BODY_BYTES" &
ORIGIN_PID=$!
sleep 0.5

# --- 2. isolated oproxy instance -------------------------------------------
OPROXY_PORT="$PROXY_PORT" \
OPROXY_BIND_HOST=127.0.0.1 \
OPROXY_STORAGE_PATH="$STORAGE_DIR" \
OPROXY_MITM_ENABLED=false \
OPROXY_LOG_LEVEL=warn \
  "$OPROXY_BIN" &
PROXY_PID=$!
sleep 1

if ! kill -0 "$PROXY_PID" 2>/dev/null; then
  echo "error: oproxy failed to start (see above)" >&2
  exit 1
fi

PROXY_URL="http://127.0.0.1:${PROXY_PORT}"
ORIGIN_URL="http://127.0.0.1:${ORIGIN_PORT}"

# --- 3. baseline: REQUESTS requests at CONCURRENCY concurrency -------------
run_baseline_with_tool() {
  local tool="$1"
  if command -v "$tool" >/dev/null 2>&1; then
    return 0
  fi
  return 1
}

echo
echo "-- baseline --"
baseline_fails=0
if run_baseline_with_tool hey; then
  # hey prints a status code distribution but exits 0 regardless of the
  # codes it saw, so this doesn't (yet) feed baseline_fails - read the
  # printed distribution by eye when hey/wrk is used.
  hey -n "$REQUESTS" -c "$CONCURRENCY" -x "$PROXY_URL" "$ORIGIN_URL/" || true
elif run_baseline_with_tool wrk; then
  # wrk works in duration terms, not fixed request counts; approximate with
  # a short fixed-duration run instead.
  wrk -t"$CONCURRENCY" -c"$CONCURRENCY" -d10s --proxy "$PROXY_URL" "$ORIGIN_URL/" || true
else
  echo "(hey/wrk not found - using built-in curl-based fallback, coarser percentiles)"
  LAT_FILE="$WORKDIR/latencies.txt"
  : > "$LAT_FILE"
  FAIL_COUNT_FILE="$WORKDIR/fail_count"
  echo 0 > "$FAIL_COUNT_FILE"

  fire_one() {
    local t0 t1 code
    t0=$(date +%s%N)
    # --noproxy '' is load-bearing: curl silently honours no_proxy/NO_PROXY
    # for the *destination* host even when -x/--proxy is given explicitly,
    # and no_proxy commonly includes 127.0.0.1/localhost (it does in this
    # project's own Docker/CI env and in plenty of shells generally). Both
    # the proxy and the origin here are on 127.0.0.1 - without this flag
    # curl silently bypasses oproxy and talks to the origin directly, and
    # the whole harness "passes" without ever exercising the proxy.
    code=$(curl -s -o /dev/null -w '%{http_code}' --noproxy '' -x "$PROXY_URL" "$ORIGIN_URL/" --max-time 10)
    t1=$(date +%s%N)
    echo "$(( (t1 - t0) / 1000000 ))" >> "$LAT_FILE"
    if [[ "$code" != "200" ]]; then
      echo 1 >> "$WORKDIR/fails.txt"
    fi
  }

  : > "$WORKDIR/fails.txt"
  sent=0
  while (( sent < REQUESTS )); do
    batch=$(( REQUESTS - sent < CONCURRENCY ? REQUESTS - sent : CONCURRENCY ))
    batch_pids=()
    for _ in $(seq 1 "$batch"); do
      fire_one &
      batch_pids+=("$!")
    done
    # `wait` with no arguments waits for *every* background job of this
    # shell - including the origin/proxy servers started earlier, which
    # never exit on their own. Wait only on this batch's PIDs.
    wait "${batch_pids[@]}"
    sent=$(( sent + batch ))
  done

  total=$(wc -l < "$LAT_FILE")
  fails=$(wc -l < "$WORKDIR/fails.txt")
  sort -n "$LAT_FILE" > "$WORKDIR/latencies.sorted"
  p() {
    local pct="$1"
    local idx=$(( (total * pct + 99) / 100 ))
    [[ $idx -lt 1 ]] && idx=1
    sed -n "${idx}p" "$WORKDIR/latencies.sorted"
  }
  echo "requests=$total failures=$fails p50=$(p 50)ms p95=$(p 95)ms p99=$(p 99)ms"
  baseline_fails="$fails"
  if [[ "$fails" -gt 0 ]]; then
    echo "BASELINE FAILED: $fails/$total requests did not return 200" >&2
  fi
fi

# --- 4. soak: sustained load + memory sampling -----------------------------
echo
echo "-- soak (${SOAK_SECONDS}s) --"
RSS_LOG="$WORKDIR/rss.txt"
: > "$RSS_LOG"
SOAK_FAILS="$WORKDIR/soak_fails.txt"
: > "$SOAK_FAILS"

sample_rss_kb() {
  awk '/^VmRSS/ {print $2}' "/proc/$PROXY_PID/status" 2>/dev/null
}

soak_worker() {
  local deadline=$(( $(date +%s) + SOAK_SECONDS ))
  while (( $(date +%s) < deadline )); do
    local path="/"
    (( RANDOM % 10 == 0 )) && path="/large"
    # See the matching comment in fire_one(): --noproxy '' stops curl from
    # honouring an ambient no_proxy=127.0.0.1 and silently skipping the proxy.
    code=$(curl -s -o /dev/null -w '%{http_code}' --noproxy '' -x "$PROXY_URL" "$ORIGIN_URL$path" --max-time 15)
    [[ "$code" != "200" ]] && echo "1" >> "$SOAK_FAILS"
  done
}

# A handful of concurrent workers hammering for the soak duration.
soak_worker_pids=()
for _ in $(seq 1 8); do
  soak_worker &
  soak_worker_pids+=("$!")
done

deadline=$(( $(date +%s) + SOAK_SECONDS ))
first_rss=""
while (( $(date +%s) < deadline )); do
  rss="$(sample_rss_kb)"
  if [[ -n "$rss" ]]; then
    echo "$(date +%s) $rss" >> "$RSS_LOG"
    [[ -z "$first_rss" ]] && first_rss="$rss"
  fi
  sleep "$SAMPLE_INTERVAL"
done

# As above: wait only on the soak workers, not on the never-exiting
# origin/proxy background jobs.
wait "${soak_worker_pids[@]}"

soak_total_fails=$(wc -l < "$SOAK_FAILS")
last_rss=$(tail -1 "$RSS_LOG" | awk '{print $2}')

echo "soak requests failed: $soak_total_fails"
if [[ -n "$first_rss" && -n "$last_rss" ]]; then
  ratio=$(awk -v a="$first_rss" -v b="$last_rss" 'BEGIN { if (a > 0) printf "%.2f", b/a; else print "n/a" }')
  echo "RSS: first=${first_rss}KB last=${last_rss}KB ratio=${ratio}x (fail threshold: ${MAX_RSS_GROWTH_RATIO}x)"
fi

status=0
if [[ "$baseline_fails" -gt 0 ]]; then
  status=1
fi
if [[ "$soak_total_fails" -gt 0 ]]; then
  echo "SOAK FAILED: $soak_total_fails request(s) failed under sustained load" >&2
  status=1
fi
if [[ -n "$first_rss" && -n "$last_rss" ]]; then
  over_limit=$(awk -v a="$first_rss" -v b="$last_rss" -v limit="$MAX_RSS_GROWTH_RATIO" \
    'BEGIN { if (a > 0 && b/a > limit) print "1"; else print "0" }')
  if [[ "$over_limit" == "1" ]]; then
    echo "SOAK FAILED: RSS grew past ${MAX_RSS_GROWTH_RATIO}x during the run - possible leak" >&2
    status=1
  fi
fi

exit "$status"
