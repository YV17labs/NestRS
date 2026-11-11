---
paths:
  - "**/service.rs"
  - "**/entity.rs"
  - "**/entities/**/*.rs"
  - "crates/nest-rs-seaorm/**/*.rs"
  - "crates/nest-rs-database/**/*.rs"
  - "crates/nest-rs-events/**/*.rs"
  - "crates/nest-rs-seaorm-macros/**/*.rs"
  - "demo/crates/migrations/**/*.rs"
---

# Data layer — transparent security and transactions

## Every access through a service, every service through `Repo`

`CrudService` is the entity's API and the one audited choke point.
Controllers, resolvers, gateways, tools and dataloader code **delegate**; they
never touch `Repo` or the ORM. `Repo` runs every query on the ambient executor
and filters reads **and** by-id writes by the ambient ability's condition — no
ability in a job is system work, unscoped, and outside one denies every row, a
save or a create then running no hook and sending nothing. Binding a subject by id (`bind = S`) goes
through `CrudService::access`.

**`Repo` hands out no connection and no statement**, held by types: its selects
run on the ambient executor themselves, and the handle a unit installs is private
to its driver, so a downcast of the port's reader finds nothing to run a statement
on; a `RepoSelect` implements none of sea-orm's traits, so nothing rewrites what
`Repo` filtered or moves a hidden column into an exposed slot, and an order key is
a column of the entity (`.claude/decisions/repo-hands-out-no-connection.md`).
What the types leave, raw text in a condition (`Expr::cust` and every other call
writing text unescaped), `Repo` holds where it lands: a condition is one parenthesized
operand beside the scope, and text that would leave it is refused before any statement
(held by `select.rs`'s, `page.rs`'s and `repo.rs`'s tests, on Postgres by the e2e's
`select.rs`). Clippy names the same calls where they are written —
`disallowed-methods` and `disallowed-types` in the root `clippy.toml` and in the one
`nestrs new` writes, an `#[expect]` the visible opening, on the module when a derive
expands to the call. A scaffold's migrations crate carries an empty file of its own, a
migration being DDL no request's value reaches.

**The named exceptions — there are no others.** The ability filter is dropped
only through a named escape, and each escape's rustdoc states the bar its
callers must clear; the method's doc is the authority, not this list.

- **`Repo::unscoped` / `unscoped_by_id`** — reads with no ability:
  pre-authentication credential lookup, `CrudService::access` (which must tell
  `Denied` from `Missing`, so it filters explicitly after the load), and global
  uniqueness probes such as `resolve_unique_slug`.
- **`Repo::insert_unscoped`** — the write pendant: pre-principal provisioning
  (a social-login user) and principal-less system work. An authorized create
  stays on `Creatable`. Its on-conflict form, `insert_unscoped_or_existing`,
  under the same bar, returns the live row already holding a unique key
  (`INSERT … ON CONFLICT (key) DO NOTHING RETURNING`, then a read), so a
  collision a concurrent caller may cause never aborts the unit and needs no
  savepoint; no savepoint wraps every write by default, a round trip per
  write on the hot path.
- **`Repo::with_tombstones`** — the ability-scoped read that keeps
  soft-deleted rows: an administrator's view of deleted records, a retention
  job. Every other read hides them. It reads only: no `Repo` write un-deletes
  a row (`Repo::restore` is planned for 7.x).
- **A truly contextless path** — a one-shot command bootstrapped outside
  every unit of work — keeps an injected `Arc<DatabaseConnection>`, because no
  executor exists there. A lifecycle hook is a unit, and reaches `Repo` like
  any handler.
- **The transactional outbox** (`SeaOrmOutbox`) — the framework's statements
  on its own table, `nestrs_outbox`: a push's record and a cancel's removal
  join the transaction the caller's unit already holds, and the relay claims
  on a transaction of its own. No developer row is read or written.
- **A job unit opened outside every unit** — `nest_rs::database::run_unit` is
  the edge port, so a unit opened on a task of its own takes the scope its
  spec declares: a job unit is an edge's (a queue attempt, a tick, a
  lifecycle hook, a third-party consumer). Feature code naming
  `ExecutorScope::Job` — a job unit opened on a task spawned off a request —
  is read as an escape.

