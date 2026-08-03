# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Headed for **2.0.0** — the workspace already carries that version; the entry
below moves under it at tag time.

**One dependency.** `nest-rs` with the feature for the capability becomes the
whole install, on every page of the documentation and every crate's crates.io
landing page. A decorator's expansion no longer obliges the developer to declare
anything.

**Breaking.** An app that names the `nest-rs-*` crates directly keeps compiling,
but a decorator used from such a crate now roots its expansion at the umbrella
when one is present. The documented path is the umbrella; the sub-crates remain
published compilation units.

### The install contract

- **`cargo add nest-rs --features <capability>`** — 17 of the 19 module pages
  now install in exactly one line. `/database/` went from 7 to 1, `/graphql/`
  from 6 to 2, `/configuration/` from 2 to 1.
- **The `validator` version pin is gone.** `#[config]` carries the `Validate`
  derive and points it back at the framework's own copy, so no `#[config]`
  struct declares `validator` or keeps a major aligned. `validate = "manual"`
  opts out for a config that validates across fields.
- **`#[expose]` carries the derives it generates** — `serde`, `schemars`,
  `validator` — each routed with a `crate = ` override, alongside the
  entity-site trio (`sea_orm` / `uuid` / `chrono`). An entity crate declares
  none of them.
- **`#[input]` is the wire-DTO shorthand on every transport**, re-exported from
  `nest_rs::{ws, queue, mcp}`. A typed payload needs no `serde` of its own.
- **`features = ["full"]`** for an app that does not want to choose yet.
- **A missing dependency now says what to add**, with a copy-pasteable line,
  instead of `E0433: cannot find nest_rs_core` blamed on the attribute.

### MCP is the whole protocol now — rmcp 2.2 → 3.1

**Breaking, and the reason it ships in a major.** rmcp 3.x is a new major of the
SDK whose types appear in the signatures `#[mcp]` hosts write: SEP-2663 tasks,
SEP-2575 discovery and subscriptions, SEP-2549 cache hints, SEP-2322 multi-round
tool responses, SEP-2243 standard HTTP headers, and the `stateful_mode` →
`legacy_session_mode` rename.

- **Every MCP capability now gets the framework's transparent security**, not
  `tools/call` alone. rmcp 3.x retired the single `Service::handle_request` seam
  the old wrapper hooked and bounded its server on `ServerHandler`, so
  `PropagatingHandler` delegates the **whole** trait: the request scope, the
  caller's ability and the operation's transaction are installed around
  `prompts/get`, `resources/read`, `completion/complete`, `logging/setLevel`,
  the `tasks/*` trio, custom methods and the protocol lifecycle. Two documented
  exceptions: notifications (nothing to commit) and `subscriptions/listen`
  (outlives any sane transaction — it gets scope and ability, no transaction).
  A method left undelegated would have reverted to an SDK default and answered
  *for* the host, so `propagate.rs` drives all 27 over a real
  streamable-HTTP endpoint and fails if one goes missing.
- **Prompts have decorators.** `#[prompt_router]` / `#[prompt]` /
  `#[prompt_handler]` are re-exported beside the tool trio and stack on one
  `impl ServerHandler`. Resources stay hand-written methods — a resource surface
  is a URI-to-row mapping, not a set of methods.
- **The protocol is re-exported wholesale** — `nest_rs::mcp::{model, service,
  handler, transport}` plus the `rmcp` escape hatch, now documented rather than
  `#[doc(hidden)]`. A capability a future rmcp adds is reachable the day it
  ships, with no framework release in between.
- **`McpConfig` + `McpModule`** expose the streamable-HTTP options on the
  dual-path rule (`NESTRS_MCP__*` over a pinned base). `McpModule` configures;
  it activates nothing — listing the `#[mcp]` provider is still what mounts an
  endpoint. A registered `dyn SessionStore` is picked up for rmcp 3.x
  cross-instance session recovery.
- **Security note for existing deployments.** rmcp 3.x validates the inbound
  `Host` header against a **loopback-only** allowlist by default
  (anti-DNS-rebinding). A server reached under a real hostname answers `421`
  until `NESTRS_MCP__ALLOWED_HOSTS` names it. That default is deliberately not
  widened by the framework.
- **`rmcp` leaves every consumer manifest.** Its macros expand to bare `rmcp::`
  paths resolved against the *call site's* scope, so `use nest_rs::mcp::rmcp;`
  supplies the name — the claim that a re-export "cannot supply it" was wrong,
  and the entry is gone from `demo/crates/features`, `nest-rs-authz` and
  `nest-rs-testing`. `nest-rs-macro-hygiene` now compiles a full tools **and**
  prompts host, with typed input, on its single `nest-rs` dependency.
- `endpoint_with_guard` is replaced by `endpoint(McpMount, factory)`;
  `McpMount::from_container` is the one place the mount resolves its guard, data
  context, config and session store. `OperationOutcome` carries a type-erased
  `OperationValue`, which is what lets one `around` wrap every capability.

### The server half of OAuth discovery — RFC 9728

An app protected by a bearer token could refuse a caller but never tell it where
to go get one. The MCP authorization spec makes that a **MUST**, and HTTP and WS
are resource servers on exactly the same terms, so the capability lives in
`nest-rs-authn` and serves all of them at once.

- **`ProtectedResourceModule::for_root(..)`** serves
  `GET /.well-known/oauth-protected-resource` (RFC 9728 §3) and stamps
  `WWW-Authenticate: Bearer resource_metadata="…"` — plus `scope` when the
  deployment advertises one — onto every `401` the process emits. The route is
  declared `#[public]`, because a client cannot hold a token before reading the
  document that says where to obtain it.
- **One seam, three transports.** The challenge is attached at the transport
  edge rather than inside `AuthError`, so it covers the guard-denial `401`, the
  WS upgrade refusal, `/mcp`'s in-band denial (`EdgePosture::Exempt` skips
  guards, not this band) and `401`s the framework never wrote itself.
  `/graphql` is the deliberate exception: it answers an unauthenticated
  operation with `200` + an `UNAUTHENTICATED` frame, so its clients discover
  through the well-known document — the equal alternative the spec defines.
- **`NESTRS_AUTHN__AUDIENCE` becomes mandatory** under this module, checked at
  `on_module_init` so import order cannot skip it, and boot fails naming the
  variable. Without it a resource server accepts any token its issuer signed,
  including one a user granted to another service — the confused deputy RFC 8707
  exists to close. A `resource` that disagrees with `aud` warns at boot.
- **`ProtectedResourceConfig`** is dual-path (`NESTRS_AUTHN__RESOURCE`,
  `__AUTHORIZATION_SERVERS`, `__SCOPES_SUPPORTED`, … over a pinned base) and
  refuses a non-canonical identity at boot: no scheme, a fragment, an empty
  authorization-server list, or a scope carrying a space are all build breaks
  rather than a document that misleads a client.
- **`McpConfig::allowed_origins` is gone.** The browser `Origin` control was a
  second knob for something the HTTP transport already owns:
  `NESTRS_HTTP__CORS_ORIGINS` rejects a disallowed origin with `403` on every
  method, and the CORS layer wraps the whole route tree, so `/mcp` inherits it.
  Set the origin allowlist there; `allowed_hosts` stays, being the
  anti-DNS-rebinding control with no transport-wide equivalent.
- `crates/nest-rs-authn/tests/integration/resource/controller.rs` is the
  conformance proof, in the spirit of `propagate.rs`: a client that knows
  only a protected URL walks `401` → `resource_metadata` → the metadata document
  → the authorization server's own metadata, every hop a real request, and the
  AS is discovered by falling through the RFC 8414 §3.1 priority order.

### The client half of OAuth discovery — scopes and the step-up refusal

Discovery told a tokenless client where to get a token. It said nothing to the
client that *had* one and was merely delegated too little — a bare `403` whose
only recovery was guesswork. That is the case MCP made ordinary, so the scope
becomes a first-class dimension of a rule and of a denial.

- **`.requires_scope("posts:read")` on an ability rule.** One declaration,
  three effects, and no second decision site: the rule is **withheld** when the
  credential does not carry the scope — not added at all, so the class gate, the
  query pre-filter and the response mask refuse together exactly as for a rule
  nobody wrote — the refusal remembers the scope, and the scope stays readable
  beside the permission it conditions. Scopes **narrow, never widen**: an admin
  token minted without `posts:write` cannot write posts, and no scope grants
  what the role denies. Call it more than once to require all of them.
- **`PrincipalIdentity::scopes()`**, defaulted, is how a credential reports what
  it carries. `None` — every existing principal — means *not scope-aware*, so
  scoped rules apply in full and a session-authenticated app is untouched;
  `Some(&[])` means *an OAuth credential delegated nothing*. `AuthnGuard`
  publishes the result as `nest_rs_guards::GrantedScopes`, which is how authn
  informs authz without either crate depending on the other.
- **`nest_rs::authn::scope::space_delimited`** parses the RFC 6749 §3.3 `scope`
  claim — accepting the array form several authorization servers emit, because
  the deployment does not choose its AS's spelling. Getting this wrong by hand
  yields one scope named `"posts:read posts:write"` that matches nothing and
  looks like an authorization bug.
- **`Denial::InsufficientScope`** is distinct from `Forbidden` for the reason
  RFC 6750 §3.1 separates them: the first is actionable, the second final. It
  reaches the edge as `RequiredScopes` on the response, where the same
  interceptor that writes the `401` pointer renders
  `WWW-Authenticate: Bearer error="insufficient_scope", …, scope="posts:write"`
  for HTTP, WS and MCP alike. An ordinary `403` still carries no challenge —
  advertising a recovery that cannot succeed is worse than a plain refusal.
- **GraphQL stops being the transport that learns less.** It has no `401` to
  enrich, but a scope refusal is an ordinary error frame, so it carries
  `code: "INSUFFICIENT_SCOPE"` and a structural `requiredScopes` list.
- **`insufficient_scope_challenge` had no caller.** It was public, tested, and
  dead — the `403` half of the RFC was declared and never wired. It is now the
  single renderer of that challenge.
- A scope a rule requires but `NESTRS_AUTHN__SCOPES_SUPPORTED` omits is a dead
  end for the client; it is reported at `warn` (`reason="scope_not_advertised"`)
  at the one point both halves are known.

### Fixed — the poem `Err` path dropped a denial's evidence

`Error::from_response(denial_to_http_response(d))` reads as a faithful
conversion and is not: poem's `into_response` ends with
`*resp.extensions_mut() = self.extensions`, overwriting whatever the carried
response held. Every denial travelling the `Err` path — the MCP ability bridge,
the global-pool MCP fallback, the `Authorize` extractor — therefore reached the
edge stripped of its extensions, at the moment a client was being refused.
`nest_rs_guards::denial_to_http_error` is now the one conversion, and the trap
is documented at the function that replaces it.

### Fixed — the well-known document ignored a path-carrying resource

RFC 9728 §3.1 inserts the well-known string **between the authority and the
resource's path**, so `https://api.example.com/mcp` publishes at
`…/.well-known/oauth-protected-resource/mcp`. The document was hung off the
origin instead, which is the URL a *different* resource sharing that host would
claim. Both forms are now served — the path-aware one is advertised, the
unsuffixed one stays for bare-origin resources and clients that skip the
challenge — and a tail that is not this resource's path answers `404` rather
than asserting an identity the deployment does not have.

### The mechanism

- `nest-rs-codegen::reroot` resolves how the call site reaches the framework —
  the umbrella for an app, the sibling crate inside the framework's own 14
  crates, which cannot depend on their own facade — and re-roots the finished
  expansion. Path literals included, so `#[serde(crate = "…")]` follows.
- The umbrella's feature matrix now pulls **everything a capability's decorators
  emit unconditionally**. `features = ["mcp"]` alone previously left
  `nest-rs-guards/mcp` off, so the documented global-guard fallback never ran.
- `nest-rs-macro-hygiene` is down to **one dependency** and gained `#[mcp]`
  coverage; it is the compile-time witness the rule names.

### Fixed

