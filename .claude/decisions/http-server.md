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

## 2026-10-11 — one write per HTTP/1 flush, a body handed over 64 KiB at a time, and `TCP_NODELAY` stays

The accept loop hands hyper a socket that takes vectored writes, so hyper's
automatic strategy sent each HTTP/1 response as one `writev(2)` of separate
slices — head and body, three more per chunk of a streamed body — where poem's
server, whose boxed socket hid vectored writes, had copied them into one
buffer. A same-session bisect of the bench put about 4.5 % of the JSON route's
throughput and 11 to 15 % of its p99 on the loop. The loop sets hyper's
`writev(false)`: what a flush sends is one buffer and one `write(2)`, as under
poem — a small response leaves in one flush, a streamed one in a flush per
chunk it waits on, and a buffer the socket takes in parts in several writes.
Over TLS rustls no longer gathers the slices into a vector on each write.
HTTP/2 keeps h2's own choice.

The copy has a price poem's server paid as well: hyper copies a body frame
whole into the connection's write buffer, which keeps its capacity until the
connection closes. A body handed over as one large frame — a static file
embedded in the binary, uncompressed by default — would hold its size on every
keep-alive connection that served it, up to the connection cap times the
largest such body, and any client can drive that. So a response body hands
hyper at most 64 KiB of a frame at a time, split without a copy, on HTTP/1 as
on HTTP/2, where the same piece keeps a stream's send deadline: the write
buffer stays within its 64 KiB cap plus one piece. Measured on hyper 1.12 with
the loop's settings, an idle keep-alive connection held about 66 KB after a
4 MiB one-frame response on the automatic strategy, 4.25 MB with
`writev(false)` alone, and 123 KB with the pieces. Each body byte is still
copied once, as under poem.

`TCP_NODELAY` stays. A response leaving in one flush never meets Nagle's
algorithm, so dropping it saves one call per connection, and it would let a
stream's next chunk — an event, a frame — wait on the acknowledgement of the
previous one (a delayed ACK, up to 40 ms on Linux).

Refused: a socket wrapper of our own that coalesces small vectored writes and
passes large ones through. It would keep a large frame uncopied, at the price
of a mechanism over every socket to maintain; hyper's switch restores the
write poem's server made, and the pieces the body already cut for HTTP/2 bound
its copy.

## 2026-10-14 — the transport above the router on nestrs's types; what the adapter owes on the wire

The accept loop hands each request to the stack as nestrs's `Request` (its peer,
local address, scheme, original URI and arrival instant inline; hyper's
`OnUpgrade` left in the extensions, where a leaf on either side of the bridge
takes it), and the edge, the bands and the response body's carrier speak
nestrs's types. poem stays behind one crate-private file, `poem_adapter.rs`:
the router, wrapped once, and the CORS and compression layers, crossed once for
both. A band still written on poem crosses the bridge around itself.

Moving those layers behind the adapter meant pinning their wire behaviour in
black-box suites a later adapter passes unchanged, and the pinning found poem
short of the standards: its CORS layer sends `Vary: Origin` only for a
wildcard pattern, so a shared cache could hand one listed origin's answer to
another; its compression reads `;q=` only without whitespace, ignores
`identity`, takes a coding weighed `q=0`, misspells `zstd`, and compresses an
event stream, whose encoder then holds every event back. The adapter adds
`Vary: Origin`, and the transport negotiates `Accept-Encoding` itself (RFC 9110
§12.5.3), leaves an event stream, a `204`, a `304` and an encoded response as
written, says `Vary: Accept-Encoding`, and keeps poem's middleware only to
decode a request body. Standards over the library decided; the conformance
suites hold it.

**On HTTP/2 a body's last byte is handed on its own.** hyper hands h2 a frame as
soon as the stream's window has room for one byte of it, so a stream's task —
and its send deadline — ended with up to one 64 KiB piece still waiting on a
window the peer withheld, sent under the idle deadline. Held back, the last
byte waits for capacity until everything before it has left, and the stream is
reset at the send deadline like any other part of the response. A body that
says it ended is split as it is handed; a stream that only tells its end when
polled again is polled once ahead on HTTP/2. The cost is one more `DATA` frame
per HTTP/2 response with a body; HTTP/1 is untouched. Refused: bounding h2's
send buffer (`max_send_buf_size`), which the correction proposed — it caps what
h2 grants as capacity, not the frame hyper hands on one byte of it, so the tail
stays; and holding back a byte of every streamed chunk, which would double the
frames of an event stream.

