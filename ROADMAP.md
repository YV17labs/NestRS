# Roadmap

This file explains three things: what 7.x plans to add, in what order, and what would force a NestRS 8.0. Nothing here is
promised until a release's changelog lists it. Each item starts only once the owner says yes to it by name, and
`Status` records that yes.

Last reviewed **2026-11-17**, against NestJS 12.1 and the npm registry on that day.

| Status | Meaning |
|---|---|
| Proposed | Written down and sized, with no yes yet |
| Accepted | The owner said yes; its open questions are answered below |
| In progress | Being built, on the branch named here |
| Shipped in 7.N | Released; its line lives in `changelog/7.N.md` |

---

## 1. The principle: 7.x lasts

7.0 is the foundation, and the 7.x line is meant to last for years. New capabilities arrive as minors.
[The semver policy](https://nestrs.dev/semver/) (`.claude/decisions/semver-7x.md`) lets a minor add each of these:

- a tier-1 item;
- a crate, an umbrella feature, a decorator key or a variable;
- a variant of a `#[non_exhaustive]` enum, or a field of a type with private fields;
- a field of a `#[config]` struct, built over `..Default::default()`.

**Every item in § 5 is additive.** Each one is a new crate, or new features of an existing one, with new variables and new
`nestrs_*` tables. None of them changes something 7.0 already exposes. They build on extension points 7.0 already has,
checked in the code on 2026-11-17:

| Seam in 7.0 | Where | What it carries |
|---|---|---|
| A guard may read the request body and set it back | `crates/nest-rs-http/src/raw_body.rs` | verifying an inbound webhook's signature (item 6) |
| An authn strategy receives the whole request, cookies included | `nest_rs::authn::Strategy::authenticate(&mut Request)` | sessions and API keys (item 8) |
| One execution pipeline, with its layers, for HTTP, a GraphQL field, a WS message, an MCP operation, a job attempt, a tick and a listener | `.claude/decisions/one-execution-pipeline.md` | idempotency, resilience and locale negotiation on every edge (items 1, 5, 7) |
| `Problem` has private fields and carries `Retry-After` | `crates/nest-rs-core/src/problem.rs` | `503` with `Retry-After`, new codes, translated problems (items 1, 5, 7) |
| A push inside a unit commits with its transaction | `.claude/decisions/transactional-outbox.md` | mail, outgoing webhooks and workflow starts bound to the commit (items 4, 6, 10) |
| Named instances of a resource | `.claude/decisions/named-instances.md` | named storage stores (item 3) |
| Ports with vendor bindings, refusing in-memory stores in production | `.claude/rules/architecture.md` | the locks and idempotency stores on Valkey and Postgres (items 2, 5) |
| Framework tables named `nestrs_<pattern>`, with migrations the CLI registers by reserved name | `transactional-outbox.md`, entry of 2026-10-21 | the tables of locks, idempotency, webhooks and workflows |
| In-process TLS test doubles | `nest_rs_testing::TestAuthority` | testing SMTP, an LLM server and a webhook receiver without a new CI service |

---

## 2. What would force 8.0: public dependencies, not features

A public dependency is one whose vocabulary an app writes in its own code (`.claude/decisions/public-dependencies.md`). Its
next major can only land in a NestRS major, and the README promises exactly that. **The length of the 7.x line is
therefore set by these libraries' release clocks, not by this roadmap.** The table is their state on 2026-11-18.

| Library | 7.0 pins | Next major | Recent pace | What an app writes in it |
|---|---|---|---|---|
| `redis` | `=1.8` | **2.0.0-rc.3** (rc.1 on 2026-10-24, rc.3 on 2026-11-14); no date given for the final | 1.0 in December 2025 | `RedisConnection` implements its `ConnectionLike` |
| `async-graphql` | `=7.2` | 8.0.0-rc.5, unchanged since 2026-04-21 (point 2) | 7.0 in January 2024; last stable release on 2026-01-20 | resolvers, derives, `Context` |
| `rmcp` | 3.11 | none on its roadmap | 1.0, 2.0 and 3.0 between March and July 2026, the last with the MCP `2026-07-28` rewrite | MCP tool and model types |
| `validator` | 0.21 | its next minor (a `0.x`) | 0.20 in January 2025, 0.21 in July 2026 | `#[validate(…)]` |
| `reqwest` | 0.13 | its next minor (a `0.x`) | 0.12 in 2024, 0.13 in December 2025 | the client an app builds |
| `reqwest-middleware` | 0.5 | its next minor (a `0.x`) | about once a year | `ReqwestMiddleware` implements its `Middleware` |
| `rustls`, `tokio-rustls` | 0.23, 0.26 | 0.24 in development builds, no release candidate | — | `nest-rs-testing` alone |
| `sea-orm` | `=2.0` | — | 1.0 in 2024, 2.0 in July 2026 | entities, `Repo`'s builder |
| `chrono` | 0.4 | none: declared unmaintained (RUSTSEC-2026-0543), with no release to move to (point 3) | 0.4.45 in June 2026 | the instants `nest_rs::seaorm::now` and an entity's timestamps hold |
| `tokio`, `serde`, `http`, `tracing`, `schemars`, `syn`, `uuid` | — | none in sight | stable for years | — |

**Settled before 7.0 is published.** A public dependency's major is free until the first publish
(`public-dependencies.md`), and what the semver policy does not say before then, it cannot say after. The first two
points apply that file's rule of 2026-11-17: an active release candidate holds the publication, a stalled one does not.

1. **`redis` 2.0 is waited for until 2026-12-31**; with no final release by then, 7.0 ships on 1.8 and 2.0 waits for
   NestRS 8. `nest-rs-redis` already passes every topology's suite against rc.3, on the `chore/redis-2` branch. Waiting
   has a cost: 6.x queue users do not compile on Rust 1.100 until 7.0 ships (`queue-redis-streams.md`). Past 2.0, a
   NestRS command API in place of implementing redis's `ConnectionLike` would turn redis majors into NestRS minors, at
   the price of the vocabulary an app writes today.
2. **`async-graphql` 8 waits for NestRS 8.** rc.5 cannot be taken: one failing nullable field answers `data: null` for
   the whole response, and it still requires the `lru` RUSTSEC-2026-0253 names, so the advisory's ignore stays either
   way (`.claude/upstream/async-graphql.md`). 7.2.1 crosses the twelve-month freshness bar on 2027-01-20 if nothing
   ships before.
3. **`chrono` stays in 7.x**: `time` would change how REST spells an instant and still leave chrono in every app with
   migrations; jiff, NestRS 8's target, waits for sea-orm and sqlx to carry it (`public-dependencies.md`, 2026-11-18).
4. **7.0 builds on rmcp 3.11 and jsonwebtoken 12**, the latest of each. `rmcp` is a smaller risk than its count of
   majors suggests: its majors followed the protocol's rewrite, 3.x passes the `2025-11-25` and `2026-07-28` conformance
   suites, `nest-rs-mcp` serves both, and the specification now keeps a deprecated feature for at least twelve months.
5. **An exported telemetry name promises what its convention does**: one from a convention in development follows it,
   its rename a minor named in the changelog (`semver-7x.md`, 2026-11-18).
6. **Every crate builds from its package before any is published**: `publish.yml` runs `cargo package` first.
7. **The public surface added since 2026-10-31 was audited**, and the three items that would have frozen a mistake
   moved (changelog, *Tier 1 holds what a developer writes*).

**When a forced major comes**, 8.0 stays mechanical: it moves the dependency, takes what that move requires, and ships an
upgrade guide. No redesign rides along. `SECURITY.md` already bounds its pace: a major ships at most every six months,
announced four weeks ahead with its upgrading guide. A rare, small major is the healthy outcome. A long line that holds a vulnerable
or outdated library is not.

What async-graphql 8 would require was measured against rc.5 on 2026-11-18. GraphQL Playground is gone upstream, so the
`playground` setting and its variable become GraphiQL's. Every `DataLoader` a `#[dataloader]` builds takes a `Spawn` and
a `Timer`. A field whose name has a digit before a letter is spelled anew (`get_2fa` becomes `get2fa`). And a `?` on a
foreign error compiles again in a resolver, so the refusal 7.x holds at compile time would have to hold at run time.

---

## 3. NestJS 12 parity that needs no new work

Between 2026-08-21 and 2026-11-05, NestJS published sixteen new first-party packages, all at `0.0.x` or `0.1.x`. These map
to what NestRS already ships:

| NestJS package | NestRS today |
|---|---|
| `@nestjs/observe` | `nest-rs-opentelemetry`: OTLP for logs, traces and metrics to any backend (`docs/src/content/docs/coming-from-nestjs/observe.mdx`) |
| `@nestjs/mcp` | `nest-rs-mcp`: Streamable HTTP, guards per tool, sessions bound to their principal. No stdio (see § 4) |
| `@nestjs/authorization` | `nest-rs-authz`: abilities, row-level filtering, per-field masking, on every edge |
| `@nestjs/outbox` | the queue's transactional push: a push inside a unit commits with it, and a relay files it (`transactional-outbox.md`) |
| `@nestjs/http-client` | the app's own `reqwest` client with `nest_rs::reqwest::ReqwestMiddleware` (trace context and client spans); retries come with item 1 |

## 4. Not planned, and why

- **An outbound `HttpModule` / `@nestjs/http-client`**: it is a hard "no" in `CLAUDE.md`. The app keeps its own `reqwest`.
- **`@nestjs/drizzle`**: NestRS supports one ORM, SeaORM.
- **`@nestjs/mau` (`nest deploy`) and the hosted `@nestjs/observe` dashboard**: they are SaaS. Operating an app is its
  backend's tooling.
- **MCP over stdio**: a server run as a subprocess of an AI client is a local tool, not a backend.
- **An `@OnOutboxMessage`-style delivery API**: a job runs through the queue's one consume loop (`transactional-outbox.md`).
- **`@WithoutOverlapping` on `#[cron]` / `#[every]`**: the run lease was built and removed (`schedule-run-lease.md`).
  Item 2's `with_lock` covers a body that must not overlap.
- **Proprietary mail HTTP APIs** (Resend, Postmark, SendGrid, SES) **and proprietary LLM APIs**: by the value "open source
  only: a backend's server is free software". Every one of those mail providers accepts SMTP, and LLMs are reached through
  the OpenAI-compatible protocol that free servers speak.

---

## 5. Proposed for 7.x

The order goes foundations first, then completing what already exists, then what users ask for, then the largest. Effort
is in engineering hours, review and gates included. It is a guess calibrated on the 7.0 foundation work.

| # | Item | Crate | Effort | Needs | Status |
|---|---|---|---|---|---|
| 1 | Resilience | `nest-rs-resilience` | 20–35 h | — | Proposed |
| 2 | Distributed locks | `nest-rs-locks`, plus Valkey and SeaORM bindings | 25–40 h | — | Proposed |
| 3 | Storage, completed | `nest-rs-storage` | 10–20 h | — | Proposed |
| 4 | Mail | `nest-rs-mail` | 25–40 h | — | Proposed |
| 5 | Idempotency keys | `nest-rs-idempotency`, plus Valkey and SeaORM bindings | 30–50 h | — | Proposed |
| 6 | Webhooks | `nest-rs-webhooks` | 40–60 h | item 5 helps | Proposed |
| 7 | Internationalization | `nest-rs-i18n` | 30–50 h | — | Proposed |
| 8 | Authentication flows | `nest-rs-authn`, plus new crates where a port is new | 70–110 h | item 4 | Proposed |
| 9 | Language models | `nest-rs-ai` | 50–80 h | item 1 | Proposed |
| 10 | Durable workflows | `nest-rs-workflows` | 100–180 h | item 2 helps | Proposed |

Every crate name follows `.claude/rules/naming.md`. The shared word is read off the NestJS package name, the market's
word, as `storage-port-by-url.md` did for `nest-rs-storage`. A new crate's name is the owner's to confirm.

NestJS ships each of these at `0.0.x`, so their APIs will move. Ours is held to 7.x semver from its first minor: each design
is settled before it ships, not iterated in public.

### 1. Resilience — `nest-rs-resilience`

- **NestJS**: `@nestjs/resilience` 0.0.3 ([docs](https://docs.nestjs.com/reliability/resilience)). It offers timeout per
  attempt, retry with jittered backoff (idempotent calls only), a circuit breaker (count window, half-open probe), a
  bulkhead and a fallback, named presets, and the codes `TIMEOUT` (504), `CIRCUIT_OPEN` and `BULKHEAD_FULL` (503).
- **Today**:
  - `nest_rs_core::Backoff`, `retry_on_conflict` for database conflicts, deadlines on each edge, the throttler for inbound
    rate, and the queue's retries;
  - nothing protects **an outbound call to a failing dependency**: a slow partner API holds every request that calls it,
    and a down one is hammered by every retry.
- **What it serves**:
  - named policies an app wraps around any call (`policy.run(|cancel| …)`): a payment provider, a carrier API, an LLM;
  - one breaker shared by every caller of a dependency;
  - a client answered `503` with `Retry-After` instead of a hung request;
  - the retry for the app's `reqwest` client that `@nestjs/http-client` builds in.
- **Constraints**: every wait is bounded. State lives per process, and the docs say so. Retries apply only where the caller
  declares the call idempotent. A transition is a `warn` and a metric.
- **Open**: whether a method attribute is added beside the policy API. Only if it does not become a second way to do the
  same thing.

### 2. Distributed locks — `nest-rs-locks`

- **NestJS**: `@nestjs/locks` 0.0.6 ([docs](https://docs.nestjs.com/reliability/locks)). It offers leases renewed every
  `ttl/3`, fencing tokens, a signal when a lock is lost, `withLock`, leader election, and Postgres and Redis stores checked
  by a contract suite.
- **Today**:
  - `replicas = "one"` on `#[cron]` and `#[every]` claims each occurrence on one replica; that matches
    `@OnOneInstance`;
  - there is no general lock, no fencing token and no leader election.
- **What it serves**:
  - code that must run on one instance at a time: a nightly export, a reconciliation, a migration step;
  - a job body that must not overlap the previous run: `with_lock` inside it;
  - continuous work with exactly one leader: a stream consumer, a cache warmer;
  - fencing tokens that let the database refuse a write from a holder that paused and lost its lease.
- **Builds on**: the port-and-bindings shape. `RedisLocksModule` covers the three Valkey shapes with TLS (Lua, hash tags in
  cluster). `SeaOrmLocksModule` uses the `nestrs_locks` table and a fencing sequence. A contract kit in `nest-rs-testing`.
- **Constraints**: `schedule-run-lease.md` stands: `#[cron]` and `#[every]` gain no overlap lease.

### 3. Storage, completed — `nest-rs-storage`

- **NestJS**: `@nestjs/storage` 0.0.5 ([docs](https://docs.nestjs.com/application/file-storage)). It offers local,
  S3-compatible and in-memory disks, named disks, local signed URLs with key rotation, safe uploads (type detected from the
  bytes, size cap without partial files, generated keys), and serving with `Range` and `ETag`.
- **Today**: `s3://` and `memory:///`, presigned `GET` and `PUT` (with the content type bound), `head`, streams, listing,
  resumable transfers, the boot proof and health.
- **What it serves**:
  - a single-server deployment, or development without an S3 server: `file://`, with HMAC-signed URLs the app serves;
  - several buckets in one app: named stores, `Storage<I>`;
  - an upload a client cannot disguise or use to overwrite another file: detection from the bytes (`415`), a size cap
    (`413`), keys generated by the server;
  - downloads that resume and cache: `Range`, `ETag`, `If-None-Match`, reusing `nest-rs-static-files`'s ranges.
- **Constraints**: it reopens two refusals of `storage-port-by-url.md` (`file://`, named stores), each a dated entry
  there. Binding `content-length` on an S3 presign stays limited by object_store.

### 4. Mail — `nest-rs-mail`

- **NestJS**: `@nestjs/mail` 0.0.2 ([docs](https://docs.nestjs.com/application/mail)). It offers typed mail classes,
  templates escaped by default with a derived text part and locale variants, SMTP (TLS, AUTH, pool, DKIM), a file
  transport for development, an in-memory transport with assertions for tests, and sending after the commit through the
  outbox.
- **Today**: nothing sends mail. An app wires its own client by hand and decides alone what happens on a rollback.
- **What it serves**:
  - transactional mail (welcome, receipt, password reset) that **never leaves for a rolled-back transaction and survives
    a crash after the commit**: sent from a unit, it rides the queue's outbox;
  - templates checked at compile time, safe from injected markup;
  - tests that assert on what was sent without an SMTP server.
- **Constraints**:
  - SMTP alone (see § 4), on `lettre`, with verified TLS; a plaintext opening is a visible line;
  - credentials and the DKIM key are `Secret`s;
  - delivery is at least once, so a duplicate is possible and the docs say so;
  - no address appears in a log line or an error;
  - tests run against an in-process SMTP double over TLS, with no new CI service.
- **History**: a mail module was declined on 2026-10-08, when NestJS had none. The owner reopened it on 2026-11-17.

### 5. Idempotency keys — `nest-rs-idempotency`

- **NestJS**: `@nestjs/idempotency` 0.0.4 ([docs](https://docs.nestjs.com/reliability/idempotency)), after the IETF draft
  `draft-ietf-httpapi-idempotency-key-header`. It offers a fingerprint of the request, `409` while in flight, the stored
  response replayed, `422` when a key is reused with another body, a scope per user, encrypted records, and Postgres and
  Redis stores.
- **Today**: a queue job is idempotent by `Attempt::job_id`, by contract. An HTTP route or a GraphQL mutation has nothing:
  a client retrying a `POST` after a timeout runs it twice.
- **What it serves**: payments, orders, anything a mobile client retries on a bad network. One `#[idempotent]` on the
  route makes a retry return the first answer instead of charging twice.
- **Builds on**: the core pipeline (HTTP and GraphQL), and the store bindings as in item 2.
- **Constraints**: replaying a response means storing it, and `CLAUDE.md` refuses a payload value in a stored record. The
  proposal is mandatory encryption at rest (AES-256-GCM, HKDF keys held as `Secret`s, with rotation). It needs the owner's
  yes, because it reads against a hard "no". Without it, the item ships deduplication without replay.

### 6. Webhooks — `nest-rs-webhooks`

- **NestJS**: `@nestjs/webhooks` 0.0.4 ([docs](https://docs.nestjs.com/http/webhooks)), signing per
  [Standard Webhooks](https://www.standardwebhooks.com/).
  - Outgoing: dispatch through the outbox, endpoints per tenant, retries over about two days, endpoints disabled after
    failing, a delivery log, rotating secrets, SSRF protection.
  - Incoming: signatures verified on the raw body (Standard Webhooks, Stripe-style, GitHub), deduplication.
- **Today**: `RawBody`, and a guard that can read it. No signing, no delivery and no endpoint registry.
- **What it serves**:
  - a SaaS notifying its customers' systems, signed and retried, sent only once the change has committed;
  - receiving Stripe or GitHub events with the signature checked in a guard, where `CLAUDE.md` puts every authentication
    decision.
- **Constraints**:
  - delivery rides the queue;
  - SSRF follows OWASP: https only, and the resolved address is checked, then pinned for the connection;
  - secrets are encrypted at rest;
  - the delivery log keeps status and timing, never a response excerpt;
  - tests run against an in-process HTTPS receiver double.

### 7. Internationalization — `nest-rs-i18n`

- **NestJS**: `@nestjs/i18n` 0.0.3 ([docs](https://docs.nestjs.com/application/i18n)). It offers the locale resolved per
  request (`Accept-Language`, query, header, cookie, with fallbacks), catalogs with CLDR plurals, translated errors and
  validation messages, formatting, and a locale on GraphQL, WS and messages.
- **Today**: `Vary: Accept-Language` and the ambient context. Every problem and validation message is in English.
- **What it serves**: an API answering each client in its language, its errors included, on every edge.
- **Constraints**:
  - negotiation follows RFC 9110 § 12.5.4 and RFC 4647;
  - catalogs are checked at compile time: a missing key or placeholder fails the build;
  - plural rules and formatting come from ICU4X;
  - the message syntax is Unicode MessageFormat 2 if a maintained Rust implementation exists. `fluent-bundle` (last
    release May 2025) is past the freshness bar.

### 8. Authentication flows — `nest-rs-authn` and new crates where a port is new

- **NestJS**: `@nestjs/authentication` 0.1.6 ([docs](https://docs.nestjs.com/security/authentication)). It offers
  password sign-in with server-side sessions in a cookie, email verification, password reset, magic links, rotating
  refresh tokens, TOTP and recovery codes, API keys, and lockout.
- **Today**:
  - JWT verification (HMAC, RSA, ECDSA, EdDSA) with JWKS, Argon2id at OWASP's parameters, an OAuth 2.0 authorization
    server (token endpoint), and OAuth client and social sign-in with PKCE;
  - an app still writes its own sign-in, session, reset and 2FA flows.
- **What it serves**: an app owning its users, with no external identity provider, and sign-in that is secure by default.
  Each sub-item is a yes of its own:
  - sessions in a `__Host-` cookie, with the CSRF defence `csrf-by-bearer.md` already chose for that day (Fetch Metadata);
  - password sign-in and lockout;
  - email verification, reset and magic links (on item 4);
  - TOTP (RFC 6238) and recovery codes;
  - API keys;
  - rotating refresh tokens.
- **Constraints**:
  - every credential check is a strategy or a guard;
  - tokens are stored hashed, and second-factor secrets encrypted;
  - OWASP ASVS and its Authentication, Session Management and Forgot Password cheat sheets apply;
  - the user table stays the app's.
- **History**: an identity server was declined on 2026-10-08 as "not NestJS-native". `@nestjs/authentication` is now
  first-party.

### 9. Language models — `nest-rs-ai`

- **NestJS**: `@nestjs/ai` 0.0.2 ([docs](https://docs.nestjs.com/ai/overview)). It offers named models, text and output
  validated against a schema, tools that run through guards as the signed-in user (the same method also served over MCP),
  streaming, embeddings, timeouts, retries, fallback models, and cost and budgets.
- **Today**: `nest-rs-mcp` exposes provider methods as MCP tools behind guards. Calling a model is left to the app.
- **What it serves**:
  - an assistant in the product whose tools are the app's own methods, authorized exactly as the app's routes;
  - one declaration serving both MCP clients and in-app agents;
  - spend bounded per call.
- **Constraints**:
  - the OpenAI-compatible protocol (see § 4); tests run against an in-process server double;
  - spans follow OpenTelemetry's GenAI conventions, which are still experimental, so their names are documented as such
    before they become API;
  - no prompt or completion content in telemetry;
  - durable agent runs wait for item 10.

### 10. Durable workflows — `nest-rs-workflows`

- **NestJS**: `@nestjs/workflows` 0.0.5 ([docs](https://docs.nestjs.com/reliability/workflows)). It offers a journal in
  the app's own database, steps run once and replayed from the journal, durable sleeps and signals, child workflows,
  compensation, versions running side by side, and a start that joins the app's transaction.
- **Today**: the queue (at least once, with the outbox) and schedules. A multi-day process (an order, an onboarding) is
  hand-written as jobs that pass their state along.
- **What it serves**: processes that wait for days, survive deploys and compensate on failure, written as one async
  function, with no workflow server to run.
- **Constraints**:
  - resumption rides the existing `queue` edge, since the edge vocabulary is closed;
  - determinism comes through the context (`now`, `random`, `uuid`);
  - each guarantee is proved by killing the worker between steps.
- **The largest item, and the most speculative**: it is built in parts that each ship green.

---

## 6. How an item becomes a release

1. The owner says yes to it by name, and this file says `Accepted`.
2. It is built on its own branch, through `.claude/rules/manifests-ci.md`'s *Shipping a capability* checklist: umbrella
   feature, docs page with `## Install`, expansion and composition witnesses, and both test suites over TLS.
3. It ships in the next minor, with its line in `changelog/7.N.md`. Its row moves to § 3, and the page
   `docs/src/content/docs/coming-from-nestjs/` gains its mapping.
4. A refusal met along the way, or a limit, goes into `.claude/decisions/` and is named here under the item.
