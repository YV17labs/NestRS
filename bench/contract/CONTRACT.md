# The scenario contract

Every provider under `../sut/` implements **exactly** this HTTP surface.
A provider that fails `conformance.sh` is not benchmarked — no number is
ever published for a non-conforming SUT.

The point of the contract is symmetry: each SUT is written the way its
own framework's documentation teaches (idiomatic NestJS vs idiomatic
NestRS), but what goes over the wire — and how deep the request travels
inside the framework — is identical.

## Tiers

| Tier | Route | Response | Measures |
|---|---|---|---|
| T0 | `GET /ping` | `200`, `text/plain`, body `pong` | Transport floor (control) |
| T1 | `GET /hello` | `200`, `application/json`, body `{"message":"Hello World"}` | Framework overhead: routing + DI + serialization |

Bodies are compared **byte for byte** against `golden/`. Golden files
carry no trailing newline. The media type must match (`text/plain` /
`application/json`); charset parameters are not compared.

## Required stack depth

- **T0** — a controller/handler returning a static string. No service
  call. This is the floor every other tier is read against.
- **T1** — the request MUST traverse: router → controller instantiated
  by the framework's DI container → **injected** service method
  returning the greeting → DTO/object serialized by the framework's
  standard JSON path. Inlining the string in the handler, bypassing DI,
  or hand-writing the serialization disqualifies the SUT.

Stack depth cannot be verified by `conformance.sh` — it is enforced by
review of the SUT source, which is deliberately small enough to read.

## Runtime posture

- Production build: `cargo build --release` / `tsc` + `node dist/` with
  `NODE_ENV=production`.
- Framework defaults **as scaffolded by each framework's own CLI** —
  no tuning, no middleware removed or added beyond what the scaffold
  ships. Two deliberate exceptions:
  1. NestJS string responses default to `text/html`, so the T0 handler
     pins `text/plain` via the documented `@Header` decorator.
  2. **Observability parity**: the NestRS scaffold ships
     `OpenTelemetryModule` (per-request span, access log, `X-Trace-Id`);
     NestJS ships nothing comparable. The NestRS SUT drops that module
     import so neither side does per-request telemetry work — measuring
     "telemetry on" vs "telemetry absent" would compare features, not
     frameworks.
- No per-request logging or telemetry on either side. Boot-time logging
  stays on — it is part of each framework's honest default.
- No reverse proxy, no TLS, no compression (bodies are below any
  threshold anyway).

## Adding a tier

A new tier is a new golden file + a table row here + one route per SUT.
It exists only once every current provider passes it — a tier no
provider implements yet is dead spec.
