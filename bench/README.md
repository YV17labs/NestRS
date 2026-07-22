# bench — NestJS vs NestRS, under one contract

A framework-agnostic harness that measures **idiomatic NestJS** against
**idiomatic NestRS** on identical HTTP scenarios. The harness knows no
framework — it knows a **contract** and a set of **providers**.

```
bench/
├── contract/        # the spec: CONTRACT.md + golden bytes + conformance.sh
├── sut/<provider>/  # one self-described SUT per framework×variant
├── harness/         # run.sh (measure) · fingerprint.sh · report.sh
├── results/         # committed, fingerprinted runs + generated REPORT.md
└── Justfile         # front door: just build | conformance | bench | report
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
