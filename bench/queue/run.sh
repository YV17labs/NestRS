#!/usr/bin/env bash
# Every queue measurement, three runs each, as one Markdown report on stdout.
# Point <PREFIX>_REDIS__URL at a Redis of its own: the bench refuses one holding other keys.
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release --quiet
bin="${CARGO_TARGET_DIR:-target}/release/queue-bench"
# The bench's own app logs on stdout too; the replicas' filter is `--log`.
export RUST_LOG="${RUST_LOG:-warn}"

echo "# Queue bench — $(git rev-parse --short HEAD), $(date -u +%Y-%m-%dT%H:%MZ)"
echo
echo "- machine: $(uname -srm), $(nproc) CPUs, $(lscpu | sed -n 's/^\(Vendor ID\|Model name\): *//p' | paste -sd ' ')"
echo "- toolchain: $(rustc -V)"

measure() {
    echo
    echo "\`queue-bench $*\`"
    "$bin" "$@"
}

measure drain --queue c1
measure drain --queue c16
measure drain --queue c16 --replicas 4
measure latency --queue c16 --rate 200
measure latency --queue c16 --rate 100
measure push --pushers 1
measure push --pushers 16
measure idle
