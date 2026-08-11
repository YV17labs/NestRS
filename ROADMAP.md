# Roadmap

NestRS is **stable at 4.0**. Every `nest-rs-*` crate publishes at the same
version in lockstep, the public API follows semver, and a breaking change waits
for the next major. The third-party types that appear in your own code — `poem`,
`sea-orm`, `async-graphql`, `rmcp`, `inventory`, `validator`, `schemars` — have
their majors tied to the NestRS major and frozen for the whole `4.x` line: one
dependency resolution, for the life of `4.x`.

**This file holds only what is *not* done.** An entry leaves the moment it
ships; what shipped is in [CHANGELOG.md](CHANGELOG.md), and the record of *what
was decided and why* is [CLAUDE.md](CLAUDE.md). It is a *direction, not a dated
commitment* — priorities move with what the community needs.

Every entry is one pickable task, written the same way:

- **Today** — what already works, so the task does not rebuild it.
- **Where** — the crates and files it lands in.
- **Done when** — the condition that closes it, and removes the entry.

Sections are ordered by priority. `Later` holds work deliberately deferred: it
carries a reason, and reopening one is an **owner decision** — see *Autonomous
work — stop and ask* in [CLAUDE.md](CLAUDE.md). Whatever you pick, finishing it
means the *Definition of done* in that same file, for every workspace touched.

