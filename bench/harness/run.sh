#!/usr/bin/env bash
# Process-mode bench runner — builds ONE SUT, gates it on the contract, then
# measures every tier with oha and writes <results-dir>/<provider>.json.
#
#   usage: run.sh <sut-dir> [results-dir]
#
# Protocol per tier: warmup (thrown away) → BENCH_RUNS measured runs → medians,
# and the SUT's RSS once the tier's last run ends.
# The SUT is pinned to BENCH_SUT_CPUS, the load generator to BENCH_LOAD_CPUS —
# they never share a core. Two regimes are published: per-core (SUT pinned to
# one core — apples-to-apples efficiency) and all-cores (BENCH_SUT_CPUS=all —
# scaling; needs a host with enough cores to also isolate the load generator).
# Tunables (defaults are the quick local protocol; published runs use
# BENCH_WARMUP=30 BENCH_RUNS=5 BENCH_DURATION=30):
#   BENCH_WARMUP=10 BENCH_RUNS=3 BENCH_DURATION=15 BENCH_CONNECTIONS=64
#   BENCH_SUT_CPUS=0 BENCH_LOAD_CPUS=1-3     ("all" = no pinning)
#   BENCH_LABEL=base                         (suffixes the result's name, to
#                                             keep two commits of one provider)
#   BENCH_TIERS=t0,t1                        (those tiers alone: a commit that
#                                             predates a tier)
#
# Beside the contract's tiers, a provider measures the variants its
# provider.toml declares (CONTRACT.md, "Variants"), each in a boot of its own:
#   h2c = "true"               t1 over HTTP/2 with prior knowledge
#   quiet_env = "K=V"          t1 with the SUT's per-request line off
#   compressed_env = "K=V"     t5 with `Accept-Encoding: gzip, br`, compression on
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

ECHO_BODY='{"message":"Hello World"}'
BEARER='Authorization: Bearer bench'
COMPRESSED='Accept-Encoding: gzip, br'

manifest="$PROVIDER_DIR/provider.toml"
val() { sed -n "s/^$1 *= *\"\{0,1\}\([^\"]*\)\"\{0,1\}\$/\1/p" "$manifest" | head -1; }
name=$(val name); variant=$(val variant); runtime=$(val runtime); port=$(val port)
build_cmd=$(val build); start_cmd=$(val start)
h2c=$(val h2c); quiet_env=$(val quiet_env); compressed_env=$(val compressed_env)
id="$name-$variant${BENCH_LABEL:+-$BENCH_LABEL}"
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

rss() { awk '/VmRSS/{print $2}' "/proc/$pid/status"; }

# boot [env K=V...] — spawn the SUT pinned and wait for its first 200 on /ping;
# sets `pid` and `booted_ms` (spawn → first 200).
boot() {
  local t0 i
  t0=$(date +%s%N)
  if [ "$SUT_CPUS" = "all" ]; then
    (cd "$PROVIDER_DIR" && exec env NODE_ENV=production "$@" sh -c "exec $start_cmd") >"$tmp/sut.log" 2>&1 &
  else
    (cd "$PROVIDER_DIR" && exec taskset -c "$SUT_CPUS" env NODE_ENV=production "$@" sh -c "exec $start_cmd") >"$tmp/sut.log" 2>&1 &
  fi
  pid=$!
  booted_ms=""
  for i in $(seq 1 3000); do
    if [ "$(curl -s -o /dev/null -w '%{http_code}' "$base/ping" 2>/dev/null)" = "200" ]; then
      booted_ms=$(( ($(date +%s%N) - t0) / 1000000 )); return
    fi
    kill -0 "$pid" 2>/dev/null || { echo "SUT DIED — $tmp/sut.log:"; tail -20 "$tmp/sut.log"; exit 1; }
    sleep 0.01
  done
  echo "SUT never answered on $base/ping"; exit 1
}

stop() { kill "$pid"; wait "$pid" 2>/dev/null || true; pid=""; }

measured=()
# measure_tier <tier> <path> [oha args...] — the tier's runs and its loaded RSS.
measure_tier() {
  local tier="$1" path="$2" i
  shift 2
  if [ -n "${BENCH_TIERS:-}" ] && [[ ",$BENCH_TIERS," != *",${tier%%-*},"* ]]; then return 0; fi
  echo "=== $id — $tier: warmup ${WARMUP}s + ${RUNS}×${DURATION}s @ ${CONNECTIONS} conns"
  pin "$LOAD_CPUS" oha -z "${WARMUP}s" -c "$CONNECTIONS" --no-tui --output-format json "$@" "$base$path" >/dev/null
  for i in $(seq 1 "$RUNS"); do
    pin "$LOAD_CPUS" oha -z "${DURATION}s" -c "$CONNECTIONS" --no-tui --output-format json "$@" "$base$path" \
      >"$tmp/$tier-run$i.json"
    sleep 1
  done
  rss >"$tmp/$tier.rss"
  measured+=("$tier")
}

