#!/usr/bin/env bash
# Conformance gate — checks a running SUT against the contract, byte for byte.
#
#   usage: conformance.sh <base-url>        e.g. conformance.sh http://127.0.0.1:3100
#
# BENCH_TIERS=t0,t1 checks those tiers alone — a commit that predates a tier.
#
# Exit 0 = the SUT may be benchmarked. Any mismatch prints the diff and exits 1.
set -euo pipefail

BASE="${1:?usage: conformance.sh <base-url>}"
GOLDEN="$(cd "$(dirname "${BASH_SOURCE[0]}")/golden" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

ECHO_BODY='{"message":"Hello World"}'
BEARER='Authorization: Bearer bench'

fail=0

# wanted <tier> — whether BENCH_TIERS (all when unset) names the tier.
wanted() {
  [ -z "${BENCH_TIERS:-}" ] || [[ ",$BENCH_TIERS," == *",$1,"* ]]
}

# check <tier> <path> <content-type> <golden> [curl args...]
check() {
  local tier="$1" path="$2" want_type="$3" golden="$4"
  shift 4
  wanted "$tier" || return 0
  local body="$TMP/$tier.body" hdr="$TMP/$tier.hdr"
  local status ctype

  status=$(curl -sS -o "$body" -D "$hdr" -w '%{http_code}' "$@" "$BASE$path" || echo "000")
  ctype=$(awk -F': ' 'tolower($1)=="content-type" {print $2; exit}' "$hdr" | tr -d '\r' | cut -d';' -f1)

  if [ "$status" != "200" ]; then
    echo "FAIL $tier $path — status $status (want 200)"; fail=1; return
  fi
  if [ "$ctype" != "$want_type" ]; then
    echo "FAIL $tier $path — content-type '$ctype' (want '$want_type')"; fail=1; return
  fi
  if ! cmp -s "$GOLDEN/$golden" "$body"; then
    echo "FAIL $tier $path — body differs from golden/$golden:"
    diff <(od -c "$GOLDEN/$golden" | head -20) <(od -c "$body" | head -20) | head -10 || true
    fail=1; return
  fi
  echo "PASS $tier $path — 200, $ctype, body matches golden/$golden"
}

# refused <tier> <path> — the route answers a client error without its credential.
refused() {
  local tier="$1" path="$2" status
  wanted "$tier" || return 0
  status=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$path" || echo "000")
  case "$status" in
    401|403) echo "PASS $tier $path — $status without its credential" ;;
    *) echo "FAIL $tier $path — status $status without its credential (want 401 or 403)"; fail=1 ;;
  esac
}

check t0 /ping text/plain t0-ping.txt
check t1 /hello application/json t1-hello.json
check t2 /layered application/json t1-hello.json -H "$BEARER"
refused t2 /layered
check t3 /stream text/plain t3-stream.txt
check t4 /echo application/json t1-hello.json -X POST -H 'Content-Type: application/json' --data-raw "$ECHO_BODY"
check t5 /large application/json t5-large.json

exit "$fail"
