# A push inside a transaction is written into it, and a relay files it after the commit

Settled on 2026-10-19 (DATA-04, 04b). A push called the backend where it was
written: inside a unit's transaction, a rollback then left an orphan job behind.
The rule's answer was "an event whose listener pushes", which loses the job when
the process dies between the `COMMIT` and the push, and turns a push the backend
refuses into a log line while the client already holds its `2xx`. Correctness
decided, and "nothing to wire" decided how it is armed.

**Chosen — the transactional outbox, on the unit's own executor.** The queue
port seals a push as before, then hands the sealed jobs to
`Executor::write_outbox` of the ambient executor: a driver keeping an outbox
writes them on the transaction that executor holds, so they commit or roll back
with the caller's writes, and the receipt names a job that exists exactly when
those writes do. A push of many is one write, so it is atomic too. The relay —
the queue port's transport, `OutboxRelay` — claims committed records with
`FOR UPDATE SKIP LOCKED`, hands them to the backend, then deletes what was filed
and makes what failed due again after the job retry backoff. Delivery stays at
least once: a relay dying between its call and its settle sends the record
again, which `JobProducer::enqueue`'s contract (an envelope whose job the
backend holds is filed once) makes harmless while the job is held, and the
handler's idempotency on `Attempt::job_id` covers a job that already ended.

- **The executor, never the request scope.** The write is a method of the
  ambient executor, the very transaction the record joins: an event listener's
  unit and a WebSocket hook carry no request scope, and they push too.
- **The port's records are opaque to the database port.** `OutboxRecord`
  carries the sealed envelope as text; only the queue port reads it back, and
  it checks the record names its own job before filing it.
- **Armed by the relay, attached by the data driver's binding.** A store writes
  nothing until the relay proved its table at boot — through the boot's proof,
  so a missing table refuses at once and a lost connection is retried within
  the store's net — and an app running no relay
  (a test harness, an app with no queue backend) files every job at once. The
  relay's module (`OutboxModule`) is imported by the binding that keeps the
  store (`SeaOrmDatabaseModule` under the umbrella's `queue` feature): a queue
  backend owes nothing beyond its id contract, and an app lists nothing. A
  missing table fails the boot naming the remedy.
- **What a caller opened itself keeps no outbox.** A pool, an
  `Executor::Txn` and a block nested in it answer `None`, so their jobs are
  filed at once: nothing ties a handle the caller built to the store a relay
  armed, and `after_commit` runs at once there for the same reason. The pool a
  unit's boundary hands on — to its after-commit work, to a step out of its
  transaction — is the binding's, so it keeps the outbox as a pool unit does:
  a push there is filed at once, a block of its own keeps its pushes until it
  commits, a cancel deletes a record no relay has filed.
- **A cancel waits for a relay's claim, never skips it.** Skipping a record a
  relay holds would answer `false` between the relay's claim and its call, and
  the job, filed a moment later, would run. The claim never waits on a unit's
  rows, so the wait cannot deadlock, and the statement budget bounds it.
- **A record is sealed only for a handle that keeps it.** The port hands
  `write_outbox` records built as they are read, so a push filed at once pays
  for no record it throws away.
- **A unique key is the relay's to check.** The backend is not asked at the
  push, so the record is superseded once committed — deleted, said at `info`
  naming both jobs — rather than refused to the caller.
- **The way down cuts no call.** Once the app stops, the relay starts none; the
  one in flight answers within the port's net, and its settle gets what is left
  of it, or the record is left to the next relay.
- **A record holding no job a relay can file is deleted at `error`**, said
  without its text: kept, it would come back on every pass and file nothing.
- **The framework's tables are `nestrs_<pattern>`**, as its datastore keys are
  `nestrs:…`; the table is created by a framework migration an app's migrator
  lists (`OutboxMigration`), and a later change ships as a migration of its own.

**Refused:**

- **A listener pushes after the commit** — the prescribed pattern this retires.
  A crash between the `COMMIT` and the push loses the job, and a refusal the
  backend answers reaches no caller.
- **A delivery API of the outbox's own**, an `@OnOutboxMessage`-style handler
  beside `#[process]`: two ways for a job to run, each with its own retries and
  idempotency. The outbox only defers the push; the job still runs through the
  queue's one consume loop.
- **Routing through the request scope** (`current_request_scope()?.root()`):
  a listener's unit and a WebSocket hook have none, so their pushes would have
  bypassed the outbox.
- **An explicit import** of the relay by the app: an app forgetting it keeps the
  lossy semantics without a word.
- **Pushing before the `COMMIT`** — the rule's second alternative — leaves an
  orphan job on every rollback.
