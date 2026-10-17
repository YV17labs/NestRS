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
ability means system work, unscoped. Route-model binding (`Bind`) goes through
`CrudService::access`.

**The named exceptions — there are no others.** The ability filter is dropped
only through a named `Repo` escape, and each escape's rustdoc states the bar
its callers must clear; the method's doc is the authority, not this list.

- **`Repo::unscoped` / `unscoped_by_id`** — reads with no ability:
  pre-authentication credential lookup, `CrudService::access` (which must tell
  `Denied` from `Missing`, so it filters explicitly after the load), and global
  uniqueness probes such as `resolve_unique_slug`.
- **`Repo::insert_unscoped`** — the write pendant, on an explicit connection:
  pre-principal provisioning (a social-login user) and principal-less system
  work. An authorized create stays on `Creatable`.
- **`Repo::with_tombstones`** — the ability-scoped read that keeps
  soft-deleted rows: an administrator's view of deleted records, a retention
  job. Every other read hides them. It reads only: no `Repo` write un-deletes
  a row (`Repo::restore` is planned for 7.x).
- **A truly contextless path** — a shutdown hook — keeps an injected
  `Arc<DatabaseConnection>`, because no executor exists there.

No webhook ingress exists; `Repo::unscoped`'s rustdoc states the bar a
signature-authenticated one must clear before it ships. Auditing the escapes
is one grep per method name.

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

**A database failure is classified once and said once.** `DbOutcome` reads a
`DbErr` into the class every edge answers — a unique violation is a conflict,
a row a write found gone is not found, a statement Postgres cancelled (`57014`)
is unavailable, a `503` said at `warn` and retryable inside a transaction,
anything else is opaque — and
`ServiceError`, `#[crud]` and the product read it rather than matching the
driver's error again. The server's message quotes the input it refused, so a
driver says its errors through a `SafeLink`, by code and schema names; held by
the unit tests beside each.

## The ambient context

Singletons read per-request state from core's one ambient context, never from
a task-local of their own: the **executor** (`nest-rs-database` owns the seam
and the `Executor` trait, `nest-rs-seaorm` the pool-or-transaction
implementation) and the **ability** (`nest-rs-authz`) are two of its values,
and a package adds its own by implementing `AmbientValue`, whose `carry` says
how far the value travels (`.claude/decisions/ambient-carry.md`).

**A continuation that can outlive the transaction never holds it.** Where a
unit continues after the future that accepted it, the executor steps down to
the pool — a `Pool` unit of the binding that opened it: a streaming body, a
DataLoader batch, work held for the commit (an emitted event's dispatch, whose
listeners each open a unit of their own through that binding) — and is dropped
on a task handed to
`TaskContext`, a connection's next message and a job's envelope, where a write
opens a unit of its own. The ability crosses everywhere but a job's envelope.
One exception is planned: a mutation operation's batches will run on the
operation's own transaction, joined before the field's savepoint settles.

**The executor** is installed by each edge running its unit of work through
the one port, `nest_rs_database::run_unit`, over the `dyn UnitOfWork` the
driver's binding provides — resolved once at mount or boot, never per unit,
or read by the bus off the unit its emitter ran under;
with none bound a unit runs bare and `Repo` fails closed. A driver implements
that one trait and serves every edge; an app lists no data context. Each edge
commits on its own unit's outcome: an HTTP request on 2xx/3xx without a
`MappedError` (a safe method runs on the pool, the unit opened by the
`DbContext` interceptor in the innermost band); a WS message unless it answers
an error frame, a connection hook unless it unwound; an MCP operation on `Ok`
with no tool error in band (`isError`); a queue attempt, a tick and a
listener call on `Ok`. A
unit of its own gets a **lazy** transaction — `BEGIN` deferred to the first
data-layer touch, **a read included**. Guards run inside the unit, so a denied
unit never touches the data layer and opens no transaction at all.

**The ability** is what `AbilityGuard` attaches (`cx.attach`): ambient for the
rest of the unit once its guard stage passed. A route no response mask shapes,
an `#[sse]` stream among them, runs under the `UnmaskedReply` marker its
shaper attaches, and
`current_ability` withholds the ability there, so a read the mask would have
scoped fails closed — and `nest-rs-http` knows nothing of authz or the ORM
(`.claude/decisions/unmasked-reply-withholds-the-ability.md`).

## What waits for the commit — the event, and nothing else

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
deadline**: opened through the binding its emitter ran under
(`current_unit_of_work`), bare under none, one transaction unless
`transactional = false`, its `Err` rolled back and filed at `error` while the
chain goes on — so an event a listener emits waits for that listener's commit.

- **Waits:** every unit of its own — a mutating HTTP request and the GraphQL
  mutation riding it, a WS message or connection hook, an MCP operation, a
  queue attempt, a scheduled tick, a listener call.
- **Runs at once:** a pool — a safe request, a read-only GraphQL batch, a
  subscription, `transactional = false`.