# ── Cold start: spawn → first 200 on /ping ───────────────────────────────────
echo "=== $id — cold start + boot"
boot
cold_ms="$booted_ms"
rss_idle=$(rss)

# ── Conformance gate — a non-conforming SUT is never measured ────────────────
echo "=== $id — conformance gate"
bash "$BENCH_DIR/contract/conformance.sh" "$base"
if [ "$h2c" = "true" ]; then
  [ "$(curl -s --http2-prior-knowledge -o /dev/null -w '%{http_version}' "$base/hello")" = "2" ] \
    || { echo "FAIL h2c — the SUT declares h2c and answered /hello over another version"; exit 1; }
  echo "PASS h2c /hello — HTTP/2 with prior knowledge"
fi

if [ "${GATE_ONLY:-0}" = "1" ]; then echo "=== $id — gate only, done"; exit 0; fi

# ── Measure: the contract's tiers ────────────────────────────────────────────
measure_tier t0 /ping
measure_tier t1 /hello
measure_tier t2 /layered -H "$BEARER"
measure_tier t3 /stream
measure_tier t4 /echo -m POST -T application/json -d "$ECHO_BODY"
measure_tier t5 /large
if [ "$h2c" = "true" ]; then measure_tier t1-h2c /hello --http-version 2; fi
rss_loaded=$(rss)
stop
if [ -n "${BENCH_TIERS:-}" ]; then quiet_env=""; compressed_env=""; fi

# ── Measure: the variants a boot of their own carries ────────────────────────
if [ -n "$quiet_env" ]; then
  echo "=== $id — quiet boot ($quiet_env)"
  boot $quiet_env
  measure_tier t1-quiet /hello
  stop
fi
if [ -n "$compressed_env" ]; then
  echo "=== $id — compressed boot ($compressed_env)"
  boot $compressed_env
  encoding=$(curl -sS -D - -o /dev/null -H "$COMPRESSED" "$base/large" \
    | awk -F': ' 'tolower($1)=="content-encoding" {print $2; exit}' | tr -d '\r')
  [ -n "$encoding" ] || { echo "FAIL t5-compressed — the SUT answered /large uncompressed"; exit 1; }
  echo "PASS t5-compressed /large — content-encoding $encoding"
  measure_tier t5-compressed /large -H "$COMPRESSED"
  stop
fi

# ── Assemble provider result ─────────────────────────────────────────────────
mkdir -p "$RESULTS_DIR"
tier_json() { # runs[] + medians, straight from oha's own JSON, and the tier's RSS
  jq -s --arg tier "$1" --argjson rss "$(cat "$tmp/$1.rss")" '{($tier): {
    runs: .,
    median: {
      rps: ([.[].summary.requestsPerSec] | sort | .[length/2|floor]),
      p50_ms: ([.[].latencyPercentiles.p50 * 1000] | sort | .[length/2|floor]),
      p90_ms: ([.[].latencyPercentiles.p90 * 1000] | sort | .[length/2|floor]),
      p99_ms: ([.[].latencyPercentiles.p99 * 1000] | sort | .[length/2|floor]),
      "p99.9_ms": ([.[].latencyPercentiles["p99.9"] * 1000] | sort | .[length/2|floor]),
      success_rate: ([.[].summary.successRate] | min)
    },
    rss_loaded_kb: $rss
  }}' "$tmp/$1-run"*.json
}
# Through a file, never an argument: Linux caps one argument at 128 KiB, which
# a provider's tiers of oha runs outgrow.
for tier in "${measured[@]}"; do tier_json "$tier"; done | jq -s 'add // {}' >"$tmp/tiers.json"
jq -n \
  --arg name "$name" --arg variant "$variant" --arg runtime "$runtime" \
  --argjson fingerprint "$(bash "$BENCH_DIR/harness/fingerprint.sh")" \
  --argjson params "{\"warmup_s\":$WARMUP,\"runs\":$RUNS,\"duration_s\":$DURATION,\"connections\":$CONNECTIONS,\"sut_cpus\":\"$SUT_CPUS\",\"load_cpus\":\"$LOAD_CPUS\"}" \
  --argjson cold "$cold_ms" --argjson rss_idle "$rss_idle" --argjson rss_loaded "$rss_loaded" \
  --slurpfile tiers "$tmp/tiers.json" \
  '{provider: $name, variant: $variant, runtime: $runtime, fingerprint: $fingerprint,
    params: $params, cold_start_ms: $cold, rss_idle_kb: $rss_idle, rss_loaded_kb: $rss_loaded,
    tiers: $tiers[0]}' \
  >"$RESULTS_DIR/$id.json"

echo "=== $id — done → $RESULTS_DIR/$id.json"
