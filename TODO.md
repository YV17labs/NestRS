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

1. **Judge a loaded row on its `Model` before wire conversion** (reopens
   `mask-provenance` for loaded rows). Recommended: not for 7.0; it needs a
   reply type carrying the models through every edge. Section 3 may settle it.
2. **Serve as nullable, in GraphQL, a field a grant may mask**, rather than
   requiring an `Option` column. Recommended: yes, as an opt-in key on the
   field's `#[expose]` in a 7.x minor, the column keeping its `NOT NULL`.
3. **A grant cuts a row's columns, never the link a relation follows**:
   `posts { author { id } }` answers though `.fields` withholds `author_id`.
   Recommended: keep for 7.0, documented (`response-masking.mdx`); refusing it
   breaks `orgs { users { name } }` for the demo's member.
4. **A body reading a row by hand can answer a withheld column as a scalar**:
   `writers.load_one(id).await?.map(|row| row.email)`. Recommended: keep for
   7.0, documented; closing it is section 3's work.
5. **The demo's roots list no `GlobalGuards` pool.** The boot refuses a unit
   with no identifying or authorizing guard, but a route under a weaker guard
   of the right phase boots without `AuthnGuard`. Recommended: keep the demo
   without a pool; one line per root puts it back, and brings back the
   probes' `503` during an authentication outage.

## 3. Field-level authorization — the main design left

**Today.** Rows are filtered in SQL. Columns are masked on the reply: each
edge reads the answer back into a `Model`, judges it and cuts what the grant
withholds.

**Openings that remain (seen).**
- Four shapes ship what they hold: a scalar the body builds; a scalar or a map
  column of an object; a `#[serde(flatten)]` beside a `#[field_resolver]` of
  that type; a renamed column read under its resolver's name.
- A body answering a scalar off a loaded row (decision 4 above).
- A row type with no `#[expose]` crosses a wire through a derive; with the
  `graphql` feature off, `SimpleObject` or `#[Object]` on a row compiles and
  answers a hidden column. Close: a probe per edge refusing a `ModelTrait`.
- A guard refusal on a `#[field_resolver]` files one line per parent row, up
  to the complexity limit.

**Candidate designs, to decide before any code.**
- **A value-less cell.** `Repo` reads whole rows; each exposed column of a
  wire row is a `Masked<T>` judged on the whole `Model` at conversion, and a
  withheld cell holds no value. Measured on the mask step alone: 0.23 to
  0.30 µs per row against 1.14 to 1.19 µs today; end to end the gain is small
  (about 1.11 ms to 1.03 ms per request at an assumed 1 ms read, not measured
  on a server). It closes most shapes by value. A review found what must
  change first:
  - a row read for another caller must not be trusted: the row carries its
    reader, is not `Clone`, a push carries a `Model`, and `#[subscription]`
    judges items itself;
  - the schema must stay true: only a column declared nullable may be
    withheld (otherwise OpenAPI's and MCP's `required` lie, and one withheld
    non-null cell nulls a whole GraphQL list);
  - rows must not `Deref` to the `Model` (the demo reads `row.title` under an
    edge), a named `into_model()` instead;
  - it must open nothing new on HTTP or `#[sse]`;
  - a write-only token's reply keeps the key; a refused row ships no `{}`;
  - a cell names its column only where the field resolves.
- **Declared columns as plain `Option<T>`** (Hasura's shape): no cell, no
  stamp, honest schema, about the cost of no mask. It loses the refusal of a
  row read for another caller, and a body reads `None` where the cell said
  `FORBIDDEN`.

**How to decide.** Prototype both on the real `#[expose]` and a real GraphQL
extension; measure what each changes in the demo's everyday code and the cost
end to end; decide by rule 1, then security, then performance. **Semver:** if
7.0 is published first, this is an 8.0 or an opt-in.

## 4. Known gaps by area

**Weak proofs and flaky tests**
- The Cluster slot-move test fails under a stall over about a second, through
  the suite's admin connections (3 of 10 runs under deliberate stalls). Close:
  `bare_client` with a connect timeout wider than a stall. *seen*
- A lapsed lease counts a job twice at a one-second lease (none at the
  default); whether it caused the original flaky soak is not shown. *seen*
- `GATED` and the queue kit's Redis cases count exact attempts on
  `RedisKit`'s lease, so a runner stalled over a second can fail one. Close: a
  longer lease offered by `KitBackend`. *read*
- The Valkey `HELLO` race is forced by no test on a real connection. *seen*
- A command blocked elsewhere still waits its whole budget. Close: a change in
  `redis`, upstream. *read*
- `RustFS` answered `404 NoSuchKey` to a first `PUT` twice; cause not found.
  Close: a reproduction against RustFS. *seen*
- nextest `LEAK` marks on passing tests, under load, on macOS only. *read*
- Each commit's own tree is not rebuilt after a rebase; only the head is
  gated. *read*
- Several assertions were never seen red alone (a `compile_fail,E0308` on an
  order key, the guards suite's `404` and `405`, seaorm's listener e2e). *read*

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
  `method`; some mask refusals name no unit; an error's `Display` can still
  quote a path. *read*
- A caller selecting N refused fields files up to N lines, bounded by the
  complexity limit. *read*

**Docs, bench, tooling**
- The benchmark figures are 4.0.0's, dated as such on every page. Close: the
  runbook in `bench/` on a dedicated Linux host. *read*
- Five docs pages exceed the 300-line budget (`guards.mdx`,
  `http/configuration.mdx`, `cli/index.mdx`, `testing/e2e.mdx`,
  `schedule/index.mdx`). Close: split each.
- No run has started the scaffold's `compose.yml`: a unit test reads it as
  YAML against every URL and credential its two profiles boot on, and
  `docker compose config` accepted it by hand (2026-11-08), but its images,
  healthchecks and bucket script have run nowhere, and the gate's container
  reaches no docker. Close: a by-hand `docker compose up` on a machine whose
  ports 5432, 6379 and 9000 are free, or a CI job running it, at the minutes it
  costs. *read*
- Copies left: the "line written once" guard eight times, the site emitter
  five times, spec vectors six times; `impl Layer for X {}` beside
  `#[derive(Layer)]` in about 260 fixtures.
- Per-request trace and span ids are minted from `Uuid` with no `fast-rng`;
  cost never measured.

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
holding: real-clock budgets where only real time tells two outcomes apart;
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
read error with nothing in flight); sse-stream 0.2.6 (keep-alive on std's
clock).

Known limits whose cause is upstream: `RUSTSEC-2026-0253` (lru through
async-graphql 7.2.1) ignored until async-graphql 8 is final; a statement cut
mid-flight is not cancelled (sqlx keeps the backend pid private); a presigned
`PUT` binds its type, not its length (object_store); `parking_lot` 0.12 and
0.13 both in the lockfiles.
