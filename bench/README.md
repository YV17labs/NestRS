# bench — NestJS vs NestRS, under one contract

A framework-agnostic harness that measures **idiomatic NestJS** against
**idiomatic NestRS** on identical HTTP scenarios. The harness knows no
framework — it knows a **contract** and a set of **providers**.

```
bench/
├── contract/        # the spec: CONTRACT.md + golden bytes + conformance.sh
├── sut/<provider>/  # one self-described SUT per framework×variant
├── harness/         # run.sh (measure) · fingerprint.sh · report.sh
├── results/         # local, fingerprinted runs + generated REPORT.md (git-ignored)
├── queue/           # the job queue over Redis, on its own (see below)
└── Justfile         # front door: just build | conformance | bench | report | queue
```

## Principles

1. **Same bytes, same depth.** Every SUT serves the exact routes in
   [contract/CONTRACT.md](contract/CONTRACT.md) — bodies compared byte
   for byte, and each tier prescribes how deep the request must travel
   (router → DI container → injected service → framework JSON). A SUT
   that shortcuts the stack is disqualified by review; a SUT that fails
   `conformance.sh` is never measured.
2. **Each SUT is idiomatic for *its* framework** — written the way its
   own docs and CLI scaffold teach, defaults untouched. The comparison
   is "framework as taught vs framework as taught", not "tuned vs naive".
3. **Compare against the opponent's best case.** NestJS runs both
   `express` and `fastify` variants; future providers follow the same
   pattern (e.g. `laravel-fpm` / `laravel-octane`).
4. **A number without its environment is not a result.** Every result
   embeds the machine fingerprint (CPU, kernel, toolchains, resolved
   dependency versions) and the exact protocol parameters.

## Running

```bash
cd bench
just build          # release/production build of every SUT
just conformance    # boot each SUT, gate it on the contract, stop it
just bench          # full run → results/<date>-<host>/ + REPORT.md
just bench-one nestrs
```

Protocol per tier: **warmup (thrown away) → N timed runs → medians**.
SUT and load generator are pinned to disjoint CPU sets (`taskset`), so
per-core efficiency is what's measured. Defaults are the quick local
protocol (10 s warmup, 3×15 s, 64 connections); published runs follow
[RUNBOOK.md](RUNBOOK.md) — long protocol, both regimes, dedicated host.

Reported per provider: RPS, p50/p90/p99/p99.9 latency, RSS idle/loaded,
cold start (spawn → first 200). Raw oha JSON is kept beside the report.

## Adding a provider

1. `mkdir sut/<framework>-<variant>` with a `provider.toml`:
   ```toml
   name = "laravel"
   variant = "octane"
   runtime = "php"
   port = 3130            # next free 31xx
   build = "composer install --no-dev && ..."
   start = "php artisan octane:start --port=3130"
   ```
2. Implement the contract idiomatically (routes, DI, service — as the
   framework's own docs teach) and pass `just conformance`.
3. Add a `Dockerfile` for containerized runs.

That is the whole interface — the harness globs `sut/*/provider.toml`.

## Modes and limits — read before quoting numbers

- **Process mode** (the scripts above) is what runs in the devcontainer.
  Numbers from a shared/virtualized host (Docker Desktop VM, CI runner)
  are for harness development and relative sanity checks — **not for
  publication**. Published runs happen on a dedicated Linux host.
- **Docker mode**: each SUT ships a `Dockerfile` (the deployable
  artifact — also what makes image size and cold start comparable).
  The Dockerfiles are authored but not yet exercised in the
  devcontainer (no Docker daemon inside); the docker-mode runner comes
  with the first published run.
- Load generator is **oha** (Rust). For published numbers, cross-check
  with **k6** (Go, neutral) so the tool choice isn't an attack surface.
- One known asymmetry accepted for now: under `taskset -c 0`, tokio
  still spawns its default worker pool confined to one core, and Node
  runs its usual single event loop — both are "the scaffold under a
  1-core budget", which is the honest reading.

## The queue bench — `queue/`

The job queue over Redis, against the adapter it replaces rather than
another framework, so it has no contract and no provider. It names
nothing below `nest_rs::queue` and `nest_rs::redis` — the same source
measures any adapter that keeps that surface — and, like `sut/nestrs`,
is its own Cargo project on the framework by path.

```bash
redis-server --port 16403 --save '' --appendonly no --daemonize yes
export NESTRS_REDIS__URL=redis://127.0.0.1:16403/   # <PREFIX>_REDIS__URL
just queue            # every measurement, 3 runs each, Markdown on stdout
```

**Every run flushes that database**, and the bench refuses to start on
the framework's default URL: give it a Redis of its own.

| Measurement | What is timed |
|---|---|
| `drain` | 5 000 jobs pushed, then 1 or `--replicas` worker processes started: spawn → last job's first completion; worker and Redis CPU, Redis commands per job |
| `latency` | an idle worker, then 2 000 jobs at `--rate`/s: each push → its handler's start (p50/p90/p99/max) |
| `push` | 5 000 single pushes from 1 or `--pushers` tasks, no worker |
| `idle` | a worker with nothing to do for 30 s: Redis commands/s, worker CPU |

The worker serves two queues, `bench-c1` and `bench-c16`, one
`#[process]` each at concurrency 1 and 16, on the adapter's default
settings. A replica is a child process, as a pod is; each reports every
handler run, and a job run twice is counted as a duplicate, never as a
second job. Times are wall-clock microseconds compared across processes,
so the bench runs on one host. `queue-bench --help` lists the flags;
`queue/run.sh` is the full set.

