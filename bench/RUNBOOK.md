# Runbook — producing a publishable run

Follow this on a **dedicated, idle Linux host** (bare metal preferred; 8+
cores ideal). The harness requires Linux: CPU pinning uses `taskset`, memory
sampling reads `/proc`. Numbers from shared or virtualized hosts are for
harness development, not publication.

## 1. Prerequisites

```bash
# Debian/Ubuntu — adapt to your distro
sudo apt-get update && sudo apt-get install -y git curl build-essential jq util-linux
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y   # Rust stable
curl -fsSL https://deb.nodesource.com/setup_26.x | sudo bash - && sudo apt-get install -y nodejs
cargo install oha just
# The OpenTelemetry Collector an `exported` variant sends to: the core
# distribution, on PATH as `otelcol`
v=0.165.0; arch=$(dpkg --print-architecture)   # amd64 or arm64
curl -fsSL "https://github.com/open-telemetry/opentelemetry-collector-releases/releases/download/v$v/otelcol_${v}_linux_${arch}.tar.gz" \
  | sudo tar -xz -C /usr/local/bin otelcol
```

Optional but recommended for stable numbers: pin the CPU governor —
`sudo cpupower frequency-set -g performance` — and make sure nothing else
runs on the box (no cron-heavy agents, no other services).

## 2. Get the harness

```bash
git clone https://github.com/YV17labs/NestRS && cd NestRS/bench
just build          # release build of every contestant
just conformance    # byte-for-byte gate — all six checks must PASS
```

Do not measure anything until conformance is green for every contestant.

## 3. The published protocol

Two regimes, run both. Adjust the CPU sets to the machine (example: 8 cores).

```bash
# Regime A — per-core (the headline): SUT on one core, load on the rest.
BENCH_WARMUP=30 BENCH_RUNS=5 BENCH_DURATION=30 \
BENCH_SUT_CPUS=0 BENCH_LOAD_CPUS=1-7 just bench

# Regime B — scaling: SUT on half the cores, load on the other half.
BENCH_WARMUP=30 BENCH_RUNS=5 BENCH_DURATION=30 \
BENCH_SUT_CPUS=0-3 BENCH_LOAD_CPUS=4-7 just bench
```

Each regime writes `results/<date>-<host>/` (per-provider JSON with the
machine fingerprint embedded + a generated `REPORT.md`). Regime B overwrites
Regime A's directory if run the same day on the same host — move Regime A's
directory aside first (e.g. suffix `-percore`).

## 4. Sanity checklist before quoting anything

- `successRate` is 1.0 for every run of every provider.
- Run-to-run spread within a tier ≤ ~5% — a larger spread means the box is
  not idle; find the noise, rerun.
- T0 ≈ T1 per provider (they measure the same fixed cost; a large gap means
  something is wrong).
- The fingerprint in the JSON matches the machine you think you ran on.

## 5. Two commits, one session

A change to the framework is judged by the same provider built from two
commits, in one session on one host: the figures of two sessions are not
comparable. Build the base in a worktree of its own and label each run, into
one results directory; the label is in each result and names its row in the
report:

```bash
dir="$PWD/results/$(date +%Y-%m-%d)-$(hostname -s)-ab"
git worktree add ../../base <base-commit>
BENCH_LABEL=base ./harness/run.sh ../../base/bench/sut/nestrs "$dir"
BENCH_LABEL=head ./harness/run.sh sut/nestrs "$dir"
./harness/report.sh "$dir"
```

This tree's harness and contract measure both. A base whose SUT predates a
tier is measured on the tiers it serves, and so is the head beside it:
`BENCH_TIERS=t0,t1` on both runs.

Alternate the two (base, head, base, head) when the spread between runs of
one commit is near the difference being judged, and state the spread beside
every figure the report quotes.

## 6. What exporting costs, signal by signal

The `exported` variant exports the three signals. What each costs alone is
the same variant with the other two set to `none`, each launch measuring T1
then its variant, in one session on one host, into one directory:

```bash
dir="$PWD/results/$(date +%Y-%m-%d)-$(hostname -s)-cost"
cost() { # cost <label> [K=V…] — T1, then t1-exported with these over the endpoint
  BENCH_WARMUP=30 BENCH_RUNS=5 BENCH_DURATION=30 BENCH_SUT_CPUS=0 BENCH_LOAD_CPUS=1-7 \
  BENCH_TIERS=t1 BENCH_LABEL="$1" \
  BENCH_EXPORTED_ENV="OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318${2:+ $2}" \
    ./harness/run.sh sut/nestrs "$dir"
}
cost all
cost traces "OTEL_LOGS_EXPORTER=none OTEL_METRICS_EXPORTER=none"
cost logs "OTEL_TRACES_EXPORTER=none OTEL_METRICS_EXPORTER=none"
cost metrics "OTEL_TRACES_EXPORTER=none OTEL_LOGS_EXPORTER=none"
```

`BENCH_TIERS=t1` measures T1 and no variant but the one `BENCH_EXPORTED_ENV`
asks for; the gate waits for the signal the variant exports, and each
`nestrs-hyper-<label>.json` records the spans, log records and metric points
the collector took, and the spans and log records the SDK's queues dropped. A variant's cost is `1/rps(t1-exported) − 1/rps(t1)`,
against its own launch's T1, and its memory under load is read against that
T1's too, which moves from one sitting to the next.

Keep a launch only when its T1 and its variant each hold their runs within 5 %
of one another, `(max − min) / min`; run the four again until each has two
kept launches, and say how many were set aside and why.

## 7. Hand back

Return the complete `results/` directories from both regimes — JSON files
and `REPORT.md`, nothing else. Do not edit anything outside `results/`.

Cross-check for published figures: replay at least T1 with a second load
generator (e.g. [k6](https://k6.io)) and confirm the ratios hold — the tool
must never be the story.
