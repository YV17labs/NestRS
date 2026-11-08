# TODO — what remains open on 7.0

This file is the hand-off: an agent or a contributor who opens the repository
cold continues from here. It lists every gap left after the 7.0 foundation
work, how each was established, and what closing it takes. Remove a line when
it closes, citing the commit that closed it.

**State, 2026-11-06, `release/7.0` at `94b8eccfa`.** Everything planned is
merged. Green on that commit or the one just before it: `just ci` (6,679
tests, 11 skipped), the demo gate (lint, 111 unit, 152 e2e), the docs build,
the rustdoc sweep of every feature alone with warnings denied (144 of 144),
the five demo apps run and `curl`ed, the framework's Valkey Sentinel and
Cluster e2e suites, the demo's five binaries on Sentinel and on Cluster, the
demo image build and the Helm chart (`helm lint`, `helm template`), and on
`3d95a42cf` CI's lint jobs on x86_64 (clippy, `cargo hack`, both rustdoc
passes, the demo's clippy, in an amd64 container). Nothing is pushed, tagged
or published.

## How to pick this up

Read `CLAUDE.md` first. Then these rules hold for every item below:

1. **The developer first.** A fix must keep everyday code easy: secure with
   nothing to think about, and fast. A fix that makes a developer's ordinary
   code harder is not taken; keep the security by another spelling, or leave
   the item open here with the reason.
2. **A bug needs its proof before any work.** Each item says how it was
   established. **seen**: a run showed it. **read**: read off the code by
   someone who ran nothing — a suspicion. Reproduce a *read* item first, with
   a failing test or an observed run. If it does not reproduce, do not fix it:
   say so here and drop it.
3. **No blind spot.** A gap that stays is written here with what closing it
   takes. A limit written only in a docs page or a decision is a limit hidden.
4. **Small and verified.** One branch per subject (`<type>/<slug>`), signed
   commits, `release/7.0` moved by fast-forward only, after `just ci` and the
   demo gate are green on the branch's tip.

Where things run: the services live in the dev container (`.devcontainer/`).
From the host, with the dev container started:

```bash
docker compose -f .devcontainer/compose/base.yml exec -T dev bash -lc 'flock /tmp/nestrs-live.lock just ci'
docker compose -f .devcontainer/compose/base.yml exec -T dev bash -lc \
  'cargo build --locked -p nest-rs-cli && export PATH=$CARGO_TARGET_DIR/debug:$PATH \
   && cd demo && just lint && just chart && just test unit && flock /tmp/nestrs-live.lock just test e2e'
docker compose -f .devcontainer/compose/base.yml exec -T -w /workspaces/nestrs/docs dev bash -lc 'npm ci && npm run build'
```

`just chart` needs the dev container's `helm` and `yq`, which a container built
before 2026-11-08 lacks: rebuild it. Valkey Sentinel and Cluster, and Postgres with
mutual TLS, are dev container environments of their own (`CLAUDE.md`, *Testing*).

## 1. Before publishing 7.0

- **The performance guard has not run in full since 2026-10-23** (`3290e995`;
  per-branch measures stopped on 2026-10-30), though much has moved on the hot
  path since. Close: the harness in `bench/` on a quiet machine, the tip
  against the base twice, within the larger noise plus 5 % rps or 10 % p99.
  Figures are relative, never for publication.