Want to shape it? Open a
[Discussion](https://github.com/YV17labs/NestRS/discussions) or pick up a
[`good first issue`](https://github.com/YV17labs/NestRS/labels/good%20first%20issue).

## Next — moving checks earlier

Each of these takes a check that already **fails closed at run time** and moves
it to compile or boot time, which is where NestRS prefers to answer a wiring
question.

### Alias-proof masking arm

Replace the by-name arming of the HTTP response shaper with a seam that does not
depend on how a type is spelled.

- **Today** — `#[routes]` arms the shaper (ambient ability + masking) by matching
  a parameter path segment named `Authorize` / `Bind`. A renamed import
  (`use Authorize as Az`) **fails closed** rather than silently unmasking:
  unarmed routes carry a `MaskProbe`, and a masking extractor that runs without
  an armed shaper turns the response into a logged `500`.
- **Where** — `crates/nest-rs-http/src/shaper.rs` (the probe and the shaper),
  `crates/nest-rs-http-macros/` (the arming decision in `#[routes]`).
- **Done when** — the extractor registers a type-erased masker + ability into a
  generic ambient-context seam and a generic shaper applies them, so arming
  survives any alias; the `MaskProbe` `500` path becomes unreachable rather than
  load-bearing, and a test proves an aliased import still masks.

## Next — extending shipped features

Additions to capabilities that already ship. Each is an extension of a working
surface, not a prerequisite for using it.

### OpenAPI — header params, multipart and streamed bodies

Describe the three request/response shapes the document still omits.

- **Today** — the document carries typed path params, expanded query params,
  `bearerAuth` on guarded routes, per-route error statuses (the effective success
  code plus `400`/`404`/`429`), RFC 9457 error responses, and a boot-written
  `openapi.json` snapshot.
- **Where** — `crates/nest-rs-openapi/`.
- **Done when** — header parameters appear on the operations that read them, and
  multipart request bodies and streamed responses carry schemas; the committed
  `openapi.json` snapshot moves with them.

### File storage — listing and streaming uploads

Finish the object-store surface.

- **Today** — presign, head, byte and delete all ship, plus streamed downloads
  (`get_stream`).
- **Where** — `crates/nest-rs-storage/src/client.rs`.
- **Done when** — a prefix can be listed and an upload can stream, both with e2e
  coverage against the devcontainer's RustFS.

### `nest-rs-resource` — enum mode, relation pagination, FK override

Three gaps in `#[expose]`, independent of each other.

- **Today** — an enum column passes through if it derives the surface traits, but
  there is no first-class mode; an auto-emitted HasMany resolver returns a
  `Vec<T>` capped at `nest_rs_seaorm::RELATION_LOAD_CAP` (100 per parent) —
  bounded, but not cursor-paginated; a non-conventional FK column needs a
  hand-written `#[field_resolver]`.
- **Where** — `crates/nest-rs-resource/`,
  `crates/nest-rs-resource-macros/src/relations.rs`.
- **Done when** — `#[expose]` has an enum mode, HasMany paginates through
  `Connection<T>`, and a `via = "..."` override reaches a non-conventional FK
  without hand-written code.

### API versioning — header and media-type selection

Add the two version strategies that need request-time dispatch.

- **Today** — URI versioning ships: `#[controller(version = "1")]` mounts under
  `/v1`, and `version_path` is the single source of truth.
- **Where** — `crates/nest-rs-http/src/transport.rs`, `crates/nest-rs-http-macros/`.
- **Done when** — a controller can select its version from a header or a media
  type, resolved per request, with the same one-source-of-truth property URI
  versioning has.

### TLS certificate hot-reload

Swap the certificate live so a renewal lands without a restart.

- **Today** — `HttpTransport::tls` loads the certificate once, at boot.
- **Where** — `crates/nest-rs-http/src/transport.rs`, `crates/nest-rs-http/src/tls.rs`.
- **Done when** — the PEM source is watched and the `rustls` config is swapped in
  place, with a test that serves across a swap without dropping the listener.

### Streaming a multipart part straight into storage

Remove the last place a large upload is buffered whole.

- **Today** — multipart uploads, streamed download bodies, SSE and response
  compression all ship (poem's `Multipart`, `Body::from_bytes_stream`, `SSE`,
  `HttpConfig.compression`), bounded by the transport-wide `max_body_bytes` cap
  that covers every extractor.
- **Where** — `crates/nest-rs-http/`, `crates/nest-rs-storage/`.
- **Done when** — a multipart *part* streams into the object store without being
  held in memory, still under the transport cap.

## Next — tooling

### Docs — a Basics / All-options tier split per section

- **Today** — a section presents every option at one level, so the 80% case and
  the long tail read with the same weight.
- **Where** — `docs/src/content/docs/`, `docs/STYLE.md` (the templates that
  decide page shape).
- **Done when** — each section separates a Basics tier from an All-options tier,
  and `STYLE.md` states the split so a new page inherits it.

### CLI — an `entity` generator and `nestrs info`

- **Today** — an entity is reachable through `g resource`; `nestrs about` covers
  most of what `info` would report.
- **Where** — `crates/nest-rs-cli/src/commands/generate/`.
- **Done when** — `nestrs g entity` scaffolds an entity on its own, and the
  remaining gap between `about` and `info` is either closed or dropped as
  redundant.

## Later — deferred

Not current priorities; each carries the reason. Picking one up is an owner
decision, not a judgement call.

### Transport-neutral guard core

One `Guard` trait, one chain and one dispatch across every transport is a
deliberate design. `check_http` sits on the base trait, so `nest-rs-guards` links
the HTTP stack even in a headless binary — a binary-size question with no
runtime, security or correctness effect. `check_graphql` / `check_ws_message` /
`check_mcp` are already feature-gated; moving `check_http` into an `HttpGuard`
extension trait touches every guard impl and the HTTP dispatch
(`crates/nest-rs-guards/src/guard.rs`), so it waits for a major that has another
reason to touch them. Three have passed without one.

### Per-job transactions

A `#[scheduled]` / `#[processor]` runs on the connection **pool**: a worker job
has no safe/mutating method to classify, the way an HTTP verb or a WebSocket
message does. Deliberately deferred until a job shape makes the classification
honest.

### A first-class SSE decorator

poem's `SSE` is reachable through the re-export today, so a route can already
stream; what is deferred is an `#[sse]` of our own, reusing the per-connection
plumbing the WebSocket gateway and the graphql-ws socket already share.

### GraphQL federation

And the dedicated schema tooling it would reintroduce.

## Not on the roadmap

By design — see the *Hard "no" list* in [CLAUDE.md](CLAUDE.md):

- No external dependency-injection library — the container is ours.
- No splitting the workspace into microservices "for scalability".
- No backwards-compatibility shims — a breaking change waits for the next major.
- **No `ClassSerializerInterceptor` / `@Exclude` / `@Expose`** — serde already owns
  serialization (`#[serde(skip)]`, or a dedicated response DTO); a per-request
  "groups" interceptor is not worth reproducing.
- **No `HttpModule` / `HttpService`** — inject a configured `reqwest::Client`; an
  axios-style wrapper would be pure ceremony.
- **No bundled `Logger` service** — `tracing` is the idiomatic, structured, superior
  answer, and is already the project's logging layer.
