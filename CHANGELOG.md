# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
  provides it. A namespaced gateway self-provides its own and needs no import.

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

[1.2.0]: https://github.com/YV17labs/NestRS/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/YV17labs/NestRS/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/YV17labs/NestRS/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/YV17labs/NestRS/compare/v0.5.0...v1.0.0
[0.5.0]: https://github.com/YV17labs/NestRS/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/YV17labs/NestRS/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/YV17labs/NestRS/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/YV17labs/NestRS/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/YV17labs/NestRS/releases/tag/v0.1.0