- **The first CI run after the push** is the only proof of a fresh
  `create-bucket.sh`, the dev-services action's `/etc/hosts` mapping, the
  default stacks on x86_64, and of four jobs no run has started yet:
  `Demo`'s `chart` (the render cases, seen green on the host) and `image`
  (the demo's image), `Bench`'s `image` for each contestant, whose two
  NestJS images were never built anywhere, and `Framework`'s `windows`, the
  one proof that the `not(unix)` branches (`way_down.rs`, `material.rs`,
  `static-files`' `disk.rs`) and the C of `aws-lc-sys` and `zstd-sys`
  compile for Windows. Sooner, for the Rust half: `cargo check --target
  x86_64-pc-windows-msvc` of `nest-rs-core` (every feature) and
  `nest-rs-config` (its defaults) on any host holding that target's
  standard library; `nest-rs-static-files` reaches the C through
  `nest-rs-http`, so it needs a C cross compiler. Nothing runs or tests on
  Windows.
- **Postgres mutual TLS never ran live**: `nest-rs-seaorm`'s two `mutual_tls`
  tests. Close: open the `postgres-mutual-tls` dev container environment, set
  `postgres := "mutual-tls"` in `test.just`, `just test e2e`.
- **The demo image was built and its binaries started, never booted against
  live services**, and `migrate up` never applied a migration from it: the
  distroless image trusts no development authority. Close: run it on the
  compose network with a certificate bundle mounted.
- **No app has booted on what the demo chart 0.7.0 renders**, each on a
  Secret of its own: what each app reads is read off its modules and 0.6.0's
  per-key boots. Close: `helm template` the install line, read each
  workload's env off the render (the ConfigMap, its `env`, its own Secret),
  start each binary in the dev container on that env alone against the live
  services, on a database of its own, and see `app ready`; `migrate up` exits
  0 on its hook Secret alone. *read*
- **`helm install` never ran against a cluster**: the chart is linted and
  rendered client-side alone (`nestrs run chart`). Close: install it on a
  local cluster (kind, k3d) with the demo's image loaded and the services
  reachable, then `helm test`.
- **`async-graphql = "=7.2"`'s floor, 7.2.0, never built**: every lockfile
  holds 7.2.1. Close: a minimal-versions resolution.

## 2. Open decisions, each with the recommended answer

Nothing waits on them; the recommendation holds until someone decides
otherwise.

1. **A `has_many` lists the children whose key a grant withholds**: `orgs {
   users { name } }` answers for a member whose grant withholds `org_id`, the
   children grouped by that key inside the batch, each judged on its own.
   Recommended: keep — the relation's `#[expose]` is its visible opening;
   refusing it breaks the demo member's directory. A `belongs_to` reads its link
   off the judged row, so a withheld key reads `null` there
   (`mask-provenance.md`, 2026-11-08).
2. **The demo's roots list no `GlobalGuards` pool.** The boot refuses a unit
   with no identifying or authorizing guard, but a route under a weaker guard
   of the right phase boots without `AuthnGuard`. Recommended: keep the demo
   without a pool; one line per root puts it back, and brings back the
   probes' `503` during an authentication outage.

## 3. Field-level authorization — what the judged row leaves

**Today.** Rows are filtered in SQL, and each wire row is judged on its whole
`Model` where `From<&Model>` builds it: a column a grant withholds is no value in
the row (`#[expose(nullable)]` or an `Option`), a grant withholding any other
fails `build()`, the row answers only the caller whose ability judged it, a
`#[field_resolver]` under a row that answers no one refuses before it runs, and a
dispatched method answering a `Model` does not compile (`mask-provenance.md`,
2026-11-08). What remains:

- **A `#[public]` GraphQL field whose own guard attaches the ability**, its
  request carrying none, answers no wire row: the rows its body builds are judged
  under the field's unit, and their columns resolve after it, where no ability is
  installed — a root field's as a subscription's items. Fail closed, with an
  `error` line; every `/graphql` whose request runs the ability guard is
  unaffected. Close: nest-rs-graphql resolving a root field's answer under the
  ability its unit attached (a wrapper `OutputType` installing it), as MCP writes
  a tool's answer inside its site. *seen* (`gate::public::a_subscription_whose_own_guard_attaches_its_ability_streams_no_row`)

## 4. Known gaps by area

**Weak proofs and flaky tests**
- A test of the Redis suite's own counting no run on its one-second lease,
  the failovers and the KEDA confinement, may fail under a stall past about
  two thirds of it, the behaviour right; neither was driven under stalls: the
  KEDA one ends, in about 0.3 s, before a driver in the dev container finds
  its process, and the failovers run alone on their topology. Correlation,
  trim and the entry deleted while delivered did not reproduce it: ten runs
  each under 1.5 s stalls from the test's boot, every one green. Close: a
  driver the test signals its start to, then each driven, and one seen red
  moved to the suite's three-second lease, as the queue kit's two renewal
  cases and the lost settle moved. *read*
- The Redis suite's three tests on its three-second lease count a job's runs:
  the queue kit's `a_long_attempt_keeps_its_lease_and_runs_once` and
  `a_lease_lost_under_a_live_worker_cuts_its_attempt_and_the_holder_decides`
  (`KitLease::Brief`), and the lost settle
  (`standalone::a_settle_whose_answer_is_lost_still_ended_the_job_once`). A
  renewal goes a third into the lease, so a runner stalled past two seconds
  lapses it and the job runs again, the behaviour right: every suite stays
  green under a stall of up to about two seconds, not more. Seen at the
  one-second lease, 1.5 s stalls lapsing the long attempt's in 30 of 30 runs;
  at three seconds, 10 of 10 green under the same stalls; a stall past two
  seconds was not driven. Close: a longer lease, `KitLease::Brief` on Redis
  and the suite's renewed lease, which lengthens the long attempt (its hold is
  half again the lease) and the lost settle (its wait is twice the lease) with
  it, each among the suite's slowest tests. *read*
- A `HELLO` sent as its connection's driver ends is neither answered nor
  dropped (B-200), and no test strands one on a real connection: the window
  lies inside `redis` 1.8's send, between `Sender::reserve` granting a permit
  and `Permit::send` pushing the message on the caller's thread, which a
  double cannot place. The boot's recovery from it is seen end to end in
  process
  (`tls::a_first_hello_nothing_answers_is_asked_again_and_the_boot_opens_on_a_dial_of_its_own`,
  red with one ask). Close: a hook in `redis`'s send, upstream (§7). *seen*
- A command blocked elsewhere still waits its whole budget. Close: a change in
  `redis`, upstream. *read*
- `RustFS` 1.0.1 answered `404 NoSuchKey` to a first `PUT` three times in a
  month of the dev container's log, each a fresh `<prefix>/a.txt` of the
  storage suite. Its own log names the cause, the server's: the `PUT`'s last
  step, renaming the object's `xl.meta` into its directory, found that
  directory gone (`reliable_rename failed … NotFound`, then `PutObject store
  write returned … NoSuchKey`), and S3 never answers a `PUT` so. Not
  reproduced on a throwaway 1.0.1: 12,800 such `PUT`s beside the suite's
  deletes, aborted uploads, bucket churn and refused requests, an empty bucket
  and one of 4,000 objects, none. Close: the upstream report with those lines;
  1.0.2's heal ("defer replacement retries until dangling grace expires") and
  scanner fixes are candidates, and the dev container pins 1.0.1 where 1.0.4
  is current, a move a reproduction proves first. *seen*

**HTTP and CORS**
- An `Ambient<P>` behind a type alias is not read off its spelling, so a
  `#[public]` unit taking one answers an anonymous caller the wiring `500`
  (`INTERNAL` in band) and a route lists no `401`. Close: a trait probe on the
  parameter's type, as `__body_of!` reads a body, on every edge. *read*
- No real proxy has run against the forwarding-header reader, and hyper's
  HTTP/2 header-list refusal and a `414` are pinned by no test. *read*
- A `multipart/form-data` type RFC 9110 §5.6.6 admits and mime 0.3 refuses,
  whitespace before a `;` or an empty parameter (`boundary=x;;`), answers
  `400` `its type does not parse`. Close: read the boundary off the
  framework's own parse of the type, as `essence` reads its name. *seen*

**GraphQL**
- A mutation naming one root field twice at its root, `mutation { a: write
  a: write }`, which GraphQL §6.3.2 runs once, is refused before anything
  runs, since async-graphql 7.2.1 runs each copy's writes. Close: merge the
  copies of one name, arguments and directives into the first before
  validation, as `own_key.rs` hands a set its key, through fragments, held
  by a test that one write ran. *seen*
- A list of lists selected twice at one place with different selections,
  `{ grid { fine } grid { inner { fine } } }` with `grid: [[Shape!]!]!`, which
  GraphQL §6.3.2 merges into one field, is refused before anything runs,
  since async-graphql 7.2.1's `insert_value` joins a later copy's list items
  into the first's only where they are objects, and answered one copy's
  cells. Close: the same merge as the mutation root's, for any key whose
  declared type nests a list in a list, or the upstream fix (§7); the
  refusal's test,
  `null_propagation::a_list_of_lists_selected_twice_otherwise_is_refused`,
  then asserts the merged answer. *seen*
- A type condition on a union applies only where that union is resolved:
  `{ shape { ... on Either { ... on Shape { fine } } } }`, `Shape` a member
  of `Either`, answers `{"shape": {}}` where GraphQL §6.3.2 answers `fine`,
  since async-graphql 7.2.1's `Fields::add_set` applies a condition naming
  the concrete type, an interface it implements or the type resolved there.
  Close: the upstream fix (§7), or, in the pass that gives an object its key,
  each such condition rewritten per member of the union the position may
  take, what cannot apply to that member pruned so validation still passes;
  the canary `null_propagation::a_condition_on_a_union_applies_only_where_that_union_is_resolved`
  turns red when it closes. *seen*

**WebSocket and MCP**
- An MCP event stream carries no frame its client answers, so a peer gone
  without a close is noticed only when the kernel nearest it gives up
  retransmitting the 15-second keep-alive (Linux: about 15 minutes), or at
  `SESSION_IDLE_TIMEOUT_SECS` for an idle session; an `#[sse]` stream likewise.
  The WebSocket gateway and the graphql-ws socket ping instead (the decision on
  socket pings). Close, for a legacy session: MCP's own `ping` request, sent by
  rmcp's peer once the session is quiet for an interval, the session ended when
  none is answered; a stateless `subscriptions/listen` has no channel for one.
  `TCP_USER_TIMEOUT` is no answer: behind a proxy the server's hop is the live
  proxy. *read*
- A withheld chain's line names `error` alone in two places, where a
  gateway's `websocket message failed` names `path` and `event`:
  `Opaque::opaque`'s line of that same sentence (one message, two field sets),
  and MCP's `mcp operation failed` and `unavailable`. Each joins its unit's line
  by its span only. Close: `.opaque()` hands the chain it withholds to the edge,
  as a handler's `Err` does through `HandlerFailed`, and MCP's `Reply::filed`
  takes the operation's address, so each edge files the chain with the unit's
  coordinates, as `envelope::file` does. *read*

**Auth**
- A token `sign` mints carries `client_id` only when the claims name one,
  and `iss` and `aud` only when configured, though RFC 9068 §2.2 requires
  all three; the
  `nestrs g auth` claims and dev token name no client. Close: `sign`
  refusing claims naming no `client_id`, and a signing `JwtService`
  refusing to start without an issuer and an audience — each a line in
  every app minting tokens (a client in its claims literal, two
  variables), which the developer-first rule weighs: the owner's call.
  *read*
- DPoP and mTLS-bound tokens are refused, not verified. *seen*

**Data, queue, storage, health**
- A bare `DbErr` an app holds from `Repo` prints the server's message and
  `detail`, the row it refused, through sea-orm's own `Debug` and `Display`;
  every framework error holding one says it by its SQLSTATE, and `?` into a
  `ServiceError` never meets it. Close: `Repo` answering an error of the
  framework's, a break of every service's `?`. *seen*
- Postgres's certificate is checked against the public authorities sqlx
  compiles in (webpki-roots) beside whatever root is named, the URL's
  `sslrootcert` or `PGSSLROOTCERT`: sqlx 0.9 seeds them under every root, so a
  private authority does not narrow trust to itself as libpq's does. Close: sqlx
  seeding none under a named root (§7). *read*

**Configuration and telemetry**
- A provider injecting a named instance's resource with no import
  (`RedisConnection<Cache>`) is refused in the kernel's words, naming the
  instance and no module: a remedy is filed by key, and that key is the app's
  type. Close: a probe at each `#[inject]` site reading a remedy the type
  carries, a change to every `#[injectable]` expansion. *seen*
- The new plaintext `warn` lines (`redis://`, `sslmode=disable`, OTLP
  `http://`) were seen by unit tests only, never on a live plaintext server.
  *read*

**Operator lines**
- `http.request`, `ws.message` and `mcp.operation` name no `host` nor
  `method`; an error's `Display` can still quote a path. *read*
- A wire row's column refused where GraphQL resolves it names the operation's
  `kind` and the root `field` once per operation and reason, never the root
  field's `host` and `method`, and a row inside a `Json` scalar, which
  async-graphql serializes itself, names no unit at all: both resolve after the
  root field's unit, which alone holds its site. Close: with the `#[public]`
  field's line of §3, the root field's answer resolved under its unit. *seen*
  (the column line); *read* (the scalar's)

**Docs, bench, tooling**
- The benchmark figures are 4.0.0's, dated as such on every page. Close: the
  runbook in `bench/` on a dedicated Linux host. *read*
- 48 docs pages still exceed the ~300-line budget of `docs/STYLE.md`, the
  longest `security/authentication/oauth2.mdx` (594),
  `websockets/messages.mdx` (571), `configuration/index.mdx` (557),
  `database/crud.mdx` (539), `opentelemetry/logs.mdx` (511) and
  `database/writing-a-driver.mdx` (508); `upgrading.mdx` (2718) is the 6.x
  checklist, one ordered path. Close: split each by subject, as the five
  longest were (`wc -l` over `docs/src/content/docs`). *seen*
- No run has started the scaffold's `compose.yml`: a unit test reads it as
  YAML against every URL and credential its two profiles boot on, and
  `docker compose config` accepted it by hand (2026-11-08), but its images,
  healthchecks and bucket script have run nowhere, and the gate's container
  reaches no docker. Close: a by-hand `docker compose up` on a machine whose
  ports 5432, 6379 and 9000 are free, or a CI job running it, at the minutes it
  costs. *read*
- The names the retired reply mask left on internal seams read as what they now
  mean, a posture naming no entity's read: `UnmaskedReply`, the HTTP guard
  stage's `StageKind::Route { unmasked }`, `RouteGuards::masked` and
  `HttpRouteMeta::masked`, the shaper's `MaskProbe`, `mask_reason`, core's `Answer`
  `mapper`, and about 150 internal comments saying "mask" for the judged row (the
  docs site, the decorators' rustdoc, the refusal sentences and the CLI's templates
  no longer do). Close: one rename with its naming set stated, across
  `nest-rs-core`, `nest-rs-http`, `nest-rs-http-macros`, `nest-rs-authz`,
  `nest-rs-openapi` and the rules naming them, a tier-2 move but for
  `HttpRouteMeta::masked`, public.

## 5. Additive features for 7.x (no caller yet)

Each can land later without a break: an MCP `SessionStore` shipped with the
framework; an opening on `#[gateway]` for a visitor or push-only gateway; a
scoped `#[sse]` stream; a public tenant seam outside a request; `OPTIONS *`;
one percent-decoder; computed response headers in OpenAPI; a cursor key
accessor; a backend-fed subscription stream; RFC 8705 / DPoP; client
authentication `none`, `client_secret_jwt`, `private_key_jwt`, the TLS
methods; RFC 7009 revocation and RFC 7662 introspection; an authorization-code grant
with PKCE for the demo's first-party login (RFC 6749 §4.1, a registered
public client identifying itself, where `<PREFIX>_SIGN_IN__CLIENT_ID` names
it today); a refresh grant on `OAuthClient`; `Repo::restore`;
`App::run_task`; public `ResourceBinding`; `nestrs generate guard` and
`interceptor`; a public `RetiredKey`; bench tiers beyond HTTP; a lease port;
named storage disks; `ModuleRef`; `AppLifecycle::spawn`; ETag / If-Match;
client TLS reload.

## 6. Limits kept on purpose

Each is recorded in `.claude/decisions/` with its reason, re-read and still
holding: a body reading a column off a whole `Model` and answering it as its
own value (`initials(row.name)`, `row.email` in a DTO), which no type tells from a
column it derives, so it stays the body's visible line, as a row a body serializes by
hand is read in review (`serde_json::to_value(&row)`, a push, a DTO holding it: a
dispatched method answering a `Model` does not compile); a list a body built by hand holding a row
its grants refuse failing the whole reply closed, where a `Repo` read never hands
it one; a push to other connections carrying no row judged for a caller, so a
per-recipient row is built per recipient or is a DTO (`mask-provenance.md`,
2026-11-08); real-clock budgets where only real time tells two outcomes apart;
`Repo::delete` sending its statement under a class denial; a storage key whose
every policy was removed booting on with its `warn` as a least-privilege one
does, since the store answers both the boot's listing in the same document
(`403`, `AccessDenied`, `Access Denied`, seen on RustFS 1.0.1) and a second call
parts them only where the policy grants that call, which a write-only key's
does not; CORS refusing an
unlisted same-origin request; a hand-written `HttpRoutes` route validating
nothing unasked; dataloaders off the operation's transaction; no layer on a
hand-written MCP `ServerHandler`; `CrudService` noticed at its first query;
a visitor's nested read answering nothing; a carrier of its author's own error
(`HttpError`, `JobError`, `QueueError::Backend`) handing it out as given, its
chain searched for what an edge answers; a GraphQL body reading its
`Context` by hand, which no boot can see, failing per request with the type
on the operator's line (the declared `&DataLoader<L>`, `Ambient<T>` and
`Scoped<T>` are checked at boot); what a GraphQL list's rows copy, bounded
per row by `max_list_argument_bytes` and never by how many rows the list
answers, which only its resolver knows once it ran, so a page of 100,000
rows each copying 4 KiB still holds 400 MB and a list under a list multiplies
by both row counts (the field's `complexity` and page bound it; closing it
takes async-graphql sharing an argument's value across the rows, §7); a guard
overriding the hidden `__attaches_ability` to claim an ability it never
attaches, met by the gate's fail-closed `500`; the text an entity's own
definition writes into a rule's scope, an `impl Iden` written by hand to
answer its name borrowed or a `save_as` cast, read as its schema (closing it:
check each identifier and cast an entity writes, once per entity); past the
cover search's bound of 1,024 partial unions, which six grants per refused
action reach, `missing_scopes` at a `Manage` site naming the smallest set it
met, which a scope lifting every action, declared after each action's own
grants, leaves larger than the smallest (closing it: an exact search, which
grows as the fourth power of the grants per refused action, on the refusal
path); the demo chart's `checksum/secret`, one sum over every Secret the
chart writes, which an install handing the chart one weak secret and nothing
else (every other app on an `existingSecret`) publishes unsalted to whoever
reads its pods (closing it: a salt kept in the cluster, which `helm
template` cannot read).

## 7. Upstream

To report (nothing filed yet): rmcp (session-restore waiters answer `404`
instead of `503`; positional reads of `JsonRpcResponse`); sqlx-core 0.9
(`close_hard` never returns on a quiet TLS connection; its compiled roots
seeded under a named `sslrootcert`); sea-orm
(`TryInsert::exec_with_returning` answers `RecordNotFound` where `Conflicted`
is documented); async-graphql 7.2.1 (an errored field omitted from `data`;
an interface field's error with no `path`; a field selected twice runs its
writes twice; an argument copied at each row of a list, `Context::var_value`
and `get_param_value` cloning, which the framework bounds meanwhile with
`max_list_argument_bytes`; a type condition on a union applied only where
that union is resolved, `Fields::add_set`; a later copy of a list of lists
dropped, `insert_value` joining list items only where they are objects, which
the framework refuses meanwhile); redis-rs (`PipelineSink` loses a
read error with nothing in flight; a command sent as its connection's driver
ends is neither answered nor dropped, so its caller waits out its budget, and
no hook in the send lets a test place one there); sse-stream 0.2.6
(keep-alive on std's clock); Rust's std on macOS (a child's pipes made in two
steps, `pipe` then `FD_CLOEXEC`, so a process another thread spawns between
them inherits them: nextest's `LEAK` mark on a test whose stdout another test
process holds, seen on a test only sleeping 30 ms, benign; `posix_spawn`'s
`POSIX_SPAWN_CLOEXEC_DEFAULT` would close it, in std or in nextest); Starlight
0.42.5 (two of the docs build's three warnings: `The collection "i18n" does
not exist or is empty` on a site defining none, its `console.warn` mute around
`getCollection("i18n")` missing Astro 7's logger; `Could not render /404 from
route /[...slug]`, the `404.md` its docs offer as the custom page rendered by
its 404 route and offered again by the catch-all — harmless, `/404.html` is
that page).

Known limits whose cause is upstream: `RUSTSEC-2026-0253` (lru through
async-graphql 7.2.1) ignored until async-graphql 8 is final; a statement cut
mid-flight is not cancelled (sqlx keeps the backend pid private); a presigned
`PUT` binds its type, not its length (object_store); `parking_lot` 0.12 and
0.13 both in the lockfiles; the docs build's third warning, `Some chunks are
larger than 500 kB`, is mermaid's own 647 kB chunk, which astro-mermaid
imports only on a page drawing a diagram (four pages), and raising the limit
would hide the next chunk to cross it (`decisions/docs-vendor-first.md`).