**No public function widens the unit it runs in.**
`nest_rs::authz::with_ability` only narrows — the intersection with the
ability the unit holds, the given one in a job, which holds none, nothing in
a request no guard authorized — and the ability is ambient through a type only
`nest-rs-authz` builds, so the ability guard is the one place it comes from
(held by types). A unit `run_unit` opens inside a request runs under the
request's scope whatever its spec declares, and a capture re-entered inside
another unit (`RequestContinuation::scope` and `enter`, `TaskContext::carry`)
holds each value as its `AmbientValue::reenter` returns there: a job's scope
as the request's, the ability narrowed to the one that unit reads under
(held by the port's and `nest-rs-authz`'s tests). The executor installers are
the port's `__private`. A service test runs its body in a unit of the app's
binding (`TestApp::unit` under an ability, `system_unit` as system work).

No webhook ingress exists; `Repo::unscoped`'s rustdoc states the bar a
signature-authenticated one must clear before it ships. Auditing the escapes
is one grep per name, `clippy::disallowed_` for a raw fragment.

**A soft-deleted row is hidden by `Repo`, declared once on the entity.**
`#[expose(..., soft_delete)]` registers the entity, and every `Repo` read and
update — the ability-less escapes included, since hiding a tombstone is no
authorization filter — ANDs `deleted_at IS NULL` after the ability's
condition; `Deletable::delete` then stamps rather than erases. `Delete` alone
reaches a tombstone: `Repo::delete` is the hard purge, and `Repo::soft_delete`
stays idempotent. `Create`'s check of a fresh row tests the ability alone, so a
row inserted already deleted (an archive import) is no denial. A service never
writes the condition.

**`Repo` has no scoped bulk delete.** A retention purge selects through
`Repo::scoped(Action::Delete)` and deletes row by row through `Repo::delete`,
bounded per run — the demo's notifications purge is the shape.

**A database failure is classified once and said once.** What the data layer
hands out is a `RepoError`, never sea-orm's `DbErr`: every method of `Repo`,
`RepoSelect`, `CrudService` and `retry_on_conflict` answers one, `ServiceError::Db`
holds one, and the driver boxes one in `AtomicError::Database` and
`OutboxError::Database`. Its variant is the class every edge answers — a unique
violation is a conflict, a row a write found gone is not found, a write the
caller's grants do not cover (a column `.fields` withholds, a create its scope
refuses) is forbidden, a statement Postgres cancelled (`57014`) is unavailable,
a `503` said at `warn` and retryable inside a transaction, anything else is
opaque — so `ServiceError`, `#[crud]` and the product `match` it rather than the
driver's error. The server's message and `detail` quote the input it refused, so
a `RepoError` holds the SQLSTATE and the names the server reported, never its
words, by every reader; a `DbErr` a caller holds of its own (a migration, a
seed, a transaction it opened) is said through `RepoError::from`, and one still
in a chain through the driver's `SafeLink`. Held by the unit tests beside each,
and through `Repo` by the in-process suite's Postgres double
(`.claude/decisions/repo-hands-out-no-connection.md`, 2026-11-11). **A `#[crud]`
create or update a unique key
refuses names the input's fields the key holds** under `errors`
(`{"email": [{"code": "unique"}]}`), never the value: the binding reads the
keys of every `#[expose]` entity's table and its partitions from the catalog
once at boot (`pg_index`, `pg_inherits`), under an edge rendering the write and
within the pool's bounds, never on the transaction the violation aborted, so the
request pays nothing; a key it did not read, or over columns the input does
not carry, names nothing (`.claude/decisions/create-fills-what-the-grants-pin.md`).

## The ambient context

Singletons read per-request state from core's one ambient context, never from
a task-local of their own: the **executor** (`nest-rs-database` owns the seam,
the `Executor` trait and the one install and read API, `nest-rs-seaorm` the
pool-or-transaction handle, tier 2, which nothing public names) and the
**ability** (`nest-rs-authz`) are two of its values, and a package adds its own
by implementing `AmbientValue`, whose `carry` says how far the value travels and
`reenter` what a captured value is inside another unit it is re-entered in
(`.claude/decisions/ambient-carry.md`).