- **Not waited for:** a transaction the caller opened itself (`Executor::Txn`,
  or one held privately like `retry_on_conflict`'s) — its commit is the
  caller's, so the caller emits after its own `commit`. Inside a unit of
  work, a listener reached sooner runs in a unit of its own beside it, never
  inside it; outside every unit it runs bare, inside the caller's
  (`.claude/decisions/listener-transaction-of-its-own.md`).
- **Handed on:** an atomic block (`nest_rs_database::atomic`) hands its
  after-commit work to the enclosing unit on release and drops it on
  rollback; on the pool it is a transaction of its own and settles its work
  like any boundary.
- **Refused:** `JobProducer::push`, a WS broadcast, a storage write. Each is a
  call whose answer its caller reads — a receipt, `UniqueKeyHeld`, a send
  failure, an object key — and a deferred call would report `Ok` for work that
  may never be filed. Inside a transaction it happens where it is written; the
  price is a job a later rollback leaves behind. Work that must follow the
  commit is an event whose listener pushes.
- **Another ORM's driver** owes `Executor::after_commit` on every handle its
  boundaries settle; the trait's default runs the work at once, which is right
  for a pool only. What it holds runs under the `Commit` capture taken where
  the work was registered, its own pool re-installed inside under the scope
  read there (the trait method's rustdoc).

## Write capability is segregated, never a placeholder

`CrudService` carries the **read** half only. The write half is three opt-in
traits a resource implements when it genuinely offers the operation:
`Creatable`, `Updatable`, `Deletable`. A read-only resource declares no
`Create`/`Update` type — no `_unused` stub, no no-op conversion.

**`#[crud]` generates only the operations a resource has** (`ops = [..]`, all
five when omitted), over HTTP and GraphQL alike. An op listed without its input
type is a compile error, and an op whose trait the service lacks fails to
resolve: a forgotten operation is a build break, never a no-op mutation on the
wire.

## Response masking — one core, every transport

Exposure is `#[expose]` (`CLAUDE.md`, hard "no"); a column a later migration
adds never leaks by omission, and the entity *is* the wire contract, so a
handler returns the exposed type (`Json<User>`), never `Model`.

The mask is `nest-rs-authz`'s `wire_mask`, value-level and **fail-closed**:
the wire JSON is rebuilt into a `Model`, masked by the ability, and cut back to
the exposed keys, so an unrestricted field grant cannot leak an unexposed
column. An irreconcilable body or a missing ambient ability fails closed on
every transport.

Rebuilding needs a value for every unexposed column. The macro defaults the
safe scalars, any other hidden column takes `#[wire_default(…)]` (a placeholder
stripped before the body ships), and the macro names every column it may
invent and the key of every column it exposes. **A rule
the in-memory mask cannot decide — a relation, an unexposed column, one the
value left out — is trusted to the `Repo` read on a decorated reply and refused
on a pushed value** (a subscription item, `masked_reply`): only the read
applied it. A decorated reply
whose rows did not come through `Repo` under the same ability keeps that trust,
which is the limit (`.claude/decisions/mask-provenance.md`).

`#[authorize(Action, Entity)]` beside the operation is the arming declaration
on every edge; the decorator emits the gate and the mask, and that is what
makes posture greppable. **The gate is one function**, `class_gate`, on every
edge; the mask is each edge's, chosen by its protocol:

- **HTTP** (`#[routes]`, which `#[crud]` emits under) — `Authorize<A, E>`'s
  class gate, run with the guards, plus the response shaper. A masked-out key
  is omitted.
- **GraphQL** (`#[operations]`) — the class gate and a mask around the
  returned value; `unmasked` keeps the gate and leaves masking to the body,
  for a shape the round-trip cannot see through (a cursor connection). A
  subscription masks each item as a row: refused means dropped. **A
  masked-out non-nullable field fails the whole operation**, since the schema
  cannot ship it: a column a field grant may mask is `Option` on the entity.
- **MCP** (`#[tools]`) — GraphQL's caveat without a selection set to soften
  it: rmcp needs the typed value back, so a mask stripping a required key
  refuses the operation. A masked operation spells its return `Result<…>`.
- **WS** (`#[messages]`) — masks like HTTP: the envelope promises no schema,
  so a stripped key is omitted. A masked message returns a *literal* `Result`,
  because the reply shape is decided syntactically.

`masked_reply` / `masked_output_ambient` are for surfaces **no decorator
reaches** — a hand-written MCP `ServerHandler`, a value built for the caller.
Both mask for the ambient principal, never per recipient: a `WsServer::emit`
to other connections carries what the sender may read. Inside a decorated
handler they bypass the posture nobody can then `rg` for.

## Extractors and bridges

**`Bind<A, S>`** loads and authorizes through the service (404 absent, 403
denied) and needs an `AbilityGuard` on the route; **`Scope<E, A>`** hands a
hand-built query the explicit `Condition`.

The authz bridges live in `nest-rs-authz` and the data-layer bridges in
`nest-rs-seaorm`, each behind the matching edge feature — **the split avoids a
dependency cycle.** What they guarantee:

- **The guard scopes the operation** on GraphQL and MCP: their bridges run the
  guard chain in-band (one authn→authz ordering) and install the caller's
  ability. WS has no bridge: a gateway's upgrade already ran the chain, and the
  ability it attached crosses the connection into each message and hook.
- **A dataloader batch** runs off-task under what its request carries into a
  batch — the request's values, never one field's, and the pool — captured
  where a field asks the loader; nothing is bound for it.
- **No edge has a data context of its own**: each runs its unit through the
  port, so commit and rollback cannot drift between edges.

## Queue attempts and ticks

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
batch goes through `Repo::scoped(Action::Read)`, so row filtering applies.
Leaving a relation unexposed opts it out; write a `#[field_resolver]` for a
custom shape.

**A service touching another entity injects that entity's service; the FK
loader belongs to its owner's service**, never the consumer's.

async-graphql allows one `#[ComplexObject]` per wire type, so a custom
`#[field_resolver]` and an auto-resolved relation cannot share an entity: pick
one source per type.
