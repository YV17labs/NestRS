# TODO — what remains open on 7.0

This file is the hand-off: an agent or a contributor who opens the repository
cold continues from here. It lists every gap left after the 7.0 foundation
work, how each was established, and what closing it takes. Remove a line when
it closes, citing the commit that closed it.

**State, 2026-11-11, `release/7.0` at `9ae8d14a2`.** Everything planned is
merged, the second pass included: twelve branches (2026-11-07 to 2026-11-11)
closing what this file listed on 2026-11-06, the CI cut to its vital minimum,
and the benchmark against NestJS 12.4 on `node:http`. Green on `97e09d5ab`, the
last change to Rust: `just ci` (6,756 tests, 11 skipped), the demo gate (lint,
112 unit, 152 e2e) and the performance guard against `3290e995`; on
`9ae8d14a2`, the docs build (161 pages). Nothing is pushed, tagged or
published.

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
   && cd demo && just lint && just test unit && flock /tmp/nestrs-live.lock just test e2e'
docker compose -f .devcontainer/compose/base.yml exec -T -w /workspaces/nestrs/docs dev bash -lc 'npm ci && npm run build'
```

The demo's Helm chart is linted and rendered by hand, never by CI and never
installed: `just chart` in `demo/`, which needs `helm` and `yq` (a dev container
built before 2026-11-08 lacks them). Valkey Sentinel and Cluster, and Postgres with
mutual TLS, are dev container environments of their own (`CLAUDE.md`, *Testing*).

## 1. Before publishing 7.0

- **The first CI run after the push** is the only proof of a fresh
  `create-bucket.sh` on a runner, the dev-services action's `/etc/hosts`
  mapping and trust-store step, and the default stacks on x86_64. Rehearsed in
  the dev container on 2026-11-07: the bucket script on a RustFS that never held
  a bucket, twice; the mapping read against every host the committed `.env`
  files name. The owner's: it follows the push.
- **Postgres mutual TLS never ran live**: `nest-rs-seaorm`'s two `mutual_tls`
  tests. Close: open the `postgres-mutual-tls` dev container environment, set
  `postgres := "mutual-tls"` in `test.just`, `just test e2e`. The owner's: the
  run's script was refused by the permission classifier on 2026-11-07.
- **`async-graphql = "=7.2"`'s floor, 7.2.0, never built**: every lockfile
  holds 7.2.1. Close: a minimal-versions resolution.

## 2. Field-level authorization — what the judged row leaves

**Today.** Rows are filtered in SQL, and each wire row is judged on its whole
`Model` where `From<&Model>` builds it: a column a grant withholds is no value in
the row (`#[expose(nullable)]` or an `Option`), a grant withholding any other
fails `build()`, the row answers only the caller whose ability judged it (a
job's, inside a job and on a push alone), a refused row answers its key in a
GraphQL mutation's reply alone, a `#[field_resolver]` under a row that answers no
one refuses before it runs, a posture naming an entity or a `#[subject]` opens
its unit's reads, and a dispatched method answering a `Model` does not compile
(`mask-provenance.md`, `unmasked-reply-withholds-the-ability.md`, 2026-11-08). A
GraphQL root field's answer resolves under the ability its own unit judged its rows
by, one its own guards attached included — its field resolvers, its batches and a
subscription's items with it (2026-11-11). Nothing of it remains open here; what it
keeps on purpose is §5's.

## 3. Known gaps by area

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
  red with one ask). Close: a hook in `redis`'s send, upstream (§6). *seen*
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
- async-graphql 7.2.1 runs no `SameResponseShape` (GraphQL §5.3.2): copies of one key
  under conditions on two object types answer in two shapes, `{ results { ... on User {
  x: name } ... on Org { x: size } } }` a string for one row and a number for the next,
  where the specification refuses the document. Close: in the read
  `crates/nest-rs-graphql/src/field_collection.rs` makes of a place's copies, compare the
  declared shape of each key's copies across the types the place may hold, and refuse as
  it refuses copies naming different fields. *read* (`overlapping_fields_can_be_merged.rs`
  compares names and arguments alone)

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

**Data, queue, storage, health**
- Postgres's certificate is checked against the public authorities sqlx
  compiles in (webpki-roots) beside whatever root is named, the URL's
  `sslrootcert` or `PGSSLROOTCERT`: sqlx 0.9 seeds them under every root, so a
  private authority does not narrow trust to itself as libpq's does. Close: sqlx
  seeding none under a named root (§6). *read*

**Configuration and telemetry**
- A provider injecting a named instance's resource with no import
  (`RedisConnection<Cache>`) is refused in the kernel's words, naming the
  instance and no module: a remedy is filed by key, and that key is the app's
  type. Close: a probe at each `#[inject]` site reading a remedy the type
  carries, a change to every `#[injectable]` expansion. *seen*

