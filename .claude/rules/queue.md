---
paths:
  - "crates/nest-rs-queue/**"
  - "crates/nest-rs-queue-macros/**"
  - "crates/nest-rs-redis/**"
  - "crates/nest-rs-schedule/**"
  - "crates/nest-rs-schedule-macros/**"
  - "crates/nest-rs-worker/**"
---

# Queue and schedule — the port, the Redis adapter, delivery

Ports & Adapters is `architecture.md`; a port's extension contract and its nets
are `container.md`. What a job's transaction promises is `data-layer.md`.

## The port owns the attempt; the adapter owns the transport

`nest-rs-queue` owns what a job attempt *is* — opening the envelope, continuing
or minting the trace, the `queue.job` span and ambient scope, catching a panic,
classifying the outcome (ok, retry, dead-letter, defer) within the method's
retry budget, the wait before a retry, the events and the operation line — in
`consume::attempt`, written and tested once. An adapter's consumer is a fetch
loop that builds a `Delivery` per job, calls it, and translates the
`AttemptOutcome` into its backend's vocabulary; `consume::discover` finds the
`#[process]` methods, module-gated. **An adapter that opens a `queue.job` span or
keeps a retry budget of its own has taken semantics it does not own**, and a
second adapter copies nothing.

- **`AttemptOutcome` is exhaustive on purpose**: a variant added later is a
  compile error in every driver, not a job one of them drops.
- **A job is named by the port.** The push mints a `JobId` (UUID v7) and seals it
  in the envelope; it keys everything kept about the job. A backend's own record
  id reaches a line only as `backend_id`. The wait before a retry is jittered by
  a hash of the id and the attempt, never a random draw, so it is reproducible.
- **A backend declares each capability it honours, one at a time** — never
  `Capabilities::ALL`, so one the port names later is not claimed before it is
  honoured — and the port refuses a declaration or a push option the backend
  lacks before the backend sees it. Held by an exhaustive table test in
  `nest-rs-queue`.
- **`#[process(concurrency = N)]` bounds one method on one replica**, from a
  permit pool of its own, so another method's jobs never wait on it; throughput
  beyond that is replicas, added by a queue-depth autoscaler rather than CPU.
  Every backend owes it, so it is not a capability
  (`.claude/decisions/process-concurrency.md`).
- **A newer wire version is handed back, for a bounded time.** An older worker
  cannot read or re-seal it, so the attempt defers without spending an attempt
  and warns per delivery naming both versions; past a patience it is
  dead-lettered once, at `error`. The wait counts from the **first hand-back**,
  which the backend keeps per job, never from the push — a delayed push would
  otherwise be dead-lettered by the rolling deploy deferral exists for. A
  backend keeping no record falls back to the id's age and says so to its driver
  authors. An older version is refused: reading an older shape is the bumping
  release's decision.
- **`#[input]` stays off queue scaffolds.** Unknown-key rejection suits an
  untrusted caller; a job's sender is a producer possibly one deploy ahead, whose
  extra field would dead-letter on attempt one. Scaffolds write tolerant serde
  derives; a payload opting into `#[input]` versions its producer and workers
  together.
- **A queue per runtime key is not offered** — `#[queue]` takes `name` and `job`
  only, and a key that varies rides in the job; why is `#[queue]`'s rustdoc.

## Delivery is at least once

`CLAUDE.md`'s hard "no" states the contract, and no rustdoc, line or page
promises more; this is how the Redis backend keeps it.

- **The worker guards every delivery in keys of its own.** An attempt runs only
  under the job's lease; its terminal outcome writes the settled mark and drops
  the lease in one script; a delivery arriving while the lease is held is
  **handed back, never acknowledged**; one arriving after the job settled is
  acknowledged without running, answered as the first was, **while the mark
  lasts**. The mark has one fixed span; a redelivery after it runs the job
  again. **The guard may delay a job, never lose one**, and each step is one
  script or one command (`.claude/decisions/queue-delivery-guard.md`).
