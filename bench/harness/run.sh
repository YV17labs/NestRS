#!/usr/bin/env bash
# Process-mode bench runner — builds ONE SUT, gates it on the contract, then
# measures every tier with oha and writes <results-dir>/<provider>.json.
#
#   usage: run.sh <sut-dir> [results-dir]
#
# Protocol per tier: warmup (thrown away) → BENCH_RUNS measured runs → medians.
# The SUT is pinned to BENCH_SUT_CPUS, the load generator to BENCH_LOAD_CPUS —
# they never share a core. Two regimes are published: per-core (SUT pinned to
# one core — apples-to-apples efficiency) and all-cores (BENCH_SUT_CPUS=all —
# scaling; needs a host with enough cores to also isolate the load generator).
# Tunables (defaults are the quick local protocol; published runs use
# BENCH_WARMUP=30 BENCH_RUNS=5 BENCH_DURATION=30):
#   BENCH_WARMUP=10 BENCH_RUNS=3 BENCH_DURATION=15 BENCH_CONNECTIONS=64
#   BENCH_SUT_CPUS=0 BENCH_LOAD_CPUS=1-3     ("all" = no pinning)
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROVIDER_DIR="$(cd "${1:?usage: run.sh <sut-dir> [results-dir]}" && pwd)"
RESULTS_DIR="${2:-$BENCH_DIR/results/$(date +%Y-%m-%d)-$(hostname -s)}"

WARMUP="${BENCH_WARMUP:-10}"
RUNS="${BENCH_RUNS:-3}"
DURATION="${BENCH_DURATION:-15}"
CONNECTIONS="${BENCH_CONNECTIONS:-64}"
SUT_CPUS="${BENCH_SUT_CPUS:-0}"
LOAD_CPUS="${BENCH_LOAD_CPUS:-1-3}"

manifest="$PROVIDER_DIR/provider.toml"
val() { sed -n "s/^$1 *= *\"\{0,1\}\([^\"]*\)\"\{0,1\}\$/\1/p" "$manifest" | head -1; }
name=$(val name); variant=$(val variant); runtime=$(val runtime); port=$(val port)
build_cmd=$(val build); start_cmd=$(val start)
id="$name-$variant"
base="http://127.0.0.1:$port"

pin() { # pin <cpuset> <cmd...> — "all" means no pinning
  local cpus="$1"; shift
  if [ "$cpus" = "all" ]; then "$@"; else taskset -c "$cpus" "$@"; fi
}

tmp="$(mktemp -d)"
pid=""
cleanup() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  rm -rf "$tmp"
}
trap cleanup EXIT

echo "=== $id — build"
(cd "$PROVIDER_DIR" && eval "$build_cmd") >"$tmp/build.log" 2>&1 \
  || { echo "BUILD FAILED — $tmp/build.log:"; tail -20 "$tmp/build.log"; exit 1; }

# ── Cold start: spawn → first 200 on /ping ───────────────────────────────────
echo "=== $id — cold start + boot"
t0=$(date +%s%N)
if [ "$SUT_CPUS" = "all" ]; then
  (cd "$PROVIDER_DIR" && exec env NODE_ENV=production sh -c "exec $start_cmd") >"$tmp/sut.log" 2>&1 &
else
  (cd "$PROVIDER_DIR" && exec taskset -c "$SUT_CPUS" env NODE_ENV=production sh -c "exec $start_cmd") >"$tmp/sut.log" 2>&1 &
fi
pid=$!
cold_ms=""
for _ in $(seq 1 3000); do
  if [ "$(curl -s -o /dev/null -w '%{http_code}' "$base/ping" 2>/dev/null)" = "200" ]; then
    cold_ms=$(( ($(date +%s%N) - t0) / 1000000 )); break
  fi
  kill -0 "$pid" 2>/dev/null || { echo "SUT DIED — $tmp/sut.log:"; tail -20 "$tmp/sut.log"; exit 1; }
  sleep 0.01
done
[ -n "$cold_ms" ] || { echo "SUT never answered on $base/ping"; exit 1; }
rss_idle=$(awk '/VmRSS/{print $2}' "/proc/$pid/status")

# ── Conformance gate — a non-conforming SUT is never measured ────────────────
echo "=== $id — conformance gate"
bash "$BENCH_DIR/contract/conformance.sh" "$base"

if [ "${GATE_ONLY:-0}" = "1" ]; then echo "=== $id — gate only, done"; exit 0; fi

# ── Measure ──────────────────────────────────────────────────────────────────
measure_tier() { # $1=tier $2=path
  local tier="$1" path="$2" i
  echo "=== $id — $tier: warmup ${WARMUP}s + ${RUNS}×${DURATION}s @ ${CONNECTIONS} conns"
  pin "$LOAD_CPUS" oha -z "${WARMUP}s" -c "$CONNECTIONS" --no-tui --output-format json "$base$path" >/dev/null
  for i in $(seq 1 "$RUNS"); do
    pin "$LOAD_CPUS" oha -z "${DURATION}s" -c "$CONNECTIONS" --no-tui --output-format json "$base$path" \
      >"$tmp/$tier-run$i.json"
    sleep 1
  done
}
measure_tier t0 /ping
measure_tier t1 /hello
rss_loaded=$(awk '/VmRSS/{print $2}' "/proc/$pid/status")

kill "$pid"; wait "$pid" 2>/dev/null || true; pid=""

# ── Assemble provider result ─────────────────────────────────────────────────
mkdir -p "$RESULTS_DIR"
tier_json() { # runs[] + medians, straight from oha's own JSON
  jq -s '{
    runs: .,
    median: {
      rps: ([.[].summary.requestsPerSec] | sort | .[length/2|floor]),
      p50_ms: ([.[].latencyPercentiles.p50 * 1000] | sort | .[length/2|floor]),
      p90_ms: ([.[].latencyPercentiles.p90 * 1000] | sort | .[length/2|floor]),
      p99_ms: ([.[].latencyPercentiles.p99 * 1000] | sort | .[length/2|floor]),
      "p99.9_ms": ([.[].latencyPercentiles["p99.9"] * 1000] | sort | .[length/2|floor]),
      success_rate: ([.[].summary.successRate] | min)
    }
  }' "$tmp/$1-run"*.json
}
jq -n \
  --arg name "$name" --arg variant "$variant" --arg runtime "$runtime" \
  --argjson fingerprint "$(bash "$BENCH_DIR/harness/fingerprint.sh")" \
  --argjson params "{\"warmup_s\":$WARMUP,\"runs\":$RUNS,\"duration_s\":$DURATION,\"connections\":$CONNECTIONS,\"sut_cpus\":\"$SUT_CPUS\",\"load_cpus\":\"$LOAD_CPUS\"}" \
  --argjson cold "$cold_ms" --argjson rss_idle "$rss_idle" --argjson rss_loaded "$rss_loaded" \
  --argjson t0 "$(tier_json t0)" --argjson t1 "$(tier_json t1)" \
  '{provider: $name, variant: $variant, runtime: $runtime, fingerprint: $fingerprint,
    params: $params, cold_start_ms: $cold, rss_idle_kb: $rss_idle, rss_loaded_kb: $rss_loaded,
    tiers: {t0: $t0, t1: $t1}}' \
  >"$RESULTS_DIR/$id.json"

echo "=== $id — done → $RESULTS_DIR/$id.json"
