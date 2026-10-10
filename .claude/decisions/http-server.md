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

## 2026-10-09 — the decouple moves into 7.0, and the accept loop is nestrs's

The owner moved the 8.0 direction into 7.0 on 2026-10-09, which supersedes "Not
in 7.0" above. The public API speaks the `http` crate's types and nestrs's own —
`Request`, `Response`, `Body`, `HttpError`, an `Endpoint` trait — and names no
poem type: the `poem` re-exports go, no generated manifest declares it, and a
`disallowed-types` lint lifted in the one adapter file holds it. The server is
an internal adapter, so a later move to axum and tower is a minor.

poem stays the request engine — its router, CORS, compression and the query,
form and multipart decoders — behind that crate-private adapter file, until the
condition above. With poem internal, 4.0 is taken at its release in a nestrs
minor, not a major, and the 2027-01-31 condition moves the adapter file, not the
API; the move to axum stays dated by it.

The accept loop becomes nestrs's, on hyper-util's `auto` connection builder and
tokio-rustls, because poem's server cannot bound what a server must: it offers
no hook for hyper's timer (the connection phases), a connection cap, accept
errors, which it drops, or a TLS handshake deadline, and it re-raises a
handler's panic in the connection task. It leaves the HTTP/2 stream cap unset
too; a setter exists, and the loop sets the cap since it changes anyway. A
fork is refused (`manifests-ci.md`: no fork, no vendoring, no `[patch]`). The
loop installs the process crypto provider when none is, as poem's TLS listener
does, so no later client finds another default.

tower stays out of the 7.0 public API: tower-layer's last release (0.3.3,
2024-08-01) is past the freshness bar, and nothing first-party needs tower once
rmcp is mounted through its own `handle`. A public server-adapter trait waits
for a second adapter.

The first paragraph's `#[handler]` sentence was already stale: `#[routes]`
writes its endpoint by hand, mirroring that expansion.

Lands with `feat/accept-loop` (the loop) and `feat/http-vocabulary` (the
types), then the branches that move the transport, the routes and each edge
onto them; `refactor/retire-replaced-contracts-and-poem` takes poem off the
public surface.
