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
| T2 | `GET /layered`, `Authorization: Bearer bench` | as T1 | A guarded route with one interceptor: the per-request layer pipeline |
| T3 | `GET /stream` | `200`, `text/plain`, body `chunk\n` × 16, written as 16 chunks | A streamed body: the per-frame cost |
| T4 | `POST /echo`, body `{"message":"Hello World"}` (`application/json`) | `200`, `application/json`, the body echoed | Reading and decoding a request body |
| T5 | `GET /large` | `200`, `application/json`, 1 000 items `{"id":n,"message":"Hello World"}` | Writing a large body (34 891 bytes) |

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
- **T2** — T1's depth behind the framework's own guard, bound on the
  route, admitting `Authorization: Bearer bench` and refusing a request
  without it (`401` or `403`), and one route-bound interceptor that runs
  the rest of the request and returns what it answered.
- **T3** — the framework's own streaming response (a `StreamableFile`
  over a `Readable`, a `Body` over a stream), never a buffered body.
- **T4** — the framework's own body binding into a DTO and its JSON
  path back; no validation on either side.
- **T5** — T1's depth, the 1 000 items built by the injected service.

Stack depth cannot be verified by `conformance.sh` — it is enforced by
review of the SUT source, which is deliberately small enough to read.

## Runtime posture

- Production build: `cargo build --release` / `nest build` + `node
  dist/main` with `NODE_ENV=production`.
- Framework defaults **as scaffolded by each framework's own CLI** —
  no tuning, no middleware removed or added beyond what the scaffold
  ships. A NestJS SUT is `nest new --skip-tests --no-observe` as
  generated, plus the `hello` module, controller and service that
  `nest generate` writes; the Fastify variant swaps the adapter the way
  NestJS's performance guide shows. One deliberate exception: on
  Express, a NestJS string response defaults to `text/html`, so that
  SUT's T0 handler pins `text/plain` with the documented `@Header`
  decorator. Fastify already answers `text/plain` and carries none.
- No per-request telemetry on either side: `nest new` offers the
  `@nestjs/observe` agent, which reports to a hosted service under an
  account's keys, and the SUT declines it (`--no-observe`, the CLI's own
  answer without a terminal). Logging stays as each framework ships it:
  NestJS writes boot lines only, and nestrs writes its access line — one
  line per request at `info` — which its `quiet` variant turns off.
- No reverse proxy, no TLS, no compression: neither framework compresses
  by default.

## Variants

A variant measures one provider's own option, in a boot of its own, and is
declared in its `provider.toml`. It is never compared across providers: a
provider that does not declare it has no number for it.

| Variant | Declared as | Tier | Measures |
|---|---|---|---|
| `t1-h2c` | `h2c = "true"` | T1 over HTTP/2 with prior knowledge on the cleartext port | The HTTP/2 path, which the transport serves without an option |
| `t1-quiet` | `quiet_env = "K=V"` | T1, the SUT booted with that environment | The per-request log line's cost, by turning it off |
| `t5-compressed` | `compressed_env = "K=V"` | T5 with `Accept-Encoding: gzip, br`, the SUT booted with that environment | Content negotiation and compression of a large body |

The gate checks a variant before it is measured: `h2c` answers `/hello` over
HTTP/2, `compressed` answers `/large` with a `Content-Encoding`.

## Adding a tier

A new tier is a new golden file + a table row here + one route per SUT.
It exists only once every current provider passes it — a tier no
provider implements yet is dead spec.