**A continuation that can outlive the transaction never holds it.** Where a
unit continues after the future that accepted it, the executor steps down to
the pool — a `Pool` unit of the binding that opened it: a streaming body, a
DataLoader batch, work held for the commit (an emitted event's dispatch, whose
listeners each open a unit of their own through that binding) — and is dropped
on a task handed to
`TaskContext`, a connection's next message and a job's envelope, where a write
opens a unit of its own. The ability crosses everywhere but a job's envelope.
One exception is planned for 7.x: a mutation operation's batches on the
operation's own transaction, joined before the field's savepoint settles.

**The executor** is installed by each edge running its unit of work through
the one port, `nest_rs_database::run_unit`, over the `dyn UnitOfWork` the
driver's binding provides — resolved once at mount or boot, never per unit,
the bus preferring the one its emitter ran under;
with none bound a unit runs bare, a host whose generated code runs `Repo`
(`#[crud]`, a bound subject, a GraphQL-exposed entity's service —
`#[expose(service = S, graphql)]`) is refused at boot naming the binding, and
any other `Repo` call fails closed
(`.claude/decisions/declared-need-refused-at-boot.md`). A driver implements
that one trait and binds it through `nest_rs_database::provide_unit_of_work`,
which serves every edge and the kernel's lifecycle hooks; an app lists no data
context. Each edge commits on its own unit's outcome: an HTTP request on
its success — a handler `Err` or a `4xx`/`5xx` it answered rolls back,
whatever a filter or an interceptor then answered (a safe method runs on the
pool, the unit opened by the edge's `run_unit` around the request's whole
pipeline, and so does a request carrying units of its own, `/graphql` and
`/mcp`; what the router answers itself opens none); a GraphQL operation
once it executed with data in its answer, each mutation root field in a
savepoint of it rolled back when the field answers `null` for an error
(`.claude/decisions/graphql-transactions.md`);
a WS message on its success — an error frame fails it, whatever a filter or an
interceptor then answered, and a frame no handler reads opens none — a connection
hook unless it unwound or was cut at
its deadline; an MCP operation on `Ok` with no tool error in band (`isError`); a
queue attempt, a tick, a listener call and a lifecycle hook (system work,
under the Job scope) on `Ok`. A unit of its own gets a **lazy** transaction —
`BEGIN` deferred to the first data-layer touch, **a read included**. Guards
run inside the unit, so a denied unit never touches the data layer and opens
no transaction at all.

**The ability** is what `AbilityGuard` attaches (`cx.attach`): ambient for the
rest of the unit once its guard stage passed. A unit whose posture names no
subject — a bare `#[public]`, one stating none — runs under the `ReadsWithheld`
marker its guard stage attaches, as a WS connection hook does, and
`current_ability` withholds the ability there, so a read no posture named fails
closed, as a write does before its hook — and `nest-rs-http` knows
nothing of authz or the ORM
(`.claude/decisions/unmasked-reply-withholds-the-ability.md`). A job withholds
nothing: `Repo` reads no ability there as system work, wider than the one it holds.

## What waits for the commit — the event and the job

Only the unit of work holding the transaction knows whether its writes
landed. `nest_rs_database::after_commit(work)` hands work to the ambient
executor; every unit settles through its driver's `OpenUnit::settle` (SeaORM's
`LazyTransaction::finalize`), which runs it once the transaction committed (or
the unit succeeded with nothing to roll back) and drops it on every other
outcome; the unit's answer waits for it until the unit's deadline, never past
it, so a slow listener never turns a committed write into a timeout. Held work runs under what its
unit carried into the commit: the pool, the scope, the ability and every
value that crosses. A panic in it is contained
and logged at `error` — the commit already stood, and an unwind would turn it
into a failed attempt a queue replays.

