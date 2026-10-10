# One ambient context, and each value says how far it travels

What a unit carries for the code it calls lives in four task-locals across
three crates — the request context in `nest-rs-core`, the executor and its
scope in `nest-rs-database`, the ability in `nest-rs-authz` — each carried by
hand where a unit continues on another future. The carriers disagree: the
after-commit hold re-installs the ability and the pool, `EventBus::emit`
captures the correlation alone, seaorm's `LoaderScope` installs the ability
around a DataLoader batch, and a streaming body re-enters the request context
and the span only, so a `Repo` read inside an `#[sse]` stream finds no executor
and no ability. A new value — a tenant, a deadline, a package's — needs every
edge changed, and a carrier that misses it fails open for a tenant filter.

**One map, in core.** The values ride inside the request context's existing
`Arc` task-local, a few inline before a spill: one task-local, one allocation
per install. A value travels only through `AmbientValue`, whose
`carry(self: Arc<Self>, into: Continuation) -> Option<Arc<Self>>` says what
crosses each continuation — itself (the default, except into a job's
envelope), a narrower value, or nothing. Implementing it is the visible line
that lets a type travel: a guard's principal type opts in
(`impl AmbientValue for Claims {}`), and nothing travels by default.

**Six continuations**, where a unit goes on once the future that accepted it is
no longer the one running: `Body` (a streaming response, polled after the
handler returned), `Batch` (a DataLoader batch it waits on), `Task` (work handed
to `TaskContext`, which may outlive it), `Commit` (work held for its commit:
after-commit callbacks, an emitted event's listeners), `Connection` (the next
units of a connection it opened: a socket's messages, a graphql-ws operation)
and `Envelope` (a job it pushes).

**A continuation that can outlive the transaction never holds it.** The
executor steps down to a pool on `Body`, `Batch` and `Commit`: a body is polled
after the commit, a batch serves every field of the request, and after-commit
work runs once the transaction is gone. It is dropped on `Task`, where a
detached write must open a unit of its own, and on `Connection`, where each
message opens its own. One exception is planned: a mutation operation's
batches run on the operation's own transaction, so its response reads its own
writes, and are joined before the field's savepoint settles, so none outlives
it. It lands with `feat/graphql-operation-unit`, or in 7.x, changing no API,
if the join proves infeasible.

**Nothing but the envelope's keys crosses into a job.** A job is system work,
run later and possibly elsewhere: it carries `traceparent`, `tracestate`,
`actor_id` — audit, never authorization — a tenant key and W3C `baggage`, and
no ability, principal or package value. A `Deadline` crosses into a `Batch`
alone, the one continuation the unit waits on.

**A nested unit never leaks into its parent's batch.** A GraphQL root field
installs a child context recording its parent, and a `Batch` capture takes the
nearest unit that is not nested, so a loader shared by several fields carries
none of one field's attachments. A WebSocket gateway captures the ambient at
the upgrade — the ability its guards attached included — and re-installs it
around each message and connection hook; a DataLoader spawner captures for
`Batch` eagerly. Neither needs a bridge or a context provider of its own.

**Reading is not deciding.** A service can read a principal through
`ambient::<T>()`, as it reads a `Ctx<T>` handed to it today; the hard "no" on an
authentication or authorization decision outside a guard stands, held by
review, and the authentication pages say so.

Prior art: OpenTelemetry's `Context` (immutable values the runtime
propagates), .NET's `AsyncLocal`, Node's `AsyncLocalStorage` (NestJS's CLS).

**Refused:**

- **A task-local per crate**, the shape it replaces: a value needs a carrier at
  every continuation of every edge, and a missing carrier fails silently.
- **A second task-local beside the request context, or an `http::Extensions`
  map in the kernel**: two installs per unit, and two places to look.
- **A carrier registry** that each continuation consults: how far a value
  travels is a property of its type, written beside it; a registry puts it a
  module away, and a value nobody registered would cross nothing, silently.
- **The pool handed to a detached task**: a write in a task that outlives its
  unit would run outside any unit of work.
- **A principal type that travels by default**: an opening is an explicit
  line.
- **A WebSocket ability bridge, or an `around_hook` on the socket context**: a
  public type added and removed within 7.0, for what the `Connection` capture
  does.

**Status (2026-10-10): decided, not landed.** It lands with
`feat/ambient-context`, which also turns the DataLoader's lazy capture into
the eager `Batch` one; it appends an entry here where it departs.
