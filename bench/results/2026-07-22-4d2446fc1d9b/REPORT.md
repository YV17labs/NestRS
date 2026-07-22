# Bench report — 2026-07-22-4d2446fc1d9b

- **Date**: 2026-07-22T21:37:16Z
- **Host**: 4d2446fc1d9b — Apple aarch64, 4 cores, kernel 6.12.76-linuxkit
- **Toolchain**: rustc 1.96.1 (31fca3adb 2026-06-26) · node v24.18.0 · oha 1.15.0
- **Versions**: nestrs 1.0.0 · @nestjs/core 11.1.28 (express) / 11.1.28 (fastify)
- **Protocol**: warmup 10s · 3 runs × 15s · 64 connections · SUT on core(s) 0, load on 1-3 — medians across runs

| Provider | Tier | RPS | p50 | p90 | p99 | p99.9 | RSS idle | RSS loaded | Cold start |
|---|---|---|---|---|---|---|---|---|---|
| nestjs-express | t0 | 59355 | 1.03 ms | 1.21 ms | 2.06 ms | 3.19 ms | 84 MB | 204 MB | 385 ms |
| nestjs-express | t1 | 56462 | 1.08 ms | 1.21 ms | 2.16 ms | 2.99 ms | 84 MB | 204 MB | 385 ms |
| nestjs-fastify | t0 | 102518 | 0.6 ms | 0.69 ms | 1.19 ms | 1.45 ms | 86 MB | 195 MB | 472 ms |
| nestjs-fastify | t1 | 100871 | 0.61 ms | 0.7 ms | 1.21 ms | 1.46 ms | 86 MB | 195 MB | 472 ms |
| nestrs-poem | t0 | 156641 | 0.41 ms | 0.46 ms | 0.53 ms | 0.84 ms | 7 MB | 9 MB | 18 ms |
| nestrs-poem | t1 | 154003 | 0.41 ms | 0.47 ms | 0.55 ms | 0.86 ms | 7 MB | 9 MB | 18 ms |

Raw per-run oha output lives beside this file in `*.json`. A number
without its fingerprint is not a result — see `../../README.md` for the
protocol and its limits.