**Docs, bench, tooling**
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
  mean, a posture naming no subject: `UnmaskedReply`, the HTTP guard
  stage's `StageKind::Route { unmasked }`, `RouteGuards::masked` and
  `HttpRouteMeta::masked`, and the names and comments of tests saying "mask"
  for the judged row (about 30 lines, in the crates' suites and unit tests; the
  docs site, every comment in the crates' sources, their descriptions and
  manifest comments, the refusal sentences, the errors a client reads and the
  CLI's templates no longer do). Close: one rename with its naming set stated, across
  `nest-rs-core`, `nest-rs-http`, `nest-rs-http-macros`, `nest-rs-authz`,
  `nest-rs-openapi` and the rules naming them, a tier-2 move but for
  `HttpRouteMeta::masked`, public.

## 4. Additive features for 7.x (no caller yet)

Each can land later without a break: an MCP `SessionStore` shipped with the
framework; an opening on `#[gateway]` for a visitor or push-only gateway; a
scoped `#[sse]` stream; a public tenant seam outside a request; `OPTIONS *`;
one percent-decoder; computed response headers in OpenAPI; a cursor key
accessor; a backend-fed subscription stream; RFC 8705 / DPoP (a bound token
is refused today, never verified); client authentication `none`, `client_secret_jwt`, `private_key_jwt`, the TLS
methods; RFC 7009 revocation and RFC 7662 introspection; an authorization-code grant
with PKCE for the demo's first-party login (RFC 6749 §4.1, a registered
public client identifying itself, where `<PREFIX>_SIGN_IN__CLIENT_ID` names
it today); a refresh grant on `OAuthClient`; `Repo::restore`; a `RepoError`
variant per integrity condition no caller branches on yet, `NotNullViolation`,
`CheckViolation` and `ExclusionViolation` (`23502`, `23514`, `23P01`, each
`Other` today);
`App::run_task`; public `ResourceBinding`; `nestrs generate guard` and
`interceptor`; a public `RetiredKey`; bench tiers beyond HTTP; a lease port;
named storage disks; `ModuleRef`; `AppLifecycle::spawn`; ETag / If-Match;
client TLS reload.

## 5. Limits kept on purpose

Each is recorded in `.claude/decisions/` with its reason, re-read and still
holding: a body reading a column off a whole `Model` and answering it as its
own value (`initials(row.name)`, `row.email` in a DTO), which no type tells from a
column it derives, so it stays the body's visible line, as a row a body serializes by
hand is read in review (`serde_json::to_value(&row)`, a push, a DTO holding it: a
dispatched method answering a `Model` does not compile); a list a body built by hand holding a row
its grants refuse failing the whole reply closed, where a `Repo` read never hands
it one; a subscription item of the body's own holding a row its subscriber may not
read arriving refused, its row answering no column, so the subscriber learns a push
arrived (an item that must not say even that is a row, a list, or one inside an
`Option` or a `Result`, which leaves the stream); a push to other connections carrying no row judged for a caller, so a
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
takes async-graphql sharing an argument's value across the rows, §6); a guard
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
path); a GraphQL document whose copies merge one field per type a position may hold, or
whose condition on a union is rewritten per member, scoring each copy, so
complexity counts more than runs for such a document alone, never less
(`graphql-transactions.md`, 2026-11-11; closing it: scoring the rewritten
document per type, which async-graphql's visitor does not do); the demo chart's `checksum/secret`, one sum over every Secret the
chart writes, which an install handing the chart one weak secret and nothing
else (every other app on an `existingSecret`) publishes unsalted to whoever
reads its pods (closing it: a salt kept in the cluster, which `helm
template` cannot read); the demo's Helm chart, linted and rendered by hand and
never installed, deployment being the owner's (2026-11-11); Windows, not claimed
and never built: the `not(unix)` branches (`way_down.rs`, `material.rs`,
`static-files`' `disk.rs`) and the C of `aws-lc-sys` and `zstd-sys` were never
compiled for it, and CI holds no Windows job (`ci-on-change.md`, 2026-11-11);
claiming it takes `cargo check --target x86_64-pc-windows-msvc` on a host holding
that target and a C cross compiler.

## 6. Upstream

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
dropped, `insert_value` joining list items only where they are objects; these
three the framework collects meanwhile, rewriting the document before
validation; `OverlappingFieldsCanBeMerged` comparing copies under one type
condition alone, which the framework refuses meanwhile, and running no
`SameResponseShape`; a `Json` scalar whose value fails to serialize answered
`null` with no error, `Json::resolve`, which the framework answers meanwhile as
its root field's refusal); redis-rs (`PipelineSink` loses a
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
