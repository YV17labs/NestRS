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
curl -fsSL https://deb.nodesource.com/setup_24.x | sudo bash - && sudo apt-get install -y nodejs
cargo install oha just
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

## 5. Hand back

Return the complete `results/` directories from both regimes — JSON files
and `REPORT.md`, nothing else. Do not edit anything outside `results/`.

Cross-check for published figures: replay at least T1 with a second load
generator (e.g. [k6](https://k6.io)) and confirm the ratios hold — the tool
must never be the story.
