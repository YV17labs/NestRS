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
- **A truly contextless path** — a shutdown hook — keeps an injected
  `Arc<DatabaseConnection>`, because no executor exists there.

No webhook ingress exists; `Repo::unscoped`'s rustdoc states the bar a
signature-authenticated one must clear before it ships. Auditing the escapes
is one grep per method name.

**`Repo` has no scoped bulk delete.** A retention purge selects through
`Repo::scoped(Action::Delete)` and deletes row by row through `Repo::delete`,
bounded per run — the demo's notifications purge is the shape.

## Two request-scoped `task_local!`s

Singletons have no other way to read per-request state: the **executor**
(`nest-rs-database` owns the seam and the `Executor` trait, `nest-rs-seaorm`
the pool-or-transaction implementation) and the **ability**
(`nest-rs-authz`).

**The executor** is installed by the `DbContext` interceptor, in the innermost
transport band, so it covers controllers and self-mounted surfaces alike. A
safe method runs on the pool; a mutating one gets a **lazy** transaction —
`BEGIN` deferred to the first data-layer touch — committed on 2xx/3xx and
rolled back otherwise, including on a `MappedError`-tagged response. Guards
run inside it, so a denied mutation never touches the data layer and opens no
transaction at all.

**The ability** is installed inside the per-route guards by the `#[routes]`
shaper — the one seam that runs after `AbilityGuard` and still wraps the
handler — so `nest-rs-http` knows nothing of authz or the ORM.

## What waits for the commit — the event, and nothing else

Only the unit of work holding the transaction knows whether its writes
landed. `nest_rs_database::after_commit(work)` hands work to the ambient
executor; every edge settles through `LazyTransaction::finalize`, which runs
it once the transaction committed (or the boundary succeeded with nothing to
roll back) and drops it on every other outcome. Held work runs on the pool,
under the scope and ability it was registered with. A panic in it is contained
and logged at `error` — the commit already stood, and an unwind would turn it
into a failed attempt a queue replays.

**`EventBus::emit` dispatches through `after_commit`, without being asked.**
An event announces a fact, and a fact its transaction rolled back never
happened: a listener never pushes a job, notifies or calls out about a write
that did not land, and a statement it fails never poisons its emitter's
writes. With nothing to wait for it dispatches before `emit` returns.

- **Waits:** every boundary that settles through `finalize` — a mutating HTTP
  request and the GraphQL mutation riding it, a WS message, an MCP operation, a
  queue attempt, a scheduled tick.
- **Runs at once:** a pool — a safe request, a read-only GraphQL batch, a
  subscription, `transactional = false`.
- **Not waited for:** a transaction the caller opened itself (`Executor::Txn`,
  or one held privately like `retry_on_conflict`'s) — its commit is the
  caller's, so the caller emits after its own `commit`.
- **Refused:** `JobProducer::push`, a WS broadcast, a storage write. Each is a
  call whose answer its caller reads — a receipt, `UniqueKeyHeld`, a send
  failure, an object key — and a deferred call would report `Ok` for work that
  may never be filed. Inside a transaction it happens where it is written; the
  price is a job a later rollback leaves behind. Work that must follow the
  commit is an event whose listener pushes.
- **Another ORM's driver** owes `Executor::after_commit` on every handle its
  boundaries settle; the trait's default runs the work at once, which is right
  for a pool only.

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
safe scalars; any other hidden column takes `#[wire_default(…)]`, a placeholder
stripped before the body ships. **It is sound only where no ability rule
predicates on that column** — otherwise the mask compares the placeholder and
silently filters rows.

`#[authorize(Action, Entity)]` beside the operation is the arming declaration
on every edge; the decorator emits the gate and the mask, and that is what
makes posture greppable:

- **HTTP** (`#[routes]`) — the `Authorize<A, E>` extractor `#[crud]` also
  emits, plus the response shaper. A masked-out key is omitted.
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
reaches** — a hand-built `WsServer::emit`, a hand-written MCP
`ServerHandler`. Inside a decorated handler they bypass the posture nobody
can then `rg` for.

## Extractors and bridges

**`Bind<A, S>`** loads and authorizes through the service (404 absent, 403
denied) and needs an `AbilityGuard` on the route; **`Scope<E, A>`** hands a
hand-built query the explicit `Condition`.

The authz bridges live in `nest-rs-authz` and the data-layer bridges in
`nest-rs-seaorm`, each behind the matching edge feature — **the split avoids a
dependency cycle.** What they guarantee:

- **The guard scopes the operation** on GraphQL and MCP: their bridges run the
  guard chain in-band (one authn→authz ordering) and install the caller's
  ability, with or without a data context. WS has no bridge: a gateway's
  upgrade already ran the chain, and only the ability is re-installed per
  message.
- **A dataloader batch** runs off-task under `LoaderScope`, which snapshots the
  ability and a pool executor.
- **WS and MCP data contexts share one dispatch path**, so their
  commit/rollback semantics cannot drift: a read-only message or tool opens
  no transaction, a writing one commits on success and rolls back on the
  transport's error shape.

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
`outcome = "abandoned"`, whether the drop came before settling or during it.
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
two predicates). **The promise is the transaction's, not the attempt's**:
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