- `nest_rs::testing::EphemeralDatabase` was unreachable through the umbrella —
  the `seaorm` feature now forwards `nest-rs-testing/orm`.
- `/websockets/server-push/` imported `nest_rs_schedule::every`, which is an
  inner attribute of `#[scheduled]` and never an item. The page had never
  compiled as written.

### Tooling and product

- `nestrs new` and `nestrs g <transport>` write the umbrella and its features;
  the generator's crate tables became feature lists.
- The Publish demo consumes the framework through **one** workspace dependency,
  down from 27.
- The docs page templates now prescribe the one-line form, so a new page cannot
  reintroduce the old shape.

## [1.3.0] - 2026-07-31

A clean-room QA campaign against the **published** 1.2.0 — crates.io releases and
the live docs site, never the repository — filed 71 findings: 3 blockers, 27
major (4 security-relevant), 41 documentation defects. Every finding is closed
below, each with the test that keeps it closed.

### Security

- **`NESTRS_STORAGE__ALLOW_HTTP=false` did not cover presigned URLs.** Signing is
  a local computation, so `object_store`'s own plain-HTTP gate never saw it: a
  production app minted working `http://` URLs carrying the SigV4 signature, on
  the flow `/storage/` calls canonical. An `http://` endpoint with plain HTTP
  disallowed is now refused **at config load**, naming the variable, so no
  transfer can be attempted and no plaintext URL can be signed; the signing path
  keeps a second check for a hand-built `Storage::new`. It was also a
  request-time `500` with nothing logged at any level — a mis-deployed app
  started healthy and passed its liveness probe.
- **`#[public]` on an OAuth2 callback turned a forged-callback `401` into a
  `500`.** The authn guard absorbs a rejected credential on a public route by
  design, which left the handler with no principal and `Ctx<Claims>` answering a
  server error — indistinguishable from a bug, and invisible to any alert, WAF
  rule or rate limit keyed on `401`. The rejection is now recorded on the
  request, and a handler that goes on to need the principal answers the deferred
  `401`. A public route that never needs one still serves.
- **A `401` produced by a guard carried no `WWW-Authenticate` challenge**, which
  RFC 9110 §11.6.1 and RFC 6750 §3 require. Only a handler-returned `AuthError`
  set it; the guard path — the one the JWT page documents — did not. Every `401`
  the framework renders now carries it, and only a `401`.
- **`/http/file-uploads/` inverted its own caveat**: it said `MAX_BODY_BYTES`
  does *not* gate `Multipart` and told readers to compensate. It does. The page
  taught operators to build a control they already had, and developers to expect
  a large direct upload to work when it `413`s at 2 MiB by default.

### Fixed

- **The `.env` cascade outranked a value pinned in `for_root`** in every
  scaffolded app — the exact inverse of the documented tier, stated in three
  places and in the generated `.env`. `Environment::init` publishes the cascade
  into `std::env` so raw `std::env::var` consumers see it, which erased the one
  distinction the deployment tier rests on. The published names are now recorded
  and subtracted from `ConfigSource::get_from_deployment`, so `real env > pinned
  in code > .env cascade` holds in a scaffolded app exactly as in a library-only
  one.
- **An unreachable `NESTRS_QUEUE__URL` blocked boot forever with zero output** —
  never healthy, never crashed, and silent at `RUST_LOG=trace`, the worst shape
  for a container platform. The connect is now bounded by
  `NESTRS_QUEUE__CONNECT_TIMEOUT_SECS` (10 s default, `0` rejected), warns per
  attempt on `nest_rs::queue`, and fails with the redacted endpoint and the knob
  that widens it.
- **`timestamps` never bumped `updated_at`.** Create ran `ActiveModelBehavior`
  through `ActiveModelTrait::insert`; update went through the query builder that
  the scope filter forces, which does not. Every resource with the flag on
  silently froze the column downstream caches, incremental sync and ETags trust.
  `Repo::update` now drives the hooks explicitly, keeping the scope filter.
- **`QueueModule::for_root` never bound `Arc<dyn JobProducer>`**, so the portable
  injection form both the queue and the driver-authoring pages prescribe compiled
  and then died at boot. Both names now resolve from the one connection.
- **A global interceptor did not run on 404s or 405s**, contradicting three doc
  statements and skipping exactly the traffic a request-id or audit interceptor
  exists to record. The router answers an unmatched path with `Err`, which
  short-circuited the documented `next.run(req).await?` body; the transport now
  renders it once the global filter pool has had its turn, so the interceptor
  bands genuinely see a response.
- **GraphQL validation errors carried no `extensions` at all**, so a client could
  not tell which field was wrong while the HTTP twin named them. Every rejection
  site now renders through one helper: `extensions.errors`, the same member name
  HTTP, WS and the queue's dead-letter event use.
- **The `fields` extension on a masking denial is a list**, not a comma-joined
  string — the natural reading of "names in the `fields` extension", and the only
  shape that survives more than one refused field.
- **A malformed id on the GraphQL bind path leaked the `uuid` crate's parse
  string** with no code. Both malformed branches now answer
  `"id must be a UUID v7"` with `INVALID_ARGUMENT`.
- **A WebSocket pipe rejection, a malformed payload and an unknown event logged
  nothing** — exactly backwards, since a client sending garbage is the case worth
  seeing. All three now `warn` on `nest_rs::ws` beside the frame.
- **A set-but-unparseable `NESTRS_OPENTELEMETRY__*` value was swallowed.** `0`
  stays the documented sentinel; a typo is reported on stderr naming the
  variable.
- **A config validation failure dropped the namespace and leaked `validator`'s
  raw debug payload** — including the rejected value — into an operator-facing
  line. It now reads `configuration validation failed for '<namespace>'` with one
  `- field: rule (bound = n)` line each, bounds kept and the submitted value
  stripped.
- **`Storage` gained `delete`** (absent keys succeed, so retention sweeps and
  failed-upload cleanup are idempotent), `put_bytes` takes anything
  `Into<Bytes>` so the read/write round-trip composes without a copy, and
  `StorageError` converts into `std::io::Error` so `get_stream` feeds
  `Body::from_bytes_stream` as the streaming page shows.
- **Swapping `Bind`'s type parameters** (the 1.1.x order) reported two unrelated
  bound failures against the `#[crud]` attribute. Both traits now carry an
  `on_unimplemented` note naming the order and the swap, snapshotted by trybuild.

### CLI

- `nestrs g ws|schedule|mcp` over a `g resource` port emitted `self.svc.count()`,
  which a `CrudService` does not have — so the page's "any adapter compiles
  immediately" guarantee broke, with rustc blaming `Iterator::count`. Every
  transport now has a CRUD twin.
- `nestrs g queue` omitted `nest-rs-redis`, `nestrs g mcp` omitted `schemars`
  and `nest-rs-guards`' `mcp` feature (without which the documented global-pool
  fallback for `/mcp` is never seeded), and `nestrs g graphql` left the app
  crate — whose `module.rs` it edits — without `nest-rs-graphql`.
- `g resource` and `g migration` disagreed about the same table: the migration
  scaffolded `created_at`/`updated_at`/`deleted_at` and the entity declared none,
  so the resource hard-deleted against a tombstone column it never wrote. The
  entity now carries `soft_delete, timestamps` and the columns, matching the
  `users/` exemplar.
- The scaffolded smoke test booted the **app root**, so wiring a resource the way
  `g resource` instructs made the no-infrastructure suite need Postgres and fail
  on a 30 s pool timeout. It boots the feature's own module now.
- `nestrs doctor` read only `std::env`, reporting `not set` for a variable the
  workspace's own generated `.env` defines — the exact mistake
  `/database/migrations/` warns tool authors against.
- `--dry-run` printed `Created feature …` directly above `no files written`;
  `--version` / `-V` were rejected with `unexpected argument`; and the scaffold
  now pins `validator` to the major the framework compiles against.

### Documentation

Forty-one defects across the site, from stale samples (`TestApp::<M>::builder()`,
a pre-RFC-9457 validation body, a `debug`-vs-`warn` contradiction on one page) to
whole sections that could not be followed: the "grow a standalone crate into a
workspace" walkthrough clobbered the scaffold's own files and ended in a
duplicate-controller boot failure, and Social login documented two routes that
exist in no framework crate.

The install stanzas got the sweep 1.2.0 gave GraphQL, WS, MCP and Queue: `/http/`
needs `nest-rs-guards`, `/configuration/` needs `validator`, `/database/` needs
`schemars` and `validator`, `/mcp/` needs `schemars`. The `nest-rs` umbrella is
documented for what it is — version alignment and a prelude, not a substitute for
the crates a decorator's expansion names, which Rust's lack of a transitive
extern prelude makes impossible.

## [1.2.0] - 2026-07-30

Twenty-nine findings from two read-throughs. The first opened the GraphQL surface
(five findings, two security-relevant); the second worked down the untested list
into queue, schedule, events, WebSockets, MCP, rate limiting, health,
OpenTelemetry and GraphQL relations (twenty-four, three security-relevant). Each
is closed with the check that keeps it closed: new suites in nine crates, eight
CLI tests, live-Postgres e2e coverage, and two new greps (`bind-order`,
`queue-name`) in `docs/scripts/lint-docs.mjs`. One reported finding did not
reproduce and is now pinned so it cannot start (the ability-less read over
WebSockets already warns — an in-process test and an e2e both assert it).

`nest-rs-testing` gains **`LogCapture`**, because a third of the second round was
about what the framework *said*: a denial that fails closed but logs nothing, a
dead-lettered job with no event, a warning filed under the wrong target. Those
lines are what an operator queries during an incident, and they now have the same
coverage as a status code.

A third campaign then ran the unpublished 1.2.0 end to end — a fresh
`nestrs new` project, real HTTP requests, raw RFC6455 frames against a live
server, live Redis — and found sixteen more findings, four security-relevant.
Fifteen are closed below, several by product decision rather than patch. The one
left open is upstream: apalis-redis's orphan sweep makes a starting replica
re-run a peer's in-flight jobs, so queue delivery is documented as
**at-least-once**, measured, and pinned by an e2e test written to fail loudly
the day the upstream fix lands.

### Security

