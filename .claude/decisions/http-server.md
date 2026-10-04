# The HTTP server under nestrs: poem, until a dated condition

poem carries every edge: `nest_rs_http::poem` and `nest_rs_ws::poem` are
re-exported, about 36 public signatures take poem's types (`Request`,
`Response`, `Endpoint`, `StatusCode`), `nest-rs-testing` re-exports its
`TestClient`, and `#[routes]` / `#[crud]` wrap poem's `#[handler]`, which
targets the call site's `poem` — the defect `macros.md` states, and why every
generated project names `poem` itself. 26 crates use it; the seven most coupled
(http, http-macros, ws, testing, openapi, graphql, mcp) hold about 31 000 lines.

**Measured on 2026-10-04** (crates.io and GitHub APIs):

- The last release is 3.1.12, 2025-07-28 — past the 12-month bar. Master is
  active in bursts (58 commits in a year, 16 of the last 24 in three days), one
  collaborator merges and cannot publish, and the one publisher's public activity
  is elsewhere. 4.0 is prepared (poem-web/poem#1200, "prepared, not yet
  released… when publication is approved"); it drops `rustls-pemfile`
  (RUSTSEC-2025-0134, accepted in `deny.toml` for poem's TLS listener).
- async-graphql, by the same publisher, has 8.0 stalled at a release candidate
  since April, and RUSTSEC-2026-0253 is fixed only there.
- The ecosystem standardised on tower: axum is owned by the tokio-rs teams (127 M
  downloads in 90 days against poem's 765 k), rmcp's server is a
  `tower::Service` that nestrs mounts through poem's `tower-compat`, utoipa and
  aide target axum, and `async-graphql-axum` is downloaded twenty times
  `async-graphql-poem`.

**Options weighed.** Stay on 3.1 with a dated condition; take 4.0 at its
release (breaking for nestrs's users, so a major); move to axum and tower, where
a handler is a plain function and the `#[handler]` defect disappears with the
macro; or first make the public API speak the `http` crate's types and nestrs's
own, with the server an internal adapter — NestJS's `HttpAdapter`
(platform-express, platform-fastify) — so that changing server breaks nothing.

**Decided (2026-10-04): stay on 3.1, flagged at the pin.** 4.0 is taken at its
release, in a nestrs major. With no poem release by 2027-01-31 the server moves.

**Proposed, awaiting the owner: the 8.0 direction** — decouple first (the
public API on `http` types and nestrs's own), then axum as the adapter. Not in
7.0: 7.0 ships the queue on Redis Streams before Rust 1.100 (2026-11-12) breaks
apalis-redis 0.7 for every queue user, and a server change would delay it.