**`EventBus::emit` dispatches through `after_commit`, without being asked.**
An event announces a fact, and a fact its transaction rolled back never
happened: a listener never pushes a job, notifies or calls out about a write
that did not land, and a statement it fails never poisons its emitter's
writes. With nothing to wait for it dispatches before `emit` returns. **Each
listener is a unit of its own, settled on its `Result` within its own
deadline**: opened through the binding its emitter ran under, the app's for an
emitter under none (a task handed to `TaskContext`, whose listeners run under a
request's scope and the ability it carried), bare in an app binding none, one
transaction unless `transactional = false`, its `Err` rolled back and filed at `error` — at
`warn` once a layer answered it — while the chain goes on — so an event a listener emits waits for that listener's commit. An open `EventBus::stream` reads the event at that same point, ahead of the listeners, and opens no unit.

- **Waits:** every unit of its own — a mutating HTTP request, a GraphQL
  operation (a savepoint per mutation root field), a WS message or connection
  hook, an MCP operation, a queue attempt, a scheduled tick, a listener call, a
  lifecycle hook.
- **Runs at once:** a pool — a safe request, a GraphQL query, a subscription,
  `transactional = false`.
- **Not waited for:** a transaction the caller opened itself (a `db.begin()`,
  as inside `retry_on_conflict`) — its commit is the caller's, so the caller
  emits after its own `commit`. A listener reached sooner runs in a unit of its
  own beside it, never inside it; in an app binding none it runs bare, with no
  executor, so its `Repo` call fails closed
  (`.claude/decisions/listener-transaction-of-its-own.md`).
- **Handed on:** an atomic block (`nest_rs_database::atomic`) hands its
  after-commit work to the enclosing unit on release and drops it on
  rollback; on the pool it is a transaction of its own and settles its work
  like any boundary.
- **Written into it:** a job pushed inside a transaction the binding opened
  (`push`, `push_many`) is a record of that transaction, filed by the queue
  port's relay once it committed and never on its rollback, so its receipt is
  honest; a unique key is checked then, a record whose key another job holds
  superseded at `info`. A pool and a binding keeping no outbox file it at once,
  a `db.begin()` of the caller's own changing nothing: a push beside one follows
  the unit around it (`.claude/decisions/transactional-outbox.md`).
- **Refused:** a WS broadcast, a storage write. Each is a call whose answer its
  caller reads — a send failure, an object key — and a deferred call would
  report `Ok` for work that may never be done. Inside a transaction it happens
  where it is written; work that must follow the commit is an event whose
  listener does it.
- **Another ORM's driver** owes `Executor::after_commit` on every handle its
  boundaries settle; the trait's default runs the work at once, which is right
  for a pool only. What it holds runs under the `Commit` capture taken where
  the work was registered, its own pool re-installed inside under the scope
  read there (the trait method's rustdoc). It owes `write_outbox` on the same
  handles, with an `OutboxStore` its binding binds beside the relay's module,
  or its pushes are filed at once.

## What a create fills — the grants first, then the entity

**An authorized create's server-side columns are never a service's to
stamp.** The tenant and the author are `Create` grants in the
`AbilityFactory`: `Creatable` fills what they
pin and the input left unset, then re-checks the row against them, and a
column they leave undecided is refused, never guessed. An author is a grant of
its own, declared only for a subject that can author, so it never narrows an
update or delete. What no grant decides — the v7 key, a constant
(`#[expose(default = …)]`) — every insert through `Repo` fills from the column
defaults `#[expose]` registers, the shape `soft_delete` registers in, so the
entity's `ActiveModelBehavior` stays its own and `timestamps` stamps
`created_at`; a table no transport exposes carries no `#[expose]` and its
migration defaults the key to `uuidv7()`
(`.claude/decisions/create-fills-what-the-grants-pin.md`). Held by
`nest-rs-authz`'s and `nest-rs-seaorm`'s tests.

## What a write sets — the grant's columns

**A grant's `.fields` limits what a write chooses, in the write's own statement**:
`Repo::update` and `Creatable` read the columns set before a hook, a pin or a default
fills the row, refuse by name one no grant of the action permits, sending nothing,
and hold the row to a grant per column in its `WHERE` or a create's re-check, so a
relational rule is exact and no round trip is added. A row's columns are the union of
the grants it meets, on a write as on a reply. The key addresses a row and is `when`'s
to hold; a soft delete's stamp is the framework's. A replacement carries back what it
read: under such a grant `Updatable::update` drops a column left as it stands from the
statement, only where the caller may read it, or the refusal would tell it what the
row holds (`.claude/decisions/field-grant-limits-a-write.md`). Held by `nest-rs-authz`'s
and `nest-rs-seaorm`'s tests.

## Write capability is segregated, never a placeholder

`CrudService` carries the **read** half only. The write half is three opt-in
traits a resource implements when it genuinely offers the operation:
`Creatable`, `Updatable`, `Deletable`. A read-only resource declares no
`Create`/`Update` type — no `_unused` stub, no no-op conversion.

**`#[crud(service = UsersService)]` reads every type off the service it
names**, reached through the host's `#[inject]` field of that type — the
entity off `CrudService`, the inputs off `Creatable` and `Updatable`, the
answer off the entity's sealed `Exposed` — and **generates only the
operations a resource has** (`ops = [..]`, all five when omitted), over HTTP
and GraphQL alike: an op whose trait the service lacks is refused naming the
trait and the op, so a forgotten operation is a build break, never a no-op
mutation on the wire (`.claude/decisions/crud-reads-the-service.md`). A method the block
writes takes its op over, and a key only that op reads (`api(<op>(…))`,
`paginate`) is then refused, never dropped.

## The wire row — judged where it is built

Exposure is `#[expose]` (`CLAUDE.md`, hard "no"); a column a later migration
adds never leaks by omission, and the entity *is* the wire contract, so a
handler returns the exposed type (`Json<User>`), never `Model`: **an `#[expose]`d
`Model` crosses no wire**, held by rustc: `#[expose]` emits, on it and on its
`ModelEx`, the impl of every trait that carries a row (serde's, `JsonSchema`, and
under the `graphql` feature async-graphql's `OutputType` and `InputType`), bounded
on what nothing answers, so using one reports what to answer instead and writing
another overlaps it, and refuses at the derive what it can read there.

**A wire row is judged once, on its whole `Model`, where `From<&Model>` builds
it** (`nest-rs-authz`'s `judged.rs`): the ability installed there decides at
`Read` which rows the caller may read and which columns, a rule the row cannot
decide counting as the `Repo` read that admitted it counted it, and as nothing on
a subscription's push. A column its grants withhold is no value in the row — an
`Option` column, or one `#[expose(nullable)]` declares, reads `None` — so no shape
a body puts the row in ships it; a grant on `Read` or `Manage` withholding any
other column is a `MalformedRuleError` at `build()` naming it, and every edge's
schema says exactly what a reply can hold (the key required, nullable where a grant
may withhold it). A job holding no ability is system work and reads every exposed
column, a row that answers inside a job and on a push to other connections alone;
anywhere else with no ability the row answers no one. A reply reads
`Read`'s grants whatever its posture's action: a write's `.fields` names writable
columns.

**The row carries who judged it, and answers no one else**: no public constructor
(a private proof field), no `Deserialize`, and the id of the ability that judged
it, which its `Serialize` and each of its GraphQL columns check against the one
installed where it answers. So a row carried into another request, a cache and a
publisher's push answer no one, a row judged for a caller refuses on a push to
other connections (`WsServer`'s `broadcast`, `emit_to`, `emit`), a system row
answers no caller, and a `#[field_resolver]` under such a parent refuses before it
runs (`nest_rs_graphql::__private::ParentProbe`). A row its grants refuse answers its
key alone in a GraphQL mutation's reply, which names the row it wrote, and
`FORBIDDEN` everywhere else, its key included, never `{}`; a list a body built
holding one fails the whole reply closed, which a `Repo` read never hands it. Held by `nest-rs-authz`'s
`judged` suites and `nest-rs-seaorm`'s `expose/dto.rs`.

`#[authorize(Action, Subject)]` or `#[public(Action, Subject)]` beside the
operation is the arming declaration on every edge: the decorator emits the gate,
and the unit reads under the caller's ability, an entity's posture and a
`#[subject]`'s alike, a visitor's where it names an entity; a unit declaring
neither withholds the ability from its reads. A site gated on `Manage` holds a row to every concrete
action where it reads (`condition_for`, `can`), and its reply shows what the
caller may `Read` (`.claude/decisions/denial-holds-whatever-the-token.md`). **The
gate is one function**, `class_gate`, on every edge; a subject `#[subject]`
declares has no rows, so its posture licenses no visitor's read. A subscription's
items are pushes: what its stream builds as the socket polls it is judged for the
subscriber, and an item a refusal makes — a row, a list's, or one inside an
`Option` or a `Result` — leaves the stream; an object of the body's own holding one
arrives, its row answering no column. A `belongs_to` reads its
link off the judged row, so a key the grant withholds reads `null`; a `has_many`
groups its children by their key in the batch, its `#[expose]` its visible
opening. An MCP tool's answer is written inside its site; a GraphQL root field's,
which async-graphql resolves once the field's unit ended, resolves under what that
unit judged its rows by (`nest_rs_graphql::__private::FieldAnswer`, through core's
`AnswerContext`, what crosses into a `Continuation::Answer`: the ability alone) — the
field resolvers below it, the batches it reads and a subscription's items included —
so an ability the field's own guards attached answers as one its request carried.
What a refused row or a
row judged for another caller files is one line per operation and per fact, naming
the unit as its `mounted` line does: the serializer hands its refusal to the edge
answering, which files it there (`nest_rs_core::__private::answering`), and one no
edge claims is filed where it is met; a GraphQL column names its root field's site,
which the operation notes as each root field runs, never the alias a client wrote,
and a refusal a scalar meets serializing a row (async-graphql's `Json`, which writes
`null` for it) is its root field's answer, the refusal's code, filed naming that site.

**A dispatched method answers no `Model`**, `#[expose]`d or not, held by rustc: every
type its return type spells is asked of `nest_rs_core::__private::RowProbe`, a
route's `#[api(response = T)]` included. **What no type tells stays the body's
visible line**: a column a body reads off a whole `Model` and answers as its own
value — `initials(row.name)`, `row.email` in a DTO — and a row a body serializes by
hand (`serde_json::to_value`, a push, a DTO field), read in review
(`.claude/decisions/mask-provenance.md`, 2026-11-08).

## Extractors and the carried ability

**`#[authorize(A, bind = S)]`** loads and authorizes through the service into the
operation's `Authorized<A, E>` parameter, and needs an `AbilityGuard` on the route:
on HTTP and GraphQL 404 absent, 403 denied; on an MCP `#[resource]`, through the
same load, one *Resource not found* for both, which hides existence.

Each edge's authz half lives in `nest-rs-authz` and its data-layer half in
`nest-rs-seaorm`, each behind the matching edge feature — **the split avoids a
dependency cycle.** What they guarantee:

- **The guard attaches the ability, and the request carries it**: on every
  edge `AbilityGuard` attaches the caller's ability at the request — a route's
  guards, a gateway's upgrade, the `EndpointGuards` a module declares for
  `/graphql` or `/mcp` (one authn→authz ordering) — and it is ambient for
  every unit the request carries: each GraphQL operation and MCP tool call,
  each WS message, a hook holding it off its reads. Nothing re-runs the chain below
  the request. A
  `#[public]` unit no ability reached, on an opened MCP endpoint or under an open
  upgrade, takes its actor's rules, or the visitor's, at its own `AbilityGuard`, the
  pool's included. **A visitor reads only inside a unit naming its read**
  (`#[public(Action, Entity)]`): `current_ability` answers none for a visitor
  anywhere else in a request, a bare `#[public]`, a nested GraphQL field, a relation's
  batch, so each fails closed where no line names the read. A job holding one reads by its
  grant, `with_ability` being the line there, and `TestApp::unit` stands in for a unit
  naming its read.
- **A dataloader batch** runs off-task under what its request carries into a
  batch — the request's values, never one field's, and the pool — captured
  where a field asks the loader; nothing is bound for it. A generated loader
  answers wire rows, judged there for that request's caller. Where a root field's
  own guards attached the ability its request did not carry, the batch carries it
  too, and a query's or a mutation's answer and loader parameters read through
  loaders of the field's own (`UnitLoaders`), so no other root field's keys join
  a batch judged under it; a subscription's opening reads the request's, its
  operation holding one root field — GraphQL §5.2.3.1, which async-graphql does
  not hold, so a second is refused before either opens.
- **No edge has a data context of its own**: each runs its unit through the
  port, so commit and rollback cannot drift between edges.

## Queue attempts and ticks

**The unit is the edge's own `run_unit` around the pipeline** — a queue
attempt's, a tick's and a listener call's through `run_unit_bounded` — so the
interceptors and exception filters run inside the transaction, and a failure
a filter answers still rolls it back.

**One transaction per attempt is the default**, settled through the same
`finalize`: a job failing halfway leaves nothing for its retry to write again.
Being lazy, it needs no safe/mutating verb — a job that never touches the
database opens none, and **a read is a touch**, so a read-only job pays a
`BEGIN`/`COMMIT` and holds a connection for the attempt.

**`transactional = false`** runs on the pool, for two shapes only: a job
bracketing long work that is not the database's, and a job keeping a
`Checkpoint` — a save is stored at once while a rolled-back attempt undoes its
writes, so a retry would resume past undone work. The decorator refuses a
`Checkpoint` on a transactional method. Such a job owns its idempotency.

**An abandoned attempt holds its row locks until its statement drains.**
Dropping an attempt mid-statement — a worker's drain window closing, the
scheduler stopping a tick at its shutdown bound — leaves its transaction open,
because sea-orm cannot send the rollback while the connection is busy. The
framework owes the event: one `warn` on `nest_rs::orm` with
`outcome = "abandoned"` for a drop before settling; a drop during it leaves the
settle running on the driver's task, which the close drains. A body cut at its
deadline is settled, not abandoned, but its rollback likewise waits for the
statement it left running, which the statement budget bounds: nothing cancels
that statement sooner.
A worker's `shutdown_timeout` is therefore the ceiling on how long a dying
worker holds locks.

**An escaped executor fails the attempt whether or not it had opened
anything** — a spawned task holding it can still open a transaction and write,
so `FinalizeOutcome::Escaped` carries no "nothing opened" flag a reader would
take for a promise.

**An unsettled attempt carries why, and the database says so.** A failure
*before* `COMMIT` left nothing durable, so connection loss and conflicts are
retryable; *at* `COMMIT` only a serialization conflict or deadlock is, and an
in-doubt commit aborts, because replaying what may have landed writes twice.
The framework replays only what it knows rolled back (`retry.rs` holds the
two predicates), so **a timeout is retryable only when it cut the body**: a
commit that did not answer within the driver's settle net is in doubt
(`Unhonoured::IN_DOUBT`), dead-lettered and reported `retryable=false`. **The promise is the transaction's, not the attempt's**:
work stepped outside it — an HTTP call, `non_transactional` — is replayed with
the body.

**The transaction bounds a retry, never a redelivery.** Delivery is at least
once (`CLAUDE.md`, hard "no"), so a worker dying between `COMMIT` and recording
the outcome redelivers a job whose writes landed. The default removes the need
for an idempotency key against the framework's retry, not against the
backend's redelivery; `transactional = false` needs one against both.

**A schedule reports the classification and does not act on it**:
`#[every]`/`#[cron]`/`#[after]` have no retry budget or dead letter, so the
next occurrence is the same either way.

## Dataloaders and relations

**A `#[dataloader]` batch method** lives on the service, uses `Repo`, and
returns `Result<HashMap<…>, E>` — infallible only when it truly cannot fail.
A database error is never an empty batch.

**Relations resolve themselves.** A `belongs_to` or `has_many` field marked
`#[expose]` on an exposed entity becomes a GraphQL field resolved by a
dataloader that `#[expose(name = "…", service = <Path>)]` emits on the owning
service, reached from the inverse side without naming the other service. Every
batch goes through `Repo::scoped(Action::Read)`, so row filtering applies, and
builds each wire row judged for the caller (*The wire row*).
The serving adapter imports the target's module, and a schema serving a
field whose loader's owner the app does not reach — a relation, a root field's
or a `#[field_resolver]`'s `&DataLoader<L>` parameter — is refused at boot naming
that module. Leaving a relation unexposed opts it out; write a
`#[field_resolver]` for a custom shape.

**A service touching another entity injects that entity's service; the FK
loader belongs to its owner's service**, never the consumer's.

async-graphql allows one `#[ComplexObject]` per wire type, so a custom
`#[field_resolver]` and an auto-resolved relation cannot share an entity: pick
one source per type.
