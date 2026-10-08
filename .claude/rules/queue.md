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

## The port runs the loop; the adapter owns the transport

`nest-rs-queue` owns what consuming a job *is*, written and tested once in its
`QueueWorker`: discovery, module-gated; each method's permits and one task per
delivery; the attempt — opening the envelope, continuing or minting the trace,
the `queue.job` span and ambient scope, catching a panic, classifying the
outcome within the method's retry budget, the wait before a retry, the events
and the operation line; the lease's renewal; and the drain. An adapter
implements `JobConsumer` — hand leased deliveries over, renew their leases, end
each with one `Disposition` fenced on its lease — in its backend's vocabulary.
**An adapter that opens a `queue.job` span, counts a budget, holds permits or
drains on its own has taken semantics it does not own**, and a second adapter
copies nothing. `nest_rs_testing::queue_kit!` holds every adapter to the
contract (`.claude/decisions/queue-redis-streams.md`).

- **Two budgets, never one.** `retries` counts attempts that answered, in the
  envelope; `STALL_LIMIT` counts deliveries that ended unanswered, off the
  backend's delivery count, which every re-filing resets. A job past it is
  dead-lettered without running.
- **A lost lease cuts its attempt.** The backend answering `Lost`, or no
  renewal confirmed for a whole lease, cuts the attempt (its line files
  `cancelled`) and hands the job back fenced: the delivery holding it decides.
- **A later backend declares what it lacks, never what it has.** `Disposition`
  is non-exhaustive, and a variant added later reaches only a backend declaring
  the capability that names it, so no driver changes behaviour under it.
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
promises more; this is how the Redis backend keeps it, on Redis Streams
(`.claude/decisions/queue-redis-streams.md`).

- **Every transition is one script, fenced on the pending entry.** A write a
  delivery makes — its outcome, a renewal, a checkpoint — first reads the
  entry's owner and delivery count and writes nothing unless both are still
  what the worker received; a reclaim bumps the count, a renewal never does.
  The outcome, the filing of the job's next record and the release of what it
  held are the one script, so there is no order between them to get wrong.
- **No record outlives its job, and none needs an expiry.** The transition that
  ends a job removes every record of it but its dead letter, so a job is never
  delivered after it ended; a second delivery happens only once a lease lapsed
  under a worker, and that worker's writes then land nowhere.
- **Redis's clock decides when a job is due** (`TIME` in the script), never a
  host's; the push measures a delay, the script anchors it.
- **The server is Valkey, its latest release** (`decisions/valkey-only.md`):
  a command its release lacks is never the path, and Redis's own commands are
  not Valkey's.
- **One queue, one hash slot**: every key a script names is the queue's own.
- **The Redis capabilities**, each proved by its own e2e: a delayed record waits
  in `due` and is filed by any worker draining the queue; a unique key is
  claimed in the script that files the job and released by the one that ends it
  — **at most once over pushes, never a lock**; a cancel removes a job still
  waiting, so `Ok(true)` means it never starts; a throttle is a fixed window per
  queue counted before a read (a window's edge can admit twice the limit, which
  the page says), a full one holding the replica's reads for the method; a
  checkpoint is a field per job, written only by the delivery holding the job.

## The Redis connection

`RedisConnection` is the connection: one link opened by `RedisModule::for_root`
over the topology the URL's scheme declares — one server, Sentinel, Cluster —
and shared by every binding, each of which declares it runs after the
connection's factory. Every binding runs on all three
(`.claude/decisions/valkey-topologies.md`). `throttler` and `schedule` are crate
features because each pulls a port crate an app may not need.

- **The boot proves the connection with a `PING`**, and what fails the same way
  every time fails at once — a budget out of range, an unparsable URL, unusable
  TLS settings, a refusal naming the deployment's own settings. **That list is an
  allow-list; every other answer is retried** within the connect budget, then
  fails naming the endpoint, never the URL, which may carry a password.
- **Every command a caller waits on answers or fails within the budget**, and
  the budget sits below every net it runs under: each binding declares its
  port's net over the connection, so the boot refuses a budget at or past it,
  and the queue binding refuses a lease a renewal sent a third in cannot outlast
  the budget in.
  A blocking command gets a connection of its own from the same client
  (`RedisConnection::dedicated`), bounded by its own wait plus the budget.
- **TLS material beside a plaintext URL fails the boot**, since it would go
  silently unused; verification is never an option (`CLAUDE.md`). An encrypted
  scheme encrypts every connection its topology opens, sentinels included.
- **Under Sentinel, every reconnection asks the sentinels again** (Valkey's
  Sentinel client spec): a connection is never reopened to the address it had.
- **A script names keys of one hash slot**, refused before it is sent on every
  topology, so what runs on one server runs on a Cluster; a script a node
  forgot is loaded again on that node alone.
- **The Valkey the docs claim is the one every suite runs on**: the dev
  container's `docker-compose.yml` pins one image tag, which CI starts through
  the `dev-services` action and the `topology-services` action runs as Sentinel
  and Cluster, and the docs name its release — they move in one change.
- **Each binding's docs page prescribes its ACL rule whole, per role** — its
  namespace, the connection's commands, every command it or a script it runs
  sends, and nothing else — plus what a topology's connection adds, written
  once on the topologies page. Held by `nest-rs-redis`'s e2e on every
  topology, which creates each user from the pages' lines verbatim on every
  node, reads every node's `ACL LOG` for any denial and, for the queue, every
  primary's `MONITOR` for a command the rule allows and nothing sends.

## A key a datastore holds is a name an operator types

A key sits in a KEDA trigger, a `SCAN` during an incident, an ACL — so it obeys
the naming law and is derived, not chosen:

```
nestrs:<concern>:<structure>[:<member>]
nestrs:queue:{<queue>}:<structure>
```

`<concern>` is the tail of the span target of the crate that **owns** the
concern (`nest_rs::throttler` → `throttler`) — never `redis`, which writes them
all and owns none. `<structure>` is one word, never the concern's own word
again. `<member>` is what varies; a queue name holds no `:` and no brace. The
queue puts its member first, in a hash tag, so one queue is one prefix to
`SCAN`, to scope an ACL, and one Cluster slot; a fact about one job is a field
of a per-queue hash, never a key. A key is the fourth surface of one
derivation: crate, span target, `<PREFIX>_<CONCERN>__*`, key.

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
