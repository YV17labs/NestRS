# A listener reached under a caller's own transaction runs in a unit of its own

Settled on 2026-10-17, when each event listener became a unit of work of its
own. A transaction the caller opens itself — `db.begin()` inside
`retry_on_conflict`, an `Executor::Txn` it installs — commits out of the
framework's sight, so the bus cannot hold a dispatch for it: an event emitted
there runs its listeners at once, before the caller's commit. What those
listeners run on had three answers. Correctness decided: a listener's answer
settles what it wrote, and nothing it fails reaches its emitter's writes,
whatever its emitter ran on.

**Chosen — a unit of its own, beside the caller's transaction.** The bus opens
the listener's unit through the binding its emitter ran under, as it does for
work held until a commit, so a listener means one thing everywhere: one
transaction (the pool under `transactional = false`), settled on its answer,
cut at its own deadline. What it costs, and the events docs say so: the
listener runs on another connection, so it does not see the rows the caller
has not committed; it waits on any row the caller locked — the caller is
itself awaiting `emit` — until its statement budget or its timeout cuts it;
and its commit stands if the caller rolls back afterwards. Emitting after the
caller's own `commit` avoids all three, and stays the advice.

**Refused:**

- **A passenger of the caller's transaction** — what a listener did before.
  Its `Err` cannot roll back what it wrote, since the caller commits it, and a
  statement it fails aborts the caller's transaction: the emitter's writes go
  down with a reaction, which the data layer's rule forbids.
- **A savepoint in the caller's transaction** (`atomic`, which sea-orm nests
  there). It sees the caller's rows and waits on none of its locks, but a
  released savepoint commits nothing: the listener's `Ok` would no longer mean
  its writes landed, they would stand or fall with a transaction the caller
  settles after the listener answered. The same listener would then be durable
  on its `Ok` after an emitter's commit and provisional under a caller's
  transaction — two meanings for one decorator. And it saves the database's
  half only: a job the listener pushed or a mail it sent outlives the caller's
  rollback either way.

**The limit.** The bus opens a unit only through the binding its emitter ran
under. An emit outside every unit of work — a task handed to `TaskContext`, a
binary of one's own, a test, a lifecycle hook while hooks run outside a unit —
has none, so its listeners run bare on whatever the emitter installed, a
caller's `Executor::Txn` included: there they are passengers. Closing it takes
a binding the bus reaches without its emitter, the one the container provides,
read when the listeners are wired; the bus holds no container today.
