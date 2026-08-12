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

Nothing is queued for 4.x right now. What remains below is work that was
**deliberately deferred**, each entry carrying the reason it was: picking one up
is an **owner decision**, not a judgement call — see *Autonomous work — stop and
ask* in [CLAUDE.md](CLAUDE.md). Finishing it means the *Definition of done* in
that same file, for every workspace touched.

Want to shape it? Open a
[Discussion](https://github.com/YV17labs/NestRS/discussions) or pick up a
[`good first issue`](https://github.com/YV17labs/NestRS/labels/good%20first%20issue).

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
