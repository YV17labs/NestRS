#!/usr/bin/env bash
# Render REPORT.md from every <provider>.json in a results directory.
#   usage: report.sh <results-dir>
set -euo pipefail

DIR="${1:?usage: report.sh <results-dir>}"
out="$DIR/REPORT.md"

files=("$DIR"/*.json)
[ -e "${files[0]}" ] || { echo "no provider results in $DIR"; exit 1; }

{
  echo "# Bench report — $(basename "$DIR")"
  echo
  jq -r '.fingerprint |
    "- **Date**: \(.date)\n- **Host**: \(.host) — \(.cpu), \(.cores) cores, kernel \(.kernel)\n- **Toolchain**: \(.toolchain.rustc) · node \(.toolchain.node) · \(.toolchain.oha)\n- **Versions**: nestrs \(.versions.nestrs) · @nestjs/core \(.versions.nestjs) (node adapter)"' \
    "${files[0]}"
  jq -r '.params |
    "- **Protocol**: warmup \(.warmup_s)s · \(.runs) runs × \(.duration_s)s · \(.connections) connections · SUT on core(s) \(.sut_cpus), load on \(.load_cpus) — medians across runs"' \
    "${files[0]}"
  echo
  echo "| Provider | Tier | RPS | p50 | p90 | p99 | p99.9 | RSS idle | RSS loaded | Cold start |"
  echo "|---|---|---|---|---|---|---|---|---|---|"
  for f in "${files[@]}"; do
    jq -r '
      def ms: (. * 100 | round) / 100;
      . as $p | (.tiers | to_entries[]) |
      "| \([$p.provider, $p.variant, $p.label // empty] | join("-")) | \(.key) | \(.value.median.rps | round) | \(.value.median.p50_ms | ms) ms | \(.value.median.p90_ms | ms) ms | \(.value.median.p99_ms | ms) ms | \(.value.median["p99.9_ms"] | ms) ms | \($p.rss_idle_kb / 1024 | round) MB | \((.value.rss_loaded_kb // $p.rss_loaded_kb) / 1024 | round) MB | \($p.cold_start_ms) ms |"' \
      "$f"
  done
  echo
  jq -r '. as $p | .tiers | to_entries[] | select(.value.spans_accepted != null) |
    .key as $tier | .value as $t | def share(n): n * 1000 / $t.requests_sent | round / 10;
    [ (if $t.spans_accepted > 0 then "\($t.spans_accepted) spans (\(share($t.spans_accepted))%)" else empty end),
      (($t.log_records_accepted // 0) as $n | if $n > 0 then "\($n) log records (\(share($n))%)" else empty end),
      (($t.metric_points_accepted // 0) as $n | if $n > 0 then "\($n) metric points" else empty end)
    ] | join(", ") as $took |
    (if ($t | has("spans_dropped") | not) then ""
     elif $t.spans_dropped == null then "; what its export queues dropped is unread: the SUT did not exit cleanly"
     else
      [ (if $t.spans_dropped > 0 then "\($t.spans_dropped) spans" else empty end),
        (if $t.log_records_dropped > 0 then "\($t.log_records_dropped) log records" else empty end)
      ] | "; its export queues dropped " + (if length == 0 then "none" else join(", ") end)
    end) as $dropped |
    "- **\([$p.provider, $p.variant, $p.label // empty] | join("-")) \($tier)**: of the \($t.requests_sent) requests sent, the collector took \($took)\($dropped)"' \
    "${files[@]}"
  echo
  echo "RSS loaded is the SUT's once each tier's last run ended. A tier named"
  echo "\`t1-h2c\`, \`t1-quiet\`, \`t5-compressed\` or \`t1-exported\` is a variant its"
  echo "provider declares (\`contract/CONTRACT.md\`), never compared with a provider"
  echo "that does not."
  echo
  echo "Raw per-run oha output lives beside this file in \`*.json\`. A number"
  echo "without its fingerprint is not a result — see \`../../README.md\` for the"
  echo "protocol and its limits."
} >"$out"

echo "wrote $out"
cat "$out"
