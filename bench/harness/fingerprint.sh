#!/usr/bin/env bash
# Emit the environment fingerprint as JSON on stdout. A number without its
# environment is not a result — every provider result embeds this object.
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# "model name" is x86-only; on aarch64 fall back to lscpu vendor + arch.
cpu_model=$(awk -F': ' '/model name/{print $2; exit}' /proc/cpuinfo 2>/dev/null)
if [ -z "$cpu_model" ] || [ "$cpu_model" = "-" ]; then
  vendor=$(lscpu 2>/dev/null | awk -F': +' '/^Vendor ID/{print $2; exit}')
  cpu_model="${vendor:-unknown} $(uname -m)"
fi
nestrs_version=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' "$BENCH_DIR/../Cargo.toml" | head -1)

nest_version() { # resolved @nestjs/core from a SUT's lockfile, if present
  local lock="$BENCH_DIR/sut/$1/package-lock.json"
  [ -f "$lock" ] && jq -r '.packages["node_modules/@nestjs/core"].version // "unresolved"' "$lock" || echo "unresolved"
}

jq -n \
  --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg host "$(hostname -s)" \
  --arg cpu "$cpu_model" \
  --argjson cores "$(nproc)" \
  --arg kernel "$(uname -r)" \
  --arg rustc "$(rustc --version 2>/dev/null || echo absent)" \
  --arg node "$(node --version 2>/dev/null || echo absent)" \
  --arg oha "$(oha --version 2>/dev/null || echo absent)" \
  --arg nestrs "$nestrs_version" \
  --arg nestjs_express "$(nest_version nestjs-express)" \
  --arg nestjs_fastify "$(nest_version nestjs-fastify)" \
  '{date: $date, host: $host, cpu: $cpu, cores: $cores, kernel: $kernel,
    toolchain: {rustc: $rustc, node: $node, oha: $oha},
    versions: {nestrs: $nestrs, "nestjs-express": $nestjs_express, "nestjs-fastify": $nestjs_fastify}}'