**2026-10-14 — the router is compiled from nestrs's table.** Every controller
and surface registers on `HttpRoutes`; the adapter compiles the table into
poem's router and does nothing else per request but match the path. The
method dispatch (`405` with `Allow`, `HEAD` served by `GET`), the parameters'
decoding, the template each entry records, the version rewrite, the global
prefix (applied at registration) and the fallback are nestrs's own, pinned by
`tests/integration/routes.rs` and its siblings, so the next adapter brings a
path matcher and nothing more.

## 2026-10-18 — the test kit drives the endpoint itself

`nest-rs-testing` no longer re-exports poem's `TestClient`: its own client
takes any `nest_rs_http::Endpoint` and sends nestrs's `Request` to it, as the
accept loop does, so a suite names no server type and the next adapter
rewrites none. The names and assertions are poem's, ported with its MIT
notice, and a failure now names the header or the JSON path it read. A request
has no peer unless the test gives it one, as poem's had none. Refused: a
client over a loopback socket, which would test the socket on every request
where `tests/integration/server.rs` already does once; and keeping poem's
client behind the bridge, which kept poem's `test` feature, and the MPL-2.0
`sse-codec` it pulls, in every suite's graph.

## 2026-10-18 — the WebSocket upgrade is nestrs's, and so is its close code

Both socket edges — a gateway and the graphql-ws socket — open on one
handshake of the transport's own, `nest_rs_http`'s hidden `websocket` module
behind the `websocket` feature: RFC 6455 §4.2.1's checks, each refusal a
problem document naming what the handshake lacks (`400` for the method and
for the `Upgrade`, `Connection` and `Sec-WebSocket-Key` lines, `426` with
`Sec-WebSocket-Version: 13` for the version), then the `101` with its accept
key, and the socket on tokio-tungstenite 0.30 over the connection hyper hands
across — its place under the connection cap and its send deadline kept,
since the accept loop's wrapper sits under hyper's upgrade. A request with no
connection under it to switch — HTTP/1.0, or the in-process test client —
answers `426`, as axum's extractor does, where poem answered a `500`. The
ports of poem's checks are stricter where the RFC is: one key of 16 bytes in
base64, one version line, the tokens read in any case from any line.

Both edges register the handshake on the route table at `GET`, so another
method meets the table's `405` and the one `Allow` the address carries. The
table hands a `HEAD` to the `GET` entry as a `GET` and marks it, and the
handshake refuses a marked request `400` (RFC 6455 §4.1: a handshake is a
`GET`): a `405` of its own would name an `Allow` it cannot know, `/graphql`
serving `POST` too. `Upgrade` is read as RFC 9110 §7.8's list by one
predicate, which the browser-origin check, the GraphQL `GET` and the
handshake share, so no upgrade the handshake admits skips the origin check.

`CloseCode` keeps its name and poem's variants, nestrs's own enum now and
`#[non_exhaustive]`; every `u16` round-trips, an application's `44xx`
through `Reserved`. It is defined beside the upgrade, not in `nest-rs-ws`,
because the graphql-ws socket closes with it too and `nest-rs-graphql` does
not depend on the gateway's crate; `nest_rs::ws::CloseCode` is its one public
path. The conversions to tungstenite's code stay private. A read past the
message cap now closes `1009 Message Too Big` on both edges, read off
tungstenite's typed capacity error, which poem flattened into an opaque
`io::Error`.

Refused: keeping the type in `nest-rs-ws` and having graphql-ws close with
bare numbers — two vocabularies for one code; a public handshake type,
which no app writes and the next adapter would have to keep; and a gateway
registered for every method, which answered another method behind the
guards, with an `Allow` beside the table's.

## 2026-10-19 — GraphQL over HTTP without async-graphql-poem

`/graphql`'s `POST` and `GET` endpoints implement the transport's `Endpoint`
and register on the route table with `HttpRoutes::route`; async-graphql-poem
leaves the workspace, and with it the last request for poem's `websocket`
feature, so tokio-tungstenite 0.27 leaves both lockfiles. The answer is written
as async-graphql-poem wrote it for 6.x, pinned by a header-set test: JSON as
`application/json; charset=utf-8`, `200` whatever `errors` holds, the schema's
cache hint as `Cache-Control` when every operation succeeded, then each header
a resolver set, copied from the `HeaderMap` async-graphql holds rather than
through the string round trip the adapter made. A multipart request is read by
async-graphql's own `receive_batch_body` over the body's stream, a file held to
the transport's body cap. What the endpoint refuses — a batch past
`max_batch_size`, a `GET` it cannot serve — reaches the self-mount's site as an
`HttpError`, where the app's exception filters read it, instead of being
rendered inside a poem leaf first.