- **On GraphQL, `#[authorize]` did not require authentication.** `/graphql` is
  one endpoint carrying the `Public` marker — the authn guard admits an
  anonymous caller so `#[public]` operations stay reachable, and the ability
  guard then hands the operation the *visitor* ability
  (`AbilityFactory::define_visitor`). The class gate consulted only the grants,
  so a visitor grant added to serve a public feed also satisfied every
  `#[authorize]` operation on that entity — while the review contract of
  `define_visitor` is that a grant there reaches `#[public]` surfaces *only*,
  and the diff a reviewer reads shows only `#[public]` routes. The ability now
  carries whether a principal backs it (`Ability::is_visitor`, set by the
  guard's visitor branch through `AbilityBuilder::build_visitor`), and the
  GraphQL gate refuses the anonymous caller with `UNAUTHENTICATED` before
  looking at a single grant. HTTP is unchanged: a non-`#[public]` route never
  reaches the visitor branch, so the marker is still what selects the policy
  half there.

- **A guard denial at the WebSocket upgrade logged nothing, at any level.** The
  client saw the right refusal — 401/403 as `problem+json`, no socket opened,
  no `on_connect` — but `GuardEndpoint::call` converted the denial without
  passing through `deny_http`, the one site carrying the "every denial visible
  at `warn`+" floor, so a token-less socket sweep was invisible in supervision.
  The per-route HTTP and GraphQL paths already went through the floor; the
  upgrade path now does too, and a suite asserts both the `warn` and its
  absence on an allowed request.

### Changed

- **`WsModule` owns every connection registry, namespaced or not.** A
  `#[gateway(namespace = N)]` used to provide `WsServer<N>` from its own
  `Discoverable::register`, so the key belonged to no module: the access graph
  admitted any consumer through the imperative-registration escape hatch, and
  registration order decided whether the provider existed at all — a service
  living in a module the gateway imports was constructed before the registry
  existed and panicked at mount, naming the wrong provider. A namespaced
  gateway now submits a link-time `WsNamespaceEntry`; `WsNamespaces`, a
  `WsModule` provider, drains it and installs each `WsServer<N>`; and the new
  `Discoverable::also_provides` hook declares those keys to the graph, which
  attributes them to `WsModule`. One rule for `Global` and namespaced alike:
  the registry comes from `WsModule`, and the import is what the graph
  verifies. **Breaking:** a namespaced gateway's module must import `WsModule`
  — the boot error names it verbatim.

- **A `Valid<T>` rejection says which field, on every transport.** The
  field-level detail travelled in `PipeError`'s `details` and both async
  transports threw it away: a WS error frame said only
  `validation failed`, and a dead-lettered job carried nothing at all — read
  in a log days later by someone who cannot replay it. The error frame now
  carries `{error, errors}` under `data` (`errors` absent, not `null`, when
  the rejection has no structured detail — the same shape HTTP uses), through
  `WsReply::pipe_error` as the single site for both the payload pipe and the
  global data pipe; and `job dead-lettered` logs the detail under an `errors`
  field. **Wire-contract change** for WS clients: keep branching on
  `data.error`, read `data.errors` to point at a field.

- **A pinned config field no longer freezes its neighbours.** `nestrs new`
  writes `HttpConfig { port: 3000, ..Default::default() }`, and
  `provide_feature` served that literal as the whole config — pinning every
  field and making `NESTRS_HTTP__*` inert, silently, against the dual-path
  rule the configuration docs promise. Resolution is per-field now, strongest
  first: real environment > pinned code > `.env` cascade >
  `Config::defaults()`. One body (`Config::from_env(env, base)`) serves the
  pinned and unpinned paths, so no field can be reachable from only one side;
  `ConfigSource::get_from_deployment` isolates the tier that outranks a pin —
  `EnvSource` restricts it to the real process environment, so a committed
  `.env` reads as another default and yields to the pin while a deployment
  variable always wins, and a third-party source (Vault, a ConfigMap) is
  deployment-tier by default, the safe direction for a secret. The one
  remaining hard pin is the builder seed (`App::builder().provide(cfg)`),
  documented as such.

### Removed

- **`concurrency` on `#[process]` — it never capped anything.** The value only
  sized the Redis read buffer, so a handler declared `concurrency = 2` ran ten
  jobs at once (measured: `peak=10` while the boot line announced
  `concurrency=2`). Rather than fix the knob, the decision removes it: nestrs
  targets the container, so a `#[process]` method runs **one job at a time**
  (`WorkerBuilderExt::concurrency(1)`, read buffer of 1 so a waiting job stays
  in Redis where another replica can claim it) and scale comes from replicas —
  the unit the platform already schedules, measures and restarts. A
  `#[process(concurrency = N)]` is refused by name with the replacement
  spelled out, not as an unknown key; a live-Redis e2e pins `peak == 1` while
  still requiring progress; and `tower` drops back to a dev-dependency of
  `nest-rs-redis`.

### Fixed

- **A field-level grant took an entity offline over GraphQL.** `.fields([...])`
  strips a column, and the GraphQL wrapper had to hand the masked value back as
  the operation's own type — which a non-null schema field cannot express, so
  *every* query on that entity failed, including one asking only for granted
  columns. Its HTTP twin served the same rows masked. The mask now follows
  GraphQL's own rule: a stripped column masks to `null` where the field is
  nullable, and where it is not, the selection set decides — an operation that
  **selects** the column is refused (`FORBIDDEN`, names in the `fields`
  extension), one that does not is served. Rows the ability refuses are dropped
  either way, and nothing unmasked ever ships.

- **`nestrs g graphql <feature>` generated code that did not compile**, from two
  independent causes. `#[resolver]` expands to
  `nest_rs_guards::{GraphqlChainCell, GraphqlChainSources,
  run_layered_graphql_chain}`, which sit behind that crate's `graphql` feature —
  and because `nest-rs-guards` is already a dependency of every scaffolded
  workspace, the generator had to enable the *feature*, not add the entry
  (`ensure_features_deps` now widens an existing entry). And over a `g resource`
  port the scaffold called `svc.count()`, a method `CrudService` does not have:
  a resource now takes the `#[crud]` resolver behind `AuthnGuard` +
  `AuthzGuard`, the twin of the HTTP controller `g resource` already writes, and
  its entity gains the `#[expose(graphql)]` flag that makes it a GraphQL object.

- **`AuthzGraphqlModule` was required but never scaffolded.** The generated
  resolver's own comment told the reader to import it, while
  `g resource` / `g auth` wrote `authz/http/` only and no command wrote the
  GraphQL bridge — leaving three providers (`AppGraphqlGuard`,
  `GraphqlAuthnGuard`, `LoaderScope`) to be reconstructed from prose.
  `g graphql` now writes `authz/graphql/` when the workspace has a policy to
  enforce, imports it from the adapter's `module.rs`, and lists it at the app's
  composition site.

- **`Bind` / `bind` generic order was inverted throughout the docs.** The real
  signatures put the **action first** (`Bind<Read, UsersService>`,
  `bind::<Read, UsersService>`, `Authorized<Update, PostEntity>`); roughly a
  dozen places wrote the reverse, and `/security/authorization/by-id-binding/`
  stated the rule backwards in prose. Fixed across every page and gated by a new
  `bind-order` check in the docs linter.

- **A panicking event listener destroyed the emitter's request.** The events page
  promises "failure is local"; a panic was contained to the *process*, not the
  listener — it abandoned the dispatch chain mid-way, unwound through `emit` into
  the emitter, and on HTTP took the response with it. The client saw a dropped
  connection rather than a 500, with the emitter's side effects already
  committed, so a retry re-ran them. Any `unwrap()` in a fire-and-forget reaction
  (`email_the_author`, `index_for_search`) was therefore a way to break an
  unrelated write path. Each listener now runs under `catch_unwind`: the panic is
  logged at `error` on `nest_rs::events` with the event type and the panic
  message, and the chain continues.

- **A `Result` reached through a type alias shipped the error struct as a success
  frame over WebSockets.** `#[subscribe_message]` read the return type's last
  path segment to decide whether a handler could fail, so
  `pub type ServiceResult<T> = Result<T, MyError>` read as an ordinary value and
  the `Err` variant was serialized straight into the reply `data` — every field
  of the error, including ones `Display` deliberately withholds, in a frame with
  no `error` key and no server-side `warn`, because nothing knew a failure had
  happened. It compiled without a warning, and only in codebases with typed
  `Serialize` errors: the ones whose errors carry the most detail. The decision
  is now made on the **type** (`ReplyValue`, inherent-impl specialization), so
  however the return is spelled an `Err` becomes the same error frame and the
  same `warn` on `nest_rs::ws`.

- **A per-message WebSocket guard denial was logged under the wrong target.** It
  landed on `nest_rs::layers`, which carries events about the layer *system*; an
  operator tailing `nest_rs::ws=warn` for denials — the filtering every other
  page teaches — saw nothing. Now on `nest_rs::ws`, beside the rest of the
  transport's events.

- **A panicking queue job was dead-lettered in silence.** `CatchPanicLayer`
  contained it correctly — the job failed, the worker survived, the next job ran
  — but it unwound past the per-job span, so the whole field set (`queue`,
  `processor`, `job_id`, `attempt`) and every event were skipped. The only trace
  was the default Rust panic hook on stderr: no target, no fields, no span, and
  nothing at all at the docs' own production filter (`nest_rs::queue=warn`),
  while a deserialization failure on the same worker reported properly. The panic
  is now caught inside the span and reported as
  `job dead-lettered: handler panicked` at `error`. The outcome is unchanged.

- **Listener dispatch order was link order, not declaration order.** The events
  page guarantees "the order their providers appear in `providers = [...]`, then
  the order their methods appear in the `#[listeners]` block". `inventory` hands
  entries back in link order — stable per binary and reshuffled by any change to
  the code, which is the worst shape a guarantee can have: three methods declared
  `first, second, third` dispatched `2, 3, 1`, a second provider's listener
  landed *between* two of the first's, and two listeners ordered deliberately got
  silently rearranged the next time somebody added a third. `#[on_event]` now
  submits its position in its block, `nest-rs-core` seeds a `ProviderOrder` from
  the module walk, and `EventsModule` sorts on the pair.

- **Two controllers in one file could not share a handler name.** `#[routes]`
  emits one module-level type per handler and derived its name from the method
  alone, so `V1Controller::ping` and `V2Controller::ping` collided in a namespace
  neither knew it shared — breaking the layout the versioning page prescribes,
  and `list` / `get` / `create` besides. The symbol is now qualified by the
  controller.

- **A missing `WsModule` panicked the app after it had already mounted the
  gateway.** The connection registry was resolved with an `.expect(...)` at
  mount, so the app compiled, logged `mounted endpoint kind="ws"`, and *then*
  died with a backtrace note — where every other boot-time misconfiguration
  exits cleanly. A gateway now declares the registry as a dependency, so the
  access graph refuses the boot naming both the missing type and the module that
  provides it. (A namespaced gateway self-provided its own registry at that
  point; the live campaign moved ownership to `WsModule` — see *Changed*.)

- **`ThrottlerGuard` could not be wired the way the page describes.** The two
  documented steps — import `ThrottlerModule::for_root(None)`, bind
  `#[use_guards(ThrottlerGuard)]` — failed the boot: `#[use_guards]` puts the
  guard under the access contract, so the *controller's* module owed a provider
  for it, and a dynamic (`for_root`) import contributes only global
  infrastructure and could never satisfy it. `ThrottlerModule` (and its Redis
  twin, through one shared `provide_guard`) now registers the guard alongside the
  store it reads. **Breaking for an app that worked around this** by listing
  `ThrottlerGuard` in `providers`: that is now a duplicate registration and fails
  the boot naming it — remove the line.

- **An attribute-bound layer no module provides was reported as
  `<unnamed dependency>`.** A guard, filter or interceptor is reached by
  `Container::get::<P>` rather than an `#[inject]` field, and the access graph's
  names list covered only the fields — so every layer fell off the end of it and
  printed as a placeholder, *including in the suggested fix*. The framework's
  best wiring diagnostic was unusable for exactly the things wired as `dyn`.
  `#[controller]`, `#[resolver]`, `#[gateway]`, `#[routes]`, `#[messages]` and
  the resolver impl now emit index-aligned labels.

- **GraphQL relations answered `database error` with no `DbErr` and no SQL.** The
  flagship "relations resolve themselves" feature failed wholesale on a wiring
  gap nothing announced: `batch_spawner` fell back to a bare `tokio::spawn` when
  no `dyn GraphqlBatchContext` was registered, and a batch on a fresh task has no
  ambient executor, so `Repo` failed before a single statement reached the
  database. Schema build now warns when loaders are seeded with no batch context,
  naming the binding (`LoaderScope as dyn GraphqlBatchContext`), and
  `Repo::conn`'s missing-executor error is logged at `error` on `nest_rs::orm`
  with every context that installs one — because the wire form of
  `ServiceError::Db` is the constant `database error` and carries nothing an
  operator can act on.

- **A failing health indicator's error was reachable from nowhere.** The probe
  body reports a fixed `"check failed"` — deliberate, since `/health/*` is
  routinely unauthenticated and an `anyhow` chain from a connection check carries
  a DSN or an internal hostname — but the field's own rustdoc and the indicators
  page both promised the stringified error. The three now agree: the detail goes
  to a `warn` on `nest_rs::health`, and a test pins both directions.

- **The skipped-indicator notice fired on every probe.** A linked-but-unreachable
  indicator is a startup fact about the module tree; repeating it per request
  turned a wiring notice into production log volume, and no other discovery seam
  does that. Named once at boot now, at `warn`, matching `nest_rs::queue` and
  `nest_rs::events`; the docs said `debug` and are corrected.

- **The metric export interval was reachable from nowhere.** All three OTel
  signals arrive, but metrics wait ~60 s while traces and logs land immediately —
  so the standard first check (wire a meter, hit the route, look at the
  collector) shows zero metrics for a full minute and reads as a broken pipeline.
  The SDK's default was in neither the env table nor `OpenTelemetryConfig`, so it
  could not be shortened for a local run either. Now
  `metric_interval` / `NESTRS_OPENTELEMETRY__METRIC_INTERVAL_SECS`, dual-path
  like every other field, with the 60 s default named as
  `DEFAULT_METRIC_INTERVAL`.

- **`#[process]` obliged its call site to depend on `nest-rs-worker`.** The
  expansion emitted bare `::nest_rs_worker::` paths, which resolve against the
  *consumer's* extern prelude — so writing a processor needed a crate named
  nowhere in the docs (`nest-rs-worker` appears once in the whole set, as an
  "ambient job context seam"), and the first `cargo check` after
  `nestrs g queue` was `could not find nest_rs_worker`. `nest-rs-queue`
  re-exports it and the macro routes through that, as its own module docs already
  claimed. `nest-rs-queue`'s integration suite declares no `nest-rs-worker`,
  which is what keeps it closed.

- **`async_trait` was re-exported by three surface crates and not by the four
  layer crates.** `nest-rs-http` / `-queue` / `-ws` did; `-interceptors`,
  `-filters`, `-exception-filters` and `-guards` did not, so the one import a
  reader needed most was the one no page could name — and the miss cascades
  (without the attribute every trait method reports a lifetime mismatch, so the
  real cause hides under four unrelated errors). All seven now do.

- **`#[expose(…, graphql)]` required async-graphql features the consumer had to
  discover one error at a time.** The macro re-emits a column's own type into the
  generated `InputObject`, and an entity's columns are `Uuid` and `DateTime*` by
  construction — so a foreign key exposed as an input failed with
  `the trait bound uuid::Uuid: InputType is not satisfied`, pointing at a field
  whose type the developer never chose. `nest-rs-resource` declares `uuid` and
  `chrono` on its optional `async-graphql`, since it is the crate whose macro
  creates the requirement.

- **A controller and a self-mounted endpoint on one path panicked poem instead
  of failing the boot.** The exclusivity rule ran inside each family only —
  `prefix_owner` for controllers, `endpoint_owner` for self-mounts, two maps
  never crossed — so `#[controller(path = "/chat")]` plus
  `#[gateway(path = "/chat")]` passed both checks, logged both mounts as
  successful, and then hit poem's `duplicate path` panic the code's own comment
  promised to catch. One combined check refuses at boot; and the collision
  message names the owners (`ChatGateway`), not just the kinds — "a ws endpoint
  and a ws endpoint both mount there" is unusable with five gateways in play.

- **A gateway binding its own guards was reported as an unguarded self-mount
  edge.** The predicate consulted only the presence of a global guard pool, so
  `#[use_guards(TicketGuard)]` on the gateway — verified working, 401/403 at
  the upgrade — still drew the boot warning, with a hint recommending exactly
  what was already done: a security signal an operator learns to ignore.
  `HttpEndpointMeta` now carries the self-mount's own posture; a gateway with
  no guard at all is still reported.

- **Destructured handler arguments failed to compile on HTTP and GraphQL.**
  `#[routes]` and `#[resolver]` forwarded each argument to the generated
  wrapper by name, and a pattern has none — so the idiomatic poem forms the
  docs print (`Path(name): Path<String>`, `Query(q)`, `Json(body)`) were
  rejected with "must be simple identifiers", while `#[messages]` and
  `#[process]`, which forward by position, accepted them. The wrapper now
  forwards under the one identifier the pattern binds — the method keeps its
  pattern, only the wrapper's parameter list is normalized, and the name comes
  from the pattern because on GraphQL it *is* the SDL argument name. `Valid<T>`
  becomes destructurable (`pub` newtype field — exposing it grants nothing;
  `Authorized<A, E>` stays sealed, that proof guards data access). A pattern
  binding zero or several names keeps a named error, pinned by a trybuild
  snapshot; one suite drives all four transports against a single app.

- **`#[input]` did not derive `JsonSchema`.** `#[routes]` documents every
  `Json<T>` / `Query<T>` argument in the OpenAPI document, so an extractor DTO
  must implement `schemars::JsonSchema` — and the decorator whose whole role
  is absorbing input-DTO boilerplate left that one derive out. The failure was
  an unsatisfied-bound error pointing at `schema_of`, naming neither the
  missing derive nor the DTO. `#[input]` derives it now, and a test pins the
  full derive set so a future edit cannot drop one.

### CLI

- **`nestrs g queue` generated code that did not compile, and the `tracing` half
  hit three generators.** `g queue`, `g schedule` and `g ws` all write a
  `tracing::` call into the handler body while adding only their own `nest-rs-*`
  crate, and a workspace scaffolded by `nestrs new` carries no `tracing` in its
  features crate. `a_skeleton_that_names_a_crate_declares_it` derives the
  requirement from the template text, so a skeleton that starts logging drags its
  dependency along on the same commit.

- **`nestrs g mcp` pinned `rmcp 1.7` while `nest-rs-mcp` builds against 2.2.**
  Two majors in one graph put two `ServerHandler` traits in scope and every
  `#[tool_handler]` method mismatched; pinning harder made it worse. The
  generator's line is now read against the workspace manifest by
  `the_rmcp_pin_matches_the_frameworks_own`, so bumping the framework's `rmcp`
  fails there until the generator follows.

- **`nestrs g ws` omitted `nest-rs-guards`' `ws` feature.** `#[messages]` expands
  to `GuardAsWsMessageCheck`, which that feature gates — and the miss was worse
  than an ordinary one: `cargo check -p features` failed while
  `cargo check --workspace` passed, because a dev-dependency elsewhere in the
  graph unified the feature in.

- **`nestrs g queue` hid its own `#[queue]` marker.** The generator declared it
  in the adapter's private `processor` module and did not re-export it, so
  `push_to::<Q>` — the enqueue path the crate designates as the default — was
  unreachable even from the feature's own service, leaving the untyped
  `push(name, job)` escape hatch as the only way to enqueue. The marker now sits
  at the port beside the payload, where `QueueName`'s own docs say it belongs.

- **`nestrs g queue`'s module imported nothing.** That held only while the
  processor stayed the inert stub; give it the shape the Queue page prescribes and
  the worker died at boot on an access violation. Every adapter module imports
  its port now, as `g http` / `g ws` / `g schedule` already did.

- **`nestrs g ws`'s module omitted `WsModule`**, so following the generator's own
  "Next steps" produced an app that mounted the gateway and then failed to boot.

- **`nestrs g ws <feature>` wrote `path = "/ws"` for every feature**, so the
  second WS adapter collided with the first at boot — right after the
  generator's own "Next steps" said to import it. The path derives from the
  feature name now, as the HTTP twin always did.

- **`nestrs g http <feature>` emitted a route with no posture** — the only one
  of the three route generators (`new`, `g graphql`, `g http`) whose output
  booted straight into the unguarded-routes warning. It writes `#[public]`
  with the same `// SECURITY:` comment as the GraphQL template.

- **A typed WS payload needed a `serde` the generator did not write.**
  `nest_rs_ws` re-exports `serde_json` only, so the first
  `#[derive(serde::Deserialize)]` payload — the messages page's normal case —
  failed to compile until `serde` was added by hand. `g ws` writes it, and the
  install stanza on the WebSockets page explains why it is on the list.

### Documentation

- **The whole `/queue/` section described a pre-`QueueName` API.** Every
  `#[process]` example named its queue with a string — a form the shipped macro
  rejects — while `#[queue(name = …, job = …)]` and `QueueName` appeared in no
  prose page at all, so the only place a reader met the real API was the
  generator output. The producer half was worse because it *compiled*:
  `/queue/producing-jobs/` taught `queue.of::<T>(AUDIO_QUEUE)`, the
  runtime-name escape hatch, and called the string "the only stringly-typed
  coupling" — the exact coupling the type removed. Rewritten across the section
  and gated by a new `queue-name` check in the docs linter.

- **Failed jobs land in `<queue>:dead`, not `<queue>:failed`.** The page tells
  readers to inspect and replay that set themselves, so the key name is the one
  detail an operator actually types. Two neighbours corrected with it: a
  deserialization failure is non-retryable and dead-letters on the first attempt
  rather than burning the budget, and every failed attempt *including the
  terminal one* logs `will retry within the budget` — read the `attempt` field,
  not the message.

- **Three snippets imported `async_trait` from `poem`, which does not export
  it**, and the global-interceptor snippet omitted
  `AppBuilderInterceptorsExt`.

- **Standalone mode loses every generator, not the two the CLI page named**, and
  the landing page's "grow into a workspace when you add apps" was promised in
  one sentence and explained nowhere. Both corrected, with the growth path
  written out as a recipe.

- **Per-section dependency stanzas** on `/queue/`, `/mcp/`, `/graphql/` and
  `/websockets/`. Every section opened with `cargo add <one-crate>`, and in
  several cases that did not compile the page's own first example — the real set
  was discovered one compiler error at a time. The stanzas also give the
  generators a spec to match, rather than leaving the generator and the docs as
  two independent guesses at the same list.

- **The decorators index pointed `#[on_connect]` / `#[on_disconnect]` at
  Messages**; both are documented on Rooms.

- **The trailing-slash trap**, on the controllers page: `#[get("/")]` under
  `#[controller(path = "/greetings")]` serves `/greetings`, and the
  trailing-slash form is a 404 that still carries a global interceptor's headers
  — convincing enough to read as a broken feature. The boot line is
  authoritative.

- **The sources page described a cascade mechanism the code does not have.** It
  said `EnvSource` triggers the `.env` cascade merge on its first `get`,
  writing into `std::env` under a `Once` — the code parses the cascade into a
  crate-internal map and never mutates the process environment on a read;
  `Environment::init` is the only publisher, called on the first line of every
  scaffolded `main` for consumers that only know `std::env::var`. The page's
  conclusion was right for the wrong reason, and its advice sent readers to
  distrust calls with no side effect. Rewritten around the real mechanism,
  three derived claims on other pages corrected, and the *actual* hermeticity
  trap documented: the first parse freezes the working directory and
  `NESTRS_ENV` that chose the files, so a test that moves either afterwards
  resolves against the previous test's cascade. Two new tests pin the one
  claim nothing covered.

- **Twelve snippets used destructured arguments that did not compile** (the
  macro fix above makes the poem-idiom six compile as printed), **and three of
  them were structurally wrong regardless**: `Valid(Json(input))` never holds
  a `Json`, and `Piped(id)` cannot be a pattern (`Piped` carries a
  `PhantomData`, and a public type projection just so a pattern works is not
  worth it) — those read `Valid(input)` and `id: Piped<…>` + `*id` now. Three
  more `#[input]` snippets relied on the `JsonSchema` derive the decorator was
  missing.

- **Two env keys were absent from the reference** — `NESTRS_HTTP__COMPRESSION`
  and `NESTRS_STORAGE__ALLOW_HTTP`, the second security-relevant — plus the
  seven OAuth2 keys of the `authn` namespace. Established by extracting every
  key each `from_env` reads and diffing against the page, across all nine
  namespaces; key counts in prose ("all fifteen keys") are gone, since a count
  rusts at the first added option.

- **Four dead internal anchors repointed**, detected by diffing every
  `](/page/#anchor)` against the built HTML's `id=` set; one had the right
  label on the wrong page. Every internal anchor in the docs now resolves.

- **`/queue/` gains a "One job at a time" section** documenting the
  concurrency decision and its two consequences (head-of-line blocking is per
  queue, no prefetch), and an idempotent-handlers aside stating the
  at-least-once contract with its cause — a starting replica re-runs a peer's
  in-flight jobs, upstream in apalis-redis — and a pointer to the e2e test
  that pins both halves of the replica behaviour.

## [1.1.1] - 2026-07-27

Six findings from the 1.1.0 read-through, each closed with the check that keeps
it closed — the generator defect is now a unit test, the scaffold wording an
integration assertion, and the three documentation classes are greps in
`docs/scripts/lint-docs.mjs`, which gates the whole corpus (the baseline is
empty).

### Fixed

- **`nestrs g migration` names the table, not the migration.** The skeleton's
  `DeriveIden` enum was rendered from the migration name, and `DeriveIden`
  snake-cases the enum straight into the DDL — so `g migration create_widgets`
  created a `create_widgets` table while the entity `g resource widgets` had
  just written read `widget`. `db up` reported success and the first query
  failed. The enum is now derived from the *subject* of the name
  (`create_widgets` → `Widget`, `add_status_to_posts` → `Post`,
  `drop_orgs_table` → `Org`), the emitted comment names the table it writes, and
  the CLI's next-steps print it.

- **The scaffolded `.env.example` says where a test override goes.** It sent
  developers to `.env.local`, which the cascade skips under `NESTRS_ENV=test` by
  design — so a machine-specific database override was silently ignored by
  `nestrs run test e2e`, and the failure named a connection rather than the
  ignored file. It now points at `.env.test.local` and says why.

- **The tutorial no longer promises unauthenticated CRUD.** The index and
  `/tutorial/validation/` still curled a guarded controller with no bearer and
  documented the pre-guard responses; a reader following them verbatim got a
  `401` where the page showed a `201` or the `400` it was teaching. Both now
  carry the token, and the index says guards arrive with the database on page 4
  instead of listing them as a step-8 addition. `/server-timing/` and
  `/rate-limiting/`, which the same check caught, carry it too.

- **Six documentation snippets that could not compile, and taught the wrong
  layer while failing to.** `/security/authorization/public-reads/`,
  `/http/versioning/`, `/security/authorization/response-masking/`,
  `/security/authorization/by-id-binding/` and `/server-timing/` all `?`-ed a
  `CrudService` read directly from a handler: those yield `Result<_, DbErr>`, and
  `DbErr` is no `ResponseError`. The fix is the exemplar's shape rather than a
  `map_err` at the route — a service method returns the **wire type** (as
  `PostsService::create_in_org` already does), so a hand-written handler is a
  one-line delegation and the `Model` conversion stays in the service. The
  by-id pages keep their `Access` → status match: mapping `Found`/`Denied`/
  `Missing` onto 200/403/404 is transport work, and it is what those pages are
  about.

- **Stale `nest-rs* = "1.0"` pins** on `/tutorial/entity/`, `/database/` and
  `/packages/`, one release behind what `nestrs g resource` writes.

## [1.1.0] - 2026-07-26

Fixes from a crash-test of the 1.0.0 release: building an app by following the
documentation end to end, on a pristine `nestrs new` scaffold. A minor rather
than a patch — `nestrs g auth` is new, `g resource` emits a different slice, and
two flags are gone (`g resource --guarded`, `new --template`).

### Added

- **Every `nestrs new` layout ships the same `hello` module** — a service with a
  greeting and one `#[public] GET /` that returns it. Previously only two of the
  four generation paths mounted a route: `nestrs new blog` **inside** a
  workspace produced an app with an empty route table, and both `--template
  empty` variants did too — while the CLI's own next-steps told you to open a
  browser at a URL that answered `404`. A freshly created project has to prove
  it started, and a `404` proves nothing to the developer looking at it.

  Workspace mode writes the greeting as a feature named after the app
  (`crates/features/src/blog/`), because the layout keeps no `service.rs` /
  `controller.rs` in an app crate; standalone writes the same two files under
  `src/`. `nestrs new <name>` now refuses when a feature already owns that name,
  rather than overwriting product code.

- **`nestrs g auth`** — the app-side authn/authz adapter (`Claims`,
  `AuthnGuard`, `AppAbility`, `AuthzGuard`, and their modules) that roughly ten
  documentation pages referenced and nothing generated. The framework is
  generic over the principal and the policy, so these types cannot ship in a
  `nest-rs-*` crate; every workspace wrote the same eight files by hand, from
  crate sources, or not at all.

- **`AbilityFactory::define_visitor`** — the anonymous branch of an app's
  policy, consulted by `AbilityGuard` on a `#[public]` route. A DB-backed
  resource anyone may read was previously not expressible: the public branch
  installed an ability built from an *empty* `AbilityBuilder` and never asked
  the app's factory, so `Authorize` answered `403` and `Repo` filtered every
  row — whatever the developer wrote. The new method defaults to granting
  nothing, so an app that does not implement it behaves exactly as before, and
  a route opened with `#[public]` still exposes nothing until a rule is
  written. One correction covers HTTP, GraphQL and the WebSocket upgrade: all
  three run `AbilityGuard::check_http`. `/mcp` deliberately carries no `Public`
  marker and keeps refusing anonymous callers.

  **The reach of `#[public]` grows with this**: the marker now selects which
  half of the policy runs, so reviewing a diff that adds it means reading
  `define_visitor` too. Documented in
  [Public reads](https://nestrs.dev/security/authorization/public-reads/).

  Additive under semver, with one exception worth naming: an app that already
  has an **inherent** `define_visitor` method on its `AppAbility` would see the
  inherent one win at every call site, and the trait method silently keep its
  empty default. The name is new, so the risk is close to zero — but it is a
  real shadowing rule, not a rounding error.

- **A malformed rule on a `#[public]` route now fails closed.** The public
  branch used `unwrap_or_default()`, degrading a rule the builder rejected into
  a deny-all ability — indistinguishable, to the caller, from an ordinary empty
  result. It goes through the same `match` as the authenticated branch and
  answers `Denial::internal`.

- **`nestrs new` scaffolds `crates/migrations/` and `crates/seed/`**, with the
  `migrate` binary behind every `nestrs run db …` verb. `nestrs g migration`
  bootstraps them for a workspace scaffolded before this.

- **`AuthError` and `CredentialError` implement poem's `ResponseError`**, so a
  handler can `?`-propagate them as the exception-filter documentation
  describes. `AuthError::Unavailable` keeps its distinct `500`.

### Removed

- **`nestrs new --template`.** With one starter that always serves `/`, the flag
  had one remaining value (`empty`) whose only effect was a project answering
  `404` on its first page. One way to do a thing.

### Changed

- **`nestrs g resource` emits the guarded `#[crud]` form** and scaffolds the
  auth adapter when the workspace has none; **`--guarded` is removed** — it is
  the only shape now. The unguarded slice it used to emit compiled but could
  not serve a single row: `Repo` filters every read by the caller's ambient
  `Ability`, which only an `AbilityGuard` installs, so every route answered
  `500` (missing ability) or read an empty table forever.

- **`Environment::init()` merges the `.env` cascade into `std::env`**, as its
  documentation always said. Without it the scaffold's own
  `NESTRS_LOG` / `NESTRS_LOG_FORMAT` / `NESTRS_LOG_SOURCE_LOCATION` in
  `.env.development` were inert, and a `migrate`-style binary reading
  `std::env::var("NESTRS_DATABASE__URL")` found nothing. It writes through
  `set_var`, so the documented obligation stands: call it at the top of `main`,
  never from a task.

- **`#[crud]`'s error mapping moved into `nest_rs_seaorm::crud_error`.** The
  status mapping is unchanged (409 on a unique violation, 403 on the ability
  re-check's `RecordNotInserted`, 404 on a vanished row); it is now one
  implementation instead of one copy per controller, and it logs the unexpected
  `DbErr` it turns into an empty-bodied 500.

### Fixed

- **A `500` from the authz or ORM path is no longer silent.** `Authorize` logs
  at `error` on `nest_rs::authz` when no ability guard ran, naming the action,
  the subject and the fix; `ServiceError`'s opaque variants log their cause at
  `error` on `nest_rs::orm` when they become a 5xx. Diagnosing one used to mean
  a custom debug handler and reading three crates' sources — at `trace`, a `500`
  produced zero records.

- **`g resource` injects the dependencies the decorators expand to** —
  `schemars` (`#[expose]` derives `JsonSchema`) and `nest-rs-authz` (`#[crud]`
  emits `Authorize<A, E>` parameters). Without them the first `cargo check`
  after generating was a wall of macro-expansion errors, invisible to
  `cargo check` on the scaffold itself.

- **`nestrs run db up` and `db seed` work on a fresh workspace.** Every
  `db.just` recipe named the `migrations` and `seed` crates, and neither
  existed.

- **`nestrs run test unit` and `test e2e` work on a fresh scaffold.** Both
  filter on `binary(e2e)`, and nextest rejects a filterset naming a binary the
  workspace does not have — so every app is now scaffolded with an empty
  `tests/e2e/main.rs`, which the docs already claimed.

- **The scaffolded smoke test compiles.** It called
  `TestAppBuilder::with_test_telemetry`, which lives behind
  `nest-rs-testing`'s optional `opentelemetry` feature. The scaffold imports no
  `OpenTelemetryModule`, so the call is simply gone.

- **The `sea-orm` pin the generator writes is `2.0`**, not the `2.0.0-rc.38`
  release-candidate floor, and its feature list matches what `nest-rs-seaorm`
  itself resolves.

- **Scaffold polish**: the hello route is `#[public]`, so a first run no longer
  greets you with the framework warning about its own template; workspace mode
  no longer writes a `.dockerignore` it ships no `Dockerfile` for; standalone
  mode no longer ships database recipes that need a workspace; and the
  generated `README.md` links resolve outside nestrs.dev.

### Documentation

- The tutorial carries the two guards from the HTTP page onward, and
  `/database/` states plainly that no row crosses the data layer without an
  ability — the previous narrative was not reproducible.
- Tutorial page 1's checkpoint is a `200 Hello World` instead of a `404` it
  taught you to expect, and `/cli/`'s template table is replaced by the one
  starter.
- 18 further corrections: wrong imports (`ServiceError` is in `nest_rs_seaorm`),
  the missing `AbilityGuard` import path, the undocumented `connect_from_env`,
  the two contradictory `Migrator` locations, `PATCH`'s whole-body semantics,
  the exact validation-error body, the scaffolded file tree, and the boot log
  lines.

## [1.0.0] - 2026-07-25

A handful of crates *are* the framework's public surface — their types appear
in signatures the macros emit. Their majors are tied to the nestrs major and
are frozen within 1.x: `poem = "3"`, `sea-orm = "=2.0"`,
`async-graphql = "=7.2.1"`, `rmcp = "2.2"`, `inventory = "0.3"`,
`validator = "0.20"`, `schemars = "1"`. sea-orm and async-graphql are
exact-pinned (not caret) because the ORM bounds and the GraphQL registry
codegen read enough of their surface that even a *minor* can shift generated
code.

### Changed

- **`nest_rs_redis::ConnectionError` is now `RedisError`** (re-exported at the
  crate root). A generic infra-error name collides in an app that imports
  several backends' error types; the house pattern is concern-prefixed
  (`ConfigError`, `StorageError`, `QueueError`), and Redis was the last one out
  of step. Rename the import; the variants and fail-closed semantics are
  unchanged.

- **`DatabaseConfig::retry_serialization_conflicts` is now
  `observe_serialization_conflicts`** (env
  `NESTRS_DATABASE__OBSERVE_SERIALIZATION_CONFLICTS`). The flag never retried
  — it tags a commit-time conflict (`40001` / `40P01` / `1213` / `1205`) as a
  structured `warn` on `nest_rs::orm` so contention is distinguishable from a
  generic commit error. The old name promised a transparency the framework
  deliberately does not offer: replaying a conflict means re-running the whole
  handler, and a handler may already have pushed a job, emitted an event, or
  written an object — none of which roll back with the transaction. Retrying
  stays the service's decision, at a boundary it knows is replayable
  (`nest_rs_seaorm::retry::retry_on_conflict`). Renamed before the freeze
  because a config key is public surface for the whole `1.x` line.

- **`AuthGuard` is now `AuthnGuard`** (`nest_rs_authn::AuthnGuard<S>`). It was
  the only half of the pair not carrying its concern's suffix, so
  `#[use_guards(AuthGuard, AuthzGuard)]` read as if the two guards answered
  different kinds of question. They don't: one establishes *who* (authn), the
  other *what they may do* (authz). Rename the import; nothing else changes.
  `OAuthGuard` and other `OAuth*` names are untouched — that `Auth` is OAuth's.

- **Social providers activate from configuration, not from a per-provider
  module import.** Importing `SocialModule` is now the whole wiring step: it
  owns every registry entry, so it is the module gate, and inside that gate
  each linked provider turns on when its credentials are set. A provider with
  no credentials is inert with one boot `warn` (its routes `404` like an
  unknown key); a *partially* set or invalid one **fails boot naming the
  provider**, so a half-configured login is never silently dropped.
  - `GithubSocialProviderModule` / `GoogleSocialProviderModule` and their
    `Setup` types are **removed**. Delete those imports; pin config the
    ordinary way by providing a `GithubSocialConfig` / `GoogleSocialConfig`
    value, which still wins over the environment.
  - `SocialProviders` is renamed **`SocialRegistry`** — it is the registry, not
    the providers.
  - `SocialProviderEntry` gains `env_namespace` and `build` (normally one
    `resolve_provider` call) and drops `provider_type_id` / `resolve`. A
    third-party provider crate is now two files, `config.rs` + `provider.rs`,
    with no module to write: a social provider is never `#[inject]`ed by type,
    so it has nothing for a module of its own to own.

- **`nestrs new` scaffolds its smoke test into `tests/integration/`** (it
  boots `TestApp` in process, no live infra — so it now runs on every
  `nestrs run test unit` instead of hiding behind the `binary(e2e)` gate).
  The scaffolded `e2e` recipe carries `--no-tests=pass` until the project
  adds a real e2e suite.

- **Capability-only guards are the documented pattern for non-CRUD routes**
  (`authn-authz.md`): a route whose response is not an entity row gates
  through a custom `Guard` checking the ability imperatively, bound via
  `#[use_guards]` — `Authorize<A, S>` would arm response masking against a
  body that is no wire model. Exemplar: `audio`'s `TranscodeGuard`.

- Third-party pins consolidated in `[workspace.dependencies]` (`redis`,
  `clap`, `toml_edit`, `tempfile`, `tower`, `libc`; `tokio-tungstenite` in
  the demo workspace). `nest-rs-redis` names `redis::RedisError` /
  `redis::aio::ConnectionManager` through the `redis` crate directly —
  apalis stays an implementation detail. Dead framework deps dropped from
  the demo apps' manifests; the worker enables the OTel `http` feature it
  actually serves.

### Added

- **`sea_orm` and `rmcp` are re-exported from their surface crates**
  (`nest_rs_seaorm::sea_orm`, `nest_rs_mcp::rmcp`), the way `nest-rs-http`
  already re-exports `poem` and `nest-rs-graphql` re-exports `async_graphql`. A
  consumer no longer carries its own `sea-orm` dependency and hand-mirrors the
  framework's exact `=2.0` pin — the lockstep version travels with the
  framework. (rmcp's `#[tool*]` macros still expand to a crate-relative `rmcp::`
  path, so a crate that *hosts* a tool keeps a direct `rmcp` dependency for that
  expansion; the re-export covers every other use.)

- **`nest_rs_ws::Scoped<T>`** resolves an `#[injectable(scope = request)]`
  provider from inside a WebSocket message handler, opening a fresh request
  scope per inbound message — the same seam the per-message guards already run
  on. This closes the four-transport parity: HTTP, GraphQL and MCP already had
  `Scoped`.

- **`#[wire_default(...)]`** (`nest-rs-resource`) — an auditable opt-in
  placeholder for an unexposed column whose type the response-masking
  reconstruction cannot default on its own (a custom enum, `Uuid`, timestamp,
  `Decimal`). Without it such a column fails the masked round-trip closed (a
  `500`); with it the reconstruction succeeds and the placeholder is stripped by
  the static expose set before the body ships — so it never reaches the wire.
  Sound only for a column no ability rule predicates on, and the macro rejects
  it on an exposed, PK or relation field. This is what lets a strict DB-backed
  enum stand in for a hidden `String` column: an unknown stored value then fails
  to load rather than being silently coerced to a default.

- **Ambient request state now reaches an MCP tool body — `Repo` works on MCP.**
  rmcp dispatches every tool call on its own spawned task, so the request
  scope, ambient executor and ambient ability installed around the endpoint
  never reached a tool. The new `PropagatingHandler` closes that gap: the
  endpoint stashes the state in the HTTP request extensions, rmcp forwards them
  as `http::request::Parts` into the operation's `RequestContext`, and the
  handler re-installs them *inside* the dispatch. A tool method now resolves
  `Scoped<T>` and reads through `Repo` with the caller's row filter applied —
  the same transparency HTTP and GraphQL have, with no filtering written in the
  tool.
  - New `McpToolContext` seam (`nest-rs-mcp`) with the first-party
    `nest_rs_seaorm::McpDataContext` behind seaorm's new `mcp` feature —
    the MCP twin of `WsDataContext`. It installs a **lazy** per-operation
    transaction: a read-only tool opens none, a writing tool commits on success
    and rolls back on error. `AuthzMcpModule` provides it.
  - Without a registered `McpToolContext` a `Repo`-backed tool still fails
    **closed and loud**, never unscoped.
  - `endpoint_with_guard` takes the context as a second argument (the `#[mcp]`
    macro resolves it from the container; hand-written call sites pass `None`).

- **MCP reaches the security sub-layer through the same wiring as GraphQL.**
  Both transports are `EdgePosture::Exempt` and gate in-band, but only
  `/graphql` had the surrounding seams; `/mcp` now has all of them, so the two
  answer identically to one app wiring.
  - **The global guard pool reaches `/mcp`.** With no `dyn McpOperationGuard`
    registered, the endpoint folds the `use_guards_global(...)` chain in-band
    (`FallbackMcpGuard` + `nest_rs_guards::GlobalPoolMcpGuard`, behind guards'
    new `mcp` feature) instead of going straight to deny-all. A global
    `ThrottlerGuard` now rate-limits a tool call — it previously could not.
    The fallback only ever *widens* what the app declared: with no pool (or an
    empty one) `/mcp` stays deny-all, and unlike `/graphql` it carries no
    `Public` marker, so a pooled `AuthnGuard` still refuses an anonymous call.
  - **`McpOperationGuard` gained `capture` + `around`** (both defaulted, so
    existing impls are unaffected): snapshot on the request, install *inside*
    rmcp's spawned dispatch — the same split `McpToolContext` already used for
    the same crossing. `McpAbilityBridge` implements them, so the **guard**
    installs the caller's ambient `Ability` on both transports and a tool body
    is now scoped even when the app registers no `McpToolContext`.
  - **One authn→authz chain.** `nest_rs_authz::run_ability_chain` holds the
    ordering once; each bridge only maps the resulting `Denial` into its own
    transport error. Side effect: an MCP denial keeps its status, so a `429`
    from a throttler in the chain reaches the client as `429` with its
    `Retry-After` instead of a flattened `401`.

- **Three test suites that never ran now run.** `cargo nextest run --workspace`
  builds every member with its *default* features, which silently excluded a
  large part of the framework's own coverage: `nest-rs-authz`'s http / graphql /
  mcp bridge tests compiled away behind `#[cfg(feature = …)]`, and the
  `nest-rs-seaorm` and `nest-rs-redis` e2e targets were skipped outright
  because their `required-features` were unsatisfied. Each crate now carries a
  path-only **self dev-dependency** that turns its own features on for its test
  targets (dev-deps do not propagate, and Cargo strips them from the published
  manifest). The workspace-wide `-E 'binary(e2e)'` step went from 1 test to 21.
  - Enabling them surfaced two real defects, both fixed: the `nest-rs-seaorm`
    e2e harness `expect`ed `NESTRS_DATABASE__URL` instead of defaulting to the
    dev container like its `nest-rs-redis` / `nest-rs-storage` siblings, and its
    shared probe tables were guarded by a per-*process* `OnceCell` while nextest
    runs each test in its own process — so a fresh database raced
    `CREATE TABLE IF NOT EXISTS` against the Postgres catalog. The DDL now
    serializes on a transaction advisory lock.

- **Two macro diagnostics are pinned by compile-fail snapshots.** Arming the
  `#[routes]` response shaper with a type that only *borrows* the
  `Authorize`/`Bind` name now has a `trybuild` fixture, as does a
  `for_root(...)` value that is not `Send` (the bound `#[module]`'s
  construct-once dynamic imports introduced). Both errors were already
  emitted; neither was guarded against silent regression.

- **`Repo::insert_unscoped`** — the write pendant of `Repo::unscoped()`, on
  an explicit connection, for pre-principal provisioning (social login) and
  principal-less system work. The social-login inserts and the slug
  uniqueness probe now route through `Repo`, so "every data access lives in
  `Repo`" holds by construction; each escape documents its bar in rustdoc.

### Fixed

- **A primary-key-less entity no longer panics on the data hot path.** `Repo`'s
  query and mutation paths `expect`ed at least one primary-key column, so a user
  modeling a view or a keyless table hit a mid-request panic — in the layer
  whose written contract is "never panic, return `DbErr`". Both sites now return
  a typed error naming the entity, logged at `error`.

- **`nestrs g mcp` scaffolds compiling code again**: the MCP tool template
  imported `Content`, an rmcp 1.x alias renamed `ContentBlock` in 2.x — the
  generated file could not compile.
- **The configured OpenTelemetry `service.name` now wins**: the SDK's
  `SdkProvidedResourceDetector` always supplies a `service.name` (env override
  or the `unknown_service:*` sentinel) and `with_schema_url` merged it *over*
  the configured attrs; `build_resource` now applies the config after the
  detector merge, with a regression test.
- **`nest-rs-testing` decides `NESTRS_ENV` before any `.env` read**: the
  set-if-absent `NESTRS_ENV=test` default moved from `TestAppBuilder::new`
  into `load_project_env`'s `Once`, so a db-first harness
  (`EphemeralDatabase::create` before `TestApp::builder`) no longer loads
  `.env.local` and skips `.env.test.local`. `nestrs run test e2e` works on
  bare metal again.
- **Macro path hygiene**: `#[hooks]` emitted a bare `::anyhow` path,
  `#[gateway]` a bare `::tracing`, `#[messages]` a bare `::nest_rs_http`, and
  the http/resource macros bare `::poem`/`::serde_json`/`::tracing`/
  `::async_trait` — all now route through their surface crate's re-exports
  (`nest_rs_core::anyhow` is new), so a downstream app without those direct
  deps compiles. Proven by the new `nest-rs-macro-hygiene` witness crate
  (workspace-internal, `publish = false`), which consumes the decorators with
  zero third-party dependencies.
- **`Authorization: basic` (any case) is accepted**: `basic_credentials` now
  matches the scheme case-insensitively per RFC 7235, mirroring
  `bearer_token`.
- **GraphQL/WS guard denials always log at `warn`**: the layered chain
  runners emit the same structural floor as HTTP's `deny_http`, so a custom
  guard that denies silently can no longer create an unobservable denial.
- Assorted robustness: the health endpoints return 500 instead of an empty
  body when the report fails to serialize; the authz predicate downcast,
  password-timing dummy, response-masking defaults, pagination-cursor header
  and conflict-retry exhaustion no longer `expect`/panic on request paths
  (each fails closed or degrades with a logged error); a broken `JobContext`
  is attributed to the new `nest_rs::worker` target instead of
  `nest_rs::queue`.

### Removed

- **`nest_rs_authz::mcp::masked_output`.** It was a one-line delegation to
  `nest_rs_authz::masked_output_ambient` — two public names for one behaviour,
  against *one way to do a thing*. Call `masked_output_ambient` directly; the
  signature and the fail-closed semantics are unchanged.

- **The unfinished offset-pagination surface.** `PageArgs`, the `<Name>Page`
  envelope emitted by `#[expose(..., paginate)]`, the `paginate` flag itself,
  and the `paginate = page` mode of `#[crud]` are all gone. The mode was never
  wired — both transports answered it with a compile error — so the types
  documented a capability the framework refused to generate. Keyset
  (`paginate = cursor`, the default) and `paginate = none` are the whole knob;
  a consumer that genuinely needs page numbers plus a total hand-writes that
  operation on its service. No caller in either workspace was affected.

- **`demo/.env.example`, and the `.env.local` the devcontainer seeded from it.**
  The demo now commits its whole configuration in `.env` + `.env.development`:
  it holds no real secret (its signing key is the dev keypair already committed
  for the test suites, its OAuth credentials are fixtures), so the git-ignored
  half had nothing legitimate to carry. It carried a `<REPLACE-ME>` placeholder
  instead, which the `postCreateCommand` copied into every fresh container and
  which the `auth` app refused to boot on. `git clone` then `nestrs run` now
  works with nothing to prime. Existing clones can delete their `demo/.env.local`
  — and their `demo/.env.test.local`, which pinned `localhost` backend URLs that
  no process inside the devcontainer can reach. The secret-handling pattern is
  unchanged where it belongs: `nestrs new` still scaffolds `.env.example` next
  to a git-ignored `.env.local`.

### Known for the 1.x line

- **`Guard::check_http` sits on the base `Guard` trait**, so `nest-rs-guards`
  depends unconditionally on `poem` and `nest-rs-http` — a queue-only binary
  still compiles the HTTP stack. Build hygiene, with no runtime, security or
  correctness effect; moving it to an extension trait touches every guard impl,
  HTTP dispatch and the boot chain-validation, so it lands in `2.0`.

## [0.5.0] - 2026-07-19

### Changed

- **WS message handlers are transactional.** `WsDataContext` installs the
  same lazy executor per message: a read-only or non-querying message opens
  no transaction, while a writing handler commits on a success reply and
  rolls back on an error reply — a multi-write handler that fails mid-way no
  longer half-persists. Guest connections stay fail-closed (deny-all without
  an ambient ability).
- **Mutating HTTP requests no longer pay `BEGIN`/`ROLLBACK` before guards
  run.** `DbContext` now installs a lazy executor (`Executor::Lazy`): the
  request transaction opens on the **first data-layer touch**, so a
  guard-denied POST — or any mutating request that never queries — opens
  zero transactions and consumes no Postgres transaction slot. Commit /
  rollback semantics, the `MappedError` rollback, and the escaped-executor
  fail-loud check are unchanged.
- **`Creatable::create` is atomic on every executor shape.** On a pool
  executor (a WS message handler, a bare `with_executor`) the insert and its
  SQL scope re-check now run in a local transaction — an out-of-scope create
  surfaces `RecordNotInserted` and persists nothing, instead of relying on
  the HTTP request transaction for the rollback.
- **`ThrottlerStore::hit` is async.** The Redis store awaits its round-trip
  on the request task instead of parking a runtime worker with
  `block_in_place` + `block_on` per rate-limit check (which also panicked on
  a current-thread runtime). Fail-closed behavior on a Redis outage is
  unchanged.
- **Guard chains are validated at boot from declared markers.** `Guard` gains
  `phase()` (authentication / authorization / other) and
  `produced_principal()` / `expected_principal()`. A chain listing authz
  before authn, or pairing an `AuthGuard` whose principal type differs from
  the `AbilityGuard`'s expected actor, now **fails boot with a named error**
  instead of answering 500 on every request; the old name-substring ordering
  heuristic is gone.

- **Response masking is cross-checked at run time.** `#[routes]` arms the
  response shaper by matching the `Authorize`/`Bind` parameter-type name; a
  renamed import (`use Authorize as Az`) used to disarm masking silently.
  Unarmed routes now carry a `MaskProbe`: when a masking extractor runs on a
  route whose shaper is not armed, the request fails closed with a logged
  `500` instead of shipping an unmasked body.
- **`Bind` / GraphQL `bind` no longer echo `DbErr` text to the client.** A
  failed by-id load logs the full error at `error` on `nest_rs::orm` and
  answers with an empty `500` (HTTP) / a generic `INTERNAL_SERVER_ERROR`
  extension (GraphQL), matching the `#[crud]` write mapper.

### Added

- **`nest_rs_authz::masked_reply`** — mask a handler's wire JSON with the
  ambient ability in one call, for surfaces with no automatic response
  shaper (a WS gateway reply, a hand-built payload). Same fail-closed core
  as the HTTP shaper and the GraphQL wrapper; the reference `users` WS
  gateway now uses it instead of ten hand-rolled masking lines.
- **`Creatable::create_from_active`** — insert a *prepared* `ActiveModel`
  through the same audited create path as `Creatable::create` (atomic
  insert + SQL scope re-check), for service methods that stamp server-side
  columns (the token's org id, a status default) before insert. The demo's
  users/posts services now use it instead of raw
  `ActiveModel::insert(&Repo::conn()?)`.

### Removed

- **Reserved cross-transport layer seams that were never invoked.**
  `Interceptor::wrap_graphql`/`wrap_ws` (with `GraphqlNext`/`WsNext`),
  `ExceptionFilter::catch_graphql`/`catch_ws`, and
  `Filter::filter_graphql`/`filter_ws` compiled but no macro or dispatcher
  ever called them — implementing one was a silent no-op. They are removed
  from the trait surfaces (along with the now-empty `graphql`/`ws` features
  of `nest-rs-interceptors`, `nest-rs-exception-filters`, and
  `nest-rs-filters`) until real wiring ships. Guards' cross-transport
  entries are unaffected; a global interceptor/filter still covers GraphQL
  and WS through the HTTP transport edge.

## [0.4.0] - 2026-07-19

### Changed

- **One error format at the HTTP boundary — RFC 9457
  `application/problem+json` everywhere.** Three shapes previously
  coexisted: the NestJS-style `{statusCode, error, message, details}`
  validation body, bare-text framework/service errors, and poem's
  plain-text transport errors (an unmounted-route `404`, a `413`). All
  now render as `ProblemDetails` (`type`/`title`/`status`, optional
  `detail`). Field-level validation errors ride as the RFC-9457
  **extension member** `errors`; `ServiceError`, guard denials
  (401/403/429, `Retry-After` preserved) and pipe rejections all map to
  the same envelope. `HttpTransport` installs a transport-edge boundary
  (`nest_rs_http::normalize_error_response`) that lifts any leftover
  raw plain-text error onto `problem+json` — a `Filter`/`ExceptionFilter`
  mapping (tagged `MappedError`) or a deliberately-typed body is left
  untouched, and internal (`5xx`) detail is dropped so no driver message
  reaches the wire. New `ProblemDetails::from_status` /
  `with_extension`.

### Added

- **The OpenAPI document is complete.** Previously skeletal — no query
  parameters, every path parameter a bare `string`, no security scheme,
  a lone `200` per operation. The generated document now carries: path
  parameters typed from the handler's `Path<T>` (a `Path<Uuid>` id is
  `string`/`format: uuid`), each `Query<T>` payload expanded into one
  query parameter per property (the `#[crud]` list op's `first`/`after`
  cursor is documented), a `bearerAuth` security scheme applied to
  guarded non-`#[public]` routes — including routes covered only by a
  `use_guards_global` pool — and per-route RFC 9457 error responses
  (401/403/404/409/422, each honest to what the route can produce)
  referencing a shared `ProblemDetails` schema. A new
  `NESTRS_OPENAPI__EMIT_DOCUMENT`/`DOCUMENT_PATH` config writes the
  document to disk at boot, the OpenAPI analogue of the GraphQL SDL
  emit, so a committed `openapi.json` stays fresh as a side effect of a
  dev run.

- **`HttpConfig.compression`** negotiates response compression (gzip /
  deflate / brotli / zstd) from each request's `Accept-Encoding` — one
  flag (`NESTRS_HTTP__COMPRESSION` or the pinned struct), off by default
  so a fronting proxy keeps ownership when it has it. A preflight
  (`OPTIONS`, no body) and an already-encoded response are left alone.

- **`Storage::get_stream`** downloads an object as a chunked byte stream
  instead of buffering the whole body ([`get_bytes`] collects), so a
  large media file flows to the client without ever sitting whole in
  process memory — feed it straight into a streamed HTTP body.

- **Streaming and multipart HTTP** are now first-class: poem's `sse`,
  `multipart` and `compression` features are enabled, so a handler can
  return `poem::web::sse::SSE` or a `Body::from_bytes_stream` response,
  or take a `poem::web::Multipart` upload, and `#[routes]` passes each
  through untouched. The demo's `audio` slice exercises all three
  (direct upload, streamed download, an SSE progress feed).

- **`nestrs g migration <name>`** scaffolds a SeaORM migration and
  registers it in **both** `crates/migrations/src/lib.rs` and
  `migrator.rs` — the `migrator.rs` vec is regenerated from the module
  list, so the two registrations can never drift (the one you forget by
  hand is the one that silently never runs).

- **`nestrs g resource --guarded`** scaffolds the hardened `#[crud]` +
  guards form (the `orgs/` shape) instead of the unguarded stub, for a
  workspace that already provides `AuthGuard` / `AuthzGuard` /
  `AuthzHttpModule`.

### Fixed

- **A duplicated concrete provider fails the boot.** Two modules (or a
  seed and a module) registering the same concrete type previously
  warned and silently last-write-wins — a wiring mistake that only
  surfaced as wrong behaviour. It now fails the boot with a named
  `DuplicateProviderError`, uniform with the other wiring checks. Keyed
  providers keep their documented last-write-wins, and `dyn Trait`
  bindings stay the intended override mechanism.

- **A missing `Ctx<T>` replies with a bare 500, not the Rust type.** The
  extractor built the response body from the internal Rust type name;
  that detail now goes to the logs and the client gets a bare 500.

- **A malformed relational rule fails ability construction instead of
  going fail-open.** `PredicateBuilder::related` rejects an invalid
  relation (composite key, or a relation not pointing at the declared
  related entity) with the `Deny` sentinel. In a `cannot(...)` that
  sentinel lowered to `1 = 0` and combined as `grant AND NOT(1 = 0)` —
  i.e. the restriction evaporated (fail-*open*). `AbilityBuilder::build`
  now returns `Result<Ability, MalformedRuleError>` and fails naming the
  faulty rule; the HTTP ability guard denies the request (fail-closed)
  when construction fails. A malformed grant, previously a silent
  deny-all, is surfaced the same way.

- **A scoped/transient provider's missing dependency fails the boot,
  not the first request.** The access graph only flagged *cross-module*
  reaches; a request-scoped or transient provider whose dependency was
  provided by no module at all passed boot and panicked at its first
  `get(...)` resolution — a runtime panic in place of the framework's
  hallmark named boot diagnostic. Lazily-built providers now report the
  names of what they inject, and the access-graph pass fails boot with a
  `MissingDependencyError` naming both the provider and the unmet
  dependency. A dependency provided imperatively (a hand-written
  `impl Module`) or by a lazy factory is still tolerated: the pass
  consults the actual registered set before declaring a dependency unmet.

- **An eagerly-built provider's missing dependency no longer panics
  before the graph check.** The synchronous register phase ran ahead of
  `validate_from_inventory`, so a missing dependency panicked with the
  generated `expect` message and preempted the named `AccessGraphError`.
  Construction now defers the miss to the graph pass, which reports the
  same unified `MissingDependencyError`; a genuine dependency cycle still
  panics with its cycle diagnostic naming the chain.

- **`#[crud]` writes return the right HTTP status.** A generated create
  / update / delete previously mapped every write failure to a blanket
  `500`, so a unique-constraint violation, an out-of-scope create the
  ability re-check rolled back, or a row that vanished mid-request all
  read as internal errors. The generated handlers now map a
  `DbErr` to its status — unique violation → `409`, `RecordNotInserted`
  → `403`, `RecordNotUpdated` / `RecordNotFound` → `404` — and a
  genuinely unexpected error to a `500` with an empty body (the driver
  message no longer leaks). A service with a manual create maps the
  unique violation to `ServiceError::conflict` for the same result.

- **Auto-resolved `has_many` relations are memory-bounded.** An
  `#[expose]`d `has_many` field's dataloader previously loaded *every*
  child of a parent (`.all()` with no `LIMIT`), so a relation with large
  fanout (`Org.posts` over millions of rows) could pull an unbounded
  result set into memory. The generated FK loader now caps its batch
  query at `RELATION_LOAD_CAP × keys` and truncates each parent's bucket
  to `RELATION_LOAD_CAP` (100), logging a `warn` when it does. A relation
  that legitimately exceeds the cap should be a paginated
  `#[field_resolver]`, not an auto-resolved list.

## [0.3.0] - 2026-07-16

### Added

- **Social login with an open provider contract.** The new
  `nest-rs-social` crate makes social login a first-class capability.
  `SocialProvider` is flow-owning — `authorize` / `exchange` default to
  the shared PKCE/CSRF flow, so a standard provider implements only
  `profile`, while a deviating one (Apple's ES256 client secret)
  overrides a step without changing the trait. Ships first-party GitHub
  and Google; a third party publishes their own provider as an
  independent crate through the same seam. Discovery is link-time and
  module-gated: an unimported provider stays inert with a boot warn, and
  a duplicate or disagreeing key fails boot rather than silently
  shadowing a login provider. Identity keys on the provider's stable
  `(provider, subject)` pair, not the email, so a user who changes their
  provider email keeps their account.
- **Keyed providers.** `#[inject(key = "…")]` fields and `provide_keyed`
  let several instances of one concrete type coexist under a
  `ProviderKey`. The access graph validates each keyed dependency
  against the global keyed set at boot, naming both type and key on
  failure. Used by the keyed OAuth clients behind social login.
- **Request-scoped providers inside GraphQL and MCP handlers.**
  `nest_rs_graphql::Scoped<T>` and `nest_rs_mcp::Scoped<T>` resolve an
  `#[injectable(scope = request)]` provider from inside a resolver or
  tool body, falling through to singletons — so both transports share
  the per-request resolution model HTTP already had.
- **Type-safe queue identity.** `#[queue(name = "…", job = …)]` declares
  a `QueueName` unit struct carrying both the wire name and the job
  type. Both sides name the type (`push_to::<Q>`,
  `#[process(queue = Q)]`) and the macro asserts the process method's
  job argument matches, so a typo is a compile error instead of a job
  that silently never drains. The stringly-typed form still works.
- **Redis-backed throttler.** `RedisThrottler` puts the fixed-window
  counter in Redis so N replicas share one budget per client instead of
  N× the limit. The window advances in a single atomic Lua script (one
  round-trip, no check-then-act race) and fails closed on a backend
  outage.
- **Per-argument pipes on every transport.** `Piped<P, T>` / `Valid<T>`
  bind on GraphQL, WebSockets, and queue handlers (value-form carriers in
  `nest-rs-pipes`, stripped by `#[resolver]` / `#[messages]` /
  `#[processor]`); HTTP keeps its extractor forms. A rejection surfaces as
  the transport's native error (GraphQL error, WS error frame, job error).
- **Relational predicate scoping.** `p.related::<R, _>(relation, |r| ...)`
  scopes an entity by a condition on a related entity through a typed
  SeaORM relation — lowered to a semi-join (`IN` subquery / correlated
  `EXISTS`), with boot-time guards on the relation target and key arity.
- **Scalar predicate variants.** `p.ne` / `p.lt` / `p.lte` / `p.gt` /
  `p.gte` (`Cmp`) and `p.is_null` / `p.is_not_null` (`IsNull`).
- **Action-typed authorization proofs.** `Authorized<E, A>` carries the
  action as a type parameter, with `bind_required::<S, A>` as the GraphQL
  subject binder — a `Read` proof no longer passes where an `Update` proof
  is required.
- **Generic client-credentials grant helper** in `nest-rs-authn`.
- **Selective `#[crud]` ops with segregated write traits.**
  `ops = [list, get, delete]` synthesises exactly those; the write half
  lives in opt-in `Creatable` / `Updatable` / `Deletable` traits, so a
  read-only resource declares no placeholder input types.
- **Generated list operations paginate by default**, with a hard
  backstop on page size.
- **`ServiceError` carries real 4xx variants** plus `Internal` — features
  stop redefining plumbing errors.
- **`resolve_unique_slug()`** for soft-deletable entities and a **`now()`**
  timestamp helper in `nest-rs-seaorm`.
- **Actor identity on the request span** — denials are attributable
  without per-site threading.
- **Per-job spans and start/ok/fail events** in the Redis queue
  consumer.
- **`#[non_exhaustive]` on the eight public error enums**, so a new
  variant is no longer a breaking change, and compiler-enforced
  unsafe-freedom via `[workspace.lints] unsafe_code = "forbid"`, opted
  into workspace-wide with three documented exceptions.
- **Bounded WebSocket connection lifetime** (`WsConfig`, default 4h)
  and an OpenAPI enable toggle.
- **`nest-rs-testing` auto-loads the project `.env`** for e2e, so every
  boot sees the same URLs the app does — no duplicated test env file.
- `nestrs run db down [N]` reverts N migrations (default one step).
- `nestrs new` ships a `compose.yml` in the workspace scaffold.

### Changed

- **Minimum supported Rust is now 1.96** (was 1.95), pinned explicitly
  in `rust-toolchain.toml` and the workspace `rust-version`.
- **`nest-rs-macros` is renamed `nest-rs-core-macros`.** Apps consuming
  the framework through the `nest-rs` umbrella are unaffected; a direct
  dependency on the old name must be repointed.
- **`async-graphql` is pinned to `=7.2.1`** (exact, not caret): the
  resolver codegen spells out a public-but-internal registry literal
  that a minor bump can silently change. Guarded by a compile-time
  canary and an SDL snapshot test; the bump procedure lives in the
  crate docs.
- **`ConfigService::var` is renamed `var_name`** — it returns the
  variable's name, not its value, and shadowed the meaning of
  `std::env::var`.
- **`nest-rs-config` no longer mutates the process environment** on the
  live path — it reads an in-crate `.env` map, with the real
  environment winning.
- **Transport dependencies are feature-gated** (interceptors, filters,
  exception-filters, guards) so an HTTP-only app skips the GraphQL and
  WebSocket stacks.
- **Access and create authorization are decided in SQL.**
  `CrudService::access` re-checks the primary key against
  `condition_for(action)` in the database instead of an in-memory
  `Ability::can` — one source of truth with the list filter, and what
  makes relational rules enforceable on the by-id and create paths.
- **GraphQL posture is mandatory and visible.** Every operation declares
  `#[authorize(Action, Entity)]` (class gate + automatic response
  masking) or `#[public]`; an operation without a posture does not
  compile, and an `Authorized<E>` parameter is not accepted as a
  standalone posture.
- **Transfer objects are named by the boundary they cross** — REST
  `Dto`, queue `Command` / `Event`, GraphQL `Input`; entity-derived
  CRUD forms stay bare (`CreateUser`), with file-role placement to
  match.
- **Framework and product split into two Cargo workspaces** (root
  `crates/nest-rs-*` vs `demo/`), the demo consuming the framework by
  relative path.

### Fixed

- **Security: a pre-release audit pass across the framework.** All authz
  denials log at `warn`; a throttler brute-force bypass is closed
  (per-bucket window + route-scoped key); JWT `aud`/`iss` are enforced;
  a failed predicate fail-closes to `Deny` instead of panicking per
  request; OAuth state compares in constant time; submitted values are
  stripped from validation-error responses; masked responses are
  retained by a static expose set.
- **Login separates store outages from credential mismatches.** Every
  `DbErr` on the login path used to map to an invalid-credentials 401,
  hiding outages and locking out returning OAuth users. Store failures
  now surface as `AuthError::Unavailable` (500, logged at `error`),
  kept distinct from the opaque credential rejection.
- Boot fails with a named error on a duplicate controller prefix
  (previously a panic).
- Lifecycle hooks whose provider is unreachable are surfaced at boot
  instead of silently never running.
- `#[crud]` GraphQL operation names derive from the snake_case entity
  name.
- `#[public]` is rejected on WS message handlers; OAuth login input
  hardened.

### Documentation

- Content overhaul: a linear onboarding journey, a request-lifecycle
  page, corrected decorator docs with macro expansion sketches, and a
  new Entities reference page.
- Shipped `STYLE.md`, page templates, and a docs lint gate.

## [0.2.0] - 2026-06-10

### Added

- **CLI generators (`nest-rs-cli`).** New scaffolding binary with
  `nestrs g feature/resource/<transport>` — transactional scaffold core that
  generates files and auto-wires modules, with context detection.
- **`nestrs run` task front door.** Single entry point that forwards to `just`
  recipes, with first-run toolchain bootstrap (installs `just`, `bacon`,
  `cargo-nextest`, binstall-preferred; opt out via `--no-bootstrap` /
  `NESTRS_NO_BOOTSTRAP`).
- **Publish suite.** Exemplar workspace with org-scoped posts spanning REST,
  GraphQL, WebSockets, queue, and MCP apps.

### Changed

- **Unified layer pool.** Guards, pipes, interceptors, filters, and
  exception-filters now resolve through a single deduplicated pool per family
  (execute exactly once per request; broadest scope wins).
- **Apps renamed** and **service-naming conventions** tightened across the
  workspace (`svc` / `<name>_svc` injection naming).

### Fixed

- **Security: hardened authn/authz, transports, the data layer, and the CLI**
  against several edge cases.
- **Security: fail closed on unwired MCP** and **enforce a minimum HS256 secret
  length** at boot.
- Access-log `duration_ms` now rounded to microsecond precision.

### Documentation

- Added the Lifecycle fundamentals page and a dedicated packages page.
- Routed all task examples through `nestrs run`.
- Refined the splash hero / landing page (mobile layout, hello code-tabs demo,
  access-log terminal lines) and slimmed the README toward contributors,
  pointing users to nestrs.dev.

## [0.1.0] - 2026-06-08

Initial public release of the nestrs framework — an opinionated Rust framework
where the developer writes business logic and the framework carries the
cross-cutting concerns (authn, authz, row-level filtering, transactions, edge
validation, discovery, lifecycle).

### Added

- **Composition & DI.** Type-id container with `#[inject]` fields, `#[module]`
  composition, four-phase `App::builder().build()`, singleton/request/transient
  scopes, and a compile-time + boot-time access graph.
- **Request layers.** Guards, pipes, interceptors, filters, and exception
  filters with symmetric scopes (global / controller / handler) and TypeId
  dedup.
- **Transports.** HTTP (`nest-rs-http`), GraphQL (`nest-rs-graphql`),
  WebSockets (`nest-rs-ws`), queue (`nest-rs-queue` + `nest-rs-redis`),
  scheduler (`nest-rs-schedule`), MCP, and OpenAPI (`nest-rs-openapi`).
- **Authn / authz.** `nest-rs-authn` (strategies, `AuthGuard`, `JwtService`)
  and `nest-rs-authz` (abilities, ability guards, response masking) with
  bridges per transport.
- **Data layer.** `nest-rs-seaorm` with transparent ability-scoped `Repo`,
  ambient executor/transaction `task_local!`s, route-model binding, and
  auto-resolved GraphQL relations from `#[expose]`.
- **Supporting crates.** Pipes, events, health, throttler, config,
  opentelemetry, and the `nest-rs` umbrella crate (`use nest_rs::prelude::*`).
- **`nest-rs-*` naming alignment** across directories, packages, and imports;
  framework-owned error types.
- Rust 1.95 / edition 2024; tag-based release CI with the `mold` linker on
  Linux.

[Unreleased]: https://github.com/YV17labs/NestRS/compare/v1.3.0...HEAD
[1.3.0]: https://github.com/YV17labs/NestRS/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/YV17labs/NestRS/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/YV17labs/NestRS/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/YV17labs/NestRS/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/YV17labs/NestRS/compare/v0.5.0...v1.0.0
[0.5.0]: https://github.com/YV17labs/NestRS/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/YV17labs/NestRS/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/YV17labs/NestRS/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/YV17labs/NestRS/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/YV17labs/NestRS/releases/tag/v0.1.0
