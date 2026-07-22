#!/usr/bin/env bash
# Conformance gate — checks a running SUT against the contract, byte for byte.
#
#   usage: conformance.sh <base-url>        e.g. conformance.sh http://127.0.0.1:3100
#
# Exit 0 = the SUT may be benchmarked. Any mismatch prints the diff and exits 1.
set -euo pipefail

BASE="${1:?usage: conformance.sh <base-url>}"
GOLDEN="$(cd "$(dirname "${BASH_SOURCE[0]}")/golden" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail=0

check() {
  local tier="$1" path="$2" want_type="$3" golden="$4"
  local body="$TMP/$tier.body" hdr="$TMP/$tier.hdr"
  local status ctype

  status=$(curl -sS -o "$body" -D "$hdr" -w '%{http_code}' "$BASE$path" || echo "000")
  ctype=$(awk -F': ' 'tolower($1)=="content-type" {print $2; exit}' "$hdr" | tr -d '\r' | cut -d';' -f1)

  if [ "$status" != "200" ]; then
    echo "FAIL $tier $path — status $status (want 200)"; fail=1; return
  fi
  if [ "$ctype" != "$want_type" ]; then
    echo "FAIL $tier $path — content-type '$ctype' (want '$want_type')"; fail=1; return
  fi
  if ! cmp -s "$GOLDEN/$golden" "$body"; then
    echo "FAIL $tier $path — body differs from golden/$golden:"
    diff <(od -c "$GOLDEN/$golden") <(od -c "$body") | head -10 || true
    fail=1; return
  fi
  echo "PASS $tier $path — 200, $ctype, body matches golden/$golden"
}

check t0 /ping text/plain t0-ping.txt
check t1 /hello application/json t1-hello.json

exit "$fail"