- **Nothing is kept forever, and nothing still owed lapses early.** A waiting
  job's records live a fixed time past its due instant, renewed by every
  delivery. A unique claim is held shorter until Redis confirms the filing, so a
  push that never learns whether its job was queued does not block every retry
  under the key — said at `warn`, erring toward at least once.
- **apalis never retries, and never ends a job, on its own.** A dead letter is
  apalis's `Abort`; a retry, a held lease, a throttle window and a shutdown
  re-file the job on the schedule (scheduled first, out of flight second), never
  a wait holding a permit. apalis's own attempt cap is lifted on every record the
  adapter files (`LIFTED_CAP`), and a unit test pins the serde field names it is
  written through.
- **A shutdown stays inside `shutdown_timeout`.** The worker stops fetching,
  lets attempts run for the window less a reserve, then interrupts and hands
  each job back, due at once for another replica. An interrupted attempt **gives
  its start back first**, the one write whose loss costs the job something;
  nothing starts past the window; what the drain still waits on at its end is
  said at `error`. The drain is the worker's own because apalis drops the
  acknowledgement of a task ending while its worker drains. An interrupted
  attempt files its line `cancelled` from the port.
- **The Redis capabilities**, each in keys of its own and proved by its own e2e:
  a delayed record is promoted by the producer that filed it and, at the
  fetch's pace, by a worker for its own queue; a unique key is claimed
  atomically before filing and released at settle or cancel — **at most once
  over pushes, never a lock**; a cancel writes its tombstone only while no
  attempt holds the lease, so `Ok(true)` means the job never starts; a throttle
  is a fixed window per queue (no two replicas need agree on a clock, and a
  window's edge can admit twice the limit, which the page says), and a refusal
  shuts that replica's fetch for the method until the window ends; a checkpoint
  is one key per job, cleared at its terminal outcome.

## The apalis boundary

apalis is the Redis job runtime the binding drives, not the port: no apalis
type leaks, and a second backend implements the port without configuring apalis.
The adapter crate is named for the storage a caller touches (`architecture.md`).

- **apalis's structures are apalis's** (`CLAUDE.md`, hard "no"). The framework
  reads and writes them only through apalis's public API, and files its own
  records beside them under words apalis does not use. An apalis behaviour the framework cannot live with is
  worked around in keys of its own and reported upstream.
- **The fetch is apalis's**, and `buffer_size` and `poll_interval` are its only
  levers short of a fork. The worker sizes the buffer to the method's
  `concurrency`, capped where apalis's scripts stay safe (`MOST_PER_FETCH`), and
  `poll_interval` stays under the orphan threshold, since apalis sweeps silent
  peers on the poll. The ceiling this sets, and the idle cost of every poll, are
  stated where the queue's scaling is documented.
- **Redis Cluster is unsupported** — apalis's scripts touch keys across hash
  slots — and the queue pages say so.

## The Redis connection

`RedisConnection` is the connection: one multiplexed manager opened by
`RedisModule::for_root` and shared by every binding, each of which declares it
runs after the connection's factory. `throttler` and `schedule` are crate
features because each pulls a port crate an app may not need.

- **The boot proves the connection with a `PING`**, and what fails the same way
  every time fails at once — a budget out of range, an unparsable URL, unusable
  TLS settings, a refusal naming the deployment's own settings. **That list is an
  allow-list; every other answer is retried** within the connect budget, then
  fails naming the endpoint, never the URL, which may carry a password.
- **Every command a caller waits on answers or fails within the budget.** A
  command whose answer is the only record of what it claimed — the fetch, the
  guard's admission — waits on the socket's liveness instead
  (`RedisConnection::without_budget`, `container.md`). `nest-rs-redis` asserts
  its default budget sits below every net it runs under.
- **TLS material beside a plaintext URL fails the boot**, since it would go
  silently unused; verification is never an option (`CLAUDE.md`).
- **The oldest Redis the docs claim is the oldest the e2e suite passed on at the
  release**, never one it has not run.
- **Each binding's docs page prescribes its ACL rule whole** — its namespace,
  the connection's commands, every command it or a script it runs sends,
  apalis's included, and nothing else; one user per binding. Held by
  `nest-rs-redis`'s e2e, which creates a user from the page's line verbatim and
  reads `ACL LOG` for any denial.

## A key a datastore holds is a name an operator types

A key sits in a KEDA trigger, a `SCAN` during an incident, an ACL — so it obeys
the naming law and is derived, not chosen:

```
nestrs:<concern>:<structure>[:<member>]
nestrs:queue:<queue>[:<structure>[:<job>]]
```

`<concern>` is the tail of the span target of the crate that **owns** the
concern (`nest_rs::throttler` → `throttler`) — never `redis`, which writes them
all and owns none. `<structure>` is one word, never the concern's own word again
and never a word apalis uses inside a queue's namespace. `<member>` is what
varies; a queue name holds no `:`. The queue puts its member first because
apalis derives every structure from the one namespace it is handed per queue,
so one queue is one prefix to `SCAN` and to scope an ACL. A key is the fourth
surface of one derivation: crate, span target, `<PREFIX>_<CONCERN>__*`, key.

- **Every fixed part is a `const` opening with `nestrs:`**, declared by the crate
  that writes the key, and a varying key is built from exactly one such constant.
- **No key prefixes another without a visible level** — `SCAN` matches by glob,
  the hazard `EnvFilter` makes of span targets; a qualifier goes one level down.
- **A key spelled outside Rust is built from one the code declares** — a chart's
  trigger or a page's `SCAN` never moves with a constant.
- **The leading segment is `nestrs`, fixed.** `NESTRS_ENV_PREFIX` renames the
  developer's variables; a key is the framework's machinery, and deployments
  sharing a Redis are separated by the logical database in the URL.

Held by a unit test over the key constants; a chart, a script or a page that
spells a key follows the constant by review. The table of keys is
`layout.rs`'s `//!`.

## The schedule

`#[scheduled]` orchestrates methods tagged with exactly one of `#[every]`,
`#[cron]` (optional `tz`) and `#[after]`; every literal, `tz` included, is
checked at compile time, and only `CronExpression` presets resolve at boot. The
scheduler is a `Transport` through `TransportContribution`.

- **A tick has no retry** — its retry is the next occurrence — and **a replica
  runs one occurrence of a job at a time**; an occurrence falling inside a run is
  skipped and counted. `concurrency` counts per replica. Work that must not be
  lost is a queue job the tick pushes.
- **Where a recurring job fires is declared, never inferred.** `replicas = "each"`
  (the default) fires on every replica; `"one"` fires **once per occurrence**, on
  the replica whose claim succeeds; `#[after]` refuses the key. The claim holds
  the occurrence and nothing else, so a run outlasting its period overlaps the
  next on another replica — stated on the page, on `Replicas::One` and on
  `OccurrenceLock` (`.claude/decisions/schedule-run-lease.md`).
- **A job is its crate, its host struct and its method.** The crate, because the
  lock is shared across a deployment's apps; not the module path, which a
  refactor moves. A rename starts a new job; `key = "…"` beside
  `replicas = "one"` pins the old identity and is refused elsewhere. Two
  once-jobs under one identity in one app fail its boot, naming both.
- **`"one"` is at most once, never at least once.** It claims through the
  `OccurrenceLock` port, selected by import (`RedisScheduleModule`); a reachable
  `"one"` job with no binding fails the boot naming it. A claim that errors, is
  answered stale or late, or is unanswered at shutdown skips its occurrence at
  `warn`; a claim answered before shutdown still fires, since no other replica
  can. An in-process lock is refused — it decides nothing across replicas — so
  the remedy sentence answers with `replicas = "each"`.
- **On the way down** the scheduler starts no tick and abandons a claim in
  flight; a tick still running gets the hooks' budget before the hooks, then is
  stopped, files its line `cancelled`, and is named in one `warn`.
