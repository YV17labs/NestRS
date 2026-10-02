# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

7.0 is the queue's release, and a major one. A 6.1 project upgrades in one pass:
the ordered checklist is [Upgrading from 6.x to 7.0](https://nestrs.dev/upgrading/),
and a queue on Redis also moves its keys, as
[Upgrading queues from 6.x](https://nestrs.dev/queue/upgrading/) lays out. Below is
everything a 6.1 caller, driver author or deployment can see, and why; what breaks
says so where it is described.

### rustls 0.23.45 — a TLS 1.3 handshake message across a key change is refused

RUSTSEC-2026-0285 (GHSA-2mjx-qc3c-rqvc, published 2026-09-14): rustls 0.23.44
accepted TLS 1.3 handshake messages sent at the wrong encryption level when they
followed a key-changing message in the same record, which RFC 8446 §5.1 requires be
refused with `unexpected_message`. Every lockfile the repository owns — the
framework's, the demo's and the benchmark's — carried 0.23.44 and now resolves
0.23.45.

A library's lockfile is not published, so **an application built on nest-rs resolves
rustls on its own: `cargo update -p rustls` is the whole fix**, and the
`rustls = "0.23"` requirement already admits it. `nestrs` does not depend on rustls,
so `cargo install --locked nest-rs-cli` was never exposed. The advisory sat in the
tree for eleven days with nothing noticing, which is what the daily advisory watch
below now exists for.

### A GraphQL operation runs its guards whatever it returns

`#[operations]` emitted the guard chain only for an operation whose return type
read as a `Result`, so under a deny-all `#[use_guards]` on the resolver, or an
app-wide guard, an operation returning `i32`, `Vec<T>` or a bare `impl Stream`
subscription served its data in 6.1. Every wrapper the macro emits now answers a
`Result` — a bare `T` becomes `async_graphql::Result<T>` — so a denial is a field
error on every query, mutation, subscription, `#[entity]` resolver and field
resolver. **Breaking for anything relying on the gap:** such an operation now
needs the posture its guards ask for. A bare return beside `#[authorize]`, beside
a `Valid<T>` or `Piped<P, T>` argument, and on an `#[entity]` is accepted now; those
refusals existed only because of the gap.

A `#[field_resolver]` runs its resolver's `#[use_guards]` and its own on every
call, and leaves the app-wide pool to the request's root field
(`GraphqlSite::Field`). It used to run nothing unless its method declared a guard,
and then the pool once per parent row.

### A handler's `Result` is known by its type at every edge, never by its name

`#[operations]` took any return type whose name ended in `Result` for one, so a
query returning `SearchResult` — an ordinary `SimpleObject` — did not compile,
while a `Result` renamed on import (`use async_graphql::Result as GqlResult`) or
behind a type alias was taken for a value: its error reached the client as data
and its operation line filed `ok`. `#[routes]`' response shapers rewrote such a
`Result`'s error status into the success code, and `#[tools]` refused it as an
operation that cannot fail. Every edge now decides by type, through the kernel's
`nest_rs_core::Answer` probe (WebSocket already did, through `ReplyValue`): a
payload named `…Result` is a value, and a renamed `Result` fails like a spelled
one — on the line, to the client, with its error's own status. Two readings stay
spelled, each refused with a sentence at the return type: a `#[subscription]`
(async-graphql's derive reads the name), and a masked MCP operation, whose mask
unwraps `Result<Json<T>, …>`.

### The queue port: a push names its job, a retry backs off, and a backend says what it honours

6.1's port was three methods over a string: `push_json(name, value)` for a backend
to implement, and `push_to::<Q>(job)` or `push(name, job)` to call. A job had no
identity the framework knew, a retry ran again at once, a delay or a unique job was
nowhere, and a backend could not say what it did not do. **Breaking, in every
producer and every driver** — the calls each producer changes are listed on the
upgrade page, and a driver is rewritten against
[Writing a driver](https://nestrs.dev/queue/writing-a-driver/).

```rust
// 6.1
queue.push_to::<AudioQueue>(command).await?;

// 7.0
let receipt = queue.push(AudioQueue, command, None).await?;
```

- **A push answers with a receipt, and the job's id is the port's.** A push mints a
  `JobId` — a UUID v7 — and seals it in the envelope; `push(Q, job, options)`,
  `push_many(Q, jobs, options)` and `push_json(name, value, options)` answer with a
  `PushReceipt { queue, id }` per job. A delivery reads the id back out of the
  envelope, never from a backend's own task id, which rides beside it as
  `backend_id` — so `messaging.message.id`, the operation line's `job_id`, a cancel,
  a unique claim and a checkpoint name one value on every backend. `push_many`
  reaches the backend `ENQUEUE_BATCH` (100) jobs per call and is not atomic, which
  it says: a push that fails after the backend accepted some of its calls answers
  `QueueError::PartiallyQueued { receipts, source }`, the receipts of every job
  already queued beside the failure that stopped it, so a retry leaves them out.
  `push_to::<Q>` and the string-taking `push(name, job)` are gone; `push_json` is
  the one hatch for a queue this binary does not declare. A job a 6.x producer sealed carries no id and still
  runs, under an id minted for each delivery until a retry seals one.
- **A queue's type is `Queue`, and `QueueName` is the checked name a backend
  receives.** `#[queue(name = "audio", job = TranscodeCommand)]` now implements
  `Queue` (its `NAME` and its `Job`) and `Destination`, what a push names:
  `<AudioQueue as QueueName>::NAME` becomes `<AudioQueue as Queue>::NAME`. A queue
  name is 1 to 128 of `[A-Za-z0-9_.-]` — no `:`, since a queue name is one level of
  a datastore key — refused at compile time for a `#[queue]` literal, at boot for a
  `#[process]` method's queue and at the push for a raw name. 6.1 checked none of
  them.
- **`PushOptions` carries what a push asks for.** `None` is an immediate push;
  `PushOptions::default().with_delay(..)` holds a job back and `.with_unique(key)`
  keeps at most one job per key pending or running — at most once over pushes, never
  a lock, which the method says. A second push under a held key is refused with
  `QueueError::UniqueKeyHeld`, naming the holder's `JobId`, and files nothing. A
  delay ending after 9999-12-31T23:59:59.999Z (`Delay::LATEST_DUE`) is refused at
  the push with `QueueError::InvalidOptions`, and `Delay::deadline` returns a
  `Result`.
- **A retry backs off, and the budget is the port's.** The envelope counts the
  attempt (absent reads as 1). A retryable failure with budget left waits
  `min(5 min, 1 s · 2^(attempt − 1))` before the next attempt, jittered into
  `[0.8, 1.2]` by FNV-1a over the job's id and the attempt number — derived rather
  than drawn, so the wait before any attempt is reproducible from the id, in a test
  or by an operator reading the job's lines. `#[process(retries = N)]` is the budget
  on every backend. A spent budget dead-letters the job once, and so, at once, does
  a failure no other attempt could clear — a payload that does not deserialize, a
  pipe rejection, a panic. A 6.1 retry ran again at once, in the worker.
- **Breaking: an attempt that never returns spends the retry budget.** An attempt
  whose process died — an abort, an OOM kill, a pod killed mid-job — filed
  nothing, so 6.1 ran the job again at the same attempt, for free, as often as it
  crashed a replica. A driver now hands the port the attempts its storage saw
  start (`Delivery::with_attempts_started(n)`), the port runs the later of that
  and the envelope's attempt, and a job past `retries + 1` is dead-lettered
  without running: `job dead-lettered: retry budget spent by attempts that never
  returned`, at `error`, with `unfinished`. `retries = 0` means tried once, crash
  or not, so a method that relied on a crash being retried declares a retry. An
  attempt cut by a shutdown's drain and handed back to the queue is not counted.
- **An older worker hands a newer release's job back instead of failing it, for
  a day at most.** During a rolling deploy, a job sealed with a wire-format version
  newer than the worker's runs nothing, spends no attempt and returns to its queue
  exactly as stored, due again after a minute (`consume::NEWER_RELEASE_WAIT`), with
  one `warn`, `job sealed by a newer release handed back unread`, naming both
  versions, the job's id, how long it has waited unread (`waited_ms`) and for how
  long it may (`patience_ms`) — filed in the newer envelope's trace when it spells
  `traceparent` as this release does, never under its actor. 6.1 failed it as an
  error and spent its retries on it. A job still unread a day after it was first
  handed back (`consume::NEWER_RELEASE_PATIENCE`) is a rollout that stopped — its
  producers rolled back, its consumers never coming — so it is dead-lettered once,
  at `error`, `job dead-lettered: a newer release sealed it, and none of its
  consumers ran it in time`, and its record stays in the dead set for a consumer
  of that release; a newer envelope naming no id this release reads cannot be
  followed from one delivery to the next, and is dead-lettered at once. Every line
  names the remedy that works: a consumer of that release kept running until the
  queue holds none of its jobs. A driver re-files `AttemptOutcome::Defer { after }`
  unchanged, through `retry_envelope()`, and hands the port how long the job has
  waited unread from a record it keeps (`Delivery::with_deferred_for`); a backend
  keeping none leaves the port the push's age. The Redis driver keeps
  `nestrs:queue:<queue>:deferred:<job_id>` in Redis's own milliseconds, so no two
  hosts' clocks are compared.
- **A retry keeps the trace it started in.** A job whose envelope carried no
  usable `traceparent` starts a trace on its first attempt; the record re-filed
  for the next one marks that trace as the consumer's (`"trace_minted": true`), so
  every attempt reports `continued_trace=false` and each later attempt is a child
  of the first. An envelope key a delivery cannot use is said once per job, not
  once per attempt.
- **`#[process(concurrency = N)]` is back**, reversing 6f787ce5 by owner decision:
  how many attempts of that method one replica runs at once, default 1 — per method
  and per replica, so another method's jobs never wait on this one's permits.
  Replicas are the horizontal bound. Not a capability: every backend owes it.
- **`#[process(throttle(limit = L, window = "1m"))]`** caps how many attempts of the
  method *start* per window across the deployment, and **a `Checkpoint<S>` parameter**
  keeps a job's progress across its retries and a replica that died holding it,
  cleared at the job's outcome; it requires `transactional = false`, so a retry never
  resumes past database work its failed attempt rolled back. A throttle window under
  a millisecond (`Throttle::MIN_WINDOW`), like a zero one, fails the boot: a
  millisecond is the finest a backend is handed.
- **`cancel(&receipt)` and `cancel_unique(Q, key)`** answer `Ok(true)` only for a
  job that had not started and now never will, and `Ok(false)` for one that started,
  finished or is unknown.
- **A backend declares what it honours.** A driver names a `QueueBackend` constant —
  its `messaging.system` and its `Capabilities`, over a non-exhaustive `Capability`:
  `DelayedPush`, `UniquePush`, `Cancellation`, `Throttle`, `Checkpoint`. The port
  refuses what a backend lacks at the earliest site that sees both facts, in one
  sentence naming the capability and the backend: the worker's boot for a
  `#[process]` key, the push for an option, the call for a cancel. The retry budget,
  a transaction per attempt and `concurrency` are owed by every backend and are not
  capabilities. The Redis backend declares all five.
- **A driver implements `enqueue`, and the port owns everything else.**
  `JobProducer` is `backend()`, `enqueue(queue, envelopes, options)` and — for a
  backend declaring cancellation — `remove` and `remove_unique`; the port checks
  the name and the options, mints the ids and seals the envelopes before any of it
  reaches the driver. On the consuming side, `consume::discover(container, &BACKEND)`
  refuses a method whose keys the backend cannot honour — and two methods draining
  one queue — and `consume::attempt` answers an `AttemptOutcome` — `Ok`,
  `Retry { after }` with the backoff above, `DeadLetter`, or `Defer { after }` for a
  job a newer release sealed — over a `Delivery` the driver builds. What 6.1
  exported for a driver to do those steps itself is gone: `envelope::seal` and
  `envelope::open` (the `envelope` module is private, its type is
  `nest_rs_queue::Envelope`), `check_duplicate_queue_claims`, which
  `consume::discover` runs, and the `Processor` trait, which the framework never
  called — a consumer is a `#[processor]` host and its `#[process]` methods.
  The `tracing` re-export, which only the 6.1 macro's expansion used, is gone too;
  a crate logging through it depends on `tracing` itself.
- **`ProcessMethod` is read through accessors.** Its fields are private;
  `name()`, `queue()` and `options()` (a `ProcessOptions` holding `retries`,
  `concurrency`, `throttle` and whether the method checkpoints) replace them.
- **A `#[process]` method returns any `Result<(), E>`** whose error converts into
  `Box<dyn Error + Send + Sync>` — `anyhow::Result<()>` remains the usual spelling —
  and may be a plain `fn`.
- **Delivery is at least once, on every backend, and a handler is idempotent.** A
  storage that loses no job delivers some twice — a sweep after a crash, an
  acknowledgement lost — and a backend may answer the common second delivery
  without running it, as the Redis backend's guard does for a fixed span, but none
  promises more. The port's documentation, the rules and every page say so.

### The Redis queue lives under `nestrs:queue:`, guards the common second delivery, and keeps every capability the port names

- **Breaking: every queue lives under `nestrs:queue:<queue>`.** apalis derives
  its lists from that namespace — `…:active` is now the list a KEDA trigger
  names — and the framework keeps its own records beside them, so a Redis user
  confined to `~nestrs:queue:*`, with the commands the queue's page lists, runs a
  queue end to end. 6.x kept jobs at the root of the keyspace under the queue's
  bare name: a 7.0 worker refuses to start beside them, with the key each sits
  under, how many jobs wait there, and the two ways out — drain them with a 6.x
  worker, or `RENAMENX` them under the namespace, as the queue documentation's
  *Upgrading queues from 6.x* page lays out and the e2e suite runs. A 7.0
  producer says so once per queue. The count is exact, the schedule included:
  under the 6.x namespace the check reads the names apalis's own `Config` getters
  derive — the waiting list, the schedule, the consumers set, and the in-flight
  sets that set lists — with `TYPE` first, then `LLEN`, `ZCARD`, `ZRANGE` or
  `SCARD` for the type found. It never writes and reads no other name — an e2e
  runs it as a Redis user allowed only those reads, and Redis denies it nothing.
  It is the one place the framework reads apalis's structures itself, a written
  exception to reaching them only through apalis's API: the root `clippy.toml`
  refuses apalis's nine structure getters in every other file. A
  key of another type at one of those names, an application's own, is not taken
  for jobs and hides none beside it: it is left alone and named in one `warn`,
  `a key at a 6.x queue name holds what 6.x never kept there; left alone, and not
  counted as jobs`.
- **A job apalis delivers twice is guarded, not promised once.** A delivery takes
  the job's lease before its attempt and leaves a settled mark after it; a second
  delivery is handed back while the lease is held, and acknowledged without
  running while the mark lasts. That answers the common second delivery — a
  replica's startup sweep, a peer taking a slow replica for dead, a job filed
  twice — and claims nothing past it: a redelivery after the mark lapsed, or one
  arriving once the lease of a replica cut off from Redis lapsed while that replica
  still runs the job, runs it again, which at least once allows. Every replica
  consumes under an id of its own, a peer sweeps a silent replica's jobs only
  after `NESTRS_REDIS__WORKER__ORPHAN_AFTER_SECS` (300), the lease lasts
  `NESTRS_REDIS__WORKER__LEASE_SECS` (30), and a shutdown hands back what still
  runs before its window closes. 6.1's replicas shared one worker id, so a replica
  starting re-ran whatever its peers were running.
- **apalis never ends a job on a count of its own.** apalis 0.7.4 counts every
  delivery of a record — a retry, a throttle deferral, a lease hand-back — against a
  private cap of five, and once the count is reached the next delivery answered
  with a plain error (a hand-back Redis refused, a panic outside the attempt) moves
  the record to apalis's dead set, with none of the port's dead-letter events. Every
  record the adapter files — push, delayed push, retry, hand-back — now carries
  apalis's context with that cap at `u32::MAX`, built through its public
  `Deserialize`, so a record a 64-bit replica filed decodes on a 32-bit one, and a
  unit test pins apalis's field names, so a bump that renames them fails the build's
  tests rather than production. A record a 6.x producer filed keeps the cap until
  its first retry or hand-back. The retry budget is the port's alone.
- **A method fetches as many jobs per poll as it can run.** apalis 0.7.4 fetches a
  buffer of records once per poll interval, and the worker asked for one, so a
  method drained at most one job per 100 ms per replica whatever its
  `concurrency`. The buffer is now the method's `concurrency`, at most 799 — Redis's
  Lua unpacks at most 7,999 values into one call, and apalis's sweep of a dead
  replica takes ten buffers in one script, which past that limit loses every job it
  popped — and a method fetches only while one of its permits is free. The interval
  is `NESTRS_REDIS__WORKER__POLL_INTERVAL_MS`. The ceiling per method per replica
  is `concurrency` per interval for short jobs. Measured on one replica draining
  500 no-op jobs from Redis 8.6.3 (release build, median of three), the one-job
  fetch 6.1 made against the one 7.0 makes:

  | Poll | `concurrency` | One job per poll | `concurrency` per poll |
  |---|---|---|---|
  | 100 ms | 1 | 9.4 jobs/s | 9.4 jobs/s |
  | 100 ms | 4 | 9.3 jobs/s | 37.5 jobs/s |
  | 100 ms | 16 | 9.3 jobs/s | 146.7 jobs/s |
  | 25 ms | 16 | 30.9 jobs/s | 485.2 jobs/s |

  A busy replica may hold up to `concurrency` jobs it fetched and has not started,
  out of sight of its peers and of a KEDA trigger, and a replica starting sweeps up
  to ten times that many of a silent peer's jobs. An idle method costs Redis about
  58 commands a second per replica at the default poll and about 420 at the floor,
  every poll a fetch and a sweep, jobs or none.
- **A fetch that fails says what it may have stranded.** A fetch that fails after
  apalis claimed jobs leaves them in the replica's flight until some replica
  starts, and its `warn` now says so. A fetch meeting a record apalis-redis cannot
  decode is its own line, at `error` — `queue fetch met a record apalis cannot
  decode; it and every job fetched beside it wait in flight until a replica
  starts` — because apalis 0.7.4 drops the whole batch it claimed at that record,
  a limit the Delivery page states and an issue drafted upstream.
- **Breaking: `RedisWorkerConfig` gains `orphan_after`, `lease` and
  `poll_interval`**, beside 6.1's `shutdown_timeout`, so a struct literal without
  `..Default::default()` no longer compiles. Every duration it takes has a floor
  and a ceiling, held for a value pinned through `RedisWorkerModule::for_root` as
  for the environment, and the boot fails naming the variable (and the field, when
  it was pinned): `ORPHAN_AFTER_SECS` from 5 seconds to a day, `LEASE_SECS` from 1
  second to half the orphan threshold — a pair breaking that names both variables
  — `POLL_INTERVAL_MS` from 10 milliseconds to the orphan threshold, since the
  sweep that recovers a crashed replica's jobs runs on the poll, and
  `SHUTDOWN_TIMEOUT_SECS` from 1 second to an hour. A value the boot accepts never
  makes apalis panic: a threshold of thirteen digits used to boot a worker whose
  first sweep panicked.
- **`Capability::DelayedPush`.** A delayed push, and a retry's next attempt, wait
  on the queue's schedule; the producer that filed them moves them onto the queue
  when due, so they reach the list an autoscaler reads with no worker running.
  **Due records reach the queue at the fetch's pace**: every worker also moves its
  queue's due records — delayed pushes, retries, hand-backs — onto the list on its
  own one-second tick through apalis's public `enqueue_scheduled`, up to
  `min(799, max(100, concurrency))` at a time while any are due, so a burst of due
  retries reaches the list, and KEDA, promptly. The scaling page counts its Redis
  cost.
- **`Capability::UniquePush`.** A push under a held key is refused with
  `QueueError::UniqueKeyHeld`, naming the job that holds it, and files nothing;
  the key is claimed in the step that reads it, so racing pushes queue one job,
  and it is let go when the job completes, dead-letters or is cancelled. The
  claim is held forty seconds (`CLAIM_HOLD`, twice the port's `BACKEND_TIMEOUT`)
  until Redis confirms the job queued, then for the job's lifetime: a push that
  never learns whether its job was queued — the claim or the filing unanswered,
  the claim not extended, the push dropped by its caller or by the port's net —
  lets the key lapse within the hold rather than refusing every retry under it
  for a week, and says so at `warn`, `unique key claimed without its job confirmed
  queued; …`, with the `step`. At-least-once is the direction it errs in.
- **`Capability::Cancellation`.** `cancel(&receipt)` and `cancel_unique(queue,
  key)` answer `true` only while no attempt runs — a job waiting on its queue, on
  its delay or for its next attempt — and the delivery that meets the cancel
  acknowledges the job without running it. A job never known, or long finished,
  answers `false`.
- **`Capability::Throttle`.** `#[process(throttle(limit, window))]` is counted
  across every replica in one fixed window per queue, opened by its first start;
  an attempt over the limit waits for the window's end, keeps its attempt number,
  and is never dropped. The first refusal stops that replica's fetch until the
  window ends, so a backlog waits on `…:active`, where peers and a KEDA trigger
  see it, rather than being fetched, refused and re-filed every poll: a
  10,000-job backlog under `limit = 1, window = "2s"` and `concurrency = 4` went
  from about 8,700 Redis calls and 339 refusals in nine seconds to 726 and 15,
  for the same starts. A job handed back unread — one a newer release sealed —
  takes its start back, inside the window that counted it only.
- **`Capability::Checkpoint`.** A job's `Checkpoint<S>` outlives its retries and
  a replica that died holding it, and goes with the job's outcome.
- **Nothing a job leaves waits forever.** Its open record, unique key,
  checkpoint, attempt count and a cancel's tombstone last a week past the instant the job is
  due, renewed by every delivery — the bound on a key whose job vanished, which
  `cancel_unique` frees sooner. A settled mark costs about 120 bytes per job and
  is written once, for one span: `max(1 h, 2 · orphan_after + lease)`, past the
  latest a sweep could hand the job to a second delivery. apalis drops the
  acknowledgements still queued when a worker stops and never retries one Redis
  refused, which leaves the job in flight until some replica starts and sweeps it,
  however much later; a job swept after its mark lapsed runs again, and the line
  that reports the lost acknowledgement says so, at `error`: `job acknowledgement
  lost; the job stays in flight until a replica sweeps it, and runs again then if
  its settled mark has lapsed`.
- **A drain hands an interrupted attempt back, its start given back first.** The
  drain lets running attempts finish for its window less a reserve, then
  interrupts them and hands their jobs back. Each attempt that goes back releases
  its lease and gives its start back in one call, ahead of the hand-back, so a
  Redis stalling at that moment cannot leave the start counted against a job that
  never failed — which, under the default `retries = 0`, would dead-letter it at
  its next delivery. The reserve is one connection budget
  (`NESTRS_REDIS__CONNECT_TIMEOUT_SECS`, 10 seconds by default), or five seconds
  when that is shorter, and never more than half the window; a delivery reaching
  its permit after the window closed goes back unadmitted. A drain that still
  stops waiting says so at `error`, `queue workers did not stop within the
  shutdown window; …`, counting the deliveries it `cut`, each attempt spent unless
  its give-back reached Redis; a lease Redis would not drop says what it was to
  give back (`gives_back`).
- **What the guard costs.** A delivery runs two more scripts than it did — the lease
  and the settle — and a push one more round trip, for the job's open record (about
  110 µs to 200 µs sequential, on loopback).
- **Unsupported deployments are stated, not implied.** Redis Cluster is unsupported
  — apalis 0.7's scripts touch undeclared keys across slots — and 7.0's Redis suite
  passed on 6.2.20, 7.0.15 and 8.6.3, so 6.2 is the floor.

### The Redis connection is the connection, and a command Redis never answers fails within the budget

A Redis outage used to hold every caller — a rate-limited request, a push, every
loop apalis runs — for as long as the client kept reopening, and the client's own
reconnect backoff, a hundredfold from one second, put a minute between attempts
after two failures. **Breaking for code that reached the manager.**

- **`RedisConnection` implements `redis::aio::ConnectionLike` and `Clone`** over
  the one `ConnectionManager` every binding shares: apalis's storage runs on it
  (`RedisStorage<_, RedisConnection>`), the rate limiter and the occurrence lock send
  their commands on a clone, and a command of your own runs on one too.
  `manager()` is no longer public — there is nothing left to hand out.
- **Every command is bounded end to end by `NESTRS_REDIS__CONNECT_TIMEOUT_SECS`**,
  the wait for a reopened connection included, and fails as a timeout the caller
  reads with `is_timeout()`. A timeout says the answer did not come, never that the
  command did not run. The reconnect backoff doubles to a two-second ceiling.
- **Except where a cut would strand work: a worker's fetch and its admission wait
  for their answer.** A fetch cut after apalis claimed its jobs left them in a
  live replica's flight — never run, never swept while that replica lived,
  invisible to the queue's length — and an admission cut after it ran left a
  phantom lease, throttle start and attempt. apalis's fetch, its heartbeat and
  its acknowledgements, which share the fetch's connection, and the delivery
  guard's admission run on the same socket without the per-command cut; what
  ends a wait on a Redis that is gone is the socket itself, whose keepalive and,
  on Linux, `TCP_USER_TIMEOUT` are set to the budget, never under a second. Every
  other command the worker sends — a hand-back, a settle, a lease renewal — keeps
  the budget, and is harmless when cut.
- **The boot proves Redis and says what went wrong.**
  `RedisConnection::connect(&RedisConfig)` replaces `connect(url)` and
  `connect_within(url, budget)`: each attempt proves Redis with a `PING` on a
  connection opened once, closed before it opens the one the app keeps, so a Redis
  with one client slot left still boots. What every attempt would repeat fails at
  once — `RedisError::InvalidUrl` for a URL the client cannot parse,
  `RedisError::Refused` for an answer naming the deployment's own settings: refused
  credentials, an ACL denying the proof, a protocol it does not speak. Every other
  answer may clear — `LOADING`, `BUSY`, `MASTERDOWN`, `TRYAGAIN`, a code the client
  does not know — and is retried within the budget, which ends as
  `RedisError::Unready`, carrying Redis's last answer, or `RedisError::Unreachable`
  when Redis never answered. A budget built in code outside its range — zero, or
  past the hour the variable allows — is refused before anything is dialled
  (`RedisError::Budget`). `RedisError::Connect` is gone. The endpoint every error
  and line names is the address dialled, never the URL.
- **A database the server will not select fails the boot at once**, as
  `RedisError::DatabaseRefused`, naming the index the URL ends in: an index past
  the server's `databases`, an ACL user without `+select`, a server in cluster
  mode. The client reports every refused `SELECT` in one sentence that drops the
  server's code, so the last two were retried for the whole budget and reported as
  a Redis "not ready … if it clears on its own". The one refusal of `SELECT` that
  clears, a server busy running a script, is still retried.
- **Each Redis binding's page gives its ACL rule whole** — the queue under
  `~nestrs:queue:*`, the rate limiter under `~nestrs:throttler:*`, a schedule's
  claims under `~nestrs:schedule:*` — as one `ACL SETUSER` line listing the
  connection's `PING` and `SELECT`, `EVALSHA` and `SCRIPT LOAD` where the binding
  sends scripts, and every command it sends or its scripts call, apalis's
  included, and nothing more: Redis checks the commands inside a script against
  the caller's ACL too. An app running several bindings gives one user each rule.
  An e2e test creates a user from each page's line verbatim and runs the binding
  through it, reading Redis's `ACL LOG` for any denial; that a line grants
  nothing its binding leaves unused is held by review.
  `CLIENT SETINFO`, which the client sends and whose refusal it ignores, is left
  out, because Redis 7.0 refuses a rule naming it.

### `rediss://` is verified TLS

- **A `rediss://` URL encrypts every connection the client opens**, the ones it
  reopens behind its callers included, and verifies Redis's certificate for the
  URL's host through rustls, against the webpki roots compiled into the client or
  the authorities in `NESTRS_REDIS__TLS_CA_CERT` (inline PEM, or `_FILE`).
  `TLS_CERT` / `TLS_KEY` present a client certificate — both or neither. The
  system's store is never read: redis 0.32 would re-read it on every connection,
  blocking the runtime while it does.
- **Verification cannot be switched off.** `#insecure` fails the boot
  (`RedisError::UnverifiedTls`), and so does every TLS setting that cannot work,
  naming what to change: material beside a `redis://` URL (`PlaintextUrl`), and
  material no handshake could use, a host no certificate can name or a handshake
  refused the same way on every attempt (`TlsRefused`).
- **A certificate refused after the boot is reported once, at `warn`**, until a
  command answers again — a renewal gone wrong is named rather than read as an
  outage.
- The process-wide rustls crypto provider is installed once when the app chose
  none, as poem's listener installs it, since a tree compiling both providers
  otherwise panics on its first handshake. `redis` gains the features that carry
  it — `tokio-rustls-comp` and `tls-rustls-webpki-roots` — and
  `tls-rustls-insecure` stays off, so nothing could honour `#insecure`.

### The rate limiter's keys carry their structure level, and a store that does not answer denies

- **Breaking for dashboards and ACLs:** the Redis store counts under
  `nestrs:throttler:buckets:<subject>`, not `nestrs:throttle:<subject>` — the concern
  read off `nest_rs_throttler::TARGET`, `buckets` off the port, so an operator
  scopes a `SCAN` to the structure. A rolling deploy starts every window over once,
  and while old and new replicas both serve, a subject can be let through up to
  twice its limit in that window.
- **`ThrottlerGuard` waits `nest_rs::throttler::HIT_TIMEOUT` — 20 seconds — for
  `ThrottlerStore::hit`**, on HTTP, GraphQL, MCP and WebSockets alike. A store still
  silent then is a store that cannot answer: the caller is denied for the window,
  fail closed — `429` with `Retry-After`, or the edge's own error frame — and the
  guard logs `throttler store did not answer within the guard's timeout; denying
  (fail-closed)` at `warn`, with `transport`, `store` and `waited_ms`. A custom
  store holding its call used to hold the request with it, and nothing times out a
  socket's messages. The bound is a net, never a store's budget: it sits above the
  Redis store's own (`NESTRS_REDIS__CONNECT_TIMEOUT_SECS`, 10 s by default, pinned
  below it by a test) and below the HTTP edge's 30-second request timeout, so a hung
  store reads as the same `429` on every edge.
- **`ThrottlerStore` gains a provided `name()`**, the implementor's type name by
  default, which the guard's line uses to say which store went quiet. No
  implementor has to change; a call written `store.name()` is ambiguous only where
  another trait in scope has a `name` method too.
- **Breaking: a throttle's window is never under a millisecond.** `Throttle`'s
  `limit` and `window` are private, read with `limit()` and `window()` as on
  `nest_rs_queue::Throttle`, and `Throttle::new` refuses a window under
  `Throttle::MIN_WINDOW` (1 ms): a compile error in a `const`, a failed boot at the
  route in `#[meta]`. `NESTRS_THROTTLER__WINDOW_SECS=0`, and a
  `ThrottlerConfig::window_secs` of `Some(0)` pinned in code, fail the boot naming
  the variable. A zero window reset every bucket on every hit, so every request
  was let through at any limit; and a window too long for Redis, up to
  `u64::MAX` milliseconds, is now kept for the longest Redis accepts, where it
  failed every hit, closed.
- `BACKEND_REMEDY` moves beside the `ThrottlerStore` contract it belongs to; its
  path from the crate root is unchanged.

### A scheduled job can fire once across replicas

`#[every]` and `#[cron]` fire on every replica of an app. That is right for a
heartbeat and wrong for a tick that enqueues work: three replicas enqueue three
jobs per occurrence, and nothing said so.

- **`replicas = "one"`** on `#[every]` or `#[cron]` fires each occurrence on the
  one replica whose claim on it succeeds — at most once per occurrence. `"each"`,
  the default, keeps the previous behaviour. `#[after]` refuses the key at compile
  time, because each replica's boot is its own event. A claim holds the occurrence
  and nothing else: a run that outlasts its period leaves the next occurrence to
  another replica, and the two runs overlap. One replica never overlaps its own
  runs.
- **The claim goes through a port, `OccurrenceLock`**: `claim` takes an
  `Occurrence` — its token, the claim's hold and the trace id of the run it fires
  — and answers `Claimed` or `ClaimedElsewhere`; `claimed(token)` asks about an
  overrun occurrence. A backend binds it as one declared factory for
  `Arc<dyn OccurrenceLock>` carrying `nest_rs::schedule::BACKEND_REMEDY`. The boot
  fails when a reachable job declares `replicas = "one"` and no lock is bound,
  naming the job and that remedy; two lock bindings fail it too.
- **Redis binds it: `nest_rs::redis::RedisScheduleModule`**, behind the new
  umbrella feature `redis-schedule` (`cargo add nest-rs --features
  redis-schedule`). A bare import beside `ScheduleModule` and
  `RedisModule::for_root`, it claims each occurrence with one `SET … NX PX` on the
  shared connection, under
  `nestrs:schedule:claims:<crate>:<provider>:<method>:<instant_ms>`, whose value
  names the replica and the trace of the run it fired, and asks about an overrun
  occurrence with `EXISTS`. A claim whose answer is lost costs that occurrence and
  nothing more. Its ACL rule is `+set +exists` beside the connection's own, as the
  schedule page gives it whole.
- **A job is its crate, its type and its method.** An API and a worker that each
  declare their own `MaintenanceTasks::sweep` in their own crates never claim each
  other's occurrences through the Redis they share, a job declared in a crate both
  apps link is one job, and moving a job's module inside its crate keeps it.
  Renaming the type, the method or the crate starts a new job. **`key = "…"`**
  beside `replicas = "one"` pins the identity it had — the path the job's boot
  line names as `key`, e.g. `key = "features::NotificationsTasks::purge_expired"`.
  A key is one or more levels joined by `::`, each non-empty and free of `:`,
  whitespace and control characters, checked at compile time. It is refused at
  `#[after]`, at `#[process]` and beside a job firing on every replica, and two
  jobs firing once under one identity fail the boot, naming both.
- **The scheduler bounds every lock call itself.** A claim, or a question about
  an overrun occurrence, not answered by the time the occurrence goes stale — its
  hold less the ten seconds of clock skew — is abandoned: the occurrence is
  skipped with a `warn`, `occurrence skipped: its lock did not answer the claim
  before the occurrence went stale`, carrying `provider`, `method`, `occurrence`
  and `waited_ms`, and an abandoned question counts as `unanswered`. A lock that
  never answered used to hold its job's loop for good, with nothing said.
- **Shutdown never waits on a lock.** A claim still in flight when shutdown is asked
  for is abandoned at once and its occurrence skipped — `occurrence skipped:
  shutdown was asked for before its lock answered the claim`, at `warn`, with
  `provider`, `method`, `occurrence` and `waited_ms` — and questions about overrun
  occurrences still out are counted `unanswered` under their own `warn`, `occurrence
  lock had not answered whether an overrun occurrence was claimed when shutdown was
  asked for`, with `abandoned` and `waited_ms`. Without it a lock that stopped
  answering held the scheduler's stop until the claim went stale: about 50 seconds
  for an interval job, most of a day for a daily cron. Every job loop checks for
  shutdown before its timer, so a tick falling due as shutdown is asked for no
  longer fires; a claim answered before it still fires, and a run already started
  gets five seconds before it is stopped, as *The way down is bounded to the exit*
  below says.
- **An `#[every]` declared `replicas = "one"` ticks on multiples of its period
  since the Unix epoch**, so replicas booted at different moments reach the same
  instants. `replicas = "each"` still first fires one period after boot.
- **At most once per occurrence, never at least once.** A claim the lock cannot
  answer skips the occurrence with a `warn` on `nest_rs::schedule` carrying
  `provider`, `method`, `occurrence` and `error`, and so does a replica reaching
  an occurrence within ten seconds of its claim's hold ending, or whose claim is
  answered that late: its peer's claim may already be gone. A replica that
  crashes after claiming loses that occurrence and only that one; work that must
  not be lost belongs in a queue job the tick enqueues. Replica clocks must agree
  within ten seconds. A claim is made under `<crate>:<provider>:<method>:<instant>`,
  or a pinned key's levels, and lasts twice the gap to the following occurrence,
  at least a minute, so a replica that overran one occurrence can still ask who
  fired it.
- **Occurrences that fall due while the previous one is claimed or run are
  counted aloud**, whichever `replicas` a job declares: one `warn`,
  `occurrences skipped: they fell due while the previous one was claimed or run`,
  with `skipped`, the `occurrence` it started from and `overrun_ms`. A cron job,
  and an `#[every]` firing once, reach the latest occurrence due, late, rather
  than firing the stale one they slept for and then the latest; an `#[every]`
  firing on every replica fires the first tick it overran, late, and skips the
  rest. A job firing once asks the lock about the first hundred it overran, so the
  ones a peer fired are told apart from the ones nobody did (`claimed_elsewhere`,
  `unanswered`, `unchecked`).
- **Breaking:** `ScheduledMethod` and `CronJobMeta` gain `replicas` and `key`,
  and `CronJobMeta` gains `origin`, the `module_path!()` of the code declaring the
  job. The `scheduled job (…)` boot lines carry `replicas`, plus `key` on a job
  firing once; the `schedule.tick` line carries `replicas`, and a job firing once
  carries the `occurrence` it claimed.
- **Breaking:** two jobs sharing one `Provider::method` in one app fail the boot,
  naming both and where each was declared: their lines carry provider and method
  and could not be told apart. A job attached by hand whose origin, provider,
  method or key has an empty level, or a level carrying a `:`, whitespace or a
  control character, fails the boot too, since each is a level of its key.
- `#[every]` and `#[cron]` refuse `concurrency` with the sentence `a replica runs
  one occurrence of a scheduled job at a time — one falling due inside a run there
  is skipped and counted`.
- A job registered by hand with a zero interval fails the boot instead of
  panicking the scheduler, and so does one under a millisecond — finer than the
  duration grammar writes or the timer resolves — and a one-shot declaring one
  replica.
- A cron occurrence reached a moment early no longer fires twice: the next
  occurrence is computed from the one just fired.
- A panic outside a scheduled method — in the scheduler's own loop, or in a run
  function before it hands back its future — ended its job in silence while the
  process reported healthy. The first is named at `error` with the job it
  stopped; the second is caught like a panicking tick, and the job fires again.
- A failed tick's `error` names every cause beneath its error, not the wrapper
  alone.

### An event waits for the transaction it announces

**Breaking.** `EventBus::emit` inside a unit of work holding a transaction — a
mutating request, a GraphQL mutation, a WebSocket message, an MCP operation, a
queue attempt, a scheduled tick — no longer runs its listeners inline. They run
once the transaction commits, and never when it rolls back, fails to commit or the
boundary fails. 6.1 ran them inside the emitter's transaction, so a listener that
pushed a job or notified subscribers did it before the commit, about a write that
might never land and that a worker could not see yet, and a listener's failed
statement poisoned the emitter's writes.

- Listeners run outside that transaction, on the pool, under the emitter's scope
  and ability.
- With nothing to wait for — a safe request, `transactional = false`, no database
  — dispatch is unchanged: before `emit` returns.
- Inside a transaction you open yourself, emit after your own commit: the
  framework cannot see that commit.
- A `push`, a WebSocket broadcast and a storage write still happen where they are
  written, since each answers its caller and that answer exists only once the
  backend is asked. To push after a commit, emit an event and push from its
  listener, as the demo's publish does.
- **Added `nest_rs::database::after_commit` and `Executor::after_commit`**, the
  seam the bus waits through. A database driver whose boundaries settle a
  transaction overrides `after_commit`; the default runs the work at once. In
  `nest-rs-seaorm`, `LazyTransaction::finalize` — the one place every edge settles
  through — runs the held work after a commit, or after a boundary that succeeded
  without opening one, and drops it otherwise with one `debug` line; a panic in
  held work is contained at `error`.

### Shutdown ends within a bound, and every backend call the framework awaits has one of its own

A process asked to stop waited on whatever had not finished — an open stream, a
lifecycle hook, a lock — and a request waited on whatever backend it reached,
bounded at best by the HTTP edge's request timeout, which the other edges do not
have. Each such wait now carries a bound of its own: a net above the budget of the
adapter beneath it, so a healthy backend is never cut short, and a wait past it is
abandoned with a `warn` naming what it was waiting on.

- **HTTP waits for open connections no longer than
  `NESTRS_HTTP__SHUTDOWN_TIMEOUT_SECS`**, handed to poem's graceful shutdown: 20
  seconds by default, so the window, half a second for what it stopped to unwind,
  the shutdown hooks' 5 seconds and OpenTelemetry's 3-second flush — 28.5
  seconds, pinned by a test from the constants themselves — end inside the 30 a
  pod gets by default between `SIGTERM` and `SIGKILL`; from 1 to 3600, pinned
  through `HttpConfig`'s `shutdown_timeout` or read from the environment alike;
  `0` is refused rather than read as an off switch. A deployment that widens the
  window keeps `terminationGracePeriodSeconds` at least 8.5 seconds above it. At
  the bound a request still running is dropped unanswered, over HTTP/1.1 and
  HTTP/2 alike, and a download is cut, and one `warn` on `nest_rs::http` —
  `connections still open as the shutdown window closes are cut; …` — carries
  `cut`, `upgraded_open` and `shutdown_timeout_ms`. What has no end of its own
  ends at the signal instead, as *Long-lived connections end at the shutdown
  signal* below says; a socket a hand-built endpoint (`HttpTransport::mount`)
  upgraded leaves poem's count at its upgrade, so the window neither waits for
  nor closes it, and the line counts it as `upgraded_open`. 6.1 handed poem no
  window, so an `#[sse]` or MCP stream held a stopping replica until its own
  four-hour ceiling or the kubelet's `SIGKILL`, which left the shutdown hooks
  unrun. **Breaking** for an `HttpConfig` struct literal without
  `..Default::default()`.
- **The shutdown lifecycle hooks share one budget** —
  `nest_rs::core::SHUTDOWN_HOOKS_TIMEOUT`, 5 seconds across `OnModuleDestroy`,
  `BeforeApplicationShutdown` and `OnApplicationShutdown` together. Once it is
  spent every later hook is still started and polled once, so a hook that does
  not wait still runs, and a hook that waits is abandoned with a `warn`, `shutdown
  hook abandoned: …`, naming its module and the hook, with `waited_ms` and
  `budget_ms`.
- **A lifecycle hook that panics no longer unwinds out of `App::run`.** At
  shutdown it is logged at `error` on `nest_rs::lifecycle`, with the `panic`, and
  the hooks after it run; at init it is logged the same way and the boot fails
  with `lifecycle hook X::y (OnModuleInit) panicked`.
- **OpenTelemetry's final flush shuts the tracer, meter and logger providers down
  together**, bounded by `nest_rs::opentelemetry::FLUSH_TIMEOUT` — 3 seconds in
  all — so a collector that stopped answering never holds the exit; a provider
  still exporting at the bound is named on stderr.
- **An MCP operation ends with the transport that carried it.** One still running
  when the HTTP transport stops, or cancelled by its client
  (`notifications/cancelled`), is dropped where it waits rather than running on
  through the shutdown hooks. A self-mount that runs units of work off their
  connections declares a `nest_rs::http::DetachedWork`
  (`HttpEndpointMeta::runs_detached`), which the transport stops after its window,
  waiting at most `nest_rs::core::SHUTDOWN_SETTLE_TIMEOUT` (500 ms) for every
  mount together, with a `warn` counting what it stopped; `#[mcp]` declares one
  per endpoint.
- **A unit of work stopped before it settles still files its operation line.**
  An HTTP request dropped at the shutdown window, or by a client that reset its
  connection before the answer, files `http.request` with `outcome="cancelled"`,
  the time it ran, and no `status` or `bytes`; a handler that panics files
  `outcome="panic"`. An MCP operation dropped as above files `mcp.operation` with
  the new `nest_rs::core::operation_log::CANCELLED`. A queue attempt its worker
  stops — a drain closing its window on a running job, or a worker torn down —
  files `queue.job` with `outcome="cancelled"` in the job's trace; it comes from
  the port, so every driver files it. A scheduled tick the scheduler stops at its
  shutdown bound files `schedule.tick` with `outcome="cancelled"`. HTTP and the
  queue used to leave no line, and MCP filed `outcome="ok"` for an operation
  nobody answered. A stream ended at the signal or cut at the window still files
  its line when its body ends, `cancelled`, with the head's status and the bytes
  written. Every other edge files both ends too, as *Every edge files a unit
  cancelled and a unit that panicked* below says.
- **`AuthnGuard` waits `nest_rs::authn::AUTHENTICATE_TIMEOUT` — 20 seconds — for
  `Strategy::authenticate`**, on every edge that reaches it: a route, the GraphQL
  and MCP fallbacks, a gateway's upgrade. Past it the credential was never
  evaluated, which is where an unreachable identity store leaves it, so the answer
  is the same — `503 authentication unavailable`, `#[public]` routes included,
  never anonymous — and the cause goes on its own `warn`, `strategy did not answer
  within the guard's timeout; denying (fail-closed)`, with `strategy`, `transport`
  and `waited_ms`. The shipped JWT strategy answers on its first poll; a strategy
  calling introspection or an API-key store used to hold its request until the
  edge's request timeout, and for good where that was switched off.
- **`OAuthClient`'s HTTP client has a connect and a total timeout**, argued against
  the identity-provider calls it makes: the code exchange and the userinfo fetch,
  which `nest-rs-social`'s GitHub and Google providers go through too. It had none,
  and answered only to the edge's request timeout, which a deployment can switch
  off.
- **The queue port bounds every call it makes on a backend** — `JobProducer`'s
  `enqueue`, `remove` and `remove_unique`, and `CheckpointStore`'s `load`, `save`
  and `clear` — under a net above the Redis adapter's connection budget, an order a
  test pins. A push past it is an error to its caller; a checkpoint past it fails
  the attempt as retryable.
- The same rule gives `ThrottlerGuard` its `HIT_TIMEOUT` and the scheduler its
  bound on every lock call, both above; the queue worker's drain was already
  bounded, and is now 20 seconds by default and at most an hour.

### The way down is bounded to the exit

- **`#[nest_rs::main]` replaces `#[tokio::main]` on every binary.** Dropping a
  tokio runtime waits for every blocking task still running, so under
  `#[tokio::main]` a hook the shutdown budget abandoned while it waited on a
  `spawn_blocking`, or a request dropped at the window mid `tokio::fs` call, held
  the exit past every bound, after the last line. `#[nest_rs::main]` builds the
  same runtime — multi-threaded, every driver enabled, sized by
  `TOKIO_WORKER_THREADS` — and tears it down within what the shutdown hooks and
  the telemetry flush left of the hooks' budget; a `main` that ran no app, such as
  a migration or seed tool, gets the whole budget. What still runs at the bound is
  abandoned with one `warn` on `nest_rs::app`, `work still running as the runtime
  is torn down is abandoned: the exit no longer waits for it`. It takes no
  argument, refusing one with the reason, and its expansion is rooted at the
  umbrella, so an app's manifest needs no `tokio` line for it. **Breaking:**
  replace `#[tokio::main]` on every binary's `main`, and move a `tokio` line kept
  only for it to `[dev-dependencies]`. The scaffold, the demo and the benchmark
  already do, and the root `clippy.toml` refuses `#[tokio::main]` in every
  crate of both workspaces.
- **A second `SIGINT` or `SIGTERM` during the way down exits at once.** In 6.1 it
  was ignored, because the first had replaced the default handlers, so a stuck
  shutdown ended only with `SIGKILL`. The process now exits with 130 or 143 after
  one `error` on `nest_rs::app`, `shutdown signal received on the way down:
  exiting at once, abandoning what still runs`, naming the transports still
  stopping, the hook running (`phase`, `provider`, `method`), or the exit. Nothing
  runs after it, the telemetry flush included.
- **A scheduled tick still running at shutdown gets five seconds, then is
  stopped.** `nest_rs::schedule::Scheduler::SHUTDOWN_TIMEOUT` is the shutdown
  hooks' budget, spent before the hooks so no tick runs through the cleanup it may
  depend on. A tick still running then is dropped where it waits, files
  `schedule.tick` with `outcome="cancelled"` in its own trace, and is named in one
  `warn`, `scheduled ticks still running at the shutdown bound are stopped; …`,
  with `jobs`, `running` and `shutdown_timeout_ms`; one that blocks its thread is
  named at `error` after the half-second settle. 6.1 waited for a running tick to
  end, so a tick that never returned held the whole way down. **Breaking** for a
  tick that relied on finishing during shutdown: push a queue job instead.
- **The Redis worker drains within 20 seconds by default**, down from 30. With the
  hooks' 5 and the flush's 3, a 30-second drain overran Kubernetes' default
  30-second grace and took the hooks and the flush with it. Of the 20, running
  attempts get 10 by default: the drain keeps one connection budget back to hand
  the rest back. **Breaking** for a deployment relying on the old default: set
  `NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS=30` and keep the grace period 8.5
  seconds above it.
- **The default way down is 28.5 seconds**: the longest transport's window plus
  half a second for what it stopped to unwind, then the hooks' 5, then the flush's
  3. A raised window keeps `terminationGracePeriodSeconds` 8.5 seconds above it.
  The settle bound is the kernel's, `nest_rs::core::SHUTDOWN_SETTLE_TIMEOUT` (500
  ms), shared by HTTP's detached work and the scheduler. A test in
  `nest-rs-testing` sums the `stop_bound()` of every transport the framework
  ships, at its defaults, and pins the 28.5.
- **Breaking for a hand-written transport: `Transport` gains a required
  `fn stop_bound(&self) -> Duration`**, the longest `serve` takes to return once
  told to stop, at the configuration `configure` left. `HttpTransport` answers
  its shutdown window plus the settle, `Scheduler` its five seconds plus the
  settle, and `RedisWorker` its drain window — read at `configure` now rather than
  at `serve`, so the bound is the deployment's own. With no default, a transport
  cannot be written without the bound it adds to the way down.
- **The boot files the way down it adds up to.** Once every transport is
  configured, one `way down bounded` line on `nest_rs::app` carries the longest
  stop bound among the transports the app mounted, settle included
  (`stop_bound_ms`), and the shutdown hooks' budget (`hooks_budget_ms`) — the demo
  api reads `stop_bound_ms=20500 hooks_budget_ms=5000`. Add OpenTelemetry's
  3-second flush when it is installed, which the kernel cannot see, and a
  deployment that raised a window reads off the line what its grace period has
  to hold.

### Long-lived connections end at the shutdown signal, the standard way

A stream with no end of its own — an `#[sse]` stream, an MCP session's `GET`
stream — held a stopping replica until something cut it, and a gateway's WebSocket
and a graphql-ws socket were never closed at all: poem stops tracking a connection
at its upgrade, so they ran on under the shutdown hooks and left their clients a
`1006` they could not tell from a network fault. Now, the moment shutdown is asked
for, what has no end of its own ends as its protocol ends one, after whatever it is
answering:

- **An event stream ends cleanly**, its last chunk written, so an `EventSource`
  reconnects at once. A handler streaming events by hand opts in with
  `response.extensions_mut().insert(nest_rs::http::OpenEndedBody)`. A download
  keeps the window and is cut at its close.
- **A gateway's socket closes with RFC 6455's `1001 Going Away`**, after the
  message it is answering and after `on_disconnect`; a message still running when
  the window closes is dropped with its socket and files `cancelled`. A peer that
  stopped reading gets five seconds (`DetachedWork::CLOSE_GRACE`) to take its
  replies and the Close frame.
- **A graphql-ws socket answers each running subscription `complete`**, in the
  negotiated protocol's own wording, answers a query or a mutation still running
  over it, then closes `1001`; its lifetime ceiling now closes the same way, where
  it dropped the socket.
- **An MCP `subscriptions/listen` is answered its final result**, and a session's
  standalone `GET` stream ends like an event stream.

Each files its line `cancelled` — an `http.request` line keeps its `status` and
`bytes` beside it — except a gateway's socket closed idle, which ran no unit: its
`ws.disconnect` hook runs and files `ok`. **Breaking:** `SseSettings::respond`
returns the marked `Response`, and `gateway_endpoint` takes the gateway's
`DetachedWork`; both are emitted by the decorators. A socket a hand-built endpoint
(`HttpTransport::mount`) upgraded is still the developer's own, and the drain line
counts it as `upgraded_open`.

Work a mount runs off the connection that asked for it — an MCP operation on
rmcp's task, a DataLoader batch on async-graphql's, a socket after its upgrade —
gets the rest of the window like a request, is stopped at its close, and is given
one half-second settle for every mount together; a stopped unit is never polled
again. A GraphQL batch in flight used to run on through the shutdown hooks.

### Every edge files a unit cancelled and a unit that panicked

A unit stopped before it settled, or one that unwound, filed nothing at several
edges. Every edge now files both:

- **An MCP tool that panics answers its client a JSON-RPC internal error**, where
  6.1 left the client waiting out its own timeout, and files `mcp.operation`
  `panic`, the panic's text logged at `error` on `nest_rs::mcp`.
- **A WebSocket handler that panics answers an error frame and its socket goes
  on**, where 6.1 lost the socket. A connect hook that panics closes the socket
  `1011`, and a graphql-ws subscription that panics does the same; a disconnect
  hook that panics files `panic` and the close completes.
- **A GraphQL field dropped with its request files `graphql.operation`
  `cancelled`**; a field that panics files `panic`, and a sibling field torn down
  by that panic files `cancelled`, since one panic is one `panic` line.
- **An event listener dropped with its emitter files `events.dispatch`
  `cancelled`.**
- A query that reads only `ok` lines of `graphql.subscription` now also meets the
  sockets a shutdown ended, as `cancelled`.

Each edge's own suite drives a unit to both ends and asserts the line it files.

### A unit's span says how it ended, and a request cut before it answers keeps its route

Every operation span now carries OpenTelemetry's `error.type` and an `Error` status
(`otel.status_code`) when its unit did not end `ok`, with the same word as the
line's `outcome`: `cancelled`, `panic` or `error`. This covers HTTP, GraphQL
operations and subscriptions, MCP, queue attempts, WebSocket messages and hooks,
events and schedule ticks. An HTTP response answered 5xx carries its status code
there instead, and a 4xx carries nothing, as the HTTP conventions ask. Before, a
backend showed a request cut at the shutdown window, or a job that panicked, as a
span that succeeded. A request dropped before it answered also exported as an
anonymous `http.request` with no `http.route`. It is now named `{method} {route}`
for the template the router matched, because every endpoint the framework mounts
notes it (`nest_rs::http::matched` wraps a self-mount's endpoint the same way). A
dashboard keyed on span status starts counting these as failures.

What a client's close files is now stated. An ordinary close (FIN) is a legal
HTTP/1.1 half-close, so hyper runs the handler to its end and the line carries the
answer's `status`. Only a reset — an HTTP/2 stream cancel, `SO_LINGER` of zero, a
crashed client — drops the request and files `outcome="cancelled"`.

### A unit of work is a typed `Unit`, and its span and its line read every slot off it

**Breaking for code reading a unit's constant as a string, or calling
`operation_span!`.** A unit was a `&str` constant beside a separate kind and
target, so a span could be opened under one name and its line filed under
another. Each `<edge>::unit::*`
constant — `nest_rs_http::unit::REQUEST` and its siblings — is now a
`nest_rs::core::operation_log::Unit`, carrying its name, its target and its
`Kind`; `.name()` gives the string.

- **A unit is declared with `nest_rs_core::unit!`** —
  `unit!("http.request", target: crate::target::HTTP, kind: Server)` — evaluated
  in a `const`, so a name off the `<edge>.<unit>` grammar, an edge outside
  `operation_log::EDGES`, a target other than `nest_rs::<edge>` and a declaring
  crate other than `nest-rs-<edge>` are compile errors.
- **`operation_span!` takes the unit as a path** —
  `operation_span!(nest_rs_http::unit::REQUEST, &correlation, …)` — and reads its
  target, name and span kind off it; a literal does not match, and a unit another
  crate declared is refused at compile time.
  `operation_log::kind::{SERVER, CONSUMER, INTERNAL}` is the enum
  `operation_log::Kind`, declared with the unit and never at the span site.
- **`operation_line!` files a unit's operation line** — its target, name,
  message, `outcome` and `duration_ms`, written once — and records the outcome on
  the unit's span, so the line and the span cannot say two things. Every edge
  files its line through it. **Field order changes:** `outcome` and
  `duration_ms` now come before the edge's own fields.
- **`contained_panic!` logs a contained panic at `error` under `panic::FIELD`.**
  Every seam that contains one — lifecycle hooks, the event bus, a GraphQL
  subscription, an MCP operation, a queue attempt, a Redis delivery, the
  scheduler, a WebSocket gateway, SeaORM's after-commit work — logs through it, so
  the field is spelled once. Those lines now put the message first, then
  `panic`, then the seam's fields.

### Every variable can be given as a file, and a family's variables carry the family as a level

- **`<PREFIX>_<NS>__<KEY>_FILE` supplies any namespaced variable from the file it
  names** — the Docker and Kubernetes secrets convention, so a secret never has to
  sit in the process environment. The file is read once at boot, must be a regular
  file of at most a mebibyte, and is never quoted back: an unreadable one fails the
  boot naming the variable, not its value. Text loses its trailing line breaks, so
  a file holding nothing else is unset.
- **One key, one spelling per tier.** `KEY` and `KEY_FILE` both set in the process
  environment, or both in the `.env` cascade, fail the boot naming both. Between the
  tiers the deployment chooses: either spelling in the process environment — empty
  included — shadows both spellings in `.env`, as it shadows a `.env` value of its
  own name.
- **Breaking for every hand-written `from_env`: `ConfigService::get` and `list`
  return a `Result`**, since a file can fail to read, so a reader propagates it with
  `?`. `env.setting("KEY")?` keeps the value with the spelling that supplied it —
  `setting.refuse(reason)` names the variable the deployment actually set, and never
  quotes what a file held — `env.material("KEY")?` hands back the bytes of a
  certificate or a key with the file they came from, and `nest_rs_config::spellings`
  words a setting that was not read, both spellings at once.
- **Breaking for deployments: a family member's variables carry the family as a
  level.** `nest-rs-oauth-client` and `nest-rs-oauth-resource` read
  `NESTRS_OAUTH__CLIENT__*` and `NESTRS_OAUTH__RESOURCE__*` — the same string as the
  paths `nest_rs::oauth::client` and `nest_rs::oauth::resource`. These fifteen
  variables are renamed, and no other variable is:
  - `NESTRS_OAUTH_CLIENT__<KEY>` → `NESTRS_OAUTH__CLIENT__<KEY>` for `CLIENT_ID`,
    `CLIENT_SECRET`, `AUTH_URL`, `TOKEN_URL`, `REDIRECT_URL`, `USERINFO_URL` and
    `SCOPES`;
  - `NESTRS_OAUTH_RESOURCE__<KEY>` → `NESTRS_OAUTH__RESOURCE__<KEY>` for `RESOURCE`,
    `AUTHORIZATION_SERVERS`, `SCOPES_SUPPORTED`, `BEARER_METHODS_SUPPORTED`,
    `RESOURCE_NAME`, `RESOURCE_DOCUMENTATION`, `RESOURCE_POLICY_URI` and
    `RESOURCE_TOS_URI`.

  The old spellings are read by nothing; a boot that still carries them says so,
  as the next section shows.
- **A required value holding only whitespace is refused** like an empty one, in the
  OAuth, social and storage configs, and **a storage credential pair is replaced
  whole**: `ACCESS_KEY` set without `SECRET_KEY`, or the reverse, fails the boot
  rather than pairing it with a default or a value pinned in code.
- **A structured value is decoded by `ConfigService::json` and refused without its
  content.** `env.json::<T>("KEY")?` decodes a JSON value, inline or through its
  `_FILE` spelling, as any `Deserialize` type; one that does not decode fails the
  boot with `ConfigError::Decode`, worded by `nest_rs::core::DecodeError` — the
  variable, where the value failed, the kind of value found and the type
  expected, never the value. A `from_env` decoding a structured value through
  `serde_json` itself and handing the error to `Setting::refuse` or
  `ConfigError::parse` no longer quotes it back either, as the next entry says.
- **Security: a refused value never reaches a boot error, whatever format worded
  the refusal.** `ConfigError::Parse` is `#[non_exhaustive]` and built only
  through `ConfigError::parse`, which `Setting::refuse` and every duration's
  refusal go through. It drops a parser's source excerpt — the `1 | key = "…"`
  lines a TOML error opens with, which repeat the refused line verbatim — and
  says serde's quoting sentences without their value, so a secret in a mistyped
  value no longer lands in the boot error a deployment's logs keep. No framework
  config reads TOML; one of your own that does is covered the same way.
  **Breaking** for code building `ConfigError::Parse { var, message }` as a
  literal: call `ConfigError::parse(var, message)`.
- `OpenTelemetryConfig::from_env` returns a `Result`, and an unparseable
  `SAMPLE_RATIO` or `METRIC_INTERVAL_SECS` — `NaN` included — fails
  `OpenTelemetry::init` with `OpenTelemetryError::Config`, naming the variable,
  where it used to keep the default after a line on stderr.

### A variable nothing reads is reported at boot

A deployment that misspelled a variable, or kept a name 7.0 renamed, got the
default and no signal: nothing asked for the value, so nothing could say it was
ignored. `#[config]` now files its namespace with a link-time registry, and
`nest_rs_config::read` — the funnel every `from_env` passes through — checks the
environment and the `.env` cascade for two shapes, each reported once, at `warn` on
`nest_rs::config`, by name and never by value:

- `config variable read by no config` — a key under a namespace this binary read
  that nothing read, with the nearest key that was read as `suggestion`;
- `config variable under a misspelled namespace` — a variable spelling a linked
  namespace otherwise than the loader reads it: with other separators,
  `NESTRS_OAUTH_RESOURCE__*` for `oauth__resource`, or `NESTRS_SEAORM_URL`, the
  spelling every `DATABASE_URL` teaches, for `NESTRS_SEAORM__URL`; in another case,
  the prefix's included (`NESTRS_Seaorm__URL`, `nestrs_seaorm__url`), since the
  loader folds none; or one misspelled segment of six letters or more away, a
  family member's included (`NESTRS_SEAROM__URL`, `NESTRS_OAUTH__RESOURSE__…`). The
  `suggestion` is always a name the binary reads.

Everything else stays silent on purpose: one `.env` serves several binaries, and
another binary's namespace is not a mistake. So a namespace near miss is held to
both halves of the variable. Its key must be one the near namespace reads, or one
edit from one — a misspelled namespace still carries the key the deployment meant,
while another binary's variable carries its own, so `NESTRS_OPENAI__API_KEY`
beside a linked `openapi` stays silent. And a misspelled segment must have six
letters or more, within a quarter of its length: a shorter one has no room for a
typo that is not also a word (`auth` and `authz` beside `authn`, `es` beside
`ws`), so there only separators and case count. `nest-rs-conformance`'s naming
check holds every namespace of both workspaces outside that reach of every other,
reading the namespaces the `#[config]` declarations state. The namespace half
runs at every read of its namespace, ended by an error or not, so a renamed
required variable is named ahead of the boot error its absence causes; the key
half runs per namespace, where that namespace's `from_env` ran, because a config's
keys are knowable only there — `HttpCors` reads five of its six keys only when
`CORS_ORIGINS` is set, so a process-wide dry run would report a correct
deployment. A read made before any subscriber listens waits for the first read
that has one, and a deployment that filters the target out pays nothing for the
scan. `nestrs doctor` stays out of it: it links no framework crate, so it cannot
know which namespaces a binary reads.

**Breaking: a namespace belongs to one type.** Two `#[config]` structs declaring
the same namespace fail the boot at the read of either, naming both
(`ConfigError::SharedNamespace`). 6.1 let both read it, and the report above
would file the keys only the second one reads as read by nothing, on a correct
deployment; from a variable, a reader now finds the one type that parses it.

### Every duration has a floor and a ceiling, refused alike from the environment and from code

**Breaking for a value that booted before, and for a config of your own that read
a duration.** One reader, `nest_rs::config::DurationBounds`, holds every duration
the framework reads — its key, the field that pins it, its unit, a floor and a
ceiling, each with its reason — and refuses a value outside the range in one
sentence naming the variable, under its `_FILE` spelling when that supplied it,
and, for a value set in code, the field (`` `Type::field` set in code is … ``).
The ceiling is a required field, and never above what the library or the kernel
the value reaches accepts: a Redis budget past the kernel's 32 767-second
keepalive limit failed every dial with `EINVAL` against a Redis that answered, and
a SeaORM timeout past what a clock holds panicked the boot inside sqlx, naming
nothing.

- `NESTRS_REDIS__CONNECT_TIMEOUT_SECS` and `NESTRS_SEAORM__CONNECT_TIMEOUT_SECS`
  hold 1 second to an hour, `NESTRS_THROTTLER__WINDOW_SECS` 1 second to a day,
  `NESTRS_HEALTH__INDICATOR_TIMEOUT_MS` and `PROBE_DEADLINE_MS` 1 millisecond to a
  minute; `0` is refused, and so is a zero pinned in code (a sub-second Redis
  budget set in code is still taken). A zero Redis budget pinned in code booted
  into `could not reach Redis … within 0ns`, a zero SeaORM timeout failed every
  acquire and blamed the pool, and a zero window let every request through at any
  limit.
- `NESTRS_AUTHN__EXPIRES_IN_SECS` holds 1 second to thirty days and
  `NESTRS_AUTHN__LEEWAY_SECS` 0 to 300 seconds (RFC 7519 §4.1.4), and so does a
  `JwtOptions` built in code, checked by `JwtService::new`; an unbounded value
  overflowed every mint or verify.
- The HTTP shutdown window and the Redis worker's durations, above, are read the
  same way.
- **`0` is off only where the declaration says so.**
  `NESTRS_HTTP__REQUEST_TIMEOUT_SECS` holds 1 second to an hour, the connection ceilings
  `NESTRS_HTTP__SSE_MAX_CONNECTION_SECS`, `NESTRS_WS__MAX_CONNECTION_SECS` and
  `NESTRS_GRAPHQL__MAX_CONNECTION_SECS` 1 second to a day,
  `NESTRS_HTTP__SSE_KEEP_ALIVE_SECS`, `NESTRS_MCP__SSE_KEEP_ALIVE_SECS` and
  `NESTRS_MCP__SSE_RETRY_SECS` 1 second to an hour, and
  `NESTRS_HTTP__TLS_RELOAD_SECS` 1 second to a day — and for each, `0` from the
  environment, or `None` in code, is off, as before. A `Some(Duration::ZERO)`
  pinned in code is refused rather than read as a zero that would cut every
  connection: off in code is `None`.
- `NESTRS_OPENTELEMETRY__METRIC_INTERVAL_SECS` holds 1 second to an hour, and `0`
  is refused rather than read as "keep the default".
- **`ConfigService::seconds` is removed.** A config of your own reads a duration
  through a `DurationBounds` declared beside it — `BOUNDS.read(env, base)?`, or
  `read_optional` for an `Option<Duration>`, with `Floor::UnitsOrOff` for a bound a
  deployment may switch off — so its refusal is the framework's sentence; see
  [Configuration](https://nestrs.dev/configuration/#configservice-api).
- **Breaking: a duration's key and its unit cannot disagree.** `DurationBounds`'
  fields are private; it is built with `DurationBounds::secs(key, field, least,
  most)` or `DurationBounds::millis(…)`, and a `secs` key not ending in `_SECS`,
  or a `millis` key not ending in `_MS`, fails to compile in a `const`. Read a
  declaration back with `key()`, `field()`, `unit()`, `least()` and `most()`.
- **Breaking: a duration is read through its bounds and nothing else.** Every
  `ConfigService` reader — `get`, `setting`, `material`, `parse`, `json`, `flag`,
  `count`, `list` — refuses a key ending in `_SECS` or `_MS` at boot, whatever the
  deployment set, with the new `ConfigError::UnboundedDuration`, which names
  `DurationBounds`.

### A contested declaration names both imports that made it

`ContestedDeclarationError` — two imports binding one port, two pins of one config
— named the type and the remedy, and the reader searched the tree for the pair. It
now names both, each as the import and its position in the `imports` of the module
that lists it: `` contested declaration: `X` is declared twice — by
`A::for_root(..)` at `imports[0]` of `AppModule`, and by … ``. **Breaking for a
literal of the error**, which gains `first` and `second`.

### An inert host is a warning only when it is this app's

A host discovered in the binary whose provider the app does not reach — a
`#[scheduled]` method, a `#[process]` method, an `#[on_event]` listener, a
lifecycle hook, a health indicator — was said at `warn` with one hint naming every
cause. In a workspace of several binaries one library crate holds the hosts every
binary imports some of, so each warned about the others' on every boot of a
correct deployment: the demo's worker filed four, telling it to import the api's
scheduled methods.

The boot now reads the cause, and says it at the level it earns. A library crate's
host this app neither imports the module of nor registers is another binary's,
said at `debug` (`skipped …: not this app's to run`, `cause="another_app"`); read
the edge's target at `debug` when a handler in a shared crate does not run. The
app's own code is a `warn` with a `cause` and a `hint` naming its remedy:
`module_not_imported` (naming the module), `not_listed`, `bound_under_another_key`
(naming the module and the key) and `registered_outside_modules`. A hint never
prescribes listing a provider a second time, which builds it twice. **Breaking for
a call to `report_inert_host!`** in an edge of your own, which takes the host's
type and the container; the inventory records it reads, `ProviderDescriptor` and
`LifecycleHook` — internal ABI the decorators emit — carry the host's type.

### A logged error names every cause beneath it, and a value in a text line cannot forge another

- **`nest_rs::core::error_message(&e)`** renders an error and every cause beneath it
  as one sentence, saying a cause its parent already inlined once. Every event the
  framework files with an `error` field goes through it — the kernel, the guards,
  the ORM, authz, health, OpenTelemetry and every edge — so a wrapper such as
  `the queue backend failed` no longer logs without the cause that explains it. Use
  it for your own `error` fields: `error = %error_message(&e)`.
- **Breaking: `.opaque()` takes an owned error that converts into
  `Box<dyn Error + Send + Sync>`**, on HTTP, GraphQL, WebSockets and MCP alike,
  where it took anything `Display`, so the line it files carries the whole chain.
  An `anyhow::Error` is boxed with every link kept (`nest_rs::core::boxed_error`),
  so a decode failure inside it is said without its value. `anyhow`, a `DbErr`, a
  `String` and every `thiserror` type qualify. A type that is only `Display`
  implements `std::error::Error` first, and a `&str` borrowed from a local becomes
  a `String`. `JobError::retry` / `abort`, `WsReply::from_handler_error` and
  `OccurrenceLockError::new` take the same.
- **A payload that does not decode is reported without its value, at every site
  the framework decodes at and in every text it renders one in.** serde's
  sentences quote what they refuse: a value, a variant, a key. The queue's
  dead-letter line and record, the WebSocket payload error, the GraphQL
  variable-pipe error, the OAuth client's read failures and authz's mask failures
  all carried it. Each now says where the payload failed, what kind of value was
  found and what type was expected, through `nest_rs::core::DecodeError`. It keeps
  a missing or duplicate field's name, answers an unknown field with the fields
  its type expects (the key is the sender's, and is dropped), and bounds what it
  keeps. `error_message` reads every link of a chain that way, and also serde's own
  quoting sentences in any text, so a cause behind `#[error(transparent)]`, a `{0}`
  wrapper or anyhow's box is covered too. **Breaking:** `WsReply::payload_error`
  takes the `serde_json::Error`, and `MaskReplyError::Irreconcilable` carries a
  `DecodeError`.
- **An HTTP request that does not decode is refused without the value it sent.**
  A `Json`, `Valid<Json>`, `Form` or `Query` that fails answers `400` with a
  `detail` naming where and what kind, e.g. `parse error: invalid type: an
  integer, expected a string at line 1 column 25`. poem's own sentence quoted the
  value, and a `400` is logged by proxies and kept by caches. The same holds for a
  handler's own decode returned as an `Err`, on every rendering path, with or
  without a wrap registered. `Header<T>` answers a type's own refusal in its own
  sentence naming the header. **Breaking:** `ProblemDetails::from_error(status,
  title, &err)` takes the error rather than any `Display`, and says its decode
  failure without the value; a sentence of your own is `.with_detail(…)`.
- **MCP arguments that do not decode are refused without the value.**
  `nest_rs::mcp::Parameters` is the framework's own, with rmcp's shape and schema.
  A call whose arguments do not decode is still a tool error the model can correct
  (`failed to deserialize parameters: invalid type: a string, expected f64`), but
  the value no longer lands in the model's transcript. **Breaking:** `Parameters`
  is no longer rmcp's type.
- **Every GraphQL error message says a decode failure without its value**, on
  `POST` and over the socket alike. That covers a resolver's `?` on serde or on an
  anyhow chain holding its error, and async-graphql's own `Json<T>` scalar
  coercing an argument. A JSON body that is not a GraphQL request — `"variables"`
  as a string — was answered `400` with nothing in it; following GraphQL-over-HTTP
  it is now answered `400` with one `errors` entry saying where and of what kind
  the decode failed, never the value sent, and no `data` member.
- **A WebSocket handler's error frame says a decode failure without its value**,
  in every tier (`Send + Sync`, local, `Display`-only), and a `WsError` returned
  through `anyhow` is sent whole instead of losing its details.
- **A Redis dead-letter record is the line's rendering**: every cause, each decode
  failure said without its value. It was the error's source verbatim.
- **Breaking: `panic_message` returns a `String`**, with any decode failure in the
  payload said without its value: `.unwrap()` on a failed decode formats serde's
  error into the panic.
- **`TextFormat` escapes every field value it writes**, the message included —
  controls, bidirectional overrides and invisible formatting characters as Rust's
  debug escapes — so a client-chosen string carrying a line break can never forge a
  second log line (CWE-117). JSON was escaped by construction and is unchanged.
- **Breaking: a trace is `minted`, `continued` or `inherited`.**
  `Correlation::mint()` becomes `Correlation::minted(actor)` — `None` wherever a
  guard fills the actor, `Some` where it cannot be re-derived: a queue job whose
  envelope names who pushed it but carries no usable `traceparent` now starts a
  trace for that actor rather than running anonymous.
- **`CapturedEvent` carries `trace_id`, `span_id` and `actor_id`**, read off the
  correlation the event was filed under, so a suite asserts the trace a line
  belongs to without parsing it.

### Every impl-half decorator reads a method with one grammar

The decorators hosted on an `#[injectable]` — `#[processor]`, `#[listeners]`,
`#[scheduled]`, `#[hooks]`, `#[indicators]` — and the edge collectors
(`#[routes]`, `#[messages]`, `#[operations]`, `#[tools]`) each judged a method's
shape their own way, and several deferred the judgement to rustc, which reported a
borrow error, `E0283` or a type mismatch inside code the developer never wrote.
They now share one grammar in `nest_rs_codegen`, each refusal worded once with a
trybuild snapshot:

- **A method compiled out takes its registration with it.** Attributes are read
  through `#[cfg]` and `#[cfg_attr]`, so a method behind a false `#[cfg]` takes its
  handler, its inventory entry, its schedule entry and its dataloader with it, and
  two methods claiming one key under true `#[cfg]`s — one route, one WS event, one
  GraphQL field, one MCP tool or prompt name — are refused naming both.
- **The receiver is judged by the rule.** `self: &Self`, an alias of the host and
  `self: &Arc<Self>` are `&self`; `&mut self`, a borrow of another pointer or a
  `'static` one is refused saying the container holds the one instance in an
  `Arc`. A generic method is refused, a role written twice is refused counting the
  copies, `-> ()` is the unit return and a raw identifier is a name.
- **A plain `fn` is accepted** for a lifecycle hook, a health indicator, an event
  listener, a scheduled trigger, a `#[process]` method, a GraphQL subscription and
  an `#[entity]` resolver, and is called without an `.await`.
- **Five grammars are worded once for every decorator that takes them**: a
  duration (`"500ms"`, `"30s"`, `"5m"`, `"1h"`), a queue name, `replicas`, a
  `#[routes]` path read with poem's own grammar, and dispatch keys. A key a sibling
  decorator takes and this one cannot — `retries` or `concurrency` on an
  `#[every]`, `tz` on a `#[process]` — is refused with the fact that makes it
  meaningless there, never as a misspelling. The job family's keys are one table,
  in `nest-rs-codegen`'s `job.rs`: every member (`JobDecorator` — `#[process]`, `#[every]`,
  `#[cron]`, `#[after]`) crossed with every key (`JobKey`) is a cell of one
  exhaustive `match`, so a key or a member added without an answer at every
  crossing does not build, and each parser reads its keys from it.
- **`#[hooks]` calls the method by its path**, so a trait on `Arc` sharing its name
  no longer runs instead of the hook. A trigger's refusals in one `#[scheduled]`
  host arrive together, and a refusal keeps the impl block, so what rustc reports
  beside it is only what is wrong in the developer's own code.

### Every value refusal opens with the decorator and the key

A refusal of a value read without its source frame — a problems list, a CI summary —
said which key and not whose, and `transactional` is a key of four decorators. Every
value refusal now opens with its site, worded once: a value of the wrong kind reads
``#[process] `retries` takes a whole number``, a value that breaks a grammar names
itself (``#[controller] `version`: "1/2" is not a path segment — …``), and a value
outside a closed set keeps ``unknown #[attr] … `x`; expected …``. The family is
closed across the macro crates — a route's path and `#[version]`, every `#[api]`
value, `#[http_code]`, `#[response_header]`, `#[redirect]`,
`#[interceptor(priority)]`, every `#[use_*]` entry, `#[expose]`'s values,
`#[queue]`, `#[process(queue)]`, `#[cron]`'s expression and `tz`, `#[gateway]`,
`#[subscribe_message]`, GraphQL's `#[authorize(bind, id_arg)]` and `#[module]`'s
lists included — and no decorator leaves a value of the wrong kind to syn's own
sentence, which names neither: `#[crud]`'s values and `#[inject(key)]` were the
last two that did. `#[api]` joins the argument family too: an unknown key reads
``unknown #[api] argument `x`; expected …`` and a key written bare
``#[api] `summary` needs a value — write `summary = ...` ``, where its own
sentence and syn's `expected =` stood. **Breaking for anything matching on a
diagnostic's text.**

Three values no longer reach a panic or a type mismatch inside an expansion.
`#[expose(name = "…")]`, and a `#[sea_orm(from = "…")]` that `#[expose]` reads,
holding something that is not an identifier are compile errors naming the key,
where they were `proc macro panicked`, and a `from` naming no column of the entity
is refused on the literal. `#[cron(3600)]` — a literal that is not a string — is
refused at the attribute. `#[http_code(201u8)]` and `#[redirect]` accept a
suffixed status and emit the validated one unsuffixed, where `201u8` passed
validation and then failed as a type mismatch.

A string literal forwarded through a `macro_rules!` as `$x:expr` is read through the
invisible group it arrives in, where it was refused as not a string literal at
`#[controller(path)]`, `#[gateway(path)]`, `#[mcp(path)]`, `#[cron(tz)]`,
`#[config(namespace)]`, `#[api(summary)]` and the version list — depending on the
argument's position — and a cron expression forwarded that way is validated at
compile time like a literal written in place. A whole `version = [...]` list
forwarded that way is read too, where it was refused.

A decorator read once refuses a second copy by name — `#[public]` at every edge,
every flag, `#[api]` and `#[inject]` — where rustc said `cannot find attribute`.
**Every `key = value` grammar refuses a repeat of every key**, through one reader in
`nest_rs_codegen`, `Grammar`, which refuses an unknown key, a repeated one and one
written bare before the decorator sees a key, and underlines a repeat at the key —
eighteen decorators, both halves of `#[expose]` among them; a repeated
`via` on the `#[expose]` written on a column used to load the rows the second
foreign key links. `#[crud(create = T, ops = [list, get])]` is refused, where the
input type of an op `ops` leaves out was dropped in silence, and so is a
`paginate` beside an `ops` that leaves out `list` — `#[crud(ops = [get], paginate
= none)]` dropped the documented opt-out in silence, though `paginate` configures
`list` alone. `#[interceptor]` names a misspelled key rather than reporting a
repeated `priority`, and a refused `#[mcp(...)]` argument no longer blames the
`#[tools]` impl beside it. `#[api("…")]` — an argument that is not a key — is
refused with the shared unknown-argument sentence naming it as written, and
`#[mcp]`'s refusal of a server-identity field points at the key rather than at
the whole argument.

**Breaking for decorator authors** building on `nest_rs_codegen`: the worker-job
helpers take the member of the family they word rather than its name —
`transactional_value(expr)` is `transactional_value(JobDecorator::Process, expr)`,
`job_argument_needs_a_value(attr, name)` is `job_argument_needs_a_value(member,
name)` — and `job_key(member, &arg)` reads the `Arg` that
`JobDecorator::grammar()` hands over against the table above. A decorator reads
its `key = value` arguments through `Grammar` — declared as a `const` with
`Grammar::new(attr, keys)`, read with `parse`, `parse2` or `parse_attr`, and
`take_all` for a list mixing positionals and keys — and the closure it hands over
receives only keys of its own grammar, each the first time it is written.
`WrittenKeys`, 6.1's `unmatched_meta` and `reject_duplicate_argument` are gone with
the per-decorator loops they served, and the root `clippy.toml` refuses
`syn::meta::parser` and `syn::Attribute::parse_nested_meta` in the repository's
own crates. `DecoratorPair` can no longer be constructed outside
`nest-rs-codegen`: the nine pairs are `nest_rs_codegen::pair::{HTTP, GRAPHQL, WS,
MCP, HOOKS, PROCESSOR, INDICATORS, SCHEDULED, LISTENERS}`, listed by `pair::ALL`,
where a macro crate reads its own. `takes_value` and `site` word the value
family's sentences, `must_be_async` is gone with the rule it worded (a method may
be synchronous; `await_if_async` emits the call), `CrudConfig` is
`CrudDeclaration`, and the grammars above are public — `cfg_attrs`,
`DispatchKeys`, `Grammar` and its `Arg`, `duration_millis`, `replicas_value` and
its `Replicas`, `key_value`, `is_valid_job_key`, `RoutePath`, `HostBorrow`,
`ungrouped_expr`, `JobDecorator`, `JobKey`, `job_keys`, `job_returns_a_result`,
`unread_job_key` and `versioning::parse_version_args` among them.

### HTTP: a route is identified as poem serves it, `off` drops one header, and the nested settings are named for HTTP

- **Breaking: the settings `HttpConfig` nests are named for the crate** —
  `TlsConfig` is `HttpTls`, `CorsConfig` is `HttpCors`, `SecurityHeadersConfig` is
  `HttpSecurityHeaders` — since `Config` names a `#[config]` and nothing else.
- **`NESTRS_HTTP__<HEADER>=off` drops that one security header** whatever its
  default — `NESTRS_HTTP__HSTS=off` — as setting its field to `None` does in code.
  A blank value is refused at boot; an empty one is unset, as every variable is.
- **`#[routes]` reads a path with poem's own grammar**, so a path gains its leading
  `/`, `/q/:id` and `/q/:other` are one route, a parameter named two ways is
  refused, a path poem cannot mount is refused at compile time, and a duplicate —
  in one version, in a shared one, in every version, under true `#[cfg]`s — is
  refused naming both methods. Two verbs on one method and a generic method are
  refused too.
- **`#[http_code]`, `#[response_header]` and `#[redirect]` work written
  path-qualified** (`#[nest_rs::http::http_code(201)]`), and a shaper `#[routes]`
  does not read — outside a `#[routes]` impl, or under an import alias — is a
  compile error, where it was a silent no-op.
- The edge's typed errors move to `error.rs` with their public paths unchanged,
  and the transport never quotes a PEM value it cannot parse.

### WebSockets, GraphQL and MCP each dispatch one key to one method

- **WebSockets.** A `#[messages]` handler keeps every error type it had: the reply
  path reports an `Error` with its whole chain and anything else by its `Display`
  (`ReplyOutcome`, `ErrorReport`). `#[messages]` refuses a second method claiming
  one event or one connection hook, a method carrying two roles, an `#[on_connect]`
  returning a value, a generic method and a receiver other than `&self`. For code
  building replies by hand, `WsReply::from_handler_error` takes the error by value
  and `payload_error` the `serde_json::Error` it reports without its value (see
  *A logged error names every cause beneath it*); `WsScopeError` moves to
  `error.rs`, its path unchanged.
- **GraphQL.** A resolver's field name is written by the macro rather than left to
  async-graphql, by async-graphql's own rule — a new word after `_` and after a
  digit, so `get_2fa` is still served as `get2Fa` and the served SDL is unchanged —
  so two methods naming one field, `a1_b` beside `a_1b` included, are refused at
  compile time with both named. `#[dataloader]` carries a method's
  `#[cfg]` onto the loader it generates and calls the batch by its path, and a
  synchronous subscription or `#[entity]` resolver is accepted. **Fixed:**
  `#[operations]` keeps an `#[expect(…)]` written on a resolver method; it kept
  only `#[allow]`, so the lint the expectation answered fired anyway.
- **MCP: rmcp 3.1 → 3.4.** `ServerInfo` is `ServerConfig` upstream, and
  `nest_rs::mcp::{ServerConfig, ServerCapabilities}` name the two at the
  framework's surface, so the next upstream rename costs one line there rather than
  one per host. A project scaffolded from 6.1 resolves rmcp 3.4.1 today, whose
  deprecation of `ServerInfo` turns a `#[tools]` expansion into a `-D warnings`
  failure; 7.0 is its fix. `#[tools]` refuses two methods claiming one tool or
  prompt name. A `#[tool]` or `#[prompt]` doc line written as
  `#[doc = include_str!(…)]` or `concat!(…)` is part of the description sent to
  the model; it was dropped, and a doc written only that way was refused as
  missing. A description that evaluates blank — a
  `#[doc = include_str!("tool.md")]` of an empty file, a `concat!("")`, a `description = CONST` holding only
  whitespace — fails the build with the sentence a missing description gets, as a
  blank literal already did: the compiler evaluates the check where only it can
  read the value.

### An HMAC secret is held to its hash's size, a key is judged once, and the config is `AuthnConfig`

`JwtService::new` is the one place a secret's length, an EdDSA key's PEM and the
pairing of two keys are judged — reached by a config-driven boot and by a
`JwtOptions` built in code alike — so `AuthnConfig::into_options` only decides
which key the settings make.

- **Breaking: `nest_rs::authn::JwtConfig` is `AuthnConfig`**, with the same
  fields. A `#[config]`'s type is named for the stem its namespace and its path
  read, and `nest-rs-authn`'s reads `NESTRS_AUTHN__*`; the variables are
  unchanged. A refusal naming the type says `an AuthnConfig or JwtOptions built in
  code`.
- **An HMAC secret is held to its algorithm's hash size** (RFC 7518 §3.2): 32 bytes
  for HS256, 48 for HS384, 64 for HS512. 6.1 held every algorithm to 32, so a
  `JwtOptions` choosing HS384 or HS512 over a shorter secret now fails to build.
  `NESTRS_AUTHN__SECRET` is HS256, as before.
- **An EdDSA pair is proved one pair** by signing and verifying a probe, an
  algorithm that does not fit its key is refused naming the fitting ones, and a
  secret set beside an EdDSA key fails the boot rather than one of them winning.
- Every refusal names the variable's two spellings, and the guard's logged errors
  render their chain.

### An identity store or provider that does not answer is a 503, with its Retry-After

**Breaking for code that matched `AuthError::Unavailable(detail)`.** An OAuth
provider that timed out, refused the connection or answered `5xx` failed as
`AuthError::Failed` — a `401` telling the person signing in that their sign-in was
wrong — and an unreachable identity store was a `500` blaming this server's code.
The caller did nothing wrong in either case (RFC 9110 §15.6.4).

- `AuthError::Unavailable { detail, retry_after }` renders `503`, with a
  `Retry-After` when the failing party gave one in delay-seconds and none invented
  when it did not; it is logged at `error` and is never a challenge.
- The OAuth client reports a transport failure, a bound it ran past, and a `5xx`
  or `429` answer from the provider — on the code exchange and on a read alike —
  as `Unavailable`, carrying the provider's own `Retry-After`. A provider that
  answers and refuses is still `Failed`, a `401`.
- `Denial::Unavailable` (`Denial::unavailable(retry_after_secs, reason)`) carries
  it through `AuthnGuard` to every edge: `503` and `Retry-After` on HTTP,
  `extensions.code = "UNAVAILABLE"` with `retryAfterSeconds` on GraphQL, `reason:
  "unavailable"` with `retryAfterSeconds` on MCP and WebSocket. A strategy that
  outlives `AUTHENTICATE_TIMEOUT` gets the same `503`.
- `TokenError::Server` — the identity store a grant needed did not answer — is a
  `503` with the opaque `server_error` code, where it was a `500`.

### A stored hash the hasher cannot run is reported, never read as a wrong password

**Fixed, and breaking for code matching `PasswordError`.** `verify_password`
answered `Ok(false)` for every verification error, so a stored PHC string that
parses but names an algorithm, a version or parameters Argon2id refuses — an
scrypt hash, a bogus memory cost — read as a wrong password: its owner was
locked out, and a login service logged the one reason that was false. Only a
password that does not match is `Ok(false)` now; anything else is
`Err(PasswordError::InvalidHash(cause))`, which a caller already logs as an
unusable record.

- `PasswordError::HashFailed` and `PasswordError::InvalidHash` carry the hasher's
  error as their `#[source]` — `HashFailed(password_hash::Error)`,
  `InvalidHash(password_hash::Error)` — so `HashFailed` says whether the RNG or
  the allocator failed, and `InvalidHash` which part of the hash was wrong. A
  `match` names them with `(_)`.

### `nestrs g events`, every generator compiles, and the doctor reads what the app reads

- **Every crate is 7.0.0**, `nest-rs-cli` included. `nestrs new` and every
  `nestrs g` write `nest-rs = "7.0"`. **The framework requires itself at exactly
  its release**: every `nest-rs-*` crate depends on its siblings at `=7.0.0`,
  because a decorator's expansion calls `#[doc(hidden)]` seams — its own runtime
  crate's and, through the code `nest-rs-codegen` writes, up to eleven others' —
  that semver does not cover. An application requiring `nest-rs = "7.0"` sees no
  difference, and a partial `cargo update -p` can no longer pair one crate's
  expansion with another's seams. A dev-dependency on a sibling is a path and
  nothing else, so a published manifest carries no test-only edge.
- **`nestrs g events <feature>`** writes an event listener adapter —
  `events/listener.rs` and `<Feature>EventsModule` — and the fact it listens for at
  the port as `event.rs`: the last edge of the closed vocabulary without a
  generator.
- **Fixed: `nestrs g graphql <feature>` over a `nestrs g resource` port wrote a
  resolver that did not compile**, in 6.0 and 6.1: it bound
  `#[use_guards(AuthnGuard, AuthzGuard)]` on a `#[resolver]`, and `AuthnGuard` has
  no GraphQL check. The generated resolver binds `AuthzGuard` alone, as the demo's
  do, and authentication runs per operation through the `AuthzGraphqlModule` bridge
  the adapter's module imports.
- **Every adapter generator is compiled, not only read.** The CLI's e2e suite runs
  all seven edges over both port shapes from inside an app, the second app
  `nestrs new` adds to a workspace and `g migration`, and holds the result to the
  scaffold's own `clippy -D warnings`; a unit test joins its edge list to the edges
  the CLI knows, so a new edge cannot ship uncompiled. The text assertions that
  covered `g graphql` read a wrong import as readily as a right one.
- **`nestrs g queue` says how to push to the queue it declared** — inject
  `Arc<dyn JobProducer>`, bring `JobProducerExt` into scope, and import
  `RedisQueueModule` in the pushing app — where it named only the worker's half.
  The generated processor notes `concurrency` (one attempt at a time per replica by
  default) and the retry budget, and the generated tick notes `replicas = "one"`,
  which needs `RedisScheduleModule` and the `redis-schedule` feature.
- **`nestrs doctor` follows the loader.** An empty shell variable hides both `.env`
  spellings, nothing is trimmed that the loader keeps, and `NESTRS_ENV=prod` sends
  it to `.env.production` rather than a `.env.prod` no app reads. Each variable is
  answered as the loader reads it — a differential test in `nest-rs-cli`'s own
  suite runs the two side by side on every shape a variable takes, the loader a
  path-only dev-dependency that `cargo install` never links: a `_FILE` naming a
  file that holds nothing is `not set`, and one naming a missing or unreadable
  file, or a variable given both inline and as a file, is reported as failing the
  app's boot and blocks. Its tests no longer read the developer's shell or working
  directory.
- `nestrs doctor -p <dir>` opens a relative `<NAME>_FILE` from `<dir>`, where the
  app started there opens it, rather than from doctor's own working directory.
- `nestrs doctor` fails, non-zero, on a `.env` naming `<PREFIX>_ENV` or
  `NESTRS_ENV_PREFIX`: every app started there aborts at its first config read,
  since the loader refuses such a cascade whole.
- **The scaffold follows 7.0**: `.env` explains the `_FILE` form and that a secret
  beside an EdDSA key fails the boot, every `main` — the app's, `migrate`'s and
  `seed`'s — is `#[nest_rs::main]`, with `tokio` a dev-dependency, the queue
  template pushes with `push(Q, job, None)`, every generated feature root declares
  its log target (`pub const TARGET: &str = "features::<feature>";`) and the WS
  template logs on it through `error_message`, and the `AGENTS.md` a new project
  carries states the rules as
  7.0 reads them — `events/` is an edge and never a plural folder, a family is a
  level of a config namespace, a namespace belongs to one `#[config]`, `Config`
  names a `#[config]` and nothing else, every type a `module.rs` declares shares
  its stem, the queue port owns a job's id and its retry budget, and an edge folder directly
  under a framework crate's `src/` takes the crate's subject
  (`nest-rs-x/src/http/controller.rs` is `XController`). It says how each edge is
  guarded — a controller and a gateway bind `AuthnGuard, AuthzGuard`, a resolver
  binds `AuthzGuard` behind `AuthzGraphqlModule`, MCP gates through
  `AuthzMcpModule` — names the paths a project depending on the umbrella can type
  (`nest_rs::config::var_name`, `nest_rs::core::EnvPrefix::var`), says
  `EphemeralDatabase` needs the `testing` and `seaorm` features, and lists all
  seven edge folders.

### Dependencies — the whole tree moved, and five moves are visible from your code

Every third-party *requirement* now sits on its publisher's newest stable
release, with two exceptions named at the bottom. Most of the movement is
invisible: `cargo update` moved 73 crates in the framework's lockfile, 126 in
the demo's and 89 in the benchmark's, and two floors followed the lock — `rmcp`
to 3.3, then 3.4, and `uuid` to 1.26 — since the minor a manifest states is the
version we actually build against. What a consumer can see is named here rather than left in a
lockfile diff.

#### `croner` 3.0 → 4.0

- **Step syntax is stricter.** The shortcut `5/5 * * * *` — "every five minutes
  from minute 5" — is rejected by default; the OCPS grammar wants the range,
  `5-59/5 * * * *`. croner's message names the fix, and `#[cron]` reports it at
  **compile time**, not at boot. No `CronExpression` preset used the shortcut
  and neither did anything in this repository. croner can be built lenient again
  (`CronParser::builder().sloppy_ranges(true)`) and `#[cron]` deliberately does
  not expose that: strict is OCPS, `sloppy_ranges` is the deviation, and the
  only place a per-site flag could sit is the attribute itself — a second
  grammar for one declaration. The compile error carries the rewrite.
- **Daylight-saving overlap iteration changed, and it changed for the better.**
  croner 3 dropped an occurrence inside the repeated hour. On a fall-back day —
  25 real hours — `EVERY_HOUR` now fires **25** times where it fired 24, and
  `EVERY_MINUTE` (1440 → 1500), `EVERY_5_MINUTES` (288 → 300) and
  `EVERY_30_MINUTES` (48 → 50) gain everywhere. `EVERY_2_HOURS` gains only where
  the repeated local hour is even — Paris 12 → 13, New York 12 → 12 — because
  `*/2` selects hours 0, 2, 4 and the repeat lands on 02:00 in one and 01:00 in
  the other. Daily and longer presets are unchanged, and so is the
  spring-forward gap.
- **A live hot spin is gone.** Inside that repeated hour croner 3 could return
  an occurrence *in the past* — the first pass over a time the clock is about to
  repeat, up to 59 minutes stale — which `Scheduler`'s
  `(next - now).to_std().unwrap_or(Duration::ZERO)` clamps to zero, so the job
  re-fires as fast as the executor can reschedule it until the hour is over,
  once a year, per zone. Sampled every second across the 2026 `America/New_York`
  fall-back, `EVERY_MINUTE` alone: **3,540 negative delays under croner 3, none
  under croner 4.** The delay is never exactly zero in either — the clamp is
  what turns a stale answer into a spin, which is why `next_delay`'s own comment
  says "clamp defensively rather than unwrap a negative span".
- The `chrono` backend is now an optional feature, still on by default and
  still what we resolve. Occurrence methods became generic over a new
  `CronDateTime` trait; our call sites infer it without annotation.

Both halves of `#[cron]` now carry a trybuild snapshot. The expression half
never had one, so deleting the compile-time validation left the suite green
while the documentation quoted its diagnostic verbatim. `CronExpression`'s
presets are pinned to the instants they fire at, too: asserting only that each
parses could not see a preset change meaning, which is precisely what a cron
library's major release does.

#### `argon2` 0.5 → 0.6

- `password-hash` 0.6 drops the explicitly-constructed `SaltString` and has
  `hash_password` draw the PHC specification's recommended 16 bytes straight
  from the system CSPRNG. `hash_password` / `verify_password` keep their
  signatures, the pinned OWASP work factor still reaches the stored string, and
  the PHC output is unchanged.
- **Credentials hashed by the previous release still verify**, pinned as a test
  against two real 0.5.3-produced hashes rather than assumed — every other test
  in that file hashes and verifies with the same build and would miss a PHC or
  B64 change that locks a deployment out. Compatibility runs the other way too:
  a hash written by 0.6 parses under 0.5.3, so a rolling deploy is safe in both
  directions.
- **One panic left a hot path.** 0.5's `SaltString::generate(&mut OsRng)`
  panics if the system RNG fails; 0.6's `try_generate_salt()?` returns an error,
  which the code maps to `PasswordError::HashFailed`.
- **Salt bounds tightened.** `phc` 0.6 requires a decoded salt of 8–48 bytes and
  at least 11 B64 characters, where `password-hash` 0.5 accepted 4. Nothing this
  framework has ever written is affected — our salts are 16 bytes — but a
  credential *imported* from another system with a shorter salt now reports
  `PasswordError::InvalidHash` where it used to fail as a wrong password.

#### `rmcp` 3.1 → 3.4

- **`ServerInfo` is `ServerConfig` in 3.4**, left upstream as a deprecated alias that
  a `-D warnings` build refuses. The delegated `get_info` follows it, and
  `nest_rs::mcp` names `ServerConfig` and `ServerCapabilities` at its root — see
  *WebSockets, GraphQL and MCP* above.
- **`ServerHandler::negotiate_initialize` is new, and this framework was not
  forwarding it.** `PropagatingHandler`, `CompositeHandler` and the object-safe
  `McpHost` view now all delegate it, which is what rmcp's own
  `impl_server_handler_for_wrapper!` does. Nothing misbehaved on the wire — the
  method is reachable through the `initialize` the wrapper already delegated —
  and that is exactly why every behavioural proof missed it: the integration
  suite's probe host records only the methods someone thought to add to it.
  **The real fix is `#[deny(clippy::missing_trait_methods)]`** on both
  `ServerHandler` impls, which turns the next method rmcp adds into a compile
  error naming it, and reaches `McpHost` too because `CompositeHandler`
  delegates through it. The module headers no longer claim a hand-written list
  is "exhaustive by construction"; the compiler says so instead.
  **`McpHost` gained a required method**, which is a breaking change for
  anything implementing that trait by hand. Almost nothing does — it is
  blanket-implemented for every `ServerHandler`, so a `#[mcp]` host is one
  without naming it — but a type implementing it directly now needs a
  `negotiate_initialize`.
- **An empty `#[tool_router]` is refused** — a host whose `impl` block serves no
  tools is a compile error upstream, which is the position `#[tools]` already
  took. If an empty router is what you want, rmcp takes
  `#[tool_router(allow_empty)]`, which is what this repo's own mount-only test
  fixtures use. Only hand-written rmcp hosts are affected; one written through
  the `#[mcp]` / `#[tools]` pair never could be empty, and `#[tools]` refuses an
  empty block with its own sentence before rmcp sees it.

#### `handlebars` 6.4.3 → 6.4.4 — every `serde_json` map the demo emits reorders

handlebars dropped `preserve_json_order` from its default features, which
switches `serde_json::Map` from insertion order to alphabetical for everything
downstream of it. It reaches the demo through the exact-pinned `async-graphql`,
so `demo/apps/api/openapi.json` is rewritten — 1360 lines, **semantically
identical**: 18 paths and 18 schemas, none added, none removed. Almost all of it
is key order; four `parameters` arrays also swapped two elements, which OpenAPI
gives no meaning to.
The two workspaces had already drifted apart on this at 6.1.0: the framework's
lockfile was on 6.4.4 and the demo's on 6.4.3, so framework crates serialized
alphabetically and the demo's did not. They agree again. Nothing pins it, and a
future handlebars that restores the default flips it back.

#### Not taken

- **`redis` 1.7.** `apalis-redis` 0.7.4 requires `redis = "0.32"`, and the two
  must share one connection type. This waits on `apalis-redis` 1.0, whose stable
  line has not moved since 2025-11-18.
- **`cargo-chef` 0.1.78** in `demo/Dockerfile`, still pinned at 0.1.77. It is a
  `cargo install` build tool rather than a requirement, and no test walks it.

### A daily advisory watch, and apalis 0.7.4 kept past the freshness bar on the record

- **`.github/workflows/security-watch.yml` notices what the local loop cannot.**
  RUSTSEC-2026-0285 sat in every lockfile for eleven days because the Definition of
  done runs when someone touches the tree. The watch runs daily, on demand and
  whenever a lockfile, a manifest or the audit policy changes on `main`: cargo-audit over the
  framework's, the demo's and the benchmark's lockfiles with warnings denied, so an
  `unsound` or `unmaintained` advisory fails too, and a build of the framework on the
  beta toolchain, where apalis-redis 0.7.4's never-type-fallback lint turns into a
  hard error six weeks before stable. A third job runs `scripts/check-features.sh`,
  which checks every framework crate under its default features, under none and
  under each feature alone, reading crates and features from `cargo metadata`:
  every other gate builds one feature union, so a crate compiling only because a
  sibling turned a feature on is invisible there — which is how `nest-rs-authz`'s
  7.0 engine came to name `nest-rs-core` while it was optional before this release
  fixed it. The script runs locally as it does there. A failure opens one issue,
  or comments on it while it stays open. It is a monitor, not a gate.
- **Fixed: `nest-rs-seaorm` compiles with only its `graphql` feature.**
  `cargo add nest-rs-seaorm --no-default-features --features graphql` failed with
  `E0432`: its GraphQL refusals go through `nest-rs-authz`'s GraphQL binding, and
  only `http` forwarded its own; `graphql` now enables `nest-rs-authz/graphql`.
  The script's first run found it. An application depending on the umbrella was
  never affected.
- **apalis-redis 0.7.4 (2025-11-18) is kept past the 12-month freshness bar, by
  decision, and the root manifest says why.** The only newer line,
  1.0.0-rc.9, fails dead-replica recovery: a worker that exits without its clean
  close — an OOM kill, a cut drain, even a 1.5 s Redis stall — leaves a
  registration its peers' sweep reads with the wrong type (`WRONGTYPE`,
  apalis-redis#103), and every surviving worker of that queue dies at its next
  heartbeat; the proposed fix (#104) recovers no job when replayed, and #76 still
  reproduces. It moves when a release passes `nest-rs-redis`'s resilience e2e
  suite — never through `cargo update`, and without forking or vendoring. apalis
  types never leave `nest-rs-redis`.
- **RUSTSEC-2026-0253 (`lru` 0.16, `LruCache::pop` unsound when a key's `Drop`
  panics) is accepted in `.cargo/audit.toml` with its reason:** async-graphql 7.2.1
  reaches `pop` only through its dataloader `LruCache` factory and the Apollo
  persisted-queries store, and nestrs builds neither — `#[dataloader]` is
  `DataLoader::new`, which does not cache. The fix ships with async-graphql 8, still a
  release candidate.

### The rules are held by types, lints and behaviour tests

For a contributor. Through 7.0's development `nest-rs-conformance` grew into a
`syn`-based scanner of the framework's own source — "joins" proving a rule was
followed, and a `blinds` join proving the others could not be evaded — and three
audit rounds kept finding constructions that hid a member from a join. None of
the 27 caught a production defect after it landed. A source scanner sees
spellings while the compiler resolves items, so a rule is now held by the first
rung that can hold it (`CLAUDE.md`, *How a rule is held*; the history is
`.claude/decisions/conformance-scanner.md`):

- **Types.** `Transport::stop_bound` is required, `DurationBounds` and `Unit` are
  built only through constructors that check them at compile time, the decorator
  pairs are `nest_rs_codegen::pair::ALL`, every `key = value` decorator reads
  through `Grammar`, and `nest-rs-queue`'s capability test is an exhaustive
  `match`, so a new `Capability` does not compile until its refusal is tested.
- **rustc and clippy.** One `clippy.toml` at the repository root covers
  `crates/`, `demo/` and `bench/`: `tokio::main` is a disallowed macro;
  `std::env::var` and `var_os`, `syn::Attribute::parse_nested_meta` and
  `syn::meta::parser`, and apalis-redis's nine structure getters outside
  `legacy_layout.rs` are disallowed methods. Every entry has a canary
  `#[expect]` — in `nest-rs-macro-hygiene`, or in the crate that can reach the
  item — because an entry whose path stops resolving is only a warning. Both
  workspaces deny `unwrap_used`, `expect_used`, `panic`, `print_stdout`,
  `print_stderr`, `map_err_ignore`, `let_underscore_must_use`,
  `allow_attributes_without_reason`, `dbg_macro`, `todo`, `unimplemented`,
  `wildcard_imports` and `self_named_module_files`, with `allow-*-in-tests` in
  `clippy.toml` and one `#![allow]` per suite root. An exception is
  `#[expect(lint, reason = "…")]` at the site, so a stale one fails the build.
  `unsafe_code` is `deny` rather than `forbid` in the root workspace, so the three
  crates that had opted out of the whole table opt in. The sweep found the five
  defects fixed above.
- **Behaviour tests, whose assertions `cargo mutants` checks on each diff** — a
  unit test over the Redis key constants, an e2e running the 6.x check as a
  read-only Redis user, the doctor's differential test in `nest-rs-cli`, an MCP
  test that a decode failure behind `anyhow` reaches the operator's line without
  its value.
- **Review**, against a written sentence, for what none of these can see.

`nest-rs-conformance` keeps only structural checks over paths, manifests and
declared constants — the naming law, the test-target layout, the snapshot
fixtures, the target-constant prefixes and the `nestrs:` keys written outside
Rust — with no baseline file: 17,875 lines and 106 tests become 3,583 and 23. The
few source-reading tests left elsewhere are replaced in the same spirit.

- **The docs lint runs the canon generator.** `docs/canon.json` and
  `docs/demo-sources.json` are no longer committed: `lint-docs.mjs` runs
  `cargo run -p nest-rs-conformance --bin canon` for the framework facts it checks
  pages against and reads `demo/` sources directly, so no derived file can be
  stale. `npm run lint:docs` needs a Rust toolchain; the docs workflow installs
  one and also triggers on `crates/**`, `demo/**`, the root manifest, the lockfile
  and the README. Its failure messages no longer cut a detail at its first `::`.
- **Two docs-lint rules replace checks `nest-rs-conformance` held.** `family-mention` fails on
  a unit of work, an operator-facing span target, a queue `Capability` variant,
  or an umbrella capability's `cargo add nest-rs --features <x>` under an
  `## Install`, that no page names; `readme-install` fails when a capability
  crate's README does not install the umbrella with its feature, or when any
  README installs a capability sub-crate.
- **`scripts/check-features.sh`** checks every framework crate under its default
  features, under none and under each feature alone, and `nest-rs-macro-hygiene`
  has one feature per decorator-owning capability, so each capability's
  decorators are proved to compile under that capability's feature alone.
- **The trybuild suites run one at a time**, in a nextest `trybuild` test group
  with `max-threads = 1`: they shared one build lock while holding a test slot,
  and the non-e2e run went from 57 to 46 seconds here.
- **The definition of done has three tiers** — while editing, before each commit
  (fmt, workspace clippy, nextest over the touched crates' reverse dependencies,
  their e2e, `cargo mutants` on the diff), and before a merge or a release (the
  full gate, a run under `NESTRS_ENV_PREFIX=ACME`, the feature script, the
  audits, the docs lint and the demo).
- **The rules are rewritten** so they agree with each other and each names how
  it is held: `CLAUDE.md`, the zone rules in `.claude/rules/`, and
  `.claude/decisions/`, one file per decision recording what was tried and what
  retired it. Rustdoc cites the zone rule that holds its decision.

### The demo follows 7.0

- **The OAuth resource variables wear the family's namespace** in `.env` and the
  chart — `NESTRS_OAUTH_RESOURCE__*` was read by nothing, so the `assistant` app
  booted without its RFC 9728 identity and refused to start. The `.env` note now
  names `/.well-known/oauth-protected-resource`, the path RFC 9728 fixes.
- **The chart's KEDA triggers poll `nestrs:queue:audio:active` and
  `nestrs:queue:notifications:active`**, the lists the 7.0 worker fills, and its
  README and install notes describe the 7.0 worker: the delivery guard, retries
  filed on the schedule, per-method concurrency, and why a deployment taking delayed
  jobs keeps `minReplicaCount: 1` when no producer runs. Every app's pod keeps the
  default 30-second grace period: the worker's drain now fits inside it, and the
  live app's sockets close at the signal. The chart is version 0.2.0, for app
  version 7.0.0.
- **The audio transcode runs four at a time**, `#[process(concurrency = 4)]`, and
  streams the object from storage into storage (`get_stream` → `put_stream`), so an
  attempt holds one multipart part rather than the whole file twice. Its KEDA
  trigger's `listLength` moves from 5 to 20 — the backlog one replica is sized for,
  now that a replica runs four — and the notifications method stays at one.
- **The worker purges notifications older than 30 days once an hour, on one
  replica**: `#[every("1h", replicas = "one")]` on a new
  `NotificationsScheduleModule`, wired beside `nest_rs::redis::RedisScheduleModule`
  under the umbrella's `redis-schedule` feature, each occurrence claimed under
  `nestrs:schedule:claims:features:NotificationsTasks:purge_expired:<instant>`. It
  deletes through `Repo`, at most a thousand rows a tick. The chart's README says
  scaling the worker does not multiply its schedule.
- **The api's synthetic transcode seed fires once per occurrence across its
  replicas**: `AudioTasks::enqueue_transcode` enqueues work every replica would
  otherwise repeat, so it declares `replicas = "one"`, and the api binds
  `RedisScheduleModule`; its heartbeat says each process is alive, so it declares
  `replicas = "each"`, written out.
- **Breaking for a copy of the demo: its OAuth issuer config is `OAuthConfig`**,
  read from `NESTRS_OAUTH__CLIENTS` and `NESTRS_OAUTH__DEFAULT_ORG_ID` — it was
  `IssuerConfig`, under `NESTRS_ISSUER__*`, a namespace its path did not name.
  `nest-rs-conformance`'s naming check now holds every `#[config]`'s namespace and
  type name to its path in both workspaces.
- **A publish whose transaction rolls back enqueues no notification**, now that
  events wait for the commit; an e2e test drives both outcomes against real
  Postgres and Redis.
- **The demo's log targets are constants its features declare**: each feature
  that logs declares `TARGET` at its module root and logs on
  `crate::<feature>::TARGET`; a literal target at a call site is a review
  finding.
- **Every error the demo logs goes through `error_message`**, at all eleven sites,
  so a wrapper such as `AudioError::Queue` names the cause beneath it.
- The posts MCP host reports itself as a `ServerConfig` read from `nest_rs::mcp`, the
  audio feature pushes through `push(AudioQueue, command, None)`, the OAuth
  config's `CLIENTS` is decoded through `ConfigService::json` — so a mistyped value
  is never quoted, a client secret included — and the e2e suites read their database and
  Redis URLs through `ConfigService`, so a `_FILE` spelling reaches them.
- The live app's e2e suite boots on a throwaway database, as every other suite
  over SeaORM does, and never opens the one `NESTRS_SEAORM__URL` names — one of its
  tests seeded that database directly, failed on an unmigrated one and could leave
  rows behind. `cargo check --locked` works in `demo/` again.
- **The demo's e2e suites no longer share Redis with a running app.** A committed
  `.env.test` puts the test profile on logical database 1, which nothing drains,
  and each test that starts a worker takes a database of its own, 2 to 8, from
  `features::testing::RedisDatabase`. The worker's e2e boots on a throwaway
  Postgres: it used to run whatever a developer's `api` or another suite had
  queued, against the main database, and a push-only suite's jobs were run by a
  developer's `nestrs run dev` worker against real data. Redis's sixteen logical
  databases are split between the two workspaces' suites — 0 the developer's, 1
  to 8 the demo's, 9 to 15 the framework's — and a database outside 9 to 15
  declared by `nest-rs-redis`'s e2e suite does not compile.

### Also

- **`nest-rs-conformance` reads every path below the repository root.** Its
  checks took the absolute path's components, so a clone under `~/src/`, or under
  a folder named like an edge, changed their verdicts; one strip now serves them
  all, and a unit test reads a path the same below any root.
- **What holds each rule 7.0 states**, now that the source scanner is gone (see
  *The rules are held by types, lints and behaviour tests* above): every
  `nestrs:` key a page, a chart or a script spells is built from a key constant
  the code declares (`nest-rs-conformance`'s keys check), and the constants obey
  the key law — each a level of its owner's concern, a queue's keys naming the
  queue first, none a twin of another or a prefix inside a level (a unit test
  over `nest-rs-redis`'s key constants); every queue `Capability` is refused
  where a backend lacks it (an exhaustive `match` in `nest-rs-queue`'s tests, so
  a new variant does not compile until it is) and is named on a page (the docs
  lint's `family-mention`); a compile-fail fixture parses, and its snapshot pins no
  resolution error the fixture does not declare (the snapshots check); every type
  a `module.rs` declares shares its stem, a module lives nowhere else, an edge
  folder directly under a framework crate's `src/` adapts the crate and takes its
  subject, and in a product crate an edge adapter belongs to a module folder (the
  naming check); every `nest_rs…::` path a docs page or a crate README names
  resolves to a public item (the paths check), which found a pagination example
  calling a private module. That every error type lives in `error.rs`, that a root
  file of an adapter crate serves more than one binding, that `Config` names a
  `#[config]` alone and that a file under an edge folder serves that edge alone
  are review items.
- **`unreachable_pub` is a workspace lint**, and every crate now opts into
  `[lints] workspace = true`, so an item no caller outside its crate reaches is
  `pub(crate)`.
- **Fixed: one control character in a `Server-Timing` `desc` no longer drops the
  whole header**, every other entry and the total with it; it is written as a
  space.
- **Fixed: a remote parent the subscriber cannot take is reported.** When the
  OpenTelemetry layer is missing from a span's subscriber, or the span started
  before the link, every continued trace exported without its parent in silence;
  one `warn` on `nest_rs::opentelemetry`, `remote parent not linked; continued
  traces export without their parent`, now says so, once per process. A span the
  layer's own filter disabled stays silent.
- **Fixed: an OAuth client endpoint URL that does not parse says why** — the
  parser's reason is in the error beside the value.
- **Four TLS tests pass on macOS.** The client trusts the fixture authority alone,
  verified by rustls with webpki on every platform, rather than merging it into the
  platform store, whose server-certificate policy rejected the long-lived fixtures.
- **The documentation gains** [Upgrading from 6.x to 7.0](https://nestrs.dev/upgrading/),
  and in the queue section *Delivery on Redis*, *Concurrency and scaling* and
  *Upgrading queues from 6.x*; the environment reference documents every variable
  above and what the boot says about one nothing reads; and every queue page's
  output is pasted from a 7.0 run. The schedule page no longer says a successful
  tick is silent, or that a tick opens a `scheduled job` span: it files one
  `schedule.tick` line on `nest_rs::operation`, in a span of that name.

## [6.1.0] - 2026-08-29

### `nestrs lint` — a file's stem, read against what it declares

Every naming rule in `architecture.md` is *derivable* from a path: a `module.rs`
under `redis/queue/` is a `RedisQueueModule` whatever it holds, so the
conformance suite reads the path and compares. One rule is not. Nothing about
`principal.rs` predicts `Principal` — only reading the file says whether the two
meet — and that rule is now a command:

```text
nestrs lint

  crates/features/src/desks/principal.rs
    `principal` reaches none of `DeskOperator`
    name the file from the type, or split it — a stem that reaches nothing
    is a slot, and a slot fills
```

- **One shape is refused, and only one:** a stem that reaches *nothing* the file
  declares. That file was named for a slot — "who acts", "what we pass around" —
  rather than for a subject, and a slot has no admission test, so the next type
  about that slot lands there too. It is a `shared/` folder at the scale of a
  file, and it is invisible from outside: both names read perfectly well alone,
  and only the pair is wrong.
- **Everything short of that passes, and the tolerance is deliberate.** The
  tighter test — the stem as the type's first or last word — reads well and is
  false on a third of this framework: the shared word may come from the folder
  (`throttler/store.rs` holds `RedisThrottler`), an inflection is the same word
  (`scope.rs` holds `Scoped`), an abbreviation is one too, and a file whose
  principal export is a function is a namespace that owes no pairing at all
  (`queue/consume.rs` exports `consume::attempt`). Files a table already names —
  `service.rs`, `controller.rs`, `registry.rs` — are that table's business.
- **The rule shipped and the rule met are one symbol.** `nest-rs-cli` grows a
  library target, and `nest-rs-conformance` calls `nest_rs_cli::lint::scan` over
  both workspaces instead of carrying a second implementation — which is how the
  two would come to disagree with nobody positioned to notice. The reserved
  vocabulary goes the same way: `nest_rs_cli::reserved_words()` derives it from
  the `architecture.md` the CLI embeds, and the suite reads it from there rather
  than re-parsing the markdown.
- **Declarations are read with `syn`, not with a text scan.** A `pub struct`
  inside a template string is what a scan counts and a parser does not, and this
  repo ships such strings.
- **It exits non-zero on a finding**, so it belongs in CI. The scaffolded `lint`
  recipe runs it, behind `nestrs run lint`.

Nothing here is an install surface: `nestrs` is still reached with
`cargo install --locked nest-rs-cli`, and the library exposes only what a second
caller needs — `lint` and `reserved_words`.

Four files in this repo were named for a slot and now pair with what they hold:
`nest-rs-throttler`'s `rate.rs` → `throttle.rs` (a private module; `Throttle` and
`DEFAULT_THROTTLE` are re-exported from the crate root as before), the demo's
`audio/http/extract.rs` → `uploaded_audio.rs`, and the hygiene crate's
`HygieneLoaders` → `HygieneDataloaders`, `HygieneTier` → `HygieneWireTier`.

**`dto`, `command` and `event` join the reserved vocabulary** as a `singulars`
row: they name role files, so a module wearing one makes every path ambiguous.
`nestrs new dto` and `nestrs g feature event` now refuse, as the plurals and the
edges already did.

### The scaffold keeps one layout, and it is the workspace

**Removed: `nestrs new --standalone`.** It wrote a shape the rest of the CLI then
refused to serve — every generator resolves paths against `crates/features/` and
wires the serving app's `module.rs`, so `g feature`, `g resource`, `g auth`,
`g migration` and the six adapters all failed inside a single crate with
`not inside a nestrs workspace`. A starter whose next command cannot run is not a
second layout, it is a dead end, and growing out of it was a documented page of
hand-moved files.

`nestrs new` writes one thing: `crates/features/` for the domain, `apps/*` for
the binaries that serve it. A second workload is another `nestrs new` inside it,
never a second repository.

- **The templates lose the standalone service, controller, manifest and `main`**,
  and with them the `{{service_use}}` seam that existed only so one template
  could serve two shapes.
- **`Dockerfile` and `.dockerignore` are no longer scaffolded.** They shipped in
  standalone mode alone, and a workspace image has to name which app it builds —
  a decision the scaffold cannot make for you. The repo's own images are
  unaffected and still hold their pins.
- **`nestrs info` drops the standalone row**; outside a workspace it says so
  plainly.

If you carry a crate scaffolded that way, nothing in it stops working — it is an
ordinary Cargo crate. The CLI page keeps the file-by-file move under *Move an
existing crate into a workspace*.

### `nestrs run test cov` works on a project minutes old

The recipe shipped; the tool behind it did not, and coverage failed on a freshly
generated project. **`cargo-llvm-cov` joins the first-run bootstrap** beside
`just`, `bacon` and `cargo-nextest` — all of it at once, whichever recipe was
asked for, because a project's own recipes are forwarded without the CLI knowing
what they run. Whoever wants to pay nothing still has `--no-bootstrap` /
`NESTRS_NO_BOOTSTRAP`.

- **A cargo subcommand is probed through its verb.** `cargo-llvm-cov --version`
  answers `expected subcommand 'llvm-cov'` and exits `1`, so a probe assuming the
  bare flag would call an installed tool missing and reinstall it on every
  `nestrs run`. The arguments are now read off the tool.
- **`llvm-tools-preview` is pinned in `rust-toolchain.toml`** — this repo's and
  every scaffold's. It is not a crate and never lands on PATH, so the bootstrap
  could not own it; and `llvm-cov` reads a `.profraw` only when it comes from the
  LLVM that rustc itself was built with, which is exactly what following the
  `channel` guarantees. A scaffold's copy now declares `clippy` and `rustfmt`
  as well — it pinned the channel alone, so those two were present only because
  rustup's *default* profile installs them, and `nestrs run lint` worked on your
  machine while failing on a `minimal` one.
- On a toolchain installed outside rustup the file is inert — point `LLVM_COV`
  and `LLVM_PROFDATA` at your own binaries.

### Also

- **The docs site wears its own design.** Logo, wordmark, favicon and social
  preview redrawn; every code fence names the file it comes from; and above
  1440px the layout caps and centres instead of stretching code blocks and tables
  while the prose held its measure — a wide window buys margin, not a wider
  column.
- **`architecture.md` gains the vocabulary-pairing rule**, the reason a `shared`
  crate is refused where `core` survives (a kernel is checkable: everything
  composes on it and it composes on nothing), and the note that a pluralized
  folder exists to carry several — one of a kind is `dto.rs`, not `dtos/` with a
  single entry.
- `resolve_start` — the `-p` / cwd fallback every path-taking command repeats —
  moves up to `commands`, one spelling for the four of them.
- `nest_rs_core`'s hex codecs read `as_chunks::<2>` rather than `chunks_exact`,
  so the pair indexing is bounds-checked once at the split.

## [6.0.0] - 2026-08-26

### Ports & Adapters — three module shapes, and a variable is a path too

The framework now states what it is: **Ports & Adapters**, as practised — the
port owns the semantics, the adapter owns only the transport, a multi-backend
library is wrapped rather than abstracted, and the composition root has
**three module shapes and no fourth**. `architecture.md` carries the section;
`naming.rs` and `units.rs` in `nest-rs-conformance` keep it true.

- **Three shapes.** `<Vendor>Module::for_root(cfg)` opens a resource —
  `SeaOrmModule` (the pool), `RedisModule` (the connection). `<Port>Module::for_root(cfg)`
  carries a capability's policy — `ThrottlerModule`, `HttpModule`.
  `<Vendor><Port>Module` binds one to the other, bare unless it owns settings —
  `SeaOrmDatabaseModule`, `SeaOrmHealthModule`, `RedisQueueModule`,
  `RedisThrottlerModule`, `RedisWorkerModule::for_root`. A composition root reads:
  `SeaOrmModule::for_root(None), SeaOrmDatabaseModule, RedisModule::for_root(None),
  RedisQueueModule, RedisWorkerModule::for_root(None), ThrottlerModule::for_root(None),
  RedisThrottlerModule`.
- **A `#[config]`'s namespace is its stem — crate subject, then binding folders,
  joined by `__` — exactly as its type is named.** `SeaOrmConfig` →
  `NESTRS_SEAORM__*` (**`NESTRS_DATABASE__*` is gone**: the universal convention
  named neither the crate nor the type that parsed it); `RedisConfig` →
  `NESTRS_REDIS__*` (**`NESTRS_QUEUE__*` is gone**); `RedisWorkerConfig` →
  `NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS`; `NESTRS_THROTTLER__*` and
  `NESTRS_SOCIAL__GITHUB__*` unchanged. `namespace_is_the_stem` compares segment
  by segment. From a variable a reader finds the type; from a module, the
  variable.
- **`nest-rs-seaorm` is the adapter that wraps sea-orm.** `SeaOrmModule::for_root`
  resolves `SeaOrmConfig` and opens the one pool; `SeaOrmDatabaseModule` is a bare
  binding that installs the ambient executor (`DbContext`, `WorkerDbContext`)
  and fails the boot naming `SeaOrmModule` if the pool is absent.
  `SeaOrmDatabaseConfig` and `SeaOrmDatabaseSetup` are gone; `connect_from_env`
  resolves `SeaOrmConfig`.
- **`nest-rs-redis` is one connection and three bindings.** `RedisModule::for_root`
  opens `RedisConnection`; `RedisQueueModule` (bare) binds `RedisQueueProducer`
  as `Arc<dyn JobProducer>`; `RedisWorkerModule::for_root` owns
  `RedisWorkerConfig`; `RedisThrottlerModule` (bare) declares the store.
  `RedisQueueConnection`, `RedisQueueConfig`, `RedisQueueSetup` and
  `RedisThrottlerSetup` are gone; the connection's events file on a
  `nest_rs::redis` target.
- **The throttler's policy lives in the port, its counters in the store.**
  `ThrottlerStore` is `hit` alone — `default_limit` is gone, and so are the
  `default` arguments of `InMemoryThrottler::new` and `RedisThrottler::new`;
  the guard carries the default (`ThrottlerGuard::new(store, default)`,
  `Throttle: Default`). `ThrottlerModule::for_root` binds the in-process store as
  an *ordinary* factory, so `RedisThrottlerModule` supersedes it wherever it sits
  in `imports` and the app removes no line; two vendor bindings still contest
  (`BACKEND_REMEDY`). `provide_guard` and `resolve` are no longer public seams.
- **The queue port owns the attempt.** `nest_rs_queue::consume::{discover, attempt,
  Attempt}` — discovery, the envelope, the trace, the `queue.job` span, the panic
  catch, the outcome classes, the events and the operation line — written once in
  the port; `RedisWorker` is a fetch loop that translates `Attempt` into apalis's
  `Abort`/`Failed`. A second adapter copies nothing, and
  `a_unit_is_opened_only_by_the_crate_that_declares_it` refuses one that tries.
- **A factory may declare the factory output it reads.**
  `ContainerBuilder::provide_factory_after`, `provide_factory_dyn_after` and
  `provide_declared_factory_after` name the type; the boot drains in dependency
  order with queue order as the tie-break, so `imports = [..]` order is a
  readability choice. A cycle is `FactoryCycleError`, naming the set.
- Demo features inject `Arc<dyn JobProducer>`; the CLI scaffold and `.env`
  templates write `_SEAORM__URL` / `_REDIS__URL`; `nest-rs-throttler`'s in-band
  caller-bucket helpers are gated on the edges that use them.

### The one guard that answers every transport lives at the root

`AbilityGuard` implements four of `Guard`'s entries — `check_http`,
`check_graphql`, `check_ws_message`, `check_mcp` — and carries the four matching
marker traits, but it sat in `nest-rs-authz/src/http/`. A folder named for one
edge held the type that answers all of them, and the cost was never only the
path:

- **`nest_rs::authz::http::AbilityGuard` is now `nest_rs::authz::AbilityGuard`**,
  beside `gate` and `chain` — the crate's other transport-agnostic decision
  points. Each `check_*` arm is gated on its own feature.
- **A transport no longer turns on `http` to reach its own guard.** `graphql`,
  `ws` and `mcp` each pull the guard's dependencies directly; a GraphQL-only app
  used to enable the HTTP surface for a type it bound on resolvers.
- **`check_ws_message` compiles under `ws`, not `http`.** The `http` feature
  forwarded `nest-rs-guards/ws` and `nest-rs-ws` for exactly one method of one
  misfiled file; both forwardings are gone, and `gate::transport::WS` drops the
  `#[cfg(any(feature = "http", feature = "ws"))]` that documented the anomaly.
- **In the product and in what the CLI scaffolds, `authz/http/` is gone.**
  `AuthzGuard` is `authz/guard.rs` and `AuthzModule` provides it, so
  `AuthzGraphqlModule`, `AuthzWsModule` and `AuthzMcpModule` import `AuthzModule`
  rather than the HTTP adapter they never served. `AuthzHttpModule` held that one
  provider and nothing else; every `imports = [.., AuthzHttpModule]` becomes
  `imports = [.., AuthzModule]`.

### A declaration is refused at the token that wrote it

The second refusal wave: what a decorator silently accepted, deferred to a
dependency's error, or worded for itself alone is now one sentence per
offence, worded once in `nest_rs_codegen`, spanned at the offending token,
with a trybuild snapshot per site — eighteen new snapshot pairs across eight
crates.

- **A mount path has a grammar, and every decorator that takes one enforces
  it.** `#[controller]`, `#[gateway]` and `#[mcp]` refuse a path that is
  empty, not absolute, longer than 256 characters, carrying an empty segment,
  or spelling a character RFC 3986 §3.3 does not allow in a segment —
  percent-encoding included, deliberately: a mount path is written, not
  received, so `%2F` asks to mount an address the router will never match.
  `#[mcp]` used to refuse only the empty string; the other two checked
  nothing, so `path = "users"` compiled and mounted.
- **`#[use_guards]` on the impl half names the struct half.** One sentence
  from the pair itself — the struct half declares the host's access posture,
  this half declares its operations — where GraphQL and MCP each had a drifted
  wording of their own and `#[routes]` / `#[messages]` had *nothing*: the
  attribute reached rustc as ``cannot find attribute `use_guards` `` with no
  transport, reason or remedy named. `#[force_guards]` is refused the same
  way, and a `#[crud]` impl inherits both.
- **A missing mandatory key shows what to write.** ``#[queue] requires `name`
  — write `name = "emails"` `` — one shape at ten sites in six crates, where
  three verbs and three shapes had grown.
- **A method declaring two roles is refused naming both**, on `#[hooks]`
  lifecycle phases, `#[indicators]` probes, `#[scheduled]` triggers, and MCP
  roles — where it closes a real defect: a method carrying both `#[tool]` and
  `#[prompt]` compiled, and the surviving second attribute was handed to rmcp
  as an operation nobody declared.
- **`#[input]` names the shape it cannot take.** A tuple or unit struct used
  to die inside `validator`'s derive — `` Unsupported shape `one unnamed
  field` `` against a `#[derive]` line the developer never wrote. The refusal
  now names the shape, the derive whose limit it is, and the enum/newtype
  remedies.
- **`#[cron(tz = …)]` is checked against the IANA time zone database at
  compile time.** The key is always a string literal over a closed name set,
  so a typo was the one half of the attribute still deferred to a deployment
  failure while the expression beside it was refused at expansion.

### A 405 names what the route serves

RFC 9110 §15.5.6 makes `Allow` on a `405` a MUST, and the framework computed
the verb set at the declaration and discarded it at the mount — so poem's
`MethodNotAllowedError` rendered a bare status. `#[routes]` now registers
through `nest_rs_http::MethodTable`, which records each verb in the same call
that mounts it, so what is served and what is advertised cannot drift —
`#[version]`-narrowed routes included, each version advertising its own set.

- **`HEAD` is advertised whenever `GET` is** — poem answers an unregistered
  `HEAD` by re-dispatching to the `GET` endpoint, so it is served. **`OPTIONS`
  never is**: serving it would put an unguarded method-listing endpoint at
  every route, so it is an owner question rather than a default.
- **Only poem's own refusal is rewritten.** A handler that deliberately
  answers `405` is stating something about its own resource and keeps its
  response; an unrouted path stays a bare `404` — inventing an `Allow` there
  would turn every probe into a confirmation that something is there.
- **Self-mounting crates are the reported gap.** `nest-rs-graphql` routes by
  method through poem directly, so `PUT /graphql` still answers a bare `405`;
  `MethodTable` is public as the drop-in for it.

### A timeout is the origin's own 503, and every header the framework knows is emitted or argued

**Breaking, wire and API.** Four corrections on the HTTP edge, each aligning a
default with the standard that owns it:

- **An overrun handler answers `503 Service Unavailable` with `Retry-After`,
  not `504`.** RFC 9110 §15.6.5 scopes `504 Gateway Timeout` to a server
  acting as a gateway or proxy; this transport is the origin, and a `504`
  invites exactly the wrong retry — a load balancer reading it as "that
  node's upstream is flaky, try another one" re-runs a handler that will
  overrun again. The delay is whole seconds rounded up, minimum 1, because
  `Retry-After: 0` reads as "retry immediately", the one instruction a server
  shedding load must not give.
- **`HttpConfig::request_timeout_secs: Option<u64>` becomes
  `request_timeout: Option<Duration>`**, read through the framework's shared
  seconds grammar — so `NESTRS_HTTP__REQUEST_TIMEOUT_SECS=0` now means *no
  timeout*, the family's sentinel, instead of zero seconds. And
  `max_body_bytes = 0` fails the boot naming the field, instead of rejecting
  every bodied request, the same floor its MCP and WS siblings already have.
- **The security-header family is closed: each member is emitted by default,
  or configurable with its default argued.** Three new defaults —
  `Referrer-Policy: strict-origin-when-cross-origin`,
  `Cross-Origin-Opener-Policy: same-origin`,
  `Cross-Origin-Resource-Policy: same-origin` — and three settable but
  deliberately off, each with its reason recorded: `Content-Security-Policy`
  (an API framework cannot know a page's sources),
  `Cross-Origin-Embedder-Policy` (breaks embeds to buy an isolation nothing
  here uses), `Permissions-Policy` (a default would be a guess about pages
  the framework does not serve). One table drives validation and emission,
  so a member cannot arrive validated but unemitted.
  `SecurityHeadersConfig::headers` now returns `HeaderName`s.
- **CORS refuses `*` beside credentials on all four lists**, not just
  `origins`: `*` is a valid `tchar`, so nothing else refuses it — the header
  is emitted, every browser drops the response, and there is nothing
  server-side to point at. The error names the config field and the response
  header it would have poisoned.

### The client behind a proxy is RFC 7239's answer, and disagreement is refused

The transport read only `X-Forwarded-For` and `X-Real-IP`, so behind an
RFC-conformant proxy speaking `Forwarded` every caller resolved to the
balancer: one rate-limit bucket for the entire internet, the balancer's
address on every access line and span, and — the peer being trusted —
`traceparent` continued for a client the transport could not identify. All
three headers are now read, in order `Forwarded`, `X-Forwarded-For`,
`X-Real-IP`, gated on the same trusted-peer check.

- **§6.2 `unknown` and §6.3 obfuscated nodes are skipped**: reading either as
  a client would key a rate-limit bucket on a string a proxy chose to
  withhold.
- **When `Forwarded` and `X-Forwarded-For` both name a client and disagree,
  neither is believed.** The origin degrades to the trusted proxy itself and
  a `warn` carries both values. The common way to reach that state is a
  proxy that appends `X-Forwarded-For` and passes unknown client headers
  straight through — nginx's default — where a caller sending
  `Forwarded: for=…` would outrank the genuine hop the proxy itself
  appended, and the trusted-proxy gate cannot catch it. A "prefer RFC 7239"
  rule is exactly that hole.
- **Breaking:** `ClientOrigin::resolve` gains a leading `forwarded`
  parameter.

### A health probe answers inside the kubelet's second

**Breaking for slow indicators.** Indicators ran serially, each under a
hardcoded five-second ceiling — four of them were a twenty-second worst case
against a kubelet whose `timeoutSeconds` defaults to **1**. They now run
concurrently, so a probe costs the slowest check rather than the sum, under
two ceilings a new `HealthConfig` owns:

| | Default | Meaning |
|---|---|---|
| `NESTRS_HEALTH__INDICATOR_TIMEOUT_MS` | `750` | one check's ceiling — expiry reports it `down` |
| `NESTRS_HEALTH__PROBE_DEADLINE_MS` | `900` | the whole response's ceiling |

- **Milliseconds, deviating from the framework's `*_SECS` grammar on
  purpose**: a grammar that cannot express a value inside one second cannot
  express the only interval that matters here. And **`0` is refused at boot**
  rather than meaning unlimited — here *off* is the defect, since a hung pool
  would hang the probe and the kubelet reads that as a dead process.
- **An unanswered indicator reports `"probe deadline exceeded"`**, the third
  fixed reason beside `"check failed"` and `"timed out"`; the real error goes
  to a `warn` on `nest_rs::health`, never to the caller.
- **Concurrency is `FuturesUnordered`, deliberately not `spawn`**: the
  operation span and the trace context are task-locals, and a spawned check
  would file its `warn` under no unit of work at all. The report body stays
  name-ordered, not completion-ordered.
- **A prefixed app says where its probes went.** Under
  `NESTRS_HTTP__GLOBAL_PREFIX=/api/v1` the kubelet must call
  `/api/v1/health/live`; a manifest written from the docs gets a `404`, the
  kubelet reads that as a failed probe, and on a liveness probe that is
  `CrashLoopBackOff` caused by documentation. The boot now logs one `warn`
  naming the real paths.
- `HealthModule::for_root(HealthConfig…)` is the pinning seam, with the
  composition test every `for_root` owes.

### A listener invocation is a unit of work

The last unused edge namespace is in use: every `#[listeners]` method files
one `events.dispatch` line, with its own span — a name nothing emits under is
the same defect as a span field nothing fills.

- **The unit is one listener invocation, not one `emit`.** A listener is
  developer code that logs, writes and can panic, while an `emit` is the
  emitter's own line, already inside whatever unit the emitter is serving.
  One emit is one cause: its listeners share one trace, and each gets its own
  `span_id` — filing them all under the inherited correlation gave two
  listeners on one event one `span_id` between them.
- **A panicking listener still files, with `outcome = panic`**, and both the
  line and the containment `error` are emitted inside the continuation, so
  neither arrives without ids.
- **Listeners with no bus are a boot report.** A provider listed in
  `providers = […]` without `EventsModule` in `imports` used to boot clean
  and react to nothing; the boot now emits one `warn` per reachable listener,
  naming it.
- `#[listeners]` registers through `subscribe_named`, so the line names the
  method; the anonymous `subscribe` files as `<anonymous>`.

### Every dispatched GraphQL field files its own line

Every query and mutation in a deployment was the same line — `POST /graphql
200` — and which field was slow, which one failed, and which one the caller
was refused at were all unanswerable from the console. `#[operations]` now
runs every `#[query]`, `#[mutation]`, `#[entity]` and `#[field_resolver]`
body through a `graphql.operation` unit: its own span under the request's,
one line with `role`, `operation`, `outcome` and `duration_ms`.

- **A failing resolver files `outcome = error`** — a GraphQL error is
  answered with a `200`, so the HTTP line alone reports a request that failed
  as one that succeeded.
- **`#[subscription]` is deliberately excluded**: its unit is the connection,
  filed as `graphql.subscription` when the socket ends; wrapping it here
  would file a second line naming the *subscribe*, which is not the work.
- **`operation` is the wire field name**, not the Rust ident — async-graphql
  renames `list_users` to `listUsers` on the wire, and a line naming the
  ident cannot be joined against a capture of the request that produced it.
- **The span deviates from the OTel GraphQL conventions where they cannot
  carry this, and says so**: `graphql.operation.role` carries a superset of
  the conventions' `operation.type` enum (`entity`, `field`), and
  `graphql.document` is deliberately absent — it is the caller's query text,
  which carries their literals.
- The generated delegating method is now always `async` for the wrapped
  roles.

### The MCP line names the wire method and what it addressed

**Breaking for log consumers.** The operation line carried the rmcp Rust
ident — `call_tool`, `initialize` — which appears in no MCP document and
cannot be joined against a capture of the wire, and every `tools/call` was
byte-identical. The line now carries **`method`**, the JSON-RPC method
(`tools/call`), and **`operation`**, the tool, prompt, resource or task it
addressed — absent where nothing is addressed, so a query for one can never
match a method that has none.

- Method strings are read from rmcp's own `ConstString` markers, so a
  protocol rename is a compile error instead of a stale literal; the one
  request whose method is *data* (`on_custom_request`) reads it off the wire.
- The span gains `mcp.method.name` / `mcp.operation.name` — one seam over
  every method, where the semantic conventions spell three per-kind keys.

### A socket ends with the code that says why

Every server-side termination dropped the `Sink`, so the peer read **1006
Abnormal Closure** — a deliberate close indistinguishable from a broken pipe,
against which a client retries identically and never learns what to do
differently. The gateway now writes a Close frame, **after flushing every
reply already queued**, with the RFC 6455 code the cause earns:

| Cause | Code |
|---|---|
| connection lifetime ceiling | `1001 Away`, reason `re-upgrade to continue` — nothing the peer sent reaches this ceiling, a clock does, so not `1008` |
| outbox full | `1008 Policy` |
| read failure | `1011 Error` |
| peer's own Close | echoed, then flushed — a second Close would be refused, not merged |

- **A Binary frame is refused in band** instead of silently dropped: an
  `error` envelope answers it and the socket survives, `1003` being the other
  conformant answer and deliberately not this one.
- The connect/disconnect lines gain `outcome = ok` — the one field of the
  family those two dropped, so a cross-edge `outcome != ok` query silently
  skipped them.

### `nest_rs_testing` drives a real upgrade

New `ws` feature: `TestAppBuilder::build_ws()` boots the app's own configured
transport on a reserved port and hands back a `WsApp`, whose
`socket(path).bearer(token).connect()` yields a `WsSocket` — `send`,
`next_envelope`, `expect_silence`, `expect_close`, `close`. `expect_close`
panics on a socket that ends without a Close frame: "the connection went
away" is never an acceptable pass. The client is `tokio-tungstenite` at the
exact version poem's own `websocket` feature already compiles, so nothing new
enters the build. The WS suites — the framework's own gateway tests included
— now assert close codes and reasons off the wire instead of accepting any
termination.

### One throttler guard meters every edge, keyed per caller

**Breaking for custom stores and for dashboards.** `ThrottlerGuard` checked
HTTP only, and `/graphql` and `/mcp` are `Exempt` at the endpoint while a WS
message runs after the upgrade chain returned — so a guard bound anywhere
left the three in-band edges unmetered. It now implements `check_graphql`,
`check_mcp` and `check_ws_message`, each behind a crate feature the matching
umbrella edge feature forwards.

- **The bucket key gains a transport segment and a caller segment.** A
  `#[query]`, a `#[tool]` and a `#[subscribe_message]` may all be called
  `search`; without the transport they would drain one budget between them.
  And the in-band edges key on `actor_id` — keyed on the operation alone,
  every caller shares one bucket and one client spending the window `429`s
  everybody, which is the difference between a rate limiter and a
  denial-of-service amplifier. An anonymous surface degrades to a shared
  bucket and says so once.
- **The wait a denial carries now reaches every caller.** It was computed by
  the throttler, carried on the `Denial`, and read by exactly one of four
  renderers — HTTP's `Retry-After`. GraphQL, MCP and WS now carry
  `retryAfterSeconds` in the structured refusal detail where `reason` and
  `requiredScopes` already ride, and a `403` never carries it. The value
  rounds up with a floor of 1 on every edge: truncation handed every denial
  in the final second of a window `Retry-After: 0`, an instruction to
  hot-retry against the limit the guard just enforced.
- **The shared-bucket dedup lock is gone from the hot path** — it serialized
  every anonymous in-band request, a ceiling on exactly the traffic a rate
  limiter exists to survive.
- **Breaking:** `ThrottlerStore::hit` is `async` and the trait no longer
  answers `trusted_proxies()` — client identity is the transport's answer.
  The `rate limit exceeded` warn drops the composite `key` field for
  `transport` plus per-edge fields and `retry_after`.

### A token minted for another service is refused

**Breaking, and security-relevant: previously-accepted tokens are now
rejected.** A verifier with no configured audience switched jsonwebtoken's
`aud` validation off entirely, so a token the shared issuer minted *for a
sibling service* verified here — the confused deputy: every app sharing an
issuer was a credential for every other. RFC 7519 §4.1.3 obliges a verifier
to reject a token carrying an `aud` it is not named in, and that clause binds
a service naming no audience of its own too.

- **The check is now always on.** An unconfigured verifier still accepts a
  token with no `aud` at all; what configuring `audience` adds is the other
  direction — the claim becomes mandatory.
- **The opt-out is named**: `allow_any_audience`
  (`NESTRS_AUTHN__ALLOW_ANY_AUDIENCE`), off by default, reporting itself at
  `warn` once per boot, and **refused beside `audience`** — one field says
  "only tokens for me", the other "tokens for anybody", and silently letting
  either win is how a deployment ends up believing the stricter one.
- **`iss` deliberately gains no twin**: §4.1.1 states no clause obliging a
  verifier to reject a token carrying an issuer it does not name, so there is
  nothing to switch on and nothing to opt out of.
- `JwtOptions` and `JwtConfig` each gain the public `allow_any_audience`
  field.

### Three authn gaps, each silent or fail-open, closed

- **A machine principal reports its scopes.** `AuthenticatedClient` inherited
  the trait's `scopes() → None`, which means *not scope-aware* — so every
  `.requires_scope(…)` rule applied in full and a client registered for
  `posts:read` satisfied rules requiring anything. It now answers `Some`,
  always: this is the framework's own OAuth credential, so an empty registry
  entry means *delegated nothing*, never *scope does not apply here*.
- **The third way to refuse an OAuth callback now logs.** A forged, replayed
  or expired transaction cookie produced no `warn` anywhere while its two
  siblings (provider mismatch, CSRF mismatch) both did. All three now file
  one `OAuth callback rejected` event with a constant `reason` code —
  `invalid_transaction`, `provider_mismatch`, `csrf_state_mismatch` — so an
  operator greps once and reads `reason` to tell them apart.
- **The Argon2id work factor is pinned at the OWASP recommendation**
  (m = 19 MiB, t = 2, p = 1) instead of riding `Argon2::default()`, which
  happens to equal it today — a security-critical parameter sat on a value an
  upstream release could lower without a line changing here. Verification
  reads the parameters out of the stored hash, so existing credentials still
  verify and the constants can move without invalidating a database.

### A denial is one event with one vocabulary, on every edge

**Breaking for saved queries.** `reason` was two value spaces: gate exits
reported machine tokens (`no_class_grant`, `insufficient_scope`) while
masking exits reported whole English sentences, so a query grouping denials
by `reason` returned tokens from one half and prose from the other — and
HTTP re-typed the tokens as literals because the constant lived behind a
feature it does not build. Every fail-closed exit in `nest-rs-authz` now
files through one emitter, with `reason` a constant, the sentence moved to
`detail`, and the fix moved to `remedy` — an incident groups by the first
and reads the other two.

- **A refusal is a record, not seven arguments** (`Refusal`): each site knows
  a different three of them, and a field added to the record reaches every
  site at once — which is the whole reason the emitter is shared.
- **Every mask failure names its transport**, and the one silent branch
  logs: `masked_output_ambient` sent an empty object out with nothing on
  `nest_rs::authz` to say masking had degraded, so a wire type whose fields
  are all optional deserialized clean and the operator had no event to find.
- **An MCP field-grant refusal is filed as the denial it is** — same event,
  same `reason` GraphQL files when a stripped key was selected — and it now
  names *which* keys the grant withheld, the one thing the serde error could
  not say. It also tells a class denial from a field grant: a row the caller
  may not read at all produces the very same failure, and filing it as
  `field_not_granted` carried the nullable remedy — advice that, if taken,
  would answer a denied row with an all-null object rather than a refusal.
  The client-visible error becomes `forbidden`, no longer internal.
- **`AbilityGuard` gains its fourth edge.** It attested HTTP, GraphQL and WS
  and not MCP, so `#[use_guards(AuthzGuard)]` on an `#[mcp]` host did not
  compile — and had it been bindable, every `check_*` defaults to `Ok(())`,
  so it would have passed every operation in silence. `check_mcp` refuses an
  operation with no ambient ability, and the `McpGuard` marker makes the
  symmetric `#[use_guards(AuthnGuard, AuthzGuard)]` its three siblings carry
  bindable here too.

### The umbrella matrix pulls what its decorators emit

**Breaking: the `resource` feature is gone.** `#[expose]` expands to
`::nest_rs_seaorm::` paths and `#[crud]` to `::nest_rs_resource::` paths, so
two separate features would each have to imply the other — a cycle Cargo
rejects. `seaorm` now activates `nest-rs-resource` directly; a manifest
saying `features = ["resource"]` drops the word.

- **`seaorm` and `graphql` imply `authz` strongly.** `#[crud]` emits
  `::nest_rs_authz::…` unconditionally and a GraphQL posture is mandatory, so
  `cargo add nest-rs --features http,seaorm` plus `#[crud]` was `E0433:
  cannot find authz in nest_rs` — an error inside a macro expansion, blamed
  on the attribute, whose natural remedy is exactly the second manifest line
  the umbrella rule forbids.
- **`redis-throttler` is a feature that finally exists.** `nest-rs-redis`
  gated `RedisThrottler` behind its own feature and nothing in the umbrella
  forwarded to it, so `nest_rs::redis::RedisThrottlerModule` existed in *no*
  umbrella build — `demo/` enables every feature and still could not name the
  type. Kept separate from `redis` so a producer/consumer app that never
  rate-limits does not compile the throttler.
- The `nest-rs-macro-hygiene` witness swaps `resource` for `seaorm`, keeping
  the one-dependency proof standing.

### The scaffold refuses reserved names, and its prose moves where an agent reads it

- **`nestrs g feature module` no longer writes `ModuleModule` in
  `module.rs`.** A feature named after the structural vocabulary — `module`,
  `service`, `http`, `apps` — collided with the layout it was scaffolded
  into, silently. The reserved set is **scraped from the architecture rules
  file the CLI already embeds**, category by category, so the rules and the
  refusal cannot drift; the refusal names the category's role and suggests
  the domain word.
- **The teaching prose leaves the generated code.** The `// SECURITY:` blocks
  in the WS, schedule and MCP skeletons, and the migration crate's header,
  move into the scaffold's `AGENTS.md` — which gains the access-posture
  recipe (declare per operation, wire the guards, the two edges that differ)
  and a migrations-and-seed section. Public scaffolded routes gain
  `#[api(summary = …)]` so the published document starts described.
- **`cargo install --locked nest-rs-cli`**, in the README and in
  `nestrs update` — an unlocked install resolves this tool's dependencies to
  versions it was never built against.

### A documented fact is derived once, and the gate fires where it broke

The docs linter derived its own facts: seven checks opened `crates/**`,
`demo/**` and the root manifest and re-derived in JavaScript, with regexes,
what `nest-rs-conformance` derives in Rust with `syn` and `toml_edit`. Two
implementations of one definition drift, and these did — the linter counted
umbrella capabilities as *features carrying a `dep:`* (27) where the join
counts `dep:` *entries* (28), while the linter's own comment claimed the two
derivations could not disagree. The word had never been defined: a feature a
developer types and a crate a feature activates are different sets that
numbered the same until `seaorm` grew a second `dep:`.

- **`docs/canon.json` and `docs/demo-sources.json` are derived by the `canon`
  join and read by the linter, which now derives nothing.** Verified by
  instrumenting `fs`: zero reads outside `docs/`. Two artefacts rather than
  one because the facts are twenty values a reviewer reads at a glance and
  the quoted demo corpus is 380 KB that churns whenever `demo/` moves.
- **The workflow's `paths: docs/**` filter became a true declaration**, where
  it named one input class of eight. A framework change that falsified a page
  did not run the job; the failure surfaced later on an unrelated docs commit,
  naming `index.mdx`, which no commit in the window had touched. Regenerating
  the canon now lands a `docs/**` diff on the commit that moved the fact, so
  the gate fires there. The `Cargo.toml` entry the filter carried for one
  check is gone with the check that needed it.
- **The linter's thirty-two rules are a family joined against themselves.**
  Each owes a fixture that makes it fire, a `STYLE.md` § F entry, and no
  violation naming anything outside `RULES` — thirty bare literals at
  thirty-two call sites became one frozen object. Nothing had proved a rule
  still fired: neutralising any regex left the gate *greener*, which
  `.claude/rules/testing.md` calls worse than an empty cell. Nothing was
  importable either, so the seam was structural rather than a missing chore.
- **The baseline contract is enforced instead of promised.** A line naming a
  violation since fixed now fails, a corpus below its floor fails, and
  `--update-baseline` is gone: it re-snapshotted every violation including the
  code-truth ones, so the remedy the failure message printed turned a
  proven-false public claim into a permanent exemption, offered to whoever
  held the red build and could not see the cause from the message. `--land
  <rule>` replaces it — one named rule, written and then failed, which is
  `baseline.rs`'s shape for its reason.
- **Two rules the corpus never had.** `link` resolves all 969 internal links
  and their anchors — a probe page linking a route that does not exist built
  clean and shipped the dead href, the only validated targets being the ~20
  sidebar slugs. Anchor ids follow GitHub's algorithm, written out rather than
  imported because `github-slugger` last published 2023-09-15, outside the
  twelve-month freshness bar, and verified against the built site: 933 of 933
  agreeing in both directions. `fence-drift` asserts that a fence titled with
  a real `demo/` file is an excerpt of it; 102 pre-existing drifts are
  baselined and the list only shrinks.
- **A `docs` join puts the framework's families against the corpus**, so a
  family declared in Rust owes a docs cell by construction rather than by
  anyone remembering. It found twelve holes and all twelve were closed rather
  than recorded: `graphql.operation` and `events.dispatch` appeared on **zero**
  of 125 pages while the table publishing the canonical unit names listed eight
  of ten, and ten config keys a deployment can set were documented nowhere —
  among them the ceiling on a GraphQL subscription's stale-privilege window and
  the bound on a federation `_entities` request.
- **What the pages were saying.** `/packages/` published `cargo add nest-rs
  --features resource` for a feature `335b80e5` had deleted, so the command
  failed for anyone following the page the landing calls the feature map, while
  `redis-throttler` was absent from it. Both authentication pages published a
  `#[module]` inside a file titled `mod.rs`, which the architecture rules the
  CLI generates into every scaffolded project forbid, and `/configuration/
  testing/` published a `#[tokio::test]` inside one titled `tests/e2e/main.rs`,
  which the locked test-layout norm forbids. One anchor pointed at a heading
  renamed out from under it.
- **Four smaller corrections in the joins themselves.**
  `sources::exported_decorators` accepted `#[proc_macro]` against the argument
  its own doc comment makes — a bang macro is applied `name!(…)`, so it opens a
  cell nothing can fill. `umbrella.rs` defined a capability as a feature while
  counting crates, and its README corpus borrowed the capability floor.
  `declared_str` reads an associated `const` as well as a free one, so the
  `docs` join derives the env prefix from `EnvPrefix::DEFAULT` rather than
  spelling it — which `env_names` had refused, correctly.

### A driver carries its own subject, and a role file is named for its role

The naming law reaching the last few types that read correctly in isolation and
wrongly against their path. **Breaking, and all of it is a rename.**

- **`nest_rs_storage::HeadMetadata` is `ObjectMetadata`**, and `Storage::head`
  returns it. The old name described the *call* that produced the value; the
  value is metadata about an object, and `head` is only one of the ways to
  reach it. Same field, `byte_size`, now with `Debug`/`Clone`/`PartialEq` so a
  test can assert on one.
- **`nest_rs_seaorm::DbHealthIndicator` is `SeaOrmHealthIndicator`.** `Db` named
  no backend — it is the bare capability word worn by one driver, which is
  exactly what `RedisThrottlerModule` exists not to be. Its module,
  `SeaOrmHealthModule`, already said the right thing; the indicator beside it
  did not.
- **`nest_rs_health::HealthController` is `pub(crate)`.** A mounted controller
  is reached over HTTP, not by name: nothing in either workspace imported it,
  and a `pub` on it promised an API nobody had priced.
- **`exception.rs` is `exception_filter.rs`**, the role the naming tables give
  the file. Internal — `ExceptionFilter` is exported from the crate root as
  before.

### A health probe's names are settled at boot, and a missing host is an outcome

Two failures a probe could not previously report, both closed where the fact is
known.

- **Two reachable indicators claiming one name on one probe fail the boot,
  naming both hosts.** The name is the probe body's JSON key and
  `ProbeReport::from_indicators` folds by it, so a collision did not merely
  shadow an entry: a `down` verdict could be overwritten by an `up` one and a
  failing check would leave a readiness probe with nothing said anywhere. Which
  of the two won was `inventory` link order, which nobody declared. The check is
  **per probe, not per registry** — `#[readiness] fn db` beside `#[startup] fn
  db` addresses nothing twice, which is the pair `nest-rs-seaorm` ships.
- **An indicator whose host the container does not hold reports `down`, it does
  not panic.** The thunk runs inside a probe request, where a panic takes the
  response down; an `Err` is the outcome the crate already renders, with the
  real sentence on `nest_rs::health`. The sentence is the kernel's
  `INERT_HOST_HINT`, so the five causes are named and no unverifiable edit is
  prescribed.

### A task boundary carries the unit of work, through one seam

`nest_rs_core::TaskContext` — capture the ambient span and request context at
the hand-off, re-install both around the spawned work. **Both halves cross, and
neither substitutes for the other**: the span is what puts `trace_id` on the
events the spawned work emits, the ambient context is what makes
`current_trace_id()` answer inside it. Carrying only the span leaves the events
*looking* correlated while every accessor below answers `None` — the more
expensive failure, because it reads as covered.

Two sites, both of them previously hand-rolling it or missing it:

- **A cancelled multipart upload's abort** is filed under the request that
  opened it. Cancellation is precisely the case where the reader holds the
  timed-out request's `trace_id` and needs the outcome line to carry it. The
  context is captured at construction rather than at `Drop`, because a dropped
  future is not guaranteed to be dropped on the task that owned it.
- **The GraphQL dataloader batch** keeps the behaviour it had and stops spelling
  it locally.

### The test harness says which protocol it speaks, and stops hiding a dead socket

- **Edge modules keep their namespace.** `nest_rs_testing::graphql::` and
  `::ws::` are the paths; the root re-exports of `GraphqlSocket`,
  `GraphqlSocketBuilder`, `WsApp`, `WsSocket`, `WsSocketBuilder`, `WsFrame` and
  `CloseCode` are **gone**. Re-exporting both ways gave every type two paths,
  and this crate was already spelling one of them two ways 200 lines apart.
- **`CloseCode` is `nest_rs_ws::CloseCode`** — RFC 6455 §7.4.1's codes,
  published by the crate that *chooses* them (a gateway closes with `Away`,
  `Error` and `Policy`), so a caller writing a gateway and a caller testing one
  name one path. `nest-rs-ws`'s own suite used to reach into `nest-rs-testing`
  for it.
- **Silence and a dead connection are different answers.** `WsSocket::read_within`
  returns `WsRead::{Frame, Silent, Aborted}` and the GraphQL driver's
  `GraphqlEvent`/`GraphqlRefusal` carry the same split. Folded into one `None`,
  a socket that died mid-test satisfied `expect_silence`: the assertion "nothing
  was sent" passed because nothing *could* be sent. `close()` now loops for the
  peer's answer too — §5.5.1 obliges it to reply "as soon as practical", which
  does not oblige it to reply *first*.
- **The driver speaks `graphql-transport-ws`, and says so.** `graphql-ws` is the
  *legacy* subprotocol identifier; the mount negotiates both and this driver
  pins the modern one, whose refusal close codes (4400, 4401, 4403, 4408, 4409,
  4429) `try_connect` now reports instead of mapping every one of them to `None`.
- **`mcp::PROTOCOL_VERSION` is pinned to the SDK's own `LATEST`, by a test.**
  It sat on `2024-11-05` for four revisions and nothing said so: rmcp accepts the
  oldest revision forever, so every MCP suite in both workspaces passed while
  negotiating a handshake no current client performs. A shared constant was never
  the guard against that. `nest_rs_mcp::ProtocolVersion` is re-exported so the
  comparison has something to make.

### A global `ExceptionFilter` covers the route table, not the edge

"Global" means a different thing for the two layer families, and only one of
them said so. A global `Filter` attaches a wrap at the transport edge, so it maps
errors raised where **no route matched**. A global `ExceptionFilter` is read
**per route** by the `#[routes]` composer — so a 404, a self-mounted surface such
as `/graphql` or `/mcp`, or a WS upgrade never reaches one, even when the error
raised there is exactly the type it claims to catch. Use a `Filter` for those.

Documented on the trait method, on `/fundamentals/exception-filters/`, and in the
dedup section, which also stops describing the two as nesting: both compose
through `compose_chain` into a single chain endpoint per route
(`ExceptionFiltersEndpoint` for the typed catches, `FilterChain` for the untyped
ones), so the dedup covers every pair of scopes including controller + method
with no global in play.

**`nest-rs-filters` no longer claims to span transports.** Its crates.io
description and README said "on HTTP, GraphQL, and WS"; a `Filter` is bound at
the HTTP edge and `reject_http_only_layers` is the compile error the other three
edges already raise. The claim was read by everyone as true of them.

### Also

- **`nest-rs-opentelemetry` loses five dependencies and gains a voice.**
  `nest-rs-http`, `nest-rs-interceptors` and `poem` were optional entries
  without `dep:`, so Cargo synthesised a public implicit feature per entry
  and `--features poem` built poem — the opposite of what the manifest's own
  comment described; `tokio` and `async-trait` were simply unused. And an
  unparseable `<PREFIX>_LOG_FORMAT` / `_LOG_SOURCE_LOCATION` now reports
  itself instead of silently keeping the default — `LOG_FORMAT=console` gave
  a production deploy text output where it asked for JSON, with nothing
  anywhere saying why.
- **Two boot internals leave the kernel's surface.**
  `nest_rs_core::access::reachable_provider_ids` and
  `container::DuplicateProvider` go `pub(crate)`, having no caller outside
  the crate in either workspace; the unit catalogue in `operation_log` now
  names the edge crates that own each constant.
- **The OpenAPI document lines file under `nest_rs::openapi`**, not
  `nest_rs::routes` — a target's one job is to say where an event came from,
  and the write happens in `nest-rs-openapi`.

- **`nest_rs_core::unresolved_host` is the run-time half of `INERT_HOST_HINT`.**
  Five decorators resolve their host with `Container::get::<Self>()`, so the
  sentence they report belongs beside the boot-time one rather than inside
  whichever capability wrote it first.
- **`OpenTelemetryMeter` moves to `meter.rs` and `PipeError` to `error.rs`** —
  a file exists for its own content, and errors live in `error.rs`.
  `nest_rs_opentelemetry::parse_bool` is **no longer re-exported**; it is
  `nest_rs_core::parse_bool` and always was.
- **`nestrs doctor` reads the namespaces that exist.** It answered for
  `NESTRS_DATABASE__URL` and `NESTRS_QUEUE__URL`, which no `#[config]` declares
  any more — it now reports `NESTRS_SEAORM__URL` and `NESTRS_REDIS__URL`. Its
  Rust floor is parsed from `CARGO_PKG_RUST_VERSION` rather than retyped, so the
  toolchain sweep no longer compares the anchor against a stale copy of itself.
- **A dry run says so in every sentence it prints.** `nestrs new` claimed
  "Created …" and pinned an HTTP port directly above "Dry run — no files
  written."; the tense is read off the report, which is the thing that knows.
- **A scaffolded skeleton no longer restates the edge's own line.** The queue
  processor and the scheduled tick each filed an `info` beside the one
  `nest_rs::operation` already files per unit of work — the same work said twice,
  on a target the documented toggle does not silence. `tracing` leaves those two
  skeletons' generated manifests with it.
- **`nestrs g mcp` names the tool after the resource, not its singular.**
  `Names::tool()` read `singular` where its six siblings read `pascal`.
- **A generated `mod.rs` exports the module and not the handler.** A controller
  reachable as `features::posts::*` is a decision the generator was making for
  every project at once.

- **`LogCapture` can assert on what it could not see.** `CapturedEvent` carries
  the event's `name:` — its metadata identity, which an OTLP log bridge exports
  as `event.name` — so a line whose `name:` had drifted from its message no
  longer passes every assertion in the repo; `CapturedSpan` carries `level`, so
  the *Level per layer* contract is assertable for spans and not just events, and
  it is exported from the crate root. The buffers survive a poisoned lock rather
  than panicking a second time on top of the first failure.
- **`EphemeralDatabase` stops dropping the host of a path-less URL.** RFC 3986
  §3.2 terminates the authority at the *first* slash after the scheme, not the
  last, so `postgres://host:5432` yielded `postgres://<db>` — surfacing much
  later as a connection error naming a URL the developer never wrote. The
  ephemeral name's prefix is one constant now, read by the reaper instead of a
  hand-counted `_` offset that a rename would have shifted silently.
- **`nest-rs-health` drops `nest-rs-interceptors`**, which it never named, and
  `OpenTelemetry::init_for_tests` is `#[doc(hidden)]` — a harness entry point,
  not a documented one. `nestrs info` stops printing a `CLI` row that
  `nestrs version` already owns.

### The documented front door is compiled

- **`use nest_rs::prelude::*` now has a reader, and it had drifted where
  nothing looked.** `#[resolver]` and `#[mcp]` were re-exported without their
  impl halves `#[operations]` and `#[tools]`, so the documented import gave the
  struct half of two decorator pairs and an unresolved attribute on the other —
  whose natural remedy is exactly the second manifest line the umbrella rule
  forbids. The witness lives in `nest-rs-macro-hygiene`, which now *applies*
  every prelude decorator rather than merely importing it, because a glob
  import cannot fail on a name it does not find.
- **`#[input]` leaves the prelude's `http` block.** It is `nest-rs-core`'s and
  every edge re-exports it, which is the statement that it belongs to none of
  them; behind `http` it left a `--features queue` worker's job payload with no
  `#[input]` and a remedy that pulls the whole HTTP stack into a headless
  binary.
- **`nest-rs-guards` documents all its features on docs.rs.** The three
  per-edge marker traits are feature-gated and docs.rs builds default features
  only, so the published page carried neither the traits nor the links naming
  them.
- **`full` states its one exception.** It pulls every capability but `testing`,
  which is a `--dev` install; folding it in would put a test harness in every
  release binary of anyone who took the undecided default. Asymmetry argued in
  the manifest, not silent.
- **Two capabilities gained their composition witness.** `nest-rs-server-timing`
  and `nest-rs-health` each boot the documented wiring in their own crate and
  read the result back — the header off a route, the probe off the controller —
  instead of leaning on `demo/` to prove it.

### Every declaration grammar refuses the same four ways

A decorator that takes `key = value` arguments owes four refusals, each a
compile error naming the decorator, the offender and the alternatives, spanned
at the offending token:

| The offence | The sentence |
|---|---|
| an unknown key | ``unknown #[crud] argument `servcie`; expected `service`, … or `paginate` `` |
| a key written bare | ``#[queue] `name` needs a value — write `name = ...` `` |
| a key written twice | ``#[controller] takes at most one `path` `` |
| a value outside a closed vocabulary | ``unknown #[injectable] scope `reqest`; expected `singleton`, `request` or `transient` `` |

The four sentences are worded once, in `nest_rs_codegen`, and every decorator
now reads them from there. Seven had adopted all of them and four had adopted
none: `#[expose]`, `#[api]`, GraphQL's `#[authorize]` and `#[inject]` each took
a repeated key and kept whichever came last in source order — on `#[api]` that
is published prose choosing itself, on `#[authorize]` it is which service loads
the authorized subject. A bare key on several grammars died on `syn`'s
`` expected `=` `` with no decorator named, and a wrong value was answered five
ways, three of which named neither the decorator nor the key.

- **What was silently wrong is now a compile error.** Code that spelled a key
  twice compiled before and does not now; what it meant was already ambiguous.
- **Fifty-six new trybuild snapshots** pin the sentences, one per refusal per
  decorator, across eight crates.
- **A `grammars` join in `nest-rs-conformance` derives the population** — a
  decorator is in it the moment a `*-macros` crate names a key set — and fails
  the site that hand-rolls or drops a refusal, so the next decorator cannot
  ship half of this.

### A concern lives in the crate that owns it

**Breaking for Rust importers; nothing changes on the wire, in a decorator, or
in a filter directive.** A cluster of kernel exports moved to the edge that owns
them, and the two strings every crate had re-typed moved into the kernel:

| Was | Is |
|---|---|
| `nest_rs_core::operation_log::unit::HTTP_REQUEST`, … | `nest_rs_http::unit::REQUEST`, `nest_rs_ws::unit::{MESSAGE, CONNECT, DISCONNECT}`, `nest_rs_queue::unit::JOB`, `nest_rs_schedule::unit::TICK`, `nest_rs_mcp::unit::OPERATION`, `nest_rs_graphql::unit::SUBSCRIPTION` |
| `nest_rs_core::{HandlerMetadata, MappedError, Public}` | `nest_rs_http::{HandlerMetadata, MappedError, Public}` |
| `nest_rs_core::current_body_limit`, and the body-limit argument on `with_request_scope` / `RequestContinuation` | `nest_rs_http::current_body_limit`; the HTTP edge carries its own limit around body polls |
| `AppBuilder::strict_resolver_membership()` | `GraphqlConfig::strict_resolver_membership` — a real config field, env-settable and pinnable through `GraphqlModule::for_root` |
| `UnreachableResolversError`, in the kernel | `OrphanResolver`, in `nest-rs-graphql`; the boot `warn` files on `nest_rs::graphql` |

- **The unit names go where the span targets went last release.** A unit name
  says which edge did the work, and the kernel does not know the edges exist.
  The `<edge>.<unit>` grammar stays `nest_rs_core::operation_log`'s; the names
  do not — the same split as targets, held by the same conformance join.
- **`HandlerMetadata` reads a `poem::Request` and marks a `poem::Response`** —
  two types the kernel cannot name — and the other edges resolve posture at
  compile time, so the "transport-agnostic" contract had one implementor and no
  second candidate for as long as it existed.
- **A capability's knob was sitting on the kernel's seam.** The body limit is
  HTTP's and the orphan-resolver policy is GraphQL's; each now lives with its
  owner, and the GraphQL one becomes a config a deployment can set rather than
  a builder method only code could call.
- **What moved *in* is what everyone had re-typed.** `nest_rs_core::parse_bool`
  is the one truthy/falsy vocabulary for every framework boolean variable — it
  sat behind the `logging` Cargo feature, so the crate reading every
  `<PREFIX>_<NS>__<KEY>` boolean had respelled it. And
  `nest_rs_core::UUID_V7_REQUIRED` is the one sentence the three id gates word
  (`#[crud]`'s, HTTP `Bind`'s, GraphQL `bind`'s) — two of the three had already
  drifted apart.
- **The kernel's access exports now match the family.** `ScopeViolationError`
  joins its six siblings — it was the one boot error a caller could not name as
  `nest_rs_core::…` — and the three graph validators went `pub(crate)`, having
  no caller outside the crate in either workspace.
- **A contained panic is logged under one field name.** The downcast ladder was
  already shared in `nest_rs_core::panic`; the field it lands in is now declared
  beside it and held by a `nest-rs-conformance` join across the three seams
  that catch — the scheduler, the event bus, the queue consumer.

### Two config types reading one variable is a boot error

`ConfigService` now records which type each resolved variable was read for, and
the second type to read the same name fails its resolve with
`ConfigError::ContestedVariable` — naming the variable, the owner and the
claimant. Several types sharing a *domain* stays deliberate (`nest-rs-authn`
ships three `authn` configs); two types sharing a *variable* meant a deployment
setting it configured whichever happened to read it, both silently — their key
sets disjoint by accident of the current fields, a fact about today rather than
a property anything held.

The recording sits on the reader rather than on the type: `HttpConfig`
delegates ten of its keys to sub-structs that are not `Config`s, and nothing
that inspects types can see those reads. The free `env_var` stays outside the
registry deliberately — it is the documented spelling for a cross-namespace
borrow, and what an unowned read should claim is an owner question, recorded in
the module doc rather than silently decided.

### The operation-log target is `nest_rs::operation`

**Breaking for operators; nothing to change in Rust.** The one line every edge
files per unit of work moved from `nest_rs::access` to `nest_rs::operation`:

```text
 INFO nest_rs::operation: tick ran provider="AudioTasks" method="warmup_on_boot" outcome="ok" duration_ms=0.302 trace_id=01a015a91d527cb1b9d15a8ba7fe8846 span_id=bd824ae7eb1f9e7a
```

| | |
|---|---|
| Old | `nest_rs::access` |
| New | `nest_rs::operation` |
| Change | `NESTRS_LOG` / `RUST_LOG` directives, log-router rules, dashboard and alert queries |

The Rust surface is unaffected — the target has always been read from
`nest_rs_core::operation_log::TARGET`, so any code importing it follows with no
edit. What breaks is every place the string was typed by hand: a filter
directive, a runbook, a saved search, a dashboard panel. A stale
`nest_rs::access` directive now matches nothing and fails silently, so grep for
it rather than waiting for a quiet console to be noticed.

- **The old name was HTTP's word for HTTP's line.** It survived the
  generalisation to six edges in 5.1, where it stopped being true of five of
  them: a scheduled tick has no caller, so nothing accesses anything. Every
  other framework target names a subsystem and is rooted at the crate that emits
  it; this one names a *category of line* that crosses all of them — exactly one
  per unit of work, carrying an `outcome` and a `duration_ms` — and a
  subsystem-shaped word hid that. `operation` is the word the operation span,
  the MCP edge's `operation served` and the owning `operation_log` module
  already used.
- **`nest_rs::access` was also silencing a target nobody meant to silence, and
  the rename fixes it.** `EnvFilter` compares a directive's target to an event's
  with `starts_with` on the raw string rather than by `::` segment, so
  `nest_rs::access` matched `nest_rs::access_graph` too — the boot `warn` naming
  resolvers unreachable from the GraphQL schema. The documented toggle,
  `NESTRS_LOG=info,nest_rs::access=off`, therefore took that startup diagnostic
  away as well, with nothing on the console to say it had. Anyone who ran that
  directive was missing those warnings; `nest_rs::operation=off` silences the
  operation log and nothing else.
- **The property is now executed, not just intended.** A join in
  `nest-rs-conformance` derives every `tracing` target both workspaces emit and
  fails when one is a prefix of another, and `nest-rs-core` asserts the
  `EnvFilter` behaviour the join rests on.
- **`NESTRS_HTTP__ACCESS_LOG` is unchanged and stays correct.** It toggles one
  edge's per-request line — which is what an access log is — rather than the
  family, and it is an app's pinned config rather than a deployment's filter.
  `nest-rs-http`'s `access_log.rs` keeps its name for the same reason.

### One canonical name per unit of work

**Breaking for operators.** The message on an operation line is now the unit's
canonical name — the same string its span carries — instead of a sentence
written per edge:

| Edge | Was | Is |
|---|---|---|
| HTTP request | `request served` | `http.request` |
| WS message | `message served` | `ws.message` |
| WS socket open/close | `socket lifecycle` + `lifecycle="connect"` | `ws.connect` / `ws.disconnect` |
| Scheduled tick | `tick ran` | `schedule.tick` |
| Queue job | `job ran` | `queue.job` |
| MCP operation | `operation served` | `mcp.operation` |
| GraphQL subscription | `subscription served` | `graphql.subscription` |

```text
 INFO nest_rs::operation: schedule.tick provider="AudioTasks" method="warmup_on_boot" outcome="ok" duration_ms=0.302 trace_id=01a015a91d527cb1b9d15a8ba7fe8846 span_id=bd824ae7eb1f9e7a
 INFO nest_rs::operation: queue.job queue="audio" processor="AudioProcessor::transcode" attempt=1 outcome="ok" duration_ms=33.733 trace_id=01a015a9252076e399f736a97ae90784 span_id=7bbcfc44c1f0c676
```

Reading which kind of work a line reports no longer means inferring it from
which fields happen to be present.

- **There were two vocabularies for the same eight things.** Four edges already
  named their span `http.request`, `ws.message`, `mcp.operation`,
  `graphql.subscription`; the other two had drifted to prose (`"scheduled job"`,
  `"process job"`), and every line invented a third wording. `nest-rs-ws` had
  already hit the wall and left the workaround in a comment — "`lifecycle`
  rather than the span's name because `tracing` offers no way to read one back".
  Now there is one name, `<edge>.<unit>`, declared in
  `nest_rs_core::operation_log::unit`.
- **It is also the log record's event name.** Each line sets `name:`, which
  `opentelemetry-appender-tracing` maps to the exported `event.name`. That field
  used to carry `event crates/nest-rs-mcp/src/propagate.rs:183` — a source path,
  one value per call site. Nothing here depends on the exporter: the constants
  are `&'static str` in the kernel and an app that exports nothing pays nothing.
- **The namespace comes from the closed edge vocabulary**, so a new transport
  cannot invent a seventh word without opening the edge deliberately.
- **`lifecycle` is gone from the WS lifecycle line** — `ws.connect` and
  `ws.disconnect` say it.
- **A join in `nest-rs-conformance` derives every naming site and refuses a
  literal at any of them**, and it reads **doctests** as well as items. It did
  not, and the one example teaching the grammar — `operation_span!`'s own — was
  therefore the one site spelling all three slots as literals, with nothing
  failing. `syn` lowers `///` to `#[doc = "…"]`, so an example is a string
  literal to a macro visitor; it is now parsed back into code and walked, since
  an example is what a developer copies.

### Every span target and span kind is a constant

230 call sites that spelled `target: "nest_rs::orm"` now name a constant, so a
typo is a compile error instead of a target carrying one event that no filter
selects. `operation_log::kind` does the same for the five `otel.kind` values.

- **The crate that *owns* the concern declares it** — `nest_rs_events::TARGET`,
  `nest_rs_seaorm::TARGET`, `nest_rs_ws::TARGET` — and everything emitting on it
  reads that constant, a sibling crate and a `*-macros` expansion included.
  Owning is not emitting: `nest_rs::routes` is filed from six crates and
  `nest_rs::queue` from a macro expansion rather than from `nest-rs-queue`
  itself. A target's one job is to name *where* an event came from, so a central
  table in the kernel would have meant `nest-rs-core` carrying a name for a
  concern it does not know exists.
- **A crate owning several concerns gets a `target` module**, and there are
  exactly two: `nest_rs_core::target` (six) and `nest_rs_http::target` (the
  transport and its route table).
- **`report_inert_host!` now takes an expression** rather than a literal target.
- Nothing an application writes is affected: an app's own `tracing::info!` on
  its own target is untouched, and only the framework's own family is policed.

## [5.1.0] - 2026-08-18

### Every log line carries the trace context of the unit of work that emitted it

**Behaviour change, both log formats.** The correlation on a line is now read
from the ambient W3C trace context — `trace_id`, `span_id`, `actor_id` — and a
line carries no span attributes and no span names at all:

```text
DEBUG features::posts: creating post title="hello" trace_id=01a01569ae687353bc034a9ee8bd8774 span_id=a3f7cc8bac648278 actor_id=01a0112ce24e75509be691162cbbab1f
```

```json
{"timestamp":"…","level":"DEBUG","fields":{"message":"creating post","title":"hello"},"target":"features::posts","trace_id":"01a015…74","span_id":"39d4f2…54","actor_id":"01a011…1f"}
```

It used to render the span scope, and that is a different question with a
different answer. A service runs under the HTTP request's span, so its every line
carried the request's `http.request.method`, `url.path`, `client.address` and
`user_agent.original` — the line stopped being about what the service did — and a
nested unit of work (an MCP operation inside a request, a job inside its enqueue)
printed one `trace_id` per level.

- **The ids come from the kernel, not from a formatter's view of the span
  stack.** `current_trace_id()` / `current_span_id()` / `current_actor_id()` are
  what the line prints, which is the same read application code does. That makes
  the line self-contained however deep the nesting runs, and it reaches where a
  span stack does not: a streaming body is polled after its handler returned, and
  the framework re-installs the request around it.
- **Nothing here depends on an exporter.** The trace context is a kernel
  primitive, so a bare app logging to a terminal has it; `nest-rs-opentelemetry`
  exports the same ids and installs the same two formatters, so an app's lines do
  not change shape because it adopted or dropped the observability stack.
- **JSON puts `trace_id` / `span_id` / `actor_id` at the top level** of the
  record rather than inside a `span` object. The rest of the envelope keeps
  tracing-subscriber's keys — `timestamp`, `level`, `fields`, `target`,
  `filename`, `line_number` — so a pipeline reading this output keeps reading it.
- **Span structure is untouched**, and none of this was a defect in the trace:
  nesting is what `tracing-opentelemetry` builds the exported tree from.

Two knobs moved and neither is silent: `.event_format(…)` replaces the `Format`
that `with_file` / `with_line_number` configure, so `file:line` is the
formatter's own `source_location` (still `NESTRS_LOG_SOURCE_LOCATION`), and ANSI
follows the writer as before. There is no variable that puts span state back on a
line.

### Every edge files one line per unit of work

HTTP has always had an access log; no other edge did. Now a WS message, a socket
opening and closing, a scheduled tick, a queue job, an MCP operation and a
GraphQL subscription each file one line on the same `nest_rs::access` target,
carrying what the work **was** — plus `duration_ms`, and `outcome`
(`ok` / `error` / `panic`) everywhere a request's `status` does not already say it
better:

```text
 INFO nest_rs::access: tick ran provider="AudioTasks" method="enqueue_transcode" outcome="ok" duration_ms=101.901 trace_id=01a015a9252076e399f736a97ae90784 span_id=052e261668035200
 INFO nest_rs::access: job ran queue="audio" processor="AudioProcessor::transcode" attempt=1 outcome="ok" duration_ms=33.733 trace_id=01a015a9252076e399f736a97ae90784 span_id=7bbcfc44c1f0c676
```

Those two lines are from two different binaries ninety seconds apart, and that is
the point: one query on `nest_rs::access` answers "what did this deployment do
and what failed" across every transport, and the ids join it to everything each
unit of work logged.

- **It is the counterpart of a line carrying no span state.** A span's
  attributes are not part of a log record, so the identity of a unit of work has
  to arrive as *event* attributes on a line the edge emits — otherwise the ids
  say two lines belong together and nothing says what they were.
- **One target is the family's toggle.** `NESTRS_LOG=info,nest_rs::access=off`
  silences all of them, so no edge grew a `#[config]` field — and the `for_root`
  seam that one would oblige — for a boolean the filter already answers.
  `NESTRS_HTTP__ACCESS_LOG` stays: it predates the family and is an app's pinned
  config rather than a deployment's filter.
- **A successful queue job is reported once**, not twice: the `job ok` event is
  gone and `job ran` replaces it. The failure events keep saying *why* and no
  longer restate `elapsed_ms`.
- `nest_rs_core::operation_log` holds the target, the outcome words and the
  duration formula, and *A new edge owes the same list* now includes this line.
- **Two of the six lines were wrong until real output was captured**, which is
  the note worth keeping. The MCP line carried no ids at all: it is emitted
  outside the scope the dispatch installs, so it now emits through the
  correlation explicitly. And the GraphQL subscription line never fired — a
  socket that is aborted rather than closed never completes its future — so it is
  filed from a `Drop` guard, the way HTTP's access log always has been. Neither
  was visible to a `LogCapture` assertion, because the ids are not fields of the
  event; only rendering the line found them.

### The HTTP access log stops spelling the ids itself

It wrote `trace_id`, `span_id` and `actor_id` as event fields while every line
already carried them, so text printed each twice and JSON put them in two
different positions. The body now re-enters the request's **context** (never its
span — the line is still nobody's child event) around filing, and the fields are
gone. Same for four WebSocket connection-lifecycle events, and for the authn
guard's `authenticated` line.

### The scaffold e2e suite no longer races itself

`nest-rs-cli`'s e2e compiles every scaffolded workspace into one shared
`CARGO_TARGET_DIR`, and nextest runs each test in its own process, so concurrent
builds raced on the fingerprints of shared dependencies. It surfaced as a
**linker** error on whichever generic crate lost — `quote`, `proc-macro2`,
`libc` — which names nothing about the cause and moved between runs, reading
exactly like a broken toolchain. `.config/nextest.toml` now puts that binary in a
`max-threads = 1` test group: the shared directory keeps making the suite fast,
and nothing else in the workspace loses a core.

### `trace_flags`, and one grammar for `LOG_FORMAT`

JSON records now carry `trace_flags`, the third field OpenTelemetry's log data
model names beside the two ids. Text does not, deliberately: a record joined
against an export needs to know whether the export exists, and a human at a
console never acts on a sampling bit.

`LogFormat` and the boolean env grammar are declared once, in
`nest_rs_core::logging`; `nest_rs_opentelemetry::LogFormat` is now a re-export of
it. They were two enums with two parsers and two build-profile defaults, which is
how an app's log shape could have come to change on adopting the exporter.

## [5.0.0] - 2026-08-18

### Correlation is W3C Trace Context, it lives in the kernel, and the homemade id is gone

A request, a WS message, an MCP operation, a queue job and a scheduled tick each
run under a **W3C trace** the framework starts or continues. `trace_id` names the
distributed operation, `span_id` names the unit of work inside it, and the pair
is readable from anywhere the framework carries work:

```rust
let trace = nest_rs::core::current_trace_id();
let span = nest_rs::core::current_span_id();
// `None` for an anonymous caller — absence is the answer, not a gap.
let actor = nest_rs::core::current_actor_id();
```

**Breaking.** `RequestId`, `current_request_id()` and `with_correlation()` are
removed, the `X-Request-Id` and `X-Trace-Id` response headers with them, and
`nest-rs-opentelemetry` no longer has an `http` feature. No shims.

- **The standard replaced a duplicate, and the argument is recorded.** The old
  `request_id` answered exactly the question `trace-id` answers, so the framework
  carried two identifiers for one thing. Three defences were offered and the
  specification retires all three: a forgeable inbound id is answered by the
  spec's own *restart trace* mutation, not by a second identifier; the standard
  needs 16 random bytes and a `format!`, not an SDK; and a trace id sorts by time
  if you choose bytes that do — ours are a UUID v7's, so the value is a
  conformant trace id **and** an ordered UUID whose right-most 7 bytes still
  satisfy the `random-trace-id` flag. The deciding fact was the ecosystem's:
  OpenTelemetry's conventions have no request-id concept, only
  `http.request.header.x-request-id` — a *captured* header. It is now recorded as
  exactly that, behind a trusted peer, and never decides what a request is called.
- **`traceparent` is gated on `NESTRS_HTTP__TRUSTED_PROXIES`**, the same list
  `X-Forwarded-For` is weighed against. It used to be honoured from **any**
  caller, so a public client could file its requests into a trace of its choosing
  and set `sampled` on traffic it generated — the denial-of-service surface the
  specification names. From an untrusted peer the trace is now restarted, which
  is what the spec defines a front gate to do, and the caller's claim is kept on
  the span so nothing is lost.
- **The request side is implemented to the letter**: version `ff` refused,
  version `00` exactly 55 characters, a higher version still read as far as this
  one understands, all-zero trace and parent ids refused, lower-case hex only,
  and `tracestate` forwarded **verbatim** wherever the trace is continued — a
  MUST whose breakage is invisible to us and fatal to whichever vendor's routing
  rides in it. The response carries `traceresponse`, the working group's form.
- **A queue job is a child of the enqueue.** `traceparent` and `tracestate`
  travel in the wire envelope, so a worker running minutes later in another
  binary appears **under** the request that enqueued it rather than beside it.
  The parent/child relation is what a flat id could not express.
- **`actor_id` is unchanged in spirit and now reachable.** You still declare it
  once on your principal; `current_actor_id()` reads it back anywhere the
  framework carries work, and answers `None` — never a sentinel — for an
  anonymous caller. It remains an **audit** identity, never an authorization
  input.

### A unit of work ends when its answer ends, not when its handler returns

A streaming response — `#[sse]`, a download, anything built over a `Stream` —
runs after its handler returned, on the connection task, and used to emit under
no span and no ambient context at all. Every event it logged was attributable to
nothing, and `current_trace_id()` answered `None` inside the developer's own
stream.

- **The HTTP edge carries the request into that gap**, re-installing it around
  every body poll, and does so **whatever `NESTRS_HTTP__ACCESS_LOG` is set to**: a
  config flag is a weaker condition than a crate, and a primitive true for short
  responses and false for long ones reads as present exactly where a long
  operation needs it.
- **The framing is unchanged, and that is load-bearing.** The wrapper forwards
  `size_hint`, so a fixed-length response still declares `Content-Length`. A body
  counter written over a byte stream turns every response in the framework
  `Transfer-Encoding: chunked`, silently, and is invisible to any assertion made
  before the response is written — so it is now read off the wire by a test.
- **Three more edges were in the same position** and are closed with it: a
  graphql-ws subscription socket, a WebSocket connection's `on_connect` /
  `on_disconnect` hooks, and the GraphQL dataloader's spawned batches, which
  carried the span but not the ambient context — so their events *looked*
  correlated while every accessor inside answered `None`.
- **A socket inherits identity, never resources.** A connection that outlives its
  upgrade takes the trace and the actor and opens its own scope per message; it
  does not pin the upgrade's.

### The observability stack enriches spans; it no longer owns any

`nest-rs-opentelemetry` mints no identifier: its `IdGenerator` **adopts** the
kernel's, so the exported trace and the log lines name one request by one value
instead of two that nothing joins.

**Breaking.** The `http` feature and its per-request interceptor are removed. An
app that listed `features = ["http"]` on this crate drops the line; the umbrella's
`http` feature no longer forwards it.

- **What the interceptor did now hangs off the span constructor.** The remote
  parent link and the sampler's verdict are seeded onto `operation_span!` at
  `init`, so they reach **every** edge — a queue job in a headless worker as much
  as an HTTP request — instead of the one transport band an interceptor could
  hang from. Attached there, a job continuing a trace out of an envelope had no
  way to say so, and its exported span carried no causal edge to the enqueue.
- **A parent is announced remote only when it is.** A queue job's parent ran in
  another process; a WS message's and an MCP operation's ran in this one, and
  claiming otherwise renders a network hop that never happened.
- **`otel.kind` is required vocabulary**, not a field a call site may add: it is
  an argument of `operation_span!`, so an edge cannot ship unclassified and
  export as `internal` where a messaging view would never find it.
- **The access log belongs to `nest-rs-http`.** Method, path, status, duration,
  client, `trace_id`, `span_id` and `actor_id` need no collector, no exporter and
  no propagator, so none is required for a deployment to answer what it did.

### The span reports the route template, and names itself for it

`http.route` used to carry the path **as addressed**. A backend groups latency and
error rates on that field, so `/users/01a0…` there is one group per identifier
and no signal at all.

- `http.route` is now the matched template (`/users/:id`); the addressed path is
  `url.path`, which is what the conventions mean by it.
- The exported span is named `{method} {route}` through `otel.name`, because
  `tracing` fixes a span name to a literal and one literal for every route
  renders a whole deployment as a single line in a trace list.
- A request that matched nothing is named by its **method alone** — the
  conventions' own fallback, and deliberately not the URL: naming an unmatched
  span after its path is how one scanner fills a backend with junk.

### An MCP operation is its own unit of work

It used to re-enter the HTTP request's span, so under rmcp's default session mode
every operation in a session filed under the request that opened it and "what did
this tool call do" had no answer. Each dispatch now opens an `mcp.operation` span:
same trace, its own span id, the request's span as its parent — the shape
`ws.message` already had.

### Also

- **A WebSocket message refused for size is recorded.** The client was told and
  the operator was not, so a cap set too low looked from the outside exactly like
  clients that stopped sending.
- **`nest_rs_testing::LogCapture` captures spans**, not only events —
  `expect_span` and `spans()`. Most of what the framework promises an operator
  lives on the operation span, and a harness that could read only events could
  assert none of it. A field declared and never recorded is **absent** there,
  which is what makes a `record` call nobody wired assertable at all.
- **[Correlation](https://nestrs.dev/fundamentals/correlation/) is a page of its
  own**, under Fundamentals rather than inside OpenTelemetry — the primitive is
  unconditional, and documenting it in an optional section left a developer who
  never installs an exporter with no path to it.
- **Minimum supported Rust is now 1.97** (was 1.96), moved in one sweep:
  `rust-toolchain.toml`, all three workspace `rust-version`s (root, `demo/`, the
  bench SUT), the three images, the publish workflow, and everything `nestrs new`
  scaffolds. 1.97.1 backports an LLVM fix for a miscompilation present since at
  least 1.87 — every release profile here is `lto = "fat"` on a single codegen
  unit, which is the regime that bug reaches. The pin stays two-component:
  `1.97` resolves to the newest patch **at install time**, so a fresh clone
  takes the fix without anyone spelling it and a machine that installed 1.97.0
  earlier takes it on the next `rustup update`. `manifests-ci.md` asserted that
  these pins agree and nothing read it, so `toolchain_pins_agree` now does —
  including the scaffold's toolchain file and Dockerfile, which the
  manifest-shaped scan could never see.
- **A scaffolded project's `rust-version` is now read by the crates under it.**
  Generated members wrote `version.workspace = true` and `edition.workspace =
  true` but never `rust-version.workspace = true`, and cargo does not inherit
  that key unopted: the floor the root declared was a value no crate in the
  project resolved, so an old compiler produced a page of type errors instead of
  one sentence naming the requirement. Both scaffold modes, and the generated
  migration and seed crates.
- **`nestrs doctor` stops reporting three different toolchain failures as one.**
  `rustc --version` collapsed "not on `PATH`", "on `PATH` and exited non-zero"
  and "printed a line I cannot parse" into a single `None`, printed as `rustc
  not found` — so a `rustc` that ran and diagnosed itself was reported absent
  and its diagnosis discarded, and `rustc 1.x.0` was reported as merely old.
  Each is now its own sentence, and every one of them names the floor, which the
  CLI page has always promised (`rustc ≥ 1.97`) and doctor never printed.

## [4.0.0] - 2026-08-16

### `#[entity]` — a schema serves as a subgraph, gated like everything else

`NESTRS_GRAPHQL__FEDERATION` serves the schema as an Apollo subgraph: the
federation directives are declared and the emitted SDL is the subgraph form —
`@key` present, `_service` / `_entities` stripped, as the spec requires. It is
one flag on a call this repo already makes; async-graphql `=7.2.1` carries the
whole thing with no cargo feature and no new dependency, and the roadmap's
recorded reason for deferring it ("the dedicated schema tooling it would
reintroduce") was simply false.

**The flag does not switch the surface on, and the boot is what makes it mean
anything.** async-graphql serves `_service` and `_entities` as soon as one
entity resolver has registered its keys, whatever the builder was told — so
`#[entity]` plus `federation = false` would publish the schema's own SDL while
the config claimed otherwise, and the flag would be a comment. Declaring an
entity without the flag now fails the boot, naming the resolver.

So does claiming one **key shape** twice. An entity is addressed by `@key`
rather than by name, so the duplicate leaves nothing in the SDL but a doubled
`@key` while the router reaches whichever body linked first — access posture
included. The shape is the identity, not the type: Apollo lets a type carry
several `@key`s and async-graphql matches a reference against the shape, so two
resolvers keying `Widget` by `id` and by `slug` are both reachable and are not a
duplicate. Two claims on `id` are. And it is checked **within** a resolver as
well as across them — two `#[entity]` methods in one `impl` are one
registration, which is the arrangement a per-registration diff cannot see and
the way the mistake is easiest to write. The existing duplicate-operation check
was blind to all of it, an entity method contributing no field; it now reads the
key shapes out of the same scratch registry it already builds.

**What it actually cost is a new operation role, and that is the whole of it.**
`_entities` is a `Query`-root field the router calls with *references* —
`{__typename, <key fields>}` — for objects the client never named, so it is the
one operation whose posture is invisible from the document a client reads. Ship
it ungated and every `@key`-ed type is readable from outside every `#[authorize]`
in the schema, which is the *Hard "no"* list's first line. `#[entity]` therefore
takes exactly what a `#[query]` takes, through the same code path: the guard
chain, a **mandatory** `#[authorize]` / `#[public]`, the argument pipes, the
response mask, and the `GraphqlGuard` bound on every guard bound at its site.
The `@key` is inferred from the resolver's own arguments, so `#[expose]` never
declares a federation key and the cross-transport declaration question never
arises.

**One thing had to be built rather than switched on**, and it is the half a
merged root loses silently: `ContainerType::find_entity` defaults to `None`, and
`DiscoveredQuery` merges its members by hand. A root that forwards only
`resolve_field` answers *Entity not found* to every reference, however many
entity resolvers the app wrote — a whole operation role missing with no field
missing. It now forwards both, under the same first-member-that-answers rule.

Seven refusals, each a named compile error with a trybuild snapshot. Three are
about the role: an `#[entity]` with no posture (whose sentence names *why* this
role is the worst one to forget), and `#[entity]` beside `#[mutation]` or
`#[subscription]` — a role, not a modifier, because `_entities` lives on the
`Query` root and no other root has one. Four are things a `#[query]` allows and
this cannot:

- **A `Result` return is required.** The guard chain is emitted only where a
  denial has somewhere to go, so a bare-return operation silently has none. On a
  `#[query]` that trade is visible in the document; an entity is reached for a
  type the client never named, so a resolver-scope `#[use_guards]` compiled out
  there shows up nowhere at all.
- **`bind = Service` is refused.** It answers `NOT_FOUND` for an absent row and
  `FORBIDDEN` for a withheld one — right for a mutation subject the caller
  named, an existence oracle on a field addressed by key.
- **`#[entity(key = "…")]` is refused.** The key is not the developer's to
  declare, and it is the first thing someone arriving from Apollo reaches for.
- **A `#[graphql(...)]` of its own is refused.** async-graphql reads the first
  one on a method and removes exactly one, so a developer's would silently take
  the slot `#[graphql(entity)]` needs and the method would stop being an entity
  resolver — reported, before this, as a leftover attribute against
  `#[operations]`. There is no working spelling to redirect to, so the sentence
  names the limit.
- **`async` and at least one argument**, reworded from async-graphql's own
  refusals, which land on the `#[operations]` attribute naming a generated type
  nobody wrote.

**Deliberately absent: composition, and the demo is not a subgraph.** Merging
subgraphs is the router's job and `rover` is a dependency this repo will not
take, so what ships is the subgraph half. And `demo/apps/api` leaves federation
off — **`_service` cannot be switched off**; `disable_introspection` does not
cover it and its field is unconditionally visible, so a subgraph publishes its
own SDL to whoever can reach it. The demo is a standalone API on a public
address with no router in front of it, so being one would publish its schema and
buy it nothing. `demo/apps/api/schema.graphql` is unchanged for that reason.

With it, `ROADMAP.md`'s deferred section is empty and gone.

### An `#[sse]` ceiling bounds its stream; its socket is a reported gap

`SseSettings::respond` composes the deadline into the response body, so it is
evaluated whenever that body is polled: **no event is produced past the ceiling
at any rate a peer reads at**, measured against a trickling client rather than
assumed. That is the stale-privilege window, and it holds.

**What outlives it is the socket.** Measured against a client that sent the
request and then read nothing: the server had ~1.6 MB queued for a peer taking
none of it, and hyper's connection task, its buffers and everything the stream
held stayed alive until that peer felt like reading.

**Closing that from inside this crate was attempted twice and neither is sound**,
which is why the gap is documented and witnessed instead. The only thing still
polled while a write is parked is the **socket** — and a socket does not know
which *response* its bytes belong to:

- Arming the connection for the stream's ceiling killed traffic that never asked
  for a stream: on HTTP/1.1 keep-alive an ordinary request issued afterwards
  died with zero bytes and no status line; on HTTP/2, where a browser
  multiplexes an origin's whole traffic onto one connection, siblings died
  mid-flight without even a `GOAWAY`.
- Narrowing it to fire only on a *parked write* was worse, not better. A parked
  write is ordinary TCP backpressure, not a peer that stopped reading: a
  full-speed client downloading 4 MB over a connection that had once carried a
  stream received **1 404 928 bytes under a declared `content-length: 4194304`**
  — a protocol lie told to a well-behaved peer — while a stream small enough to
  fit the socket buffer never parked at all and so was never bounded.

The information the control needs — *which response is stalled* — lives in the
body, which is precisely what stops being polled; poem 3.1 and hyper 1 expose no
per-response write deadline and no abort handle usable while parked. So the
ceiling stays what it is, `sse::a_peer_that_stops_reading_still_holds_its_socket_past_the_ceiling`
asserts the residual so it cannot be quietly reclassified as closed, and the
answer for now is poem's own `Server::idle_timeout` or the reverse proxy —
controls that do not need to know whose bytes are queued.

### A commit that fails the same way every time no longer costs a retry budget

Per-job transactions above turned an unsettleable attempt into a failure, which
was right, and then classified every one of them as **retryable**, which was
not. A constraint checked at `COMMIT` — a deferred unique index, the shape whose
whole point is that it fires late — fails identically on every attempt. The job
therefore replayed its body once per unit of retry budget, **including every
side effect that is not the database's** (an HTTP call, an S3 write, a mail),
and dead-lettered anyway. `#[process]` already aborts on its three other
deterministic failures — an unsupported wire version, an undeserializable
payload, a missing provider — and did the opposite here.

The classification now comes from the database. `CommitError::is_retryable_conflict`
already existed and the HTTP interceptor already consulted it; the worker
context could not, because `JobSettlement` was a payload-free `Copy` enum and
`run_in_job_context`'s `unhonoured: fn() -> T` was a bare function pointer with
nothing to capture. It carries an `Unhonoured { reason, retryable }` now, and a
`LazyTransaction` records the same verdict on the **first failed statement**, so
a poisoned transaction answers the question the same way a failed commit does —
the first is the one that aborted the transaction, and everything after it fails
with `25P02`, which says nothing about why.

**An in-doubt commit aborts, and that is the deliberate half.** A connection lost
during `COMMIT` may have landed. Replaying it turns "may have written once" into
"wrote twice", so the framework re-attempts only what it *knows* rolled back —
which is exactly what keeps the default's promise standing: a `PerAttempt` job
needs no idempotency key because a retry has nothing left to repeat, and
`transactional = false` needs one because the transaction never was. The
documentation said both; only one of them is now true of the code as well.

**A schedule reports the classification rather than acting on it**, and the
asymmetry is the answer rather than an omission: `#[every]` / `#[cron]` /
`#[after]` have no retry budget and no dead-letter, so the next occurrence is
the same whichever way it went. What it owes is the sentence, and it logs it —
the context's own, not one the scheduler invented.

**Breaking for anyone who wrote their own `JobContext`:** `JobSettlement::Unhonoured`
takes an `Unhonoured`, and the `unhonoured` argument of `run_in_job_context` is
handed one. `FinalizeOutcome::Poisoned` gained a `retryable` field.

### Registering a global guard could open `/mcp` instead of closing it

`use_guards_global` reached `/graphql` and the WS message loop and **not** the
MCP operation: the per-operation chain excluded the app-wide pool outright, so a
global guard overriding only `check_mcp` was never consulted. Its presence still
made the pool non-empty, which disarmed the endpoint's deny-all tail — so the
same host answered a tool call *with* the guard registered and refused it
without. A guard written to close an endpoint opened it.

The pool now composes into the per-operation chain on both in-band transports,
through the one `compose` in `nest-rs-guards/src/dispatch/chain.rs` — same three
buckets, same `TypeId` dedup, no per-transport switch over where the pool runs.
The rule underneath it: **an `Exempt` endpoint guard is handed a request and runs
`check_http`; the site is handed the operation and runs `check_mcp` /
`check_graphql`.** Two questions, so neither answers the other, and an edge that
ran the first never shortens the second.

**`McpOperationGuard::already_ran` is gone**, and it is what expressed the false
claim: it reported HTTP-scope execution and was subtracted from an MCP-scope
chain, so its only possible effect was to skip a `check_mcp` that had never run.
Deleting it took the task-local, the mount-time snapshot and the per-request
`Arc` clone that carried it into rmcp's dispatch with it. A custom
`McpOperationGuard` loses a method it could not implement correctly; the
canonical bridge and the pool fallback each lose an override.

**Deliberately unchanged: the deny-all tail still keys on an empty pool.** What
made it wrong was a pool that could not reach the operation, and it can now.
Refusing to arm the fallback for a pool holding no *authentication* guard is a
different rule about a different failure, and it would decide from
`GuardPhase` — a declaration, not a check.

**What this closes is the operation, and `tools/list` is not one.** rmcp's own
server methods — `initialize`, `tools/list`, `prompts/list` — carry no operation
for a `check_mcp` to be handed, so the only thing gating them is the endpoint's
`check_http`. A pool holding *only* `check_mcp` guards still takes `/mcp` from
deny-all to a handshake that discloses the tool inventory. Gating discovery
means a guard with a `check_http`, and the framework cannot tell the two apart
in a pool: `use_guards_global` takes no capability bound, deliberately, because
requiring one would refuse a guard written for a single edge. Reported here
rather than closed by inventing a per-edge global list.

### A boundary that swallows a database error can no longer report success

Postgres aborts the whole transaction on the first failed statement and refuses
everything after it (`25P02`) — and a `COMMIT` on an aborted transaction
*succeeds*, having rolled back. So the commonest shape there is — loop, log what
fails, carry on — reported `Ok`, was told the commit worked, and persisted
**nothing**, with no framework event anywhere. It was reachable on any mutating
HTTP request, and per-job transactions below would have made every queue and cron
job reachable too.

`LazyTransaction` now records the first failed statement, and `finalize` refuses
to report a success it cannot honour: it rolls back, logs at `error` on
`nest_rs::orm`, and returns `FinalizeOutcome::Poisoned`. Every settle site
already had this shape for an escaped handle and a failed commit, so all three —
HTTP's `DbContext`, the shared `with_data_context` for WS/MCP, and the worker
context — treat it the same way: an otherwise-successful outcome fails loudly
rather than losing its writes.

Nested transactions are unaffected: `begin_nested` runs its statements on its own
handle, so a `SAVEPOINT`ed insert that fails still leaves the outer transaction
committable — which is what keeps `Creatable::create`'s scope re-check working.

### A job attempt is atomic, so a retry has nothing left to repeat

A `#[process]` / `#[every]` / `#[cron]` / `#[after]` ran on the connection
**pool**: every statement committed on its own. A job that wrote a row and then
failed left that row behind, the queue retried the job, and the second attempt
wrote it again. The framework carried the transaction for every
request-carrying edge and left the one execution path whose failures are
*expected* — that is what a retry budget is — running without it.

`WorkerDbContext` now installs the same lazy transaction the request edges use,
settled through the same `LazyTransaction::finalize`: commit on `Ok`, roll back
on `Err`, and nothing at all when the job never touched the database. Commit
failure and an escaped transaction handle turn a "successful" attempt into a
failed one rather than reporting writes that were never made — the rule
`with_data_context` already applied per request, now applied per job.

**The classification the roadmap was waiting for turned out not to be needed.**
A job has no safe/mutating method to read, but the transaction is **lazy**: what
opens it is the first data-layer touch, which is a fact about the job rather
than a guess about its intent, so no verb has to be invented for one. A job that
never reaches the database opens nothing at all. **A read is a touch**, though —
a job that only reads still pays a `BEGIN`/`COMMIT` and holds its connection for
the attempt, which is what not having a verb costs and is what
`transactional = false` is there to decline.

**`transactional = false` is the opt-out**, one key, the same word on all four
decorators, parsed and worded once in `nest_rs_codegen::job` with a trybuild
snapshot on each half of the family. It is for the job whose shape defeats the
default — read, then minutes of work that is not the database's, then write —
where the default would pin a pooled connection across the middle. Such a job
owns its own consistency, and an idempotency key is what makes its retry safe.

**The seam changed shape**, which is breaking for anyone who wrote their own
`JobContext`: `scope` now takes the declared `JobTransaction` and an `inner`
yielding whether the job succeeded, and returns a `JobSettlement`.
`run_in_job_context` grew the matching `succeeded` / `unhonoured` pair — the
same two functions `with_data_context` takes on the request edges, because the
settling rule is one rule and a job is not an exception to it.

### `#[sse]` is a route of the framework's own, with the ceiling its peers have

A route could already stream: `nest-rs-http` re-exports poem, so a handler
returned `poem::web::sse::SSE` and the document typed it `text/event-stream` off
the return type. What that cost was a `use poem::web::sse::{Event, SSE};` in the
developer's own controller — the one import the umbrella exists to remove — and a
hand-written `.keep_alive(Duration::from_secs(15))` at every site that remembered
to write one.

`#[sse("/path")]` is a `GET` that answers `text/event-stream`. The handler returns
an `SseStream` of `SseEvent`s; the decorator owns the response. Both types, and
the `futures_util` combinators that build a stream, come through `nest_rs::http`,
so a controller that streams still declares one dependency.

**A stream now carries the ceiling WS and graphql-ws already had.** A stream
authenticates **once**, when the request arrives, then emits with those
privileges for as long as it lives — it outlived an expired token, a logout and a
revoked grant, with nothing to stop it. `NESTRS_HTTP__SSE_MAX_CONNECTION_SECS`
ends it and the client's `EventSource` reconnects, re-running the guard chain:
same reading, same 4-hour default, same `0` ⇒ unlimited spelling as
`NESTRS_WS__MAX_CONNECTION_SECS` and `NESTRS_GRAPHQL__MAX_CONNECTION_SECS`.
`NESTRS_HTTP__SSE_KEEP_ALIVE_SECS` is the interval the demo used to hand-write.

**The namespace is `http`, not `sse`, and that is the one deliberate difference
from its two peers.** SSE is a response shape of the HTTP transport, not a
module; it owns no `#[config]`, so under *one seam per config* it gets no
`for_root` of its own — `HttpModule::for_root` is already its in-code path.

Three refusals ship with it, each a named compile error rather than a key that is
quietly ignored:

- **`#[authorize]`** — the posture masks the response against the entity model,
  and an event stream is no wire model to reconcile; the mask could only fail
  closed at 500 on every request. Gate a stream with a capability-only guard, the
  same answer a presigned URL and a computed report already have.
- **`#[http_code]` / `#[redirect]` / `#[response_header]`** — one sentence for the
  family, so a fourth response decorator inherits the refusal. All three shape a
  response that completes; a stream does not.
- **`#[api(response_content_type = …)]`** — the route answers
  `text/event-stream`; declaring another can only make the published document
  describe something it never sends.

**Deliberately absent: `SseStream` is a named type, not `impl Stream<Item = SseEvent>`.**
An `async fn` on `&self` returning an opaque type captures the `&self` lifetime
under the 2024 rules, so the stream is not `'static` and cannot outlive the call —
which is the one thing a response body must do. The alternative was making every
developer write `+ use<>` and know why.

### A guard bound on a route now declares that it checks HTTP

`Guard::check_http` defaults to `Ok(())` exactly like its three siblings, so an
empty `impl Guard for X {}` bound with `#[use_guards(X)]` compiled, read as a
protection, and passed every request. GraphQL, WS and MCP closed that this
release with a capability marker each. HTTP was left out on the argument that
`check_http` is the trait's base entry, so every `Guard` has it and a bound
there could never fail — an argument about the wrong thing. The bound never
proved a *method* exists; the default gives every guard all four. It proves the
author **declared** that this guard checks this edge.

`HttpGuard: Guard` is the fourth marker. `#[controller]`, `#[routes]`, `#[crud]`
and the `#[gateway]` struct assert it for every path in a `#[use_guards]` /
`#[force_guards]`, through the same `nest_rs_codegen::guard_capability_bounds`
the other three go through, so the refusal is worded once and cannot drift per
edge. There are three emitters — `#[controller]`, `#[routes]` and the
`#[gateway]` struct — and each underlines the decorator the guard was written
under. Two ship a trybuild snapshot; the gateway site is asserted by the
`nest-rs-macro-hygiene` witness rather than by a snapshot of its own.

**A `#[gateway]`-struct guard attests `HttpGuard`, not `WsGuard`** — those run on
the upgrade, which is an HTTP `GET`, while `WsGuard` belongs to the per-message
scope. That split was documented folklore; it is now checked.

**Deliberately absent: `check_http` did not move onto the marker.** Every
execution site holds `Arc<dyn Guard>`, so an extension trait carrying the method
needs a second erasure and two container registrations for any guard serving HTTP
*and* another edge — the cost already weighed and refused for the other three. It
would also save nothing: `nest-rs-guards` depends on `nest-rs-http`
unconditionally, so every build that links the guard core links the HTTP stack
and a `cfg` on the trait method saves no bytes.
The binary-size argument that deferred this work belonged elsewhere —
`nest-rs-queue` carried an unused `nest-rs-http` dependency, removed here, its
`#[input]` re-export having come from `nest-rs-core` for some time.

**Migrating:** write `impl HttpGuard for YourGuard {}` beside the `check_http` it
attests. The compiler names every site, at the `#[use_guards]` line.

### An `operationId` is sanitised as one string, not one half of one

The version fragment was mapped onto what an identifier can carry and the
controller and handler fragments were not, so a raw-ident handler
(`async fn r#type`) published `probe_r#type` — an id no client generator can name
a method after. The map now runs over the **composed** id, which covers all three
halves at once; a collision it newly creates (`r#type` beside `r_type`) is
reported by the existing duplicate-id `warn`, whose remedy now names the handler
rename too.

### `#[mcp]` answers every server-level identity key by name

`description`, `website_url` and `icons` got a bare "unknown key" while `version`
and `instructions` named their owner — the same mistake in three more spellings,
and a bare "unknown key" is silence. All four now come from one table and one
sentence pointing at `McpModule::for_root(McpOptions { server })`, with a drift
guard that reads `McpIdentity`'s builders and fails if one gains no answer. A key
that is genuinely nobody's still gets the accepted-key list.

### One casing rule, and it reads a run of capitals as one word

`snake_case` inserted `_` before every interior capital, so `HTTPServer` became
`h_t_t_p_server` and `APIKey` became `a_p_i_key`. A run of capitals is now one
word: `http_server`, `api_key`, `io_handler`.

It had gone unnoticed because every consumer fed `format_ident!` — private Rust
identifiers inside macro expansions, which nobody reads. The `operationId` above
is the first output a **developer** reads, and it is where the rule had been
copied rather than shared.

The copy is gone. `#[routes]` computes the controller's identifier token at
compile time, through the one `nest_rs_codegen::snake_case`, and
`HttpControllerMeta` carries it. A runtime crate cannot reach `codegen` without
dragging `syn` into every app's dependency graph, so deriving it in the macro is
what keeps the rule single — the alternative is a second implementation, which is
exactly what was there.

**One shape no general rule resolves**, and it is documented rather than
special-cased: `OAuth` → `o_auth`. Nothing distinguishes a single capital
followed by a capitalised word from a genuine one-letter prefix without a
dictionary, and a heuristic for it would fire on names it has no business
touching — the same reasoning that removed the `v`-plus-digits heuristic from
version detection. Rust's own convention is the answer: acronyms count as one
word (`Uuid`, not `UUID`), so the type is spelled `Oauth`.

### Every `operationId` is unique, which OpenAPI requires and we were not

`operationId` was the handler's method name alone, so `#[crud]` — which names
every resource's handlers identically — published one id for many operations.
The reference product's own document carried **12 collisions across 27
operations**: `list` four times, `get` four times, `create` / `update` / `delete`
three each. OpenAPI 3.1 §4.8.10.1 requires the id to be unique across the
document; a generator either errors or silently renames, so the loser's method
went missing from every generated SDK.

An id is now `<controller>_<handler>` — the controller struct name with its
trailing `Controller` stripped, snake_cased, from the same metadata field that
already supplies the operation's default tag. `posts_list`, `users_get`. This is
what `@nestjs/swagger` does, for the same reason.

- **A version composes on top**: `reports_list_v1`, and a version that is not a
  bare integer still reads as an identifier (`reports_list_v2024_08_11`). So a
  `version = ["1", "2"]` controller and the two-controller layout both name their
  operations apart.
- **What survives is reported, not published silently**: two controllers whose
  names reduce to one token — differing only by the `Controller` suffix or by
  casing — raise a `warn` on `nest_rs::openapi` naming both addresses. Never a
  boot failure: a duplicate id degrades a generated client, it does not break the
  running app.
- `demo/apps/api/openapi.json` regenerates to 27 operations with 27 distinct ids
  and boots with no warning.

### `TestApp` boots the transport the app configured, not a fresh one

`TestApp::build` constructed a bare `HttpTransport::new()`. Every field
`HttpModule::for_root(cfg)` sets was therefore absent under test — the global
prefix, the versioning strategy, the body cap, the request timeout, CORS,
compression, the security headers — so a suite asserted against a transport the
deployment never runs. An app pinning `global_prefix: "/api"` served `/widgets`
in its own e2e and `/api/widgets` in production, and every test passed.

That is the failure e2e exists to catch: the testing section of `CLAUDE.md`
opens with "wiring bugs don't surface in unit tests", and the wiring was what
the harness dropped.

- **`HttpTransport::from_config(&HttpConfig)`** is now the one place a config
  becomes a transport. `HttpModule`'s `TransportContribution` calls it, and so
  does the harness — the logic used to live inline in a closure where nothing
  else could reach it.
- `TestAppBuilder::http(transport)` still overrides, for a test that needs a
  transport the app does not declare. Pinning an `HttpConfig` on the module is
  now the way to assert against non-default settings.
- `harness_parity` in `nest-rs-testing`'s suite pins the two fields that change
  a request's **address**, because those are the ones whose absence turns a
  green suite into a `404` in production.

### A version is declared the same way wherever a client can select one

Versioning was HTTP's, and every other transport was silent about it — so a
developer who read `#[controller(version = "1")]` and reached for the same word
on a resolver or a processor got "unknown key" and had to go find out what that
transport does instead. Silence is the failure of unity here, not the absence of
a mechanism: versioning is *addressing*, and the transports genuinely differ in
whether they have an address a client picks.

So the declaration is unified where it can be, and **refused with the
transport's own answer where it cannot**:

- **`#[controller]`** — full, and wider than before (below).
- **`#[gateway]`** — `version = "1"` mounts the socket at `/v1/ws`, through the
  same `version_path` the HTTP routes use, so the two cannot word the `/v{n}`
  prefix differently. Two gateways may share a `path` under different versions;
  the duplicate-mount boot error compares the versioned path.
- **`#[resolver]`, `#[mcp]`, `#[processor]`, `#[scheduled]`, `#[listeners]`** —
  a compile error naming what to reach for instead: `#[graphql(deprecation = …)]`
  on the field a GraphQL schema is retiring, `#[mcp(path = "/mcp/v1")]` for an
  MCP endpoint, a versioned queue name or payload evolution for a job, and
  nothing at all for a clock or an in-process event. The five sentences are
  worded once in `nest_rs_codegen::Edge`, beside `DecoratorPair` and
  `PostureRules` — `rg 'Edge::' crates/*-macros/src/` names every site — and
  each ships a trybuild snapshot.

`#[processor]` and `#[scheduled]` took an argument list and **silently ignored
whatever was in it**; both now refuse, `version` by name first.

`#[mcp]` refuses `version` in **every** shape. The word was briefly spelled there
for `serverInfo.version`, and that was one declaration too many: a feature library
knows neither the binary's version nor, on a shared endpoint, the whole surface,
so the server's version has one owner — the app's single
`McpModule::for_root(McpOptions { server, .. })`. A host declares only which
endpoint stands apart, `#[mcp(name = …, title = …)]`. The refusal points at
`#[mcp(path = "/mcp/v1")]` for an address and at that seam for a version.

This follows `reject_http_only_layers` — the framework already answers a layer
family an edge does not bridge with a named error rather than ignoring it.

### Selection asks the app's real shape, and poem always decides last

Under `header` / `media_type`, the selector answers one question per request:
does a version have anything to select here? It reads three exact lists the
transport hands it at boot — the routes that carry a version, the addresses
answered without one, and the paths self-mounted endpoints own — and matches a
request against them with the router's own segment forms (`:name`, `<regex>`,
`*rest`, and a literal and a parameter sharing one segment: `/@:handle`).

**Precedence, which is the whole behaviour:**

- **A self-mounted path is neutral against everything.** `/graphql`, `/mcp`,
  `/api-json`, a WebSocket gateway — each owns its path outright and has no
  version to be rewritten to.
- **A stated version beats an unversioned route at the same address.** An
  explicit request is the strongest signal a caller sends; yielding to a
  neighbour would leave `#[controller(version = …)]` unreachable there.
- **A default version yields to it.** The caller asked for nothing, so nothing
  moves under them. This is what stops `NESTRS_HTTP__DEFAULT_VERSION` from
  rewriting the whole surface: a versioned `#[controller(path = "/")]` with a
  catch-all route would otherwise swallow every unversioned controller beside it.
- **A version that serves no route, while another version does, is a `404`** —
  answering with a different version's body is the silent failure the design
  exists to prevent. Where no version serves the address at all, the request is
  passed through and the router answers.

**The matcher answers loosely on purpose, and that is the load-bearing part.**
Every outcome ends at poem: a match produces a rewritten path the router must
still recognise, a non-match passes the request on as written. So a false match
costs a `404` the request was heading for anyway, while a false non-match once
served one controller's body to a caller who asked for another's. Loose in the
direction the router can correct, never in the direction it cannot see — which
is what a first attempt at this, matching controller *prefixes*, got backwards
in both directions at once.

**The rewrite endpoint is not installed at all when nothing is versioned.** A
strategy set with no versioned controller would otherwise pay an extra routing
layer per request, forever, for an outcome it could never change. The endpoint
also resolves behind an immutable borrow, so a neutral or version-less request
allocates nothing.

Two boot-time consequences:

- **A `NESTRS_HTTP__DEFAULT_VERSION` naming a version nothing declares fails the
  boot**, in every app. The check used to live in `nest-rs-openapi`, so an app
  publishing no document got no answer at all and every caller stating no version
  silently fell through. `declared_versions` moved to `nest-rs-http` and both
  consumers share it.
- **Two controllers overlapping on one version are told which version.** The
  failure used to read "give each one a distinct path" — advice against the
  two-controller layout the docs prescribe, about a string (`/v2/posts`) neither
  developer wrote.

### A second API version stops costing a second controller

`#[controller(version = ["1", "2"])]` mounts one controller under every version
it lists, and `#[version("2")]` on a method narrows one route out of the rest —
the shape a real v2 has, where most routes are unchanged and one is new.

- `HttpControllerMeta` carries `versions: &'static [&'static str]` and yields one
  mount per version; the boot log, the OpenAPI document and the served routes all
  iterate the same list.
- A `#[version]` naming something the controller never declared **does not
  compile**. It would otherwise mount nowhere — the transport loops over the
  controller's versions — leaving a handler that builds, registers, documents
  itself and answers nothing. `versions_declare` is a `const fn`, so the error
  lands at the route.
- A declared version is validated as a path segment at compile time, the same set
  the wire validator refuses at runtime.
- A verb absent from a version answers `405` on a path its siblings serve, not
  `404` — the path exists, that method does not.

### Response masking is armed by the compiler, so a rename cannot disarm it

`#[routes]` used to decide whether a route masks its response by matching a
parameter's **path segment** against `Authorize` / `Bind`. That is a question
about how a type is *spelled*, and `use Authorize as Az` gave a different answer
than `Authorize` for the same type. The hole was covered — an unarmed route ran
inside a `MaskProbe` and a masking extractor that ran there failed the request
closed — but the cover was load-bearing, which is exactly what the roadmap
entry existed to end.

The macro no longer asks the question. It hands each parameter *type* to
`nest_rs_http::ShaperProbe` and the **compiler** answers whether that type is a
`RouteResponseShaper`, through the two-arm autoref selection `shaper_of!` spells
once. A rename changes the spelling and not the type, so the alias arms
identically — `an_aliased_authorize_masks_a_raw_model_body` used to assert a
`500` and now asserts a masked `200`.

- **The trait moved from "capture + run" to "capture into a `ResponseShaping`".**
  `RouteResponseShaper::capture` returns the request-independent half as a boxed
  object, so the selection can be a plain function pointer rather than a type
  parameter threaded through the endpoint. One `Box::pin` per *armed* request
  buys the alias-proofing; an unarmed route pays nothing it did not pay before.
- **`masked` in the OpenAPI document reads off the same selection**, so a
  document can no longer describe an aliased route as unmasked.
- **The probe stays, with a smaller job.** A signature scan of any kind cannot
  see an extractor reached *indirectly* — nested in another extractor, or run by
  a hand-rolled `FromRequest`. That is now the only thing the `500` covers, and
  its message says so.
- The eager HTTP-D1 diagnostic survives, and got shorter: a local type wearing
  the name `Authorize` without implementing the trait is still a spanned error
  naming `RouteResponseShaper`, no longer trailed by a transitive `Endpoint`
  bound failure from the mount site.

### A certificate renewal no longer needs a restart

`HttpTransport::tls` loaded the PEM pair once, at boot, so certbot's
`--deploy-hook` ended in `systemctl restart`. Material read from **files** —
`NESTRS_HTTP__TLS_CERT_FILE` + `_KEY_FILE`, or the new `TlsConfig::from_files` —
is now watched, and a renewed pair is swapped into the running `rustls` config.
The listener is not rebuilt: the port stays bound and in-flight connections
finish.

`NESTRS_HTTP__TLS_RELOAD_SECS` is the interval (60 by default, `0` disables).
Polling rather than an OS watch is deliberate — a renewal is a minute-scale
event, and a file watcher's hardest case is precisely the one renewal tools hit,
an atomic replace that swaps the inode out from under the watch.

Two refusals are as load-bearing as the swap: **a pair that cannot be read
leaves the current certificate serving** (a tool that unlinks before it writes
would otherwise take the listener down between two syscalls), and **a pair is
installed only once it reads back identical twice**, so a file caught mid-flush
is never the one that gets served.

The second rule is stated as what polling can actually guarantee, because a
first attempt claimed more: a renewal whose two halves land more than one
interval apart looks settled in between. Certbot and cert-manager both swap
atomically and never hit it; `a_pair_that_is_still_changing_is_never_installed`
pins what does hold, and the docs name the residue rather than implying it away. Inline PEM has no source to watch and is
loaded once, as before. `a_renewed_certificate_is_served_without_dropping_the_listener`
proves it through a real handshake: two leaves under one CA, and which hostname
verifies is the only thing the swap changes.

### A version is declared once; how a caller selects one is deployment config

URI versioning shipped; `Accept: application/json; version=2` and
`X-API-Version: 2` did not, and the docs told you to write a guard. Now
`NESTRS_HTTP__VERSIONING` picks between `uri` (the default), `header` and
`media_type`, and **no controller changes** — `#[controller(version = "2")]`
remains the one place a version is declared and `version_path` the one place it
becomes a path.

The two request-time strategies are a rewrite in front of routing, inside the
global prefix, so one route table serves all three. The boot log follows: it
reports the address a *client* uses and moves the version into its own field,
because `/v1/posts` is where the route is mounted, not where it is called.
Three behaviours are decisions, not defaults:

- **An unknown version is a `404`, never a fallback.** Quietly serving `v1` to a
  client that asked for `v9` is how a client talks to the wrong API for a month.
- **The URI form stops being a second address** under the other two strategies —
  one way to ask, or every version has two names.
- **A malformed version token is a `400` before it reaches a path.** The token is
  spliced into a URL, so it is validated first: alphanumerics, `.` and `-`, 32
  bytes. `a_version_that_could_reach_another_path_is_refused` is the witness.

`NESTRS_HTTP__DEFAULT_VERSION` answers a caller that states none — on the paths
that have versions. The selector learns which mounted prefixes carry one at
configure time, so a default is a default *among versions* and never a rewrite of
everything the app mounts: an unversioned controller, and every self-mounted
endpoint (`/graphql`, `/mcp`, `/api-json`, `/health`), is served as written.
`a_default_version_does_not_rewrite_paths_that_have_no_version` pins it.

**The generated document follows**, in the same release: OpenAPI 3.1 cannot key
two operations on one path, so a non-URI strategy publishes one document per
version — see *The document describes the addresses a client actually calls*
below.

### An upload never exists whole, at either end

`Storage` grew the two methods the object-store surface was missing, and the
HTTP side grew the one seam that lets them be reached without buffering:

- **`Storage::list(prefix)`** streams `ObjectEntry { key, byte_size,
  last_modified }` rather than returning a `Vec`, so a large prefix is never
  held in memory. It leaks no new third-party type — `last_modified` is a
  `std::time::SystemTime`.
- **`Storage::put_stream(key, content_type, stream)`** drives a real multipart
  upload, sequentially, one part in memory at a time. Every failure path —
  the source stream's, a part's, the tail's, completion's — **aborts the
  upload** first, because parts left behind are billed and invisible to `list`.
  A failing abort is reported at `warn` on `nest_rs::storage` rather than
  swallowed.
- **`nest_rs_http::PartExt::into_byte_stream()`** is the other half of poem's
  `Field::bytes()`: the part as it arrives. The demo's direct upload now goes
  part → storage without a `Vec<u8>` anywhere, and its service names a stream
  rather than a transport type, so the same method serves a multipart part, a
  proxied download or a test fixture.

Streaming bounds **memory**, not the request: `max_body_bytes` remains the
ceiling, and it still covers `Multipart` like every other extractor.

### `nestrs g entity` and `nestrs info`

`g entity <feature>[/<name>]` scaffolds one `#[expose]` entity into an existing
feature without the CRUD slice around it. Placement follows the feature: its
first entity is the lone `entity.rs`, and a feature already keeping several in
`entities/` gets one more file there.

A feature with a lone `entity.rs` that now needs a second one is **refused with
the four steps to take**, not migrated. SeaORM spells relation module paths
inside string literals (`belongs_to = "super::org::Entity"`), so moving
`entity.rs` into `entities/` either misses those or, reaching into strings,
corrupts prose that merely mentions one — silently, in code the developer wrote.

`nestrs info` reports the project the current directory sits in — layout, root,
apps, features, the framework version the manifests pin, the env prefix in
force, the toolchain — and says so plainly outside a project rather than
failing. That is the line between it and `about`: `about` is seven lines
identical on every machine, `info` reads the tree it stands in.

### A relation is a page, an enum is a declaration, and a second foreign key no longer needs a hand-written resolver

Three gaps in `#[expose]`, closed independently.

**`#[wire_enum]` — the enum half of the umbrella promise.** An exposed enum column
made the developer hand-write nine derives and put `schemars` and `async-graphql`
in their manifest for nobody's code but the macro's. `#[wire_enum]` emits the wire
derives with their `crate = ` overrides routed through `nest-rs-resource`, and
`#[wire_enum(graphql)]` adds `async_graphql::Enum`. It deliberately emits **none**
of the SeaORM half — `DeriveActiveEnum`, `rs_type`, the per-variant `string_value`
— because how a column is *stored* is the developer's decision, and their own
source legitimately writes it.

The `graphql` flag is explicit rather than read off the crate feature: a Cargo
feature is additive across a workspace, so one GraphQL app would otherwise put an
`Enum` derive on every enum in every sibling crate.

`nest-rs-macro-hygiene` gains its first witness from the `#[expose]` family: an
entity cannot live in a zero-dep crate, but an **enum can**, so the derive routing
that was invisible to review is now compiled against `nest-rs` and nothing else.

**`Connection<T>` — a relation you can page, and a defect that is gone rather than
documented.** An auto-emitted HasMany resolver returned a `Vec<T>` capped at
`RELATION_LOAD_CAP` (100 per parent) through one `WHERE fk IN (…) LIMIT cap × keys`
query — a shape in which low-FK parents could consume the whole budget and leave
later parents with `[]`, indistinguishable from "no children" (DATA-R2). The field
is now a Relay `Connection` with `first` / `after` / `PageInfo`, backed by a new
`Repo::relation_pages` that ranks with `ROW_NUMBER() OVER (PARTITION BY fk ORDER BY
pk)` — one round trip, a page **per parent**.

- **`RELATION_LOAD_CAP` is deleted, and so is the DATA-R2 comment.** Both existed
  only to bound the broken shape and make its starvation loud. `clamp_page_size`
  (1..=100) is the only bound now, with `DEFAULT_PAGE_SIZE = 20`.
- **The loader key carries the window** (`RelationKey { parent, first, after }`).
  Keyed on the parent alone, two sibling selections asking for different pages of
  one relation would be served the same batch entry.
- **The `after` predicate is inside the ranked subquery**, and a unit test asserts
  the SQL text says so. Applied outside, a parent's rank would count rows the
  caller already has and page 2 would come back short — a bug that returns exactly
  as many plausible rows as a correct query, which is why the assertion is on the
  text and not the count.
- **Complexity is `first * child_complexity`** instead of a flat `10 *`. The
  constant was a guess because there was nothing to multiply by; the field now
  takes its page size, so `first: 5` costs a twentieth of `first: 100` — which is
  what a ceiling is for. A 3-deep unannotated chain scores 20³ against the
  documented `max_complexity` default, and that is the ceiling working on a query
  that really can materialise 8000 rows.
- Masking is unchanged and still per item, in the loader. `unmasked` turns out to
  be irrelevant here rather than needed: a `#[ComplexObject]` field resolver is not
  an `#[operations]` operation, so the value-level round-trip never sees the
  connection.

`last` / `before` are deliberately absent: the underlying keyset is forward-only,
and a backward argument would be a lie in the SDL.

**`via = "…"` — naming which foreign key, when the type cannot.** The ambiguity was
always on the `has_many` side: a `belongs_to` names its column in `from = "…"`,
while the parent had only the child's *entity type*, and `RelatedTo<Parent>` was
keyed on that type alone. Two `belongs_to` at one parent meant two impls of one
trait, which is why the macro used to refuse the shape outright.

`RelatedTo<Parent, Via>` now carries a per-FK marker the child's `#[expose]` emits,
and the `SoleForeignKey` default impl is emitted **only when the child targets that
parent exactly once**. That absence is the enforcement: the ambiguous case fails to
compile at the relation that is ambiguous, with a note naming
`#[expose(via = "…")]`. The framework never picks whichever `belongs_to` came
first. (`via` on a `HasOne` is a compile error pointing back at `from`.)

One half stays the developer's, and it is SeaORM's rule rather than ours: two
relations to one entity need `relation_enum` / `via_rel` on the entity itself.
That is entity-site code their own source writes — and sea-orm gates its own
`Related<E>` impl on the same "targeted exactly once" rule.

### The document describes the addresses a client actually calls

Shipping header and media-type versioning left the generated document naming
`/v1/posts` while clients had to call `/posts` plus a header — paths that answer
`404`. A document that lies is worse than one that omits, so it was recorded on
the roadmap the day it appeared and is closed here.

Under a non-URI strategy the path keys become the **client-facing** ones and each
operation gains its version parameter — the configured header, or `Accept` for
media-type — `required` exactly when the deployment names no default. OpenAPI 3.1
keys operations by path, so two versions cannot share one entry; **each version
gets its own document at `/api-json/v{n}`**, built through `version_path` rather
than a second `format!` so the document's own address and the routes it describes
cannot disagree about the `/v{n}` spelling. `/api-json` describes the default
version when one is named, and otherwise aggregates, resolving a contested path
to the highest version — deterministic and stated, where link order would have
been neither.

**Under the URI strategy nothing changes**: same paths, same operations, no
version parameter, no extra route, and the committed document's line order is
left exactly as discovery hands it over — an aggregate sort that reordered a JSON
object would have rewritten every line of a committed snapshot for no reader's
benefit.

A `NESTRS_HTTP__DEFAULT_VERSION` naming a version no controller declares now
**fails the boot**. It used to publish an empty document to every client that
read it.

The second half of the entry is smaller and older: a required `Query<T>` property
produces a `400` the document did not advertise, while a required header already
did. One rule computed from the parameters actually emitted now covers both, and
the committed snapshot moved with it.

### Every `#[expose]` diagnostic is pinned, and pinning them found three that were wrong

`#[expose]` raised 27 distinct compile errors, `#[wire_enum]` four more, and
**nothing pinned any of them** — eleven other macro crates carry a trybuild
suite, this one did not. It does now: `crates/nest-rs-resource/tests/integration/diagnostics/`,
31 `.rs`/`.stderr` pairs, same arrangement as its siblings.

Writing them down is not the point; **reading them back** is. Three of the 31
were wrong, and two of those had shipped:

- **`#[expose]` on an enum** answered with syn's `expected struct`. It now names
  its sibling `#[wire_enum]`, mirroring the refusal that already went the other
  way. Each half's name is a `const` the *other* one's message reads, so neither
  can name a decorator that has moved.
- **`from = "…"` naming a column that does not exist** claimed the column was
  "not exposed" while checking only existence — two different remedies, one
  message. The two copies of that check became one function.
- **`from = "…"` naming a column that exists but is unexposed passed silently**,
  then failed as ``no field `org_id` on type `Post` `` from inside the expansion,
  because the field resolver reads the key off the wire object. It is refused at
  the field now, naming both remedies.

Two diagnostics are deliberately unpinned and say so in the suite's header: the
`graphql`-feature refusals cannot exist in a build that can run the suite (the
dev-dep that compiles the GraphQL fixtures turns the feature on), and pinning
them would need a third suite name, which is locked.

`#[crud]`'s GraphQL list op also stopped spelling its own `20` and reads
`nest_rs_seaorm::DEFAULT_PAGE_SIZE`. The HTTP half never drifted — it reads
`PageParams::limit()`, which reads the constant.

### The document stops omitting the three shapes it could not describe

Header parameters, multipart request bodies and streamed responses were invisible
to the generated OpenAPI document: `/audio/uploads/direct` carried **no**
`requestBody` at all, the streamed download and the SSE feed carried a bare
`"200": {"description": "OK"}`, and no operation anywhere declared a header. The
docs page even promised a `Header<T>` extractor that did not exist.

**`nest_rs_http::Header<T>`** now does — the header-map twin of poem's
`Query<T>`: one struct field per header, `#[serde(rename)]` for the wire name,
case-insensitive lookup, values parsed into the field's type, `Option<_>`
optional. It composes like every other extractor (`Valid<Header<T>>`,
`Piped<P, Header<T>>`), and the operations that read one describe it.

Its rejections **name the header and never quote its value.** Headers carry
credentials, so a compound shape is refused by our own arm rather than left to
serde, whose `invalid_type` message would print the value it could not parse.

The two body shapes follow the same rule — say what the framework emits, never
guess:

- **`#[api(multipart = T)]`** documents `multipart/form-data` with `T`'s schema.
  A handler taking `poem::web::Multipart` and declaring nothing still documents
  `multipart/form-data` with a free-form object, because silence was the defect.
  The two are **one enum**, not a media-type string beside an optional schema, so
  "media type disagrees with schema presence" is unrepresentable.
- **`#[api(response_content_type = "…")]`** declares a non-JSON success body and
  gets the stream schema that goes with it. **SSE is inferred** from `-> SSE`:
  that return type serializes as `text/event-stream` and nothing else, so reading
  it off the signature states a fact rather than repeating a declaration — the
  same reading that already infers the response schema from `-> Json<T>`. An
  explicit `response_content_type` still wins.

A required header also makes the operation advertise its `400`, computed from the
parameters actually emitted, so an all-optional header DTO adds nothing.

The demo carries a real use site rather than a stub: `GET /audio/events` reads
`Last-Event-ID`, so a reconnecting `EventSource` is not re-sent progress ticks it
already displayed.

### The docs say which pages you need and which ones you may not

A section was one flat list, so `Controllers`, `Extractors`, `Compression`,
`Versioning` and `File uploads` read with the same weight and nothing told a
newcomer which four to open. Every section of five or more pages now splits into
**Basics** — what you need to ship the section's common case — and **All
options** — configuration and tuning, opt-in capabilities, operational
behaviour, extension seams, reference tables.

The mechanism keeps ordering in one place: a page declares `tier:` in its
frontmatter, `sidebar.order` still sorts *within* a tier, and
`docs/src/sidebar.mjs` builds the groups. Adding a page needs no config edit and
cannot be silently dropped from the sidebar. Both halves fail closed — a missing
or unknown tier stops the **build**, and `lint:docs` reports it — and
`STYLE.md` §G states the rule so a new page inherits it.

The tier follows **why a reader opens a page**, not its content mix, which is
what puts `http/extractors` (a reference page you read to write a handler) in
Basics and `websockets/guards` in Basics against the size logic — "All options"
must never read as "security is optional".

### GraphQL subscriptions, and a posture that keeps applying after it is granted

`#[subscription]` joins `#[query]` and `#[mutation]` in the same `#[operations]`
impl, under the same mandatory posture, served over graphql-ws on the path that
already serves `POST` — an upgrade takes the socket, a plain `GET` still takes
the playground. The discovered schema builds with a real subscription root;
`EmptySubscription` is gone, and a schema that declares none still carries no
`Subscription` type.

What is new is not the operation kind but *when* the declaration is enforced. A
query's posture is spent once. A subscription's is not: the guard decides at
subscribe, and items keep arriving afterwards.

- **The mask moved onto the stream.** Every item is evaluated against the
  ability captured at subscribe — row rules first, then field grants — and an
  item outside the grant is **dropped**, never nulled. Two subscribers on one
  stream therefore read two different sequences, which is the whole guarantee;
  `an_item_outside_the_grant_never_reaches_that_subscriber` is the witness.
- **A socket may not outlive the decision that opened it.**
  `NESTRS_GRAPHQL__MAX_CONNECTION_SECS` is the same control, spelling and
  four-hour default as a WebSocket gateway's, and it is now asserted rather than
  configured: a connection that never ends is closed when the deadline elapses.
- **Three things the upgrade lends, and one it does not.** The principal and its
  ability travel with the connection. A database handle travels too, but always
  **outside** the upgrade's transaction — carrying it would pin a pooled
  connection for hours and write through something the `101` already finalized.
  The `RequestScope` does not travel at all: a request-scoped provider built once
  at connect and shared for four hours is not request-scoped, so `Scoped<T>`
  reports it absent instead of handing back something that looks right.
- **`GraphqlOperationGuard::around` scopes a `()` future**, so one method covers
  one operation and one socket. Without it the socket ran with no ambient
  ability and every subscribe was refused — the guard chain was correct and the
  state it reads was missing.
- **Two compile errors that name themselves**: a `#[subscription]` must be
  `async`, and a fallible one spells its return `Result<…>` literally
  (async-graphql reads the last path segment, so an aliased `Result` is taken for
  an ordinary value). A third, wider one landed with them: an operation taking a
  `Piped<P, T>` argument must return `Result<…>`, because a pipe rejects and the
  rejection has to reach the client.
- **`TestApp::graphql_socket()`** drives the graphql-ws protocol in-process, so a
  suite asserts what a subscriber receives without binding a socket.

### A decorator may not put a line in your manifest, and now something checks

`#[crud]` emitted `::uuid::Uuid` for three HTTP routes and one GraphQL resolver
argument. A controller file names `std`, `nest_rs` and `crate::` — never `uuid`
— so a crate that wrote `#[crud]` and nothing else failed with `E0433` blamed on
the attribute, against the hard "no" that a macro expansion never obliges the
developer to declare a crate.

It survived two witnesses. `nest-rs-macro-hygiene` cannot reach `#[crud]` (it
needs a real entity and service, documented and still true), and the generator's
e2e — the substitute witness — passes for an unrelated reason: `g resource`
bootstraps `g auth`, whose claims type names `uuid` and drags the dependency in.
Remove the auth adapter and the generated CRUD stops compiling.

- **Routed through `::nest_rs_resource::uuid`**, whose re-export exists for this
  and is unconditional — `nest-rs-seaorm`'s own `uuid` is optional behind two
  features, so routing there would have made resolution depend on which
  transport the crate enabled.
- **The rule became a test.** `framework.md` has always stated it as checkable —
  *"a `*-macros` crate emits only `::std`/`::core` paths or paths routed through
  its surface crate's re-exports — never a bare third-party path"* — and nobody
  had run it. `nest-rs-macro-hygiene/tests/integration/emissions.rs` reads every
  macro source and fails on a path rooted outside the framework. It is an
  allowlist, not a banned-crate list: a decorator written next year reaching for
  a crate nobody thought to ban fails on the day it is written.

### Two transports could contradict themselves at boot; both now refuse

**GraphQL — two resolvers claiming one operation name.** The merge folds member
fields with `IndexMap::extend`, so the *last* registration's metadata reached the
schema, while `resolve_field` returns from the *first* member that answers. A
client read one resolver's signature from the SDL and reached another resolver's
body, with the argument it was told to send dropped. Both orders follow
`inventory::iter` — link order. The security half is why this is an error rather
than a `warn`: `#[authorize]` expands *inside* the operation's body, so the
posture that ran belonged to whichever body won the dispatch, not to the
operation the schema documented. Now an `HttpBootCheck` naming both resolvers,
the seam MCP already used. A query and a mutation may still share a name — two
root objects, not a collision.

**Queue — two `#[process]` methods claiming one queue.** A backend builds one
worker per registry entry, so both polled the same stream: each job went to
whichever popped it, and the retry budget forked with it. `nest-rs-queue` owns
the check (any backend draining the registry owes the same refusal) and
`QueueWorker::configure` runs it after module-gating, so a processor another app
owns cannot fail this app's boot.

### Every client-facing transport withholds what a client must not read

A handler's error type is the framework's only source for the message a client
reads, and `Display` is the wrong default: a `DbErr` carries SQL, column names
and sometimes row values. MCP had the seam first, because its reader is a
language model that may repeat what it is told — but a GraphQL error frame, a WS
error frame and an HTTP error body are read by clients just as untrusted.

- `nest_rs_graphql::Opaque`, `nest_rs_ws::Opaque` and `nest_rs_http::Opaque` join
  `nest_rs_mcp::Opaque`: `.opaque()?` logs the real error at `error` on the
  transport's own target and substitutes the shared
  `nest_rs_core::OPAQUE_CLIENT_MESSAGE`.
- **Four traits, one constant** — deliberately, and the reason is measured rather
  than aesthetic. The trait's output *is* the transport's error type, which is
  what lets `.opaque()?` infer from the enclosing function's return type; one
  trait generic over the output has four applicable impls, so the receiver stops
  deciding and every call site needs a turbofish. Sharing stops at the value the
  four must agree on.
- GraphQL's opaque error carries the same `INTERNAL` code an internal denial
  does, so a client cannot tell an unexpected failure from a refusal it was owed
  no explanation for. HTTP's is a `ProblemDetails` 500 for the same reason: the
  envelope a client parses must not change because the *reason* is withheld.
- The three non-request edges get no trait and want none — a job, a tick and an
  event have no caller to tell anything.
- **One site on the WS edge bypassed the seam it already had**, and it was the
  one nothing could see: `WsReply::reply` builds the frame from a handler's
  *return*, and when that value would not serialize it handed the socket
  `serde_json::Error`'s `Display` — which names the handler's types and can
  carry the value that failed — while emitting no event at all. Its neighbour
  `pipe_error` had learned the same lesson a release earlier and says so in its
  own doc. It now routes through `Opaque` like everything else: the operator
  gets the cause on `nest_rs::ws`, the client gets the constant. A reply that
  cannot be built is the server failing, not the caller.

### The testing harness can watch a thread it did not start

`LogCapture` is thread-local — `set_default`, which is what keeps parallel tests
from reading each other's events. That is the right default and it is blind in
one place, and the place is not a corner: an event the framework emits from a
task it *spawned* — a `spawn_blocking` write, a socket's writer half — never
runs on the test's thread, so the assertions worth writing were exactly the ones
that could not be.

- `LogCapture::install_global()` captures on every thread for the rest of the
  process. Sound because nextest gives each test its own process; permanent,
  because `tracing` allows one global default and offers no way back. It panics
  if anything already took the slot — including an `App` boot, which installs
  the console fallback — so it is the first statement of a test, never a line
  near the assertion. Never beside `install()`: a thread-local default shadows
  the global one silently, and a *negative* assertion then passes for the wrong
  reason.
- `LogCapture::expect_none(target, message)` is the quiet half, worded once. A
  negative log assertion is the easiest to write and the easiest to write
  uselessly — it passes when the event is absent, and equally when the target is
  misspelt or nothing ran — so the panic names both coordinates and dumps what
  *was* captured, which is the half a hand-rolled
  `assert!(logs.find(..).is_empty())` keeps leaving out.

### The diagnostics each pair owed

Eight snapshots existed; seven were missing, all for pairs whose wrong shape
reported syn's `expected impl` instead of naming the sibling.

- `#[processor]`, `#[scheduled]`, `#[listeners]`, `#[indicators]` and `#[hooks]`
  each get their wrong-shape case — `schedule`, `events` and `health` had no
  diagnostics suite at all.
- MCP gets the two GraphQL and WS already had: the no-posture refusal and the
  `reject_http_only_layers` one. Both fixtures decorate the impl half only —
  pairing them with `#[mcp]` cascades a second error about the `ServerHandler`
  the refused expansion never wrote, pinning rmcp's internals in a snapshot whose
  job is one sentence of ours.
- That last snapshot caught a wording bug in the shared helper: the fifth caller
  passes `"operation"`, and the template said `on a {site}`.

### Every decorator is now witnessed, and the two exclusions are separated

`#[config]`, `#[queue]`, `#[processor]`, `#[indicators]`, `#[interceptor]`,
`#[redirect]` and `#[dataloader]` gained use sites in `nest-rs-macro-hygiene`,
which required adding `config`, `health` and `queue` to its single dependency's
feature list — the assertion being that each capability's feature pulls
everything its decorators emit. `#[dataloader]` reads as needing a data layer
because its intended use runs each batch through `Repo`; the macro only reads a
key type off the argument and a value type off the return, so it belongs where
the manifest is the assertion. Writing it surfaced a real constraint now
recorded there: `Loader::Error` is bound `Send + Clone + 'static`, one error
being handed to every caller waiting on a batch.

`#[crud]` and `#[expose]` stay out, but no longer for one reason. An entity
cannot live in a zero-dep crate — `DeriveEntityModel` roots its expansion at the
call site's `sea_orm` and offers no `crate = ` override, checked against
sea-orm-macros 2.0 rather than assumed. That excuses `#[expose]`, which sits on
an entity. It does not excuse `#[crud]`, which sits on a controller whose source
writes nothing but `std`, `nest_rs` and `crate::` — and that conflation is what
let the `uuid` emission ship.

So `#[crud]` gets the tree that can actually observe it:
`crud_needs_no_dependency_the_controller_does_not_name` generates a resource,
drops the auth modules from the module tree, the guards from the controller and
`uuid` from the manifest, leaving a crate whose only claim on `uuid` is whatever
the decorator emits. Reintroducing the bug fails it while
`a_generated_crud_resource_compiles` still passes — which is the measurement of
what the older test was worth here.

### A guard bound where it does not run is a compile error

Every `Guard::check_*` defaults to `Ok(())` — right as "this guard does not apply
to that transport", and also the reason this compiled, read as a protection, and
throttled nothing:

```rust
#[resolver]
#[use_guards(ThrottlerGuard)]   // ThrottlerGuard implements only check_http
pub struct PostsResolver { /* … */ }
```

The per-operation chain called `check_graphql`, got the default, and passed. The
fix is a bound rather than a boot check: it fails at the `#[use_guards]` line, and
needs no plumbing into the four places each transport composes its chain.

- **`GraphqlGuard` / `WsGuard` / `McpGuard`** in `nest-rs-guards`, each a marker a
  guard declares beside the `check_*` it attests. `#[resolver]`/`#[operations]`,
  `#[messages]` and `#[mcp]`/`#[tools]` emit one bound per declared guard;
  `#[diagnostic::on_unimplemented]` names the missing method and the two remedies.
  Three trybuild snapshots pin the wording.
- **HTTP has no marker.** `check_http` is the trait's base entry — the one method
  not behind a feature — so `HttpGuard` is blanket-implemented and the bound could
  never fail. Emitting it would prove something already true, per route, per guard.
- **A `#[gateway]`-struct guard is an HTTP guard**, and the bound is where that
  stops being folklore: those run on the upgrade, which is a `GET`. Only a
  `#[use_guards]` beside a `#[subscribe_message]` owes `WsGuard`.
- **It found a live one on the first run.** `demo`'s three GraphQL resolvers bound
  `#[use_guards(AuthnGuard, AuthzGuard)]`, and `AuthnGuard` has no `check_graphql`
  — on a resolver it ran nothing, authentication having always been the bridge's
  job in band. Dropped; the dependency lives on the bridge, which is what runs it.
  All 88 demo e2e tests pass unchanged, which is the proof it was inert.

### WebSocket messages declare a posture, and the mask comes with it

A `#[subscribe_message]` returning entity rows had to mask them by hand:

```rust
// before — the posture is an argument buried in a call nobody can `rg` for
async fn list(&self) -> Result<Value, ServiceError> {
    let rows = self.svc.list().await?;
    let wire = serde_json::to_value(rows.iter().map(User::from).collect::<Vec<_>>())
        .map_err(|e| ServiceError::Masking(e.to_string()))?;
    masked_reply::<UserEntity>(Action::Read, wire)
        .map_err(|e| ServiceError::Masking(e.to_string()))
}

// after — `#[messages]` emits the gate and the mask
#[subscribe_message("users.list")]
#[authorize(Read, UserEntity)]
async fn list(&self) -> Result<Vec<User>, ServiceError> {
    Ok(self.svc.list().await?.iter().map(User::from).collect())
}
```

Two things were wrong with the old shape, and the second is the serious one. The
posture was an `Action::Read` argument rather than a greppable `#[authorize]`,
against this repo's rule that every authn/authz decision be findable at one of
three sites. And a handler that simply *forgot* the call shipped unmasked rows and
compiled — on GraphQL and MCP that is a compile error.

- **Breaking, and deliberately loud.** Every `#[subscribe_message]` now declares
  `#[authorize(Action, Entity)]` or `#[public]`; no posture is a compile error
  naming both forms. `#[public]` used to be *rejected* on a message (there is no
  anonymous fast-path to take on WS) — it is now the declaration that the posture
  was decided and the message is deliberately ungated.
- **`nest-rs-authz` gains a `ws` feature**: `ws::authorize` (the class gate, whose
  decision is the same shared `gate` GraphQL and MCP use) and
  `ws::masked_reply_for` (the reply mask). No `WsAbilityBridge` — a gateway is
  `EdgePosture::Guarded`, so its upgrade already ran the real HTTP chain; the
  ambient ability comes from `WsDataContext` as before.
- **A gate refusal and a guard refusal reach the client through one frame**, via a
  new `denial_to_ws_error` carrying `reason` and `requiredScopes` under
  `data.errors` — where every other structured rejection on this transport rides.
- **WS masks like HTTP, not like MCP.** GraphQL and MCP reconstruct the return type
  after masking, so a stripped required key refuses the operation. A WS envelope
  carries JSON and promises no schema, so the key is simply absent from the frame,
  exactly as the HTTP response shaper omits it from a body — a field-restricted
  caller reads a smaller object, and the reply type needs no `DeserializeOwned`.
  Fail-closed is reserved for a missing ambient ability and a body that cannot be
  reconciled with the entity at all.
- One compile rule, whose reason is *not* masking: a masked message returns a
  **literal** `Result<T, E>`, because the reply shape is decided syntactically and
  a `Result` behind an alias would be masked as the `Result` itself — a
  success-shaped frame carrying `{"Ok": …}`.
- Ordering per message is chain → gate → pipes → call → mask, so a caller the
  gate refuses never pays for validation.

### Every decorator pair names its sibling on the wrong shape

The rule shipped with the MCP pair; only GraphQL and MCP obeyed it.
`#[controller]`/`#[routes]` and `#[gateway]`/`#[messages]` still reported syn's
`expected struct` — the error the rule exists to forbid, since the shape the
developer reached for does exist and is spelled with the other decorator.

- All six pairs now parse through one `DecoratorPair` const per edge, read by
  **both** halves, so the two sentences cannot drift into naming a decorator that
  no longer exists. Eight trybuild snapshots (two per pair) pin the wording.
- The impl-half decorators whose struct half is the generic `#[injectable]` —
  `#[processor]`, `#[scheduled]`, `#[listeners]`, `#[indicators]`, `#[hooks]` —
  get the same treatment through `DecoratorPair::on_provider`, naming
  `#[injectable]` and the methods they collect instead of `expected impl`.
- The posture grammar is deduplicated the same way: one `PostureRules` in
  `nest-rs-codegen`, adopted by `#[tools]` and `#[messages]`. GraphQL keeps its
  own parser, which carries `bind = Service` / `id_arg` that no other transport
  can express.

### One decorator, one item shape

`#[resolver]` and `#[mcp]` each answered to two item shapes — the struct and its
`impl` — and nothing else in the framework did. An attribute macro is a single
path in the macro namespace: the shape is discriminated *after* `syn::parse`, so
the name bought one rustdoc page for two argument grammars, one symbol for
go-to-definition, and the same `in this expansion of #[mcp]` note on every error
whichever half emitted it. `rg '#\[mcp\]'` could not tell a host from an
operations block either — and this repo's security rules lean on exactly that
kind of grep. Both are now pairs, like every other edge:

| Edge | on the struct | on the impl |
|---|---|---|
| HTTP | `#[controller(path)]` | `#[routes]` |
| WS | `#[gateway(path)]` | `#[messages]` |
| GraphQL | `#[resolver]` | **`#[operations]`** |
| MCP | `#[mcp]` | **`#[tools]`** |

```rust
#[mcp]
#[use_guards(TenantGuard)]
#[derive(Clone)]
pub struct UsersTool { #[inject] svc: Arc<UsersService> }

#[tools]                            // was: a second #[mcp]
impl UsersTool { /* #[tool] / #[prompt] methods */ }
```

- **Breaking, with no shim.** Rename the impl-block attribute and add the
  sibling to the import: `use nest_rs::graphql::{operations, resolver};`,
  `use nest_rs::mcp::{mcp, tools};`. `#[crud]` is unchanged — it still stands in
  for the impl-form decorator and now re-emits under `#[operations]`.
- **The wrong shape names its sibling.** `#[mcp]` on an impl does not report
  "expected struct"; it says the methods go under `#[tools]`, and vice versa.
  Four trybuild snapshots pin the wording.
- **The impl half is named for what it collects**, the way `#[routes]` and
  `#[messages]` are. `#[tools]` carries `#[prompt]` methods too: rmcp routes both
  through the one `ServerHandler` the expansion writes, so they are one host's
  operations. MCP keeps the protocol's name on the struct — the role word moved
  to the impl half, where a host's methods are.

The rule is now in the hard "no" list, with a testable form: an entrypoint in a
`*-macros` crate matches at most one `Item::` variant.

### MCP operations take the request layers every other edge has

`#[mcp]` mounted on the HTTP transport from the start, but an operation was the
one handler in the framework that could not declare anything about itself: no
guards, no access posture, no pipes. A tool wanting validation wrote
`params.validate().map_err(…)?` by hand — the inline edge conversion the layer
rules call drift — and a tool returning entity rows shipped them past a mask
that was never armed. That is closed: an MCP operation now declares the same
things a `#[query]` does, expanded in the same order.

```rust
#[mcp]
#[use_guards(TenantGuard)]          // host scope, like a #[controller] / #[resolver]
#[derive(Clone)]
pub struct UsersTool { #[inject] svc: Arc<UsersService> }

#[tools]
impl UsersTool {
    /// List the people the caller may see.
    #[tool]
    #[authorize(Read, UserEntity)]                   // class gate + response mask
    async fn list_people(
        &self,
        Parameters(page): Parameters<Valid<PageDto>>, // validated before the body
    ) -> Result<Json<Vec<User>>, McpError> { /* … */ }
}
```

- **Posture is mandatory, and that is a breaking change.** A `#[tool]` /
  `#[prompt]` carrying neither `#[authorize(Action, Entity)]` nor `#[public]` is
  a compile error, for the reason it already was on a `#[query]`: an operation
  nobody thought about must not ship ungated and unmasked to a language model.
  Every existing host needs one line per operation.
- **Guards bind per host and per operation.** `#[use_guards(...)]` on the struct
  and beside an operation compose into one chain per site, memoized per app in
  the `SiteChainCell` GraphQL already used — extracted rather than copied, since
  this is the second in-band transport to need it. `Guard` grows `check_mcp`,
  whose context carries the app and which operation is running; the caller's
  `Ability` is ambient, so a capability-only guard reads it the same way it does
  on HTTP.
- **The edge checks the request; the chain checks the operation.** `/mcp` gates
  the HTTP request at its `McpOperationGuard` and the operation in band, and the
  two are different questions — `check_http` against a request, `check_mcp`
  against an operation — so neither shortens the other. (This shipped first as a
  reported-and-subtracted set, `McpOperationGuard::already_ran`; *Registering a
  global guard could open `/mcp` instead of closing it* above is what replaced
  it, in the same release. That method is not part of 4.0.0.)
- **Pipes reach the fifth transport.** `Parameters<Valid<T>>` /
  `Parameters<Piped<P, T>>` expose `T` as the operation's JSON Schema — a client
  never sees the carrier — and a rejection is `invalid_params` (`-32602`)
  carrying the field errors, which is the one MCP error a model can act on.
- **Response masking is automatic, and fails closed.** `#[authorize]` masks the
  returned value through the caller's field grants, `Json<T>` unwrapped and
  rewrapped so the structured content is what gets masked. MCP has no selection
  set to excuse a stripped **required** field the way GraphQL does, so such a
  mask refuses the operation rather than serving the row; `unmasked` is the
  opt-out for a deliberate projection. `#[authorize]` on a `CallToolResult` is a
  named compile error pointing at it, rather than a trait bound failing inside
  the expansion.
- **`bind = Service` has no MCP form, and says so.** A resolver exposes named
  scalar arguments a wrapper can turn into an id; an operation takes one
  `Parameters<T>` struct no macro may reach into.
- **The authored method keeps its signature.** The expansion emits a delegating
  wrapper carrying `#[tool(name = "…")]` rather than rewriting the body, so a
  unit test still calls the method directly, the wire name stays the authored
  one, and a compile error about the body points at the body.

`nest-rs-macro-hygiene` now consumes the guard chain and the pipe carrier, still
on its single `nest-rs` dependency — the compile-time proof that none of this
puts a second line in a host's manifest.


### A tool host writes one decorated `impl`

MCP was the only edge in the framework that made a developer write three blocks
and name a foreign trait. `#[mcp]` now decorates the `impl` too — the same name
as on the struct, because it is one concern and because a `#[tools]` sitting a
letter from the `#[tool]` beneath it would read as a typo.

```rust
#[mcp]
#[derive(Clone)]
pub struct UsersTool { #[inject] svc: Arc<UsersService> }

#[mcp]
impl UsersTool {
    /// List the people the caller is allowed to see, by name.
    #[tool]
    async fn list_people(&self) -> Result<String, McpError> {
        let rows = CrudService::list(&*self.svc).await.opaque()?;
        Ok(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join("\n"))
    }
}
```

Gone from the file: `#[tool_router]`, `#[prompt_router]`, `#[tool_handler]`,
`#[prompt_handler]`, `impl ServerHandler`, `get_info`, `ServerCapabilities`,
`ServerInfo`, `description = "…"` — and `use nest_rs::mcp::rmcp;`, the import
whose only job was someone else's macro hygiene and which the CLI template
shipped with three lines of comment explaining it.

- **`use rmcp;` is gone for good, not hidden behind a prelude.** The expansion
  emits rmcp's macros inside a private child module that carries the import.
  Two Rust facts make it sound, both asserted in
  `nest-rs-mcp/tests/integration/mcp_impl.rs`: an inherent impl may live in any
  module of the defining crate and still reach the parent's private fields, and
  an item's own visibility — not the module it sits in — decides who may name
  it. `#[tool]` and `#[prompt]` reach `#[mcp]` as inert tokens, so they need no
  import either.
- **The second fact is load-bearing.** rmcp generates `tool_router()` without
  `pub`, so reading it from the parent silently yields an empty tool list and
  the duplicate-tool boot check would go blind. The expansion emits its own
  `pub(crate)` accessor; `DefaultToolRouter` is replaced by
  `DefaultDeclaredTools`.
- **Capabilities are derived from the operations present.** A `#[tool]` method
  advertises `tools`, a `#[prompt]` method `prompts`, and nothing claims a
  surface no method serves. This closes a silent defect the CLI template itself
  shipped: every scaffolded host declared `impl ServerHandler for T {}`, routing
  tools it never advertised, so a client reading capabilities could decide never
  to ask.
- **Descriptions come from the doc comment.** The sentence was written twice —
  once for a reader, once for the model — and two copies of one sentence drift.
- **`Opaque::opaque` carries the log-real/return-opaque posture.** A tool body
  talks to a language model, and a `DbErr`'s `Display` carries schema, column
  names and sometimes values. Three features had hand-rolled the same four
  lines; it is one call now, and the real error still reaches the operator at
  `error` on `nest_rs::mcp`. A deliberate `McpError::invalid_params` is returned
  directly and never routed through it.
- **The escape hatch is explicit.** A host serving a hand-written
  `ServerHandler` surface (resources, completion) cannot have a second one
  generated beside it, so it stays on rmcp's raw shape; `#[mcp]` on a trait impl
  is a compile error saying so. `demo`'s `posts` is that host, kept deliberately
  as the witness of the raw form.

### The endpoint is declared where the tools are

An MCP host's `path` is now optional, and every argument that says what the
endpoint *is* moved onto the decorator. A feature contributing tools to the
app's server writes a bare `#[mcp]`; one that wants its own endpoint writes the
whole URL path.

```rust
#[mcp]                                                  // → /mcp
#[mcp(path = "/mcp/posts", name = "assistant-posts")]   // → /mcp/posts
```

- **The path is a join key, not a namespace.** Unlike a `#[controller]`'s,
  nothing nests under it: it names the one endpoint the host joins, which is why
  peers writing the same path share it. So it is written whole, the way a client
  config carries it, and `nest_rs_mcp::DEFAULT_PATH` (`/mcp`) is what a bare
  `#[mcp]` takes — a constant, not configuration, because no other mount path in
  the framework is settable from the environment either.
- **Two spellings of one mount are one owner.** `Route::nest` appends a trailing
  `/` internally, so `/mcp` and `/mcp/` collapse to one key inside poem and route
  assembly panics — while `claim_exclusive_path`, written to keep exactly that
  from reaching poem, compared the two raw strings and saw two owners.
  `HttpEndpointMeta::new` now normalizes, so every self-mount is canonical before
  anything compares it: MCP, WS, OpenAPI, and GraphQL, whose path is deployment
  input (`NESTRS_GRAPHQL__PATH`) rather than a literal an author controls. New
  public surface on `nest-rs-http`: `normalize_mount_path`.

- **Identity moved to its two real owners.** The app declares *itself* once,
  through `McpOptions { server: Some(McpIdentity::new(name, version)) }` — its
  `name`, `version`, branding, and the `instructions` a client may fold into the
  model's system prompt. All of it is the app's because a shared feature library
  knows neither the deployment's version nor, on an endpoint several features
  share, what the whole surface is. A host declares only which endpoint stands
  apart — `#[mcp(name = …, title = …)]`, each overriding the app's per field.
  **`version` is not among them**: `#[mcp(version = …)]` is a compile error
  naming the app's seam, for the same reason the rest of the identity is the
  app's.
- **`instructions` is not a `#[mcp]` argument**, and writing one is a compile
  error naming the seam that takes it. It describes the *server*; what each tool
  does is already carried by its own `#[tool(description = "…")]`. Hosts that
  write instructions through `ServerHandler::get_info` are still joined when the
  app declares none — a fallback, not a second spelling.
- **The per-path `McpOptions::endpoints` list is gone**, and with it
  `McpEndpoint` and `declared_endpoint`. An app no longer restates, in its
  composition root, a path its features already declare.
- **Two boot errors, both naming what to fix**: two hosts on one path each
  declaring an identity, and a host that names its endpoint in an app that
  never named itself (a name with no version behind it). An identity declared
  in an app with no `#[mcp]` host at all still fails boot.
- **One boot `warn`, down from two**: an endpoint neither its hosts nor the app
  named reports rmcp's own build identity, which is what a client would read.
- Public surface: `McpIdentity`, `ResolvedIdentity`, `endpoint_identity`,
  `DEFAULT_PATH`, `McpOptions::server`.
- Migration: `#[mcp(path = "/mcp")]` becomes `#[mcp]`; any other path is
  unchanged; an `endpoints` entry becomes either `server` (the app's name) or
  a distinct `name` on the host.

### One seam per module: `for_root` takes one value

Configuring a module had grown a second spelling. `McpModule` let you chain
`for_root(None).endpoint(..)`, and added `McpModule::endpoint(..)` to soften
the `None` — three ways to write one import, where every other module had
exactly one. `StorageModule` had drifted the other way: it owned a `#[config]`
with no `for_root` at all, so its config was reachable from the environment
only.

The rule is now written down (`.claude/rules/framework.md`, *`for_root` — one
seam, one value, no chain*): **a module is configured in exactly one place, by
exactly one value.** The `DynamicModule` it returns is opaque — no public
method on a `*Setup`, no second constructor on the module type. A declaration
that does not fit into the argument makes the argument grow a *field*, never
the seam a *method*.

- **`McpModule::for_root` takes an `McpOptions`.** Server options and endpoint
  identity travel together:

  ```rust
  McpModule::for_root(McpOptions {
      server: Some(McpIdentity::new("assistant", env!("CARGO_PKG_VERSION"))),
      ..Default::default()
  })
  ```

  `McpModule::endpoint(..)` and `McpSetup::endpoint(..)` are gone. An
  `McpConfig` still converts into `McpOptions`, so a call site that pins only
  server options is unchanged.
- **`StorageModule::for_root(config)`** — the missing half of the dual-path
  rule. Importing the bare `StorageModule` still declares the dependency and
  leaves the config to `NESTRS_STORAGE__*`.
- **`nest_rs_throttler::provide_guard` / `resolve`** are `#[doc(hidden)]` —
  cross-crate seams for the store backends, never an app-facing way in.
- **`ConfigSetup<M, C>`** is the shared `DynamicModule` behind a `for_root`
  whose whole job is "pin the config, then recurse": `WsSetup` and
  `StorageSetup` are now aliases for it. Built by `ConfigModule::setup` rather
  than a `ConfigSetup::new`, so a shared setup stays as opaque as a
  hand-written one. Modules whose `collect` queues a pool or whose `register`
  does more than recurse keep their own type.

### `for_root` configures, `for_feature` registers

The two seams had started to overlap, and an overlap here is not cosmetic: two
factories for one config type resolve by import order, silently. `imports =
[AudioModule, StorageModule::for_root(cfg)]` dropped `cfg` on the floor,
because `AudioModule` imports `StorageModule` and queued the environment-only
factory first. No warning, and the app ran on defaults.

nestrs now takes NestJS's split literally — `forRoot` configures a module,
`forFeature` registers against an already-configured one — and enforces it:

- **`ConfigModule::for_feature::<C>()` takes no value.** It declares that `C`
  must load; the module that owns `C` is where a base is pinned. A config
  reachable through two seams is a config whose value depends on import order.
- **A pinned base supersedes a bare import's factory**, wherever the two fall
  in `imports`. New `ContainerBuilder::provide_declared_factory` carries the
  distinction.
- **Two declarations for one type fail the boot** naming it
  (`ContestedDeclarationError`), instead of letting position decide. Not
  config-only: `ThrottlerModule` and `RedisThrottlerModule` both bind
  `Arc<dyn ThrottlerStore>`, so importing both is now a named failure rather
  than whichever `imports` listed first. Here we exceed NestJS deliberately,
  which lets the last registration win in silence.
- **The synchronous `App::new` refuses what it cannot resolve.** It runs
  `register` but never the factory phase, so a `Module::for_root(cfg)` used to
  boot with the config simply absent — visible only as a `None` at first read.
  It now runs a dynamic import's `collect` (which is what made
  `ConfigModule::for_root()` register `Environment` on that path at all) and
  fails with `UnresolvedFactoryError` when a queued factory would never run.
- **The `for_root` obligation stays scoped to `nest-rs-*`**, as the dual-path
  rule always said. The split is who can edit the struct: you cannot touch
  `HttpConfig::default`, so `HttpModule::for_root(cfg)` is your only in-code
  path; your own `IssuerConfig` already has one in its `impl Default`. A
  product's feature module writes `for_feature` and no seam.

The ownership table lives in `architecture.md`, which every session loads and
which the CLI embeds into each scaffolded project's `AGENTS.md`.

Several module docs claimed a pinned config "wins over the environment". It
does not: the deployment's real environment outranks a pin, which only
outranks the `.env` cascade and the defaults. `seaorm`'s went further and
offered the pin as a test hatch — that is the *seed*
(`App::builder().provide(cfg)`), the one tier nothing overrides. Ten sites
across `seaorm`, `graphql`, `openapi`, `throttler`, `redis` and the docs now
match the precedence table.

The rule has a converse, and `SocialModule` is what made it explicit: **a
module that owns no `#[config]` gets no `for_root`.** A social provider carries
its own config and its registry entry names it, so *discovering* a provider is
what loads its credentials — the module never learns which providers exist, and
has nothing to be configured about. `SocialModule` therefore stays a bare
import: giving it a seam would force a list of mutually unrelated config types,
hence type erasure, hence no duplicate detection and a hand-written `Debug` to
keep a client secret out of the format — all to declare something discovery had
already handled.

`OpenTelemetry::init_with(config)` stays the one recorded exception: the
global tracer must exist before any module registers, and its guard's `Drop`
flushes, so it belongs to `main`.

## [3.1.0] - 2026-08-07

### An MCP endpoint aggregates several features

`#[mcp(path = "/mcp")]` used to mean *this struct owns that URL*. Hosts that
declare the same path now merge into one endpoint, so a product exposing several
domains over MCP keeps one `mcp/` adapter per feature instead of folding them
into a single cross-domain host.

- **Why it had to change.** MCP namespaces tools per endpoint and every shipped
  client config points at a single URL. One host per path therefore made the
  framework's own layout rule — one adapter sub-folder, one `<Feature><Edge>Module`
  per transport — impossible to follow for any product with more than one domain
  to expose. That was a framework defect, and it is closed.
- **Nothing changes for a host.** It stays a plain `ServerHandler` and never
  learns that it shares. A path with a single host is served **verbatim** — the
  merge only engages beyond one.
- **What merging means, per operation.** `tools/list`, `prompts/list`,
  `resources/list` and `resources/templates/list` are the union of every host on
  the path; `tools/call`, `prompts/get`, `resources/read`, `tasks/*` and custom
  methods are routed to the host that owns the name; `logging/setLevel` and every
  notification are broadcast; declared capabilities are unioned and protocol
  versions intersected.
- **One new failure mode, and it fails boot.** Two hosts on one path serving the
  same tool name is a boot error naming the tool and both providers — MCP
  addresses a tool by bare name within an endpoint, so the loser would silently
  be unreachable, and which one lost would depend on registration order.
- **Distinct paths stay distinct.** Grouping is by path, so an app that
  deliberately serves two endpoints keeps their tool namespaces apart.
  `demo/apps/assistant` mounts `audio` + `users` on `/mcp` and `posts` on
  `/mcp/posts`.
- **Module-gating is unchanged and structural**: a host whose module the app does
  not import contributes nothing, because metadata is attached from `register`.
- New public surface on `nest-rs-mcp`: `McpHost` (the object-safe `ServerHandler`
  view a host is merged through), `CompositeHandler`, `McpHostMeta`, `hosts_on`.
  New on `nest-rs-core`: `ContainerBuilder::attached_meta`, the mid-build read of
  the metadata index a surface needs to attach an aggregated mount exactly once.

### The app names its MCP endpoint

An MCP endpoint is one server to every client that reaches it — the protocol
carries one `serverInfo` and one `instructions`, whatever the endpoint is made
of. That identity is now the app's to declare:

```rust
McpModule::for_root(McpOptions {
    server: Some(McpIdentity::new("acme-assistant", env!("CARGO_PKG_VERSION"))),
    ..Default::default()
})
```

- **This is the ecosystem's shape**, not an invention: the TypeScript SDK creates
  one server *with* its identity and registers tools onto it, and a FastMCP
  parent "retains its own name and serves as the orchestrator" when it mounts
  children. A `#[mcp]` host owns a feature, so on a shared path none of them can
  speak for the endpoint.
- **Identity is declared; capabilities are observed.** The declaration replaces
  what it states — `serverInfo` always, `instructions` when written — and can
  never claim a capability no host serves. Omit the instructions and the hosts'
  own are joined rather than dropped.
- **Optional and additive.** A lone host that names itself is a complete server
  and nothing changed for it.
- **Two boot `warn`s where the answer used to be silent**: a shared endpoint
  nobody declared reports its *first* host's identity (a function of
  `imports = [..]` order), and an endpoint no one named at all reports **rmcp's
  own** name and version — `ServerInfo::new` leaves the SDK's build identity in
  place, so an unnamed nestrs endpoint has been introducing itself to clients as
  `rmcp`. Both events carry the remedy.
- **Two boot errors**: an identity declared for a path no `#[mcp]` host serves
  (a typo that would otherwise do nothing at all), and two declarations for one
  path that disagree.
- New public surface on `nest-rs-mcp`: `McpIdentity`, `ResolvedIdentity`,
  `endpoint_identity`.

### The invariant behind it

**A transport aggregates contributions from several providers onto one mount
point; owning a whole mount is the exception and has to be justified.** Six
transports already honoured it. MCP was the exception and now aggregates; WS was
audited and keeps its per-gateway mount deliberately — nothing pushes a product
to share a socket path the way MCP clients push it to share a URL, and
cross-gateway fan-out is already `WsServer<N>`'s job.

### A new project ships the architecture rules it is built on

`nestrs new` now writes `AGENTS.md` at the project root, plus a `CLAUDE.md` that
imports it. Both are committed, so the layout and naming conventions reach every
contributor — and every coding agent — without anyone having to look them up.

- **Why the scaffold carries them.** A tree of four files says nothing about the
  fifth. A project that has to re-derive where a second service goes, or what to
  call a provider that is not a service, derives it differently each time; the
  conventions were reachable only by reading the framework's own repository.
- **What the file states.** Four naming levels and the rule that none overflows
  into the next (the project's name stops at the workspace); the two module files
  and their two jobs, with `mod.rs`'s `pub use` list named as the export contract
  the framework has no `exports` key for; a three-question procedure for naming
  any provider, since `#[module]` carries no `controllers` list and the name is
  therefore the only thing that says what a type is for; the role table; what
  happens when a role repeats; and the structural words a module may not take.
- **One source, three readers.** The text lives once, in
  `nest-rs-cli`'s templates: the CLI embeds it, this repository symlinks it into
  `.claude/rules/`, and `/architecture/` restates its two tables under a docs-lint
  check that fails when they drift. Two agent files were disagreeing about the
  same rule before this; now they cannot.
- **Two shapes, one body.** The crate-type table describes `apps/` plus
  `crates/features`, so it belongs to the workspace layout header and is absent
  from a standalone project — doctrine about a layout the reader does not have is
  worse than none.
- `--env-prefix` reaches the new files like every other artifact that names a
  variable, and the span-target example is rooted at the crate that would emit
  it: the shared feature library in a workspace, the app's own name standalone.

### Documented — the conventions have a page

`/architecture/` carries the model for a reader rather than for a generator, with
the reasoning the shipped file leaves out. It is promoted in `llms.txt`, so a
coding agent that finds the site reads the layout rules before anything else.

## [3.0.0] - 2026-08-04

### The env prefix is the deployment's, and it is one variable

2.1.0 made the prefix the application's, declared in source with
`nest_rs::env_prefix!("ACME")`. It is now the *deployment's*, set on the
process like everything else it governs:

```yaml
environment:
  NESTRS_ENV_PREFIX: ACME
  ACME_SEAORM__URL: postgres://…
```

**Breaking.** `env_prefix!` and `EnvPrefixDecl` are removed — no shim. A project
on 2.1.0 deletes its declarations and sets one variable instead.

- **One write, and everything downstream follows.** The link-time declaration
  had to be repeated per binary — the `migrations` and `seed` tools link neither
  the feature crate nor each other — and a binary added later silently resolved
  `NESTRS_*` against an `ACME_*` cascade. A variable on the process reaches every
  binary, every test and every container without being written anywhere twice,
  and the CLI reads the same variable from its own environment, so
  `[workspace.metadata.nestrs] env-prefix` is gone too: there is no second source
  left to disagree.
- **`.env` cannot carry it, and saying so is enforced.** The prefix selects the
  cascade, so a value inside that cascade arrives after it was needed. It would
  have renamed nothing, silently; `Environment::init` now aborts naming both
  values instead.
- **A malformed value aborts on first read** rather than falling back to
  `NESTRS` — which would be just as wrong, and quiet. The shape is unchanged
  (uppercase ASCII, digits, underscores, no trailing `_`); the check simply moved
  from compile time to the first read.
- **`nestrs new --env-prefix ACME`** now writes the variable into the generated
  `Justfile` and `Dockerfile` — the processes the project starts — and the `.env`
  cascade under the new names. **`nestrs doctor` reports where the prefix came
  from**, and says plainly when the shell names none, because a project whose
  deployment renames its variables looks untouched from a terminal that does not.
- **`NESTRS_ENV_PREFIX` is spelled literally**, joining `RUST_LOG` and
  `NESTRS_NO_BOOTSTRAP` as a name that is not the app's. It is the one name no
  prefix can rename.

Existing apps that never renamed anything are unaffected: setting nothing keeps
`NESTRS`.

### Fixed — a created `Location` named a path that was not the caller's

The `Location` a `#[crud]` create stamps was read off poem's
`original_uri()`, which the hyper path populates and `Request::builder()` does
not: the same route answered `/orgs/<id>` on the wire and `/<id>` under
`TestClient`, so an in-process witness failed on a route that was correct.
Reading `uri()` instead only moves the wrongness — a global prefix is mounted
with `Route::nest`, which strips itself off before the handler runs, so the
header would name a path that `404`s on that very app.

The edge now captures the URI once, after canonicalization and before the
router sees it, and `nest_rs_http::caller_path` reads that capture — one value
in process and on the wire. A trailing slash resolves to the canonical spelling
rather than being echoed back, so a resource has one `Location` whichever
spelling was typed. `nest-rs-http`'s global-prefix suite is the executed
witness for both.

## [2.1.0] - 2026-08-04

### The env prefix is the application's

`NESTRS_` was a fixture; it is now a default. An app declares its own once and
every framework variable follows — `ACME_ENV`, `ACME_LOG`, `ACME_HTTP__PORT`,
`ACME_SEAORM__URL`:

```rust
nest_rs::env_prefix!("ACME");
```

- **A link-time declaration, not a setter.** The prefix is read before anything
  else — `<PREFIX>_ENV` selects the `.env` cascade, `<PREFIX>_LOG` configures
  the subscriber before `main` has built anything — so a setter would carry an
  ordering rule nobody can verify. `inventory` makes the declaration a fact
  already true at the first read, wherever in the binary it is written.
  Resolution caches into a `OnceLock<&'static str>`: no allocation added to any
  path that was allocation-free.
- **It is a rename, not an alias.** Once declared, `NESTRS_HTTP__PORT` is inert.
  A fallback would let a stale value silently win.
- **Every name is built from it**, including the ones outside the
  `<PREFIX>_<DOMAIN>__<KEY>` scheme: `<PREFIX>_ENV`, the three `<PREFIX>_LOG*`
  variables, and the OpenTelemetry namespace. `RUST_LOG` keeps its name — it is
  the ecosystem's, not ours.
- **Error messages name the variable the operator actually has.** Every
  hardcoded `NESTRS_AUTHN__SECRET`-style literal in authn, redis, seaorm and the
  test harness now goes through `nest_rs_config::var_name`, the readerless
  primitive `ConfigService::var_name` delegates to.
- **`nestrs new <name> --env-prefix ACME`** writes the declaration, the `.env`
  cascade under the new names, and a `[workspace.metadata.nestrs] env-prefix`
  entry — which is how `nestrs doctor` and `nestrs g auth` learn the project's
  names instead of assuming ours. `doctor` reports the prefix it resolved.
  One declaration per generated *binary*: the `migrations` and `seed` tools link
  neither the feature crate nor each other, so `nestrs run db up` would
  otherwise resolve `NESTRS_*` against an `ACME_*` cascade.
- **Compile-time shape check** (uppercase ASCII, digits, underscores, no
  trailing `_`), and two conflicting declarations in one binary abort at boot
  naming both sites.

Existing apps are unaffected: declaring nothing keeps `NESTRS`.

## [2.0.0] - 2026-08-03

**One dependency.** `nest-rs` with the feature for the capability becomes the
whole install, on every page of the documentation and every crate's crates.io
landing page. A decorator's expansion no longer obliges the developer to declare
anything.

**Breaking.** An app that names the `nest-rs-*` crates directly keeps compiling,
but a decorator used from such a crate now roots its expansion at the umbrella
when one is present. The documented path is the umbrella; the sub-crates remain
published compilation units.

### The install contract

- **`cargo add nest-rs --features <capability>`** — 17 of the 19 module pages
  now install in exactly one line. `/database/` went from 7 to 1, `/graphql/`
  from 6 to 2, `/configuration/` from 2 to 1.
- **The `validator` version pin is gone.** `#[config]` carries the `Validate`
  derive and points it back at the framework's own copy, so no `#[config]`
  struct declares `validator` or keeps a major aligned. `validate = "manual"`
  opts out for a config that validates across fields.
- **`#[expose]` carries the derives it generates** — `serde`, `schemars`,
  `validator` — each routed with a `crate = ` override, alongside the
  entity-site trio (`sea_orm` / `uuid` / `chrono`). An entity crate declares
  none of them.
- **`#[input]` is the wire-DTO shorthand on every transport**, re-exported from
  `nest_rs::{ws, queue, mcp}`. A typed payload needs no `serde` of its own.
- **`features = ["full"]`** for an app that does not want to choose yet.
- **A missing dependency now says what to add**, with a copy-pasteable line,
  instead of `E0433: cannot find nest_rs_core` blamed on the attribute.

### MCP is the whole protocol now — rmcp 2.2 → 3.1

**Breaking, and the reason it ships in a major.** rmcp 3.x is a new major of the
SDK whose types appear in the signatures `#[mcp]` hosts write: SEP-2663 tasks,
SEP-2575 discovery and subscriptions, SEP-2549 cache hints, SEP-2322 multi-round
tool responses, SEP-2243 standard HTTP headers, and the `stateful_mode` →
`legacy_session_mode` rename.

- **Every MCP capability now gets the framework's transparent security**, not
  `tools/call` alone. rmcp 3.x retired the single `Service::handle_request` seam
  the old wrapper hooked and bounded its server on `ServerHandler`, so
  `PropagatingHandler` delegates the **whole** trait: the request scope, the
  caller's ability and the operation's transaction are installed around
  `prompts/get`, `resources/read`, `completion/complete`, `logging/setLevel`,
  the `tasks/*` trio, custom methods and the protocol lifecycle. Two documented
  exceptions: notifications (nothing to commit) and `subscriptions/listen`
  (outlives any sane transaction — it gets scope and ability, no transaction).
  A method left undelegated would have reverted to an SDK default and answered
  *for* the host, so `propagate.rs` drives all 27 over a real
  streamable-HTTP endpoint and fails if one goes missing.
- **Prompts have decorators.** `#[prompt_router]` / `#[prompt]` /
  `#[prompt_handler]` are re-exported beside the tool trio and stack on one
  `impl ServerHandler`. Resources stay hand-written methods — a resource surface
  is a URI-to-row mapping, not a set of methods.
- **The protocol is re-exported wholesale** — `nest_rs::mcp::{model, service,
  handler, transport}` plus the `rmcp` escape hatch, now documented rather than
  `#[doc(hidden)]`. A capability a future rmcp adds is reachable the day it
  ships, with no framework release in between.
- **`McpConfig` + `McpModule`** expose the streamable-HTTP options on the
  dual-path rule (`NESTRS_MCP__*` over a pinned base). `McpModule` configures;
  it activates nothing — listing the `#[mcp]` provider is still what mounts an
  endpoint. A registered `dyn SessionStore` is picked up for rmcp 3.x
  cross-instance session recovery.
- **Security note for existing deployments.** rmcp 3.x validates the inbound
  `Host` header against a **loopback-only** allowlist by default
  (anti-DNS-rebinding). A server reached under a real hostname answers `421`
  until `NESTRS_MCP__ALLOWED_HOSTS` names it. That default is deliberately not
  widened by the framework.
- **`rmcp` leaves every consumer manifest.** Its macros expand to bare `rmcp::`
  paths resolved against the *call site's* scope, so `use nest_rs::mcp::rmcp;`
  supplies the name — the claim that a re-export "cannot supply it" was wrong,
  and the entry is gone from `demo/crates/features`, `nest-rs-authz` and
  `nest-rs-testing`. `nest-rs-macro-hygiene` now compiles a full tools **and**
  prompts host, with typed input, on its single `nest-rs` dependency.
- `endpoint_with_guard` is replaced by `endpoint(McpMount, factory)`;
  `McpMount::from_container` is the one place the mount resolves its guard, data
  context, config and session store. `OperationOutcome` carries a type-erased
  `OperationValue`, which is what lets one `around` wrap every capability.

### The server half of OAuth discovery — RFC 9728

An app protected by a bearer token could refuse a caller but never tell it where
to go get one. The MCP authorization spec makes that a **MUST**, and HTTP and WS
are resource servers on exactly the same terms, so the capability lives in
`nest-rs-authn` and serves all of them at once.

- **`OAuthResourceModule::for_root(..)`** serves
  `GET /.well-known/oauth-protected-resource` (RFC 9728 §3) and stamps
  `WWW-Authenticate: Bearer resource_metadata="…"` — plus `scope` when the
  deployment advertises one — onto every `401` the process emits. The route is
  declared `#[public]`, because a client cannot hold a token before reading the
  document that says where to obtain it.
- **One seam, three transports.** The challenge is attached at the transport
  edge rather than inside `AuthError`, so it covers the guard-denial `401`, the
  WS upgrade refusal, `/mcp`'s in-band denial (`EdgePosture::Exempt` skips
  guards, not this band) and `401`s the framework never wrote itself.
  `/graphql` is the deliberate exception: it answers an unauthenticated
  operation with `200` + an `UNAUTHENTICATED` frame, so its clients discover
  through the well-known document — the equal alternative the spec defines.
- **`NESTRS_AUTHN__AUDIENCE` becomes mandatory** under this module, checked at
  `on_module_init` so import order cannot skip it, and boot fails naming the
  variable. Without it a resource server accepts any token its issuer signed,
  including one a user granted to another service — the confused deputy RFC 8707
  exists to close. A `resource` that disagrees with `aud` warns at boot.
- **`OAuthResourceConfig`** is dual-path (`NESTRS_AUTHN__RESOURCE`,
  `__AUTHORIZATION_SERVERS`, `__SCOPES_SUPPORTED`, … over a pinned base) and
  refuses a non-canonical identity at boot: no scheme, a fragment, an empty
  authorization-server list, or a scope carrying a space are all build breaks
  rather than a document that misleads a client.
- **`McpConfig::allowed_origins` is gone.** The browser `Origin` control was a
  second knob for something the HTTP transport already owns:
  `NESTRS_HTTP__CORS_ORIGINS` rejects a disallowed origin with `403` on every
  method, and the CORS layer wraps the whole route tree, so `/mcp` inherits it.
  Set the origin allowlist there; `allowed_hosts` stays, being the
  anti-DNS-rebinding control with no transport-wide equivalent.
- `crates/nest-rs-authn/tests/integration/resource/controller.rs` is the
  conformance proof, in the spirit of `propagate.rs`: a client that knows
  only a protected URL walks `401` → `resource_metadata` → the metadata document
  → the authorization server's own metadata, every hop a real request, and the
  AS is discovered by falling through the RFC 8414 §3.1 priority order.

### The client half of OAuth discovery — scopes and the step-up refusal

Discovery told a tokenless client where to get a token. It said nothing to the
client that *had* one and was merely delegated too little — a bare `403` whose
only recovery was guesswork. That is the case MCP made ordinary, so the scope
becomes a first-class dimension of a rule and of a denial.

- **`.requires_scope("posts:read")` on an ability rule.** One declaration,
  three effects, and no second decision site: the rule is **withheld** when the
  credential does not carry the scope — not added at all, so the class gate, the
  query pre-filter and the response mask refuse together exactly as for a rule
  nobody wrote — the refusal remembers the scope, and the scope stays readable
  beside the permission it conditions. Scopes **narrow, never widen**: an admin
  token minted without `posts:write` cannot write posts, and no scope grants
  what the role denies. Call it more than once to require all of them.
- **`PrincipalIdentity::scopes()`**, defaulted, is how a credential reports what
  it carries. `None` — every existing principal — means *not scope-aware*, so
  scoped rules apply in full and a session-authenticated app is untouched;
  `Some(&[])` means *an OAuth credential delegated nothing*. `AuthnGuard`
  publishes the result as `nest_rs_guards::GrantedScopes`, which is how authn
  informs authz without either crate depending on the other.
- **`nest_rs::authn::scope::space_delimited`** parses the RFC 6749 §3.3 `scope`
  claim — accepting the array form several authorization servers emit, because
  the deployment does not choose its AS's spelling. Getting this wrong by hand
  yields one scope named `"posts:read posts:write"` that matches nothing and
  looks like an authorization bug.
- **`Denial::InsufficientScope`** is distinct from `Forbidden` for the reason
  RFC 6750 §3.1 separates them: the first is actionable, the second final. It
  reaches the edge as `RequiredScopes` on the response, where the same
  interceptor that writes the `401` pointer renders
  `WWW-Authenticate: Bearer error="insufficient_scope", …, scope="posts:write"`
  for HTTP, WS and MCP alike. An ordinary `403` still carries no challenge —
  advertising a recovery that cannot succeed is worse than a plain refusal.
- **GraphQL stops being the transport that learns less.** It has no `401` to
  enrich, but a scope refusal is an ordinary error frame, so it carries
  `code: "INSUFFICIENT_SCOPE"` and a structural `requiredScopes` list.
- **`insufficient_scope_challenge` had no caller.** It was public, tested, and
  dead — the `403` half of the RFC was declared and never wired. It is now the
  single renderer of that challenge.
- A scope a rule requires but `NESTRS_AUTHN__SCOPES_SUPPORTED` omits is a dead
  end for the client; it is reported at `warn` (`reason="scope_not_advertised"`)
  at the one point both halves are known.

### Fixed — the poem `Err` path dropped a denial's evidence

`Error::from_response(denial_to_http_response(d))` reads as a faithful
conversion and is not: poem's `into_response` ends with
`*resp.extensions_mut() = self.extensions`, overwriting whatever the carried
response held. Every denial travelling the `Err` path — the MCP ability bridge,
the global-pool MCP fallback, the `Authorize` extractor — therefore reached the
edge stripped of its extensions, at the moment a client was being refused.
`nest_rs_guards::denial_to_http_error` is now the one conversion, and the trap
is documented at the function that replaces it.

### Fixed — the well-known document ignored a path-carrying resource

RFC 9728 §3.1 inserts the well-known string **between the authority and the
resource's path**, so `https://api.example.com/mcp` publishes at
`…/.well-known/oauth-protected-resource/mcp`. The document was hung off the
origin instead, which is the URL a *different* resource sharing that host would
claim. Both forms are now served — the path-aware one is advertised, the
unsuffixed one stays for bare-origin resources and clients that skip the
challenge — and a tail that is not this resource's path answers `404` rather
than asserting an identity the deployment does not have.

### Fixed — round-12 QA against the local 2.0.0

The last pass before the tag read the released pages and pasted them. Six
framework defects, each closed with the test that keeps it closed.

- **GraphQL was still the one capability that could not hold "one
  dependency".** `#[resolver]` wraps async-graphql's own `#[Object]`, and a
  third-party macro roots its expansion at whatever `proc-macro-crate` finds in
  the *call site's* manifest — falling back to a bare `::async_graphql`. So the
  lead snippet of `/graphql/` did not compile behind the documented install
  line, and the page said so instead of fixing it. Every async-graphql
  attribute and derive the framework emits now carries a `crate = ` override
  built from the re-export's own tokens rather than a re-typed string, so a path
  that stops resolving is a compile error rather than a silent fallback.
  `nest-rs-macro-hygiene` gains the `resolver` witness that fails the day the
  override is dropped. `async-graphql` returns to the ordinary rule: yours when
  *your* code writes its derives, and reachable as
  `nest_rs::graphql::async_graphql` when you only need its types.
- **`nest-rs-codegen` matched `crate = ` against a list of two derive names.**
  The next wrapped third-party macro would have fallen back to the call site's
  prelude unnoticed; it matches the shape at any position now, so a macro is
  covered the day it lands.
- **A half-wired tombstone is a boot refusal.** `#[expose(..., soft_delete)]` on
  an entity whose service never overrode `CrudService::soft_delete_column`
  answered `DELETE` with the same `204` a real tombstone answers, having
  destroyed the row — no warning at boot, none at the delete, and a wire
  response byte-for-byte identical to the successful case. `#[expose]` submits
  the entity/service pair at link time and `DatabaseModule` refuses boot naming
  both halves and both ways out. `audit_soft_delete_bindings` is public for an
  app composing the ORM some other way.
- **The scaffolded features crate declared neither `tracing` nor `anyhow`**, so
  the first `#[hooks]` method a reader pastes out of `/fundamentals/lifecycle/`
  failed on `E0433` — then failed again on `anyhow` once `tracing` had been
  added by hand. Thirteen pages write `tracing::` in feature code and none says
  to add it. Both ship from `nestrs new` now, standalone included, and an e2e
  compiles that page's snippet in a freshly generated tree. `g feature` also
  stopped printing a `Reference:` path that does not exist on the reader's disk.
- **An exception type needs `ResponseError` to reach a filter at all.** A filter
  claims by downcast off an error that is *already* a `poem::Error`, so the
  natural `Result<_, DomainError>` out of a handler did not compile — with an
  `E0277` on `IntoResult` naming neither the trait nor the default status it
  supplies. The suite only ever raised hand-built `poem::Error`s, the one shape
  that needs no impl, so it proved dispatch while never touching the requirement
  a reader meets first. The page now shows the impl, the handler, and what the
  same route answers with the filter unbound — the filter *replaces* that
  status, it does not create it.
- **The imperative-mount refusal had no proof it fires.** Nothing in the corpus
  reaches `HttpTransport::mount`, so no QA pass could trigger the release's one
  remaining fail-closed guard by following a page. `fail_secure` drives it and
  the two controls that must still boot, and `/http/configuration/` says what an
  imperative mount is — writing an application route is not how you get one.

On the pages: **24 `rust` blocks imported their types and dropped the
decorator** — `use nest_rs::openapi::OpenApiModule;` above a `#[module(...)]`,
which is `cannot find attribute` on the first build — and `/configuration/` and
`/http/configuration/` held seven between them, the pages a reader opens *to
copy a stanza out of*. Two `Layer` impls were missing where the surrounding
prose assumed them, one of them in a quote of a real framework file with the
line stripped. `/storage/` published five of `StorageConfig`'s seven keys under
a sentence calling the list exhaustive — the missing `ALLOW_HTTP` being the one
that decides a boot refusal — and printed the dev branch of a profile-split
default as *the* default, so a reader preparing a deployment concluded there
was nothing to pose.

`lint-docs.mjs` grows six checks so none of that recurs. Three **derive** their
rule from the framework's own source — every `#[proc_macro_attribute]` under
`crates/*-macros/`, every `pub trait <T>: Layer` under `crates/`, a `#[config]`
struct's fields — rather than restating it, because a hand-written list is
wrong the day a decorator or a sub-trait lands: the first cut of the `Layer`
check listed four sub-traits and missed `GlobalPipe`.

### Fixed — round-9 QA against the local 2.0.0

The same pass re-read the docs against the round-8 fixes. Two behaviours were
left to the owner to settle; both are settled the way the rules settle them —
the framework carries the concern, and there is one way to do it.

- **`#[crud]`'s `201 Created` carried no `Location`.** RFC 9110 §15.3.2 asks a
  created response to name what it created, and the generated route knows both
  halves: the collection the caller posted to and the primary key it inserted.
  It emits `Location: <collection>/<id>` — an absolute-path reference, because
  the only host a request carries is the `Host` header. Read from
  `original_uri`, so a global prefix and a version segment are in it; absent for
  an entity that does not key on a `Uuid`. The create handler now builds its
  response, so it declares its schema with `#[api(response = …)]` the way the
  paginated list already did — the document still advertises `201` + the entity.
- **…and the OpenAPI document declares it.** The header shipped and was
  described in prose, which no generated client reads — the same gap the
  throttler's `429` had already closed by declaring its `Retry-After`, left open
  on the success side. `HttpRouteMeta::sets_location` carries it, set by
  `#[crud]`'s create and by `#[redirect]` (whose response *is* a `Location`), so
  both producers are written rather than one. `required` is deliberately absent:
  a create omits the header for an entity that does not key on a `Uuid`.
- **A trailing slash answered `404`.** `/kitchen` served and `/kitchen/` did
  not, and that `404` came from the router — before the route's guards,
  interceptors and filters, so it read as a broken feature rather than as a
  spelling. The transport edge now trims the trailing slash before anything
  routes on it, and it is not configurable: a flag would be a second way to
  spell one path. Interior slashes are untouched (`/a//b` is a different path),
  a genuine `404` stays one, and the query string survives. It rides in the
  fused `EdgeEndpoint` rather than a `NormalizePath` middleware, so a canonical
  path costs one `ends_with('/')` test and no allocation.
- **The demo's own e2e disagreed with itself about the `201`.** The `create_org`
  fixture expected `201` while the test beside it still asserted `200` on the
  same route — a failure the round-8 change introduced and that no run caught.
  Both now assert the status *and* the `Location`.
- **`#[input]`'s published derive list was missing `Serialize`** — the derive
  that lets a wire DTO come back out as `Json<T>`, which is what the rewritten
  response snippets do. A reader trusting the list adds it by hand and gets
  `E0119` from a conflicting impl. The rustdoc, `/http/controllers/`,
  `/http/extractors/` and the decorator index all name four derives now, and a
  unit test reads the expansion's own `quote!` block back against the rustdoc so
  the two cannot drift again.
- **`nestrs run test cov` is the one recipe with a prerequisite the CLI does not
  bootstrap** — `cargo-llvm-cov` shells out to LLVM tools that a non-rustup
  toolchain does not have. The generated `test.just` and `/testing/` name
  `rustup component add llvm-tools-preview` and the `LLVM_COV` / `LLVM_PROFDATA`
  escape hatch; the generator's suite asserts the recipe still says so.
- **`/tutorial/entity/` printed a manifest the generator does not write.** The
  feature crate gets the dotted form and the `authn` feature (`g resource`
  bootstraps the auth adapter); the page now matches `nestrs g resource posts`
  byte for byte, and the generator's suite pins the feature set the page copies.

### Fixed — round-8 QA against the local 2.0.0

A QA pass read the local docs and applied them literally against the unreleased
workspace. Four framework defects, each closed with the test that keeps it
closed.

- **`#[routes]` broke on a parameter named `body` or `req`.** The generated
  wrapper bound its own `req`/`body`/`__ctrl` locals in the same namespace as
  the developer's parameters, so `Json(body): Json<T>` — the destructure
  `/http/extractors/` teaches — masked the `RequestBody` every *later* extractor
  read. The error landed on the `#[routes]` attribute and named neither the
  parameter nor the collision. The wrapper's locals now sit on
  `Span::mixed_site()`, which closes the class rather than the case;
  `nest-rs-codegen::mixed_site_ident` is the seam every decorator emitting a
  local should use. `nest-rs-macro-hygiene` gained a controller, which also
  proves what the old exclusion note denied: `#[routes]` emits no bare `poem`
  path, so a controller crate needs no `poem` line.
- **`ClientIp` never read `X-Forwarded-For` or `X-Real-IP`.** On TCP the peer
  branch always matched, so the documented chain's remaining steps were dead and
  `forwarded` was always `false` — behind a load balancer the extractor reported
  the balancer, forever. Resolution now lives in one place,
  `nest_rs_http::ClientOrigin`, gated on a trusted-proxy list and taking the
  **rightmost** non-trusted `X-Forwarded-For` hop.
- **The trusted-proxy list moved to the transport.** `HttpConfig::trusted_proxies`
  / `NESTRS_HTTP__TRUSTED_PROXIES` replaces `NESTRS_THROTTLER__TRUSTED_PROXIES`,
  and `ThrottlerStore::trusted_proxies` is gone from the trait — a store counts
  hits; who a hit belongs to is the transport's answer. Two lists is how the
  throttler came to key on the real client while `ClientIp` reported the
  balancer; one list means a `429` and the log line explaining it can never name
  different callers.
- **`#[crud]` routes published no response schema.** An ability shaper was read
  as "the field set depends on the caller, so say nothing", which typed every
  generated client's CRUD response as `any` — on exactly the surface `#[expose]`
  exists to serve. The document now publishes the shape and says in the response
  description that the fields are ability-dependent. `#[api(response = Type)]`
  is the new escape hatch for a handler that builds its own `Response`; the
  paginated list uses it. A `#[crud]` create answers **`201 Created`**.
- **`nestrs g ws` and `nestrs g mcp` named authz modules no generator wrote.**
  Both now scaffold `authz/ws/` and `authz/mcp/` the way `g graphql` scaffolds
  `authz/graphql/`, and wire them into the app — without the MCP bridge the
  endpoint denies every call. The three bridges are one table rather than three
  copies of the same trio of helpers.
- **A framework capability the app never imported warned at boot.** Every
  scaffolded auth app printed `skipped lifecycle hook … provider="AudienceBinding"`
  — a security check, named at `warn`, on an internal type, with nothing the
  developer could do. An inert hook owned by the framework reports at `debug`
  now; an inert hook owned by the app still warns, because that one is leftover
  code the developer can act on.
- **`#[redirect]` made a clean build dirty.** The macro answers without calling
  the handler, so rustc reported the method as never used; the expansion carries
  the `allow` itself.
- **`nestrs run test cov` did not exist** in a scaffolded workspace, though two
  pages and `apps.md` document it. The scaffolded `test.just` has it.

### Documentation — round 8

- **The umbrella sweep reached the five pages it had missed** — `/packages/`
  (the install page, which published a hybrid 8-line manifest under "list the
  individual crates"), `/tutorial/scaffold/`, `/tutorial/entity/`,
  `/tutorial/http/`, `/opentelemetry/`. Eight crates labelled *(direct dep)* are
  umbrella features and now say so.
- **`/tutorial/entity/` did not compile as written**: it declared three
  `nest-rs-*` crates absent from the `[workspace.dependencies]` block it had
  just written, and replaced the scaffold's feature list with one that dropped
  `seaorm` and `testing`. It now mirrors what `nestrs g resource` produces.
- **`EphemeralDatabase` does not use testcontainers.** Three pages sent a
  developer whose e2e suite failed looking for a Docker daemon; it reads
  `NESTRS_SEAORM__URL` and creates a database on that server.
- **Two security statements were wrong.** `/database/crud/` said an
  invisible-at-row-level row answers `404`; it answers `403`, deliberately, as
  `/security/authorization/by-id-binding/` documents in detail — a reader
  trusting the table believed they had closed an existence oracle that was open.
  `/rate-limiting/` described the throttler as keying on the **leftmost**
  `X-Forwarded-For` hop, which is the spoofable rule the code exists to avoid.
- **Four pages printed a boot line the framework never emits** (`bound N routes
  on …`), one of them building an instruction on it — on the page explaining why
  a route looks missing.
- **Every HTTP snippet imported `poem` and `schemars` bare**, which no crate
  following the one-dependency install can resolve. They route through
  `nest_rs::http::poem` and `#[input]` now. `async_trait` is reachable as
  `nest_rs::core::async_trait` — a service trait had to go through
  `nest_rs::guards` to find one.
- **`/configuration/env-reference/` claimed to be exhaustive** while omitting six
  `authn` keys and the whole `mcp` and `social` namespaces.
- Smaller: the GraphQL playground is off by default (`/tutorial/graphql/` said
  otherwise), `/fundamentals/guards/` contradicted itself on a postureless
  route, `cli/index.mdx` cited a `db migrate` recipe that is `db up`, and
  `g resource` bootstrapping the auth slice is now stated where a reader meets it.

### The mechanism

- `nest-rs-codegen::reroot` resolves how the call site reaches the framework —
  the umbrella for an app, the sibling crate inside the framework's own 14
  crates, which cannot depend on their own facade — and re-roots the finished
  expansion. Path literals included, so `#[serde(crate = "…")]` follows.
- The umbrella's feature matrix now pulls **everything a capability's decorators
  emit unconditionally**. `features = ["mcp"]` alone previously left
  `nest-rs-guards/mcp` off, so the documented global-guard fallback never ran.
- `nest-rs-macro-hygiene` is down to **one dependency** and gained `#[mcp]`
  coverage; it is the compile-time witness the rule names.

### Fixed

- `nest_rs::testing::EphemeralDatabase` was unreachable through the umbrella —
  the `seaorm` feature now forwards `nest-rs-testing/orm`.
- `/websockets/server-push/` imported `nest_rs_schedule::every`, which is an
  inner attribute of `#[scheduled]` and never an item. The page had never
  compiled as written.

### Tooling and product

- `nestrs new` and `nestrs g <transport>` write the umbrella and its features;
  the generator's crate tables became feature lists.
- The Publish demo consumes the framework through **one** workspace dependency,
  down from 27.
- The docs page templates now prescribe the one-line form, so a new page cannot
  reintroduce the old shape.

## [1.3.0] - 2026-07-31

A clean-room QA campaign against the **published** 1.2.0 — crates.io releases and
the live docs site, never the repository — filed 71 findings: 3 blockers, 27
major (4 security-relevant), 41 documentation defects. Every finding is closed
below, each with the test that keeps it closed.

### Security

- **`NESTRS_STORAGE__ALLOW_HTTP=false` did not cover presigned URLs.** Signing is
  a local computation, so `object_store`'s own plain-HTTP gate never saw it: a
  production app minted working `http://` URLs carrying the SigV4 signature, on
  the flow `/storage/` calls canonical. An `http://` endpoint with plain HTTP
  disallowed is now refused **at config load**, naming the variable, so no
  transfer can be attempted and no plaintext URL can be signed; the signing path
  keeps a second check for a hand-built `Storage::new`. It was also a
  request-time `500` with nothing logged at any level — a mis-deployed app
  started healthy and passed its liveness probe.
- **`#[public]` on an OAuth2 callback turned a forged-callback `401` into a
  `500`.** The authn guard absorbs a rejected credential on a public route by
  design, which left the handler with no principal and `Ctx<Claims>` answering a
  server error — indistinguishable from a bug, and invisible to any alert, WAF
  rule or rate limit keyed on `401`. The rejection is now recorded on the
  request, and a handler that goes on to need the principal answers the deferred
  `401`. A public route that never needs one still serves.
- **A `401` produced by a guard carried no `WWW-Authenticate` challenge**, which
  RFC 9110 §11.6.1 and RFC 6750 §3 require. Only a handler-returned `AuthError`
  set it; the guard path — the one the JWT page documents — did not. Every `401`
  the framework renders now carries it, and only a `401`.
- **`/http/file-uploads/` inverted its own caveat**: it said `MAX_BODY_BYTES`
  does *not* gate `Multipart` and told readers to compensate. It does. The page
  taught operators to build a control they already had, and developers to expect
  a large direct upload to work when it `413`s at 2 MiB by default.

### Fixed

- **The `.env` cascade outranked a value pinned in `for_root`** in every
  scaffolded app — the exact inverse of the documented tier, stated in three
  places and in the generated `.env`. `Environment::init` publishes the cascade
  into `std::env` so raw `std::env::var` consumers see it, which erased the one
  distinction the deployment tier rests on. The published names are now recorded
  and subtracted from `ConfigSource::get_from_deployment`, so `real env > pinned
  in code > .env cascade` holds in a scaffolded app exactly as in a library-only
  one.
- **An unreachable `NESTRS_QUEUE__URL` blocked boot forever with zero output** —
  never healthy, never crashed, and silent at `RUST_LOG=trace`, the worst shape
  for a container platform. The connect is now bounded by
  `NESTRS_QUEUE__CONNECT_TIMEOUT_SECS` (10 s default, `0` rejected), warns per
  attempt on `nest_rs::queue`, and fails with the redacted endpoint and the knob
  that widens it.
- **`timestamps` never bumped `updated_at`.** Create ran `ActiveModelBehavior`
  through `ActiveModelTrait::insert`; update went through the query builder that
  the scope filter forces, which does not. Every resource with the flag on
  silently froze the column downstream caches, incremental sync and ETags trust.
  `Repo::update` now drives the hooks explicitly, keeping the scope filter.
- **`QueueModule::for_root` never bound `Arc<dyn JobProducer>`**, so the portable
  injection form both the queue and the driver-authoring pages prescribe compiled
  and then died at boot. Both names now resolve from the one connection.
- **A global interceptor did not run on 404s or 405s**, contradicting three doc
  statements and skipping exactly the traffic a request-id or audit interceptor
  exists to record. The router answers an unmatched path with `Err`, which
  short-circuited the documented `next.run(req).await?` body; the transport now
  renders it once the global filter pool has had its turn, so the interceptor
  bands genuinely see a response.
- **GraphQL validation errors carried no `extensions` at all**, so a client could
  not tell which field was wrong while the HTTP twin named them. Every rejection
  site now renders through one helper: `extensions.errors`, the same member name
  HTTP, WS and the queue's dead-letter event use.
- **The `fields` extension on a masking denial is a list**, not a comma-joined
  string — the natural reading of "names in the `fields` extension", and the only
  shape that survives more than one refused field.
- **A malformed id on the GraphQL bind path leaked the `uuid` crate's parse
  string** with no code. Both malformed branches now answer
  `"id must be a UUID v7"` with `INVALID_ARGUMENT`.
- **A WebSocket pipe rejection, a malformed payload and an unknown event logged
  nothing** — exactly backwards, since a client sending garbage is the case worth
  seeing. All three now `warn` on `nest_rs::ws` beside the frame.
- **A set-but-unparseable `NESTRS_OPENTELEMETRY__*` value was swallowed.** `0`
  stays the documented sentinel; a typo is reported on stderr naming the
  variable.
- **A config validation failure dropped the namespace and leaked `validator`'s
  raw debug payload** — including the rejected value — into an operator-facing
  line. It now reads `configuration validation failed for '<namespace>'` with one
  `- field: rule (bound = n)` line each, bounds kept and the submitted value
  stripped.
- **`Storage` gained `delete`** (absent keys succeed, so retention sweeps and
  failed-upload cleanup are idempotent), `put_bytes` takes anything
  `Into<Bytes>` so the read/write round-trip composes without a copy, and
  `StorageError` converts into `std::io::Error` so `get_stream` feeds
  `Body::from_bytes_stream` as the streaming page shows.
- **Swapping `Bind`'s type parameters** (the 1.1.x order) reported two unrelated
  bound failures against the `#[crud]` attribute. Both traits now carry an
  `on_unimplemented` note naming the order and the swap, snapshotted by trybuild.

### CLI

- `nestrs g ws|schedule|mcp` over a `g resource` port emitted `self.svc.count()`,
  which a `CrudService` does not have — so the page's "any adapter compiles
  immediately" guarantee broke, with rustc blaming `Iterator::count`. Every
  transport now has a CRUD twin.
- `nestrs g queue` omitted `nest-rs-redis`, `nestrs g mcp` omitted `schemars`
  and `nest-rs-guards`' `mcp` feature (without which the documented global-pool
  fallback for `/mcp` is never seeded), and `nestrs g graphql` left the app
  crate — whose `module.rs` it edits — without `nest-rs-graphql`.
- `g resource` and `g migration` disagreed about the same table: the migration
  scaffolded `created_at`/`updated_at`/`deleted_at` and the entity declared none,
  so the resource hard-deleted against a tombstone column it never wrote. The
  entity now carries `soft_delete, timestamps` and the columns, matching the
  `users/` exemplar.
- The scaffolded smoke test booted the **app root**, so wiring a resource the way
  `g resource` instructs made the no-infrastructure suite need Postgres and fail
  on a 30 s pool timeout. It boots the feature's own module now.
- `nestrs doctor` read only `std::env`, reporting `not set` for a variable the
  workspace's own generated `.env` defines — the exact mistake
  `/database/migrations/` warns tool authors against.
- `--dry-run` printed `Created feature …` directly above `no files written`;
  `--version` / `-V` were rejected with `unexpected argument`; and the scaffold
  now pins `validator` to the major the framework compiles against.

### Documentation

Forty-one defects across the site, from stale samples (`TestApp::<M>::builder()`,
a pre-RFC-9457 validation body, a `debug`-vs-`warn` contradiction on one page) to
whole sections that could not be followed: the "grow a standalone crate into a
workspace" walkthrough clobbered the scaffold's own files and ended in a
duplicate-controller boot failure, and Social login documented two routes that
exist in no framework crate.

The install stanzas got the sweep 1.2.0 gave GraphQL, WS, MCP and Queue: `/http/`
needs `nest-rs-guards`, `/configuration/` needs `validator`, `/database/` needs
`schemars` and `validator`, `/mcp/` needs `schemars`. The `nest-rs` umbrella is
documented for what it is — version alignment and a prelude, not a substitute for
the crates a decorator's expansion names, which Rust's lack of a transitive
extern prelude makes impossible.

## [1.2.0] - 2026-07-30

Twenty-nine findings from two read-throughs. The first opened the GraphQL surface
(five findings, two security-relevant); the second worked down the untested list
into queue, schedule, events, WebSockets, MCP, rate limiting, health,
OpenTelemetry and GraphQL relations (twenty-four, three security-relevant). Each
is closed with the check that keeps it closed: new suites in nine crates, eight
CLI tests, live-Postgres e2e coverage, and two new greps (`bind-order`,
`queue-name`) in `docs/scripts/lint-docs.mjs`. One reported finding did not
reproduce and is now pinned so it cannot start (the ability-less read over
WebSockets already warns — an in-process test and an e2e both assert it).

`nest-rs-testing` gains **`LogCapture`**, because a third of the second round was
about what the framework *said*: a denial that fails closed but logs nothing, a
dead-lettered job with no event, a warning filed under the wrong target. Those
lines are what an operator queries during an incident, and they now have the same
coverage as a status code.

A third campaign then ran the unpublished 1.2.0 end to end — a fresh
`nestrs new` project, real HTTP requests, raw RFC6455 frames against a live
server, live Redis — and found sixteen more findings, four security-relevant.
Fifteen are closed below, several by product decision rather than patch. The one
left open is upstream: apalis-redis's orphan sweep makes a starting replica
re-run a peer's in-flight jobs, so queue delivery is documented as
**at-least-once**, measured, and pinned by an e2e test written to fail loudly
the day the upstream fix lands.

### Security

- **On GraphQL, `#[authorize]` did not require authentication.** `/graphql` is
  one endpoint carrying the `Public` marker — the authn guard admits an
  anonymous caller so `#[public]` operations stay reachable, and the ability
  guard then hands the operation the *visitor* ability
  (`AbilityFactory::define_visitor`). The class gate consulted only the grants,
  so a visitor grant added to serve a public feed also satisfied every
  `#[authorize]` operation on that entity — while the review contract of
  `define_visitor` is that a grant there reaches `#[public]` surfaces *only*,
  and the diff a reviewer reads shows only `#[public]` routes. The ability now
  carries whether a principal backs it (`Ability::is_visitor`, set by the
  guard's visitor branch through `AbilityBuilder::build_visitor`), and the
  GraphQL gate refuses the anonymous caller with `UNAUTHENTICATED` before
  looking at a single grant. HTTP is unchanged: a non-`#[public]` route never
  reaches the visitor branch, so the marker is still what selects the policy
  half there.

- **A guard denial at the WebSocket upgrade logged nothing, at any level.** The
  client saw the right refusal — 401/403 as `problem+json`, no socket opened,
  no `on_connect` — but `GuardEndpoint::call` converted the denial without
  passing through `deny_http`, the one site carrying the "every denial visible
  at `warn`+" floor, so a token-less socket sweep was invisible in supervision.
  The per-route HTTP and GraphQL paths already went through the floor; the
  upgrade path now does too, and a suite asserts both the `warn` and its
  absence on an allowed request.

### Changed

- **`WsModule` owns every connection registry, namespaced or not.** A
  `#[gateway(namespace = N)]` used to provide `WsServer<N>` from its own
  `Discoverable::register`, so the key belonged to no module: the access graph
  admitted any consumer through the imperative-registration escape hatch, and
  registration order decided whether the provider existed at all — a service
  living in a module the gateway imports was constructed before the registry
  existed and panicked at mount, naming the wrong provider. A namespaced
  gateway now submits a link-time `WsNamespaceEntry`; `WsNamespaces`, a
  `WsModule` provider, drains it and installs each `WsServer<N>`; and the new
  `Discoverable::also_provides` hook declares those keys to the graph, which
  attributes them to `WsModule`. One rule for `Global` and namespaced alike:
  the registry comes from `WsModule`, and the import is what the graph
  verifies. **Breaking:** a namespaced gateway's module must import `WsModule`
  — the boot error names it verbatim.

- **A `Valid<T>` rejection says which field, on every transport.** The
  field-level detail travelled in `PipeError`'s `details` and both async
  transports threw it away: a WS error frame said only
  `validation failed`, and a dead-lettered job carried nothing at all — read
  in a log days later by someone who cannot replay it. The error frame now
  carries `{error, errors}` under `data` (`errors` absent, not `null`, when
  the rejection has no structured detail — the same shape HTTP uses), through
  `WsReply::pipe_error` as the single site for both the payload pipe and the
  global data pipe; and `job dead-lettered` logs the detail under an `errors`
  field. **Wire-contract change** for WS clients: keep branching on
  `data.error`, read `data.errors` to point at a field.

- **A pinned config field no longer freezes its neighbours.** `nestrs new`
  writes `HttpConfig { port: 3000, ..Default::default() }`, and
  `provide_feature` served that literal as the whole config — pinning every
  field and making `NESTRS_HTTP__*` inert, silently, against the dual-path
  rule the configuration docs promise. Resolution is per-field now, strongest
  first: real environment > pinned code > `.env` cascade >
  `Config::defaults()`. One body (`Config::from_env(env, base)`) serves the
  pinned and unpinned paths, so no field can be reachable from only one side;
  `ConfigSource::get_from_deployment` isolates the tier that outranks a pin —
  `EnvSource` restricts it to the real process environment, so a committed
  `.env` reads as another default and yields to the pin while a deployment
  variable always wins, and a third-party source (Vault, a ConfigMap) is
  deployment-tier by default, the safe direction for a secret. The one
  remaining hard pin is the builder seed (`App::builder().provide(cfg)`),
  documented as such.

### Removed

- **`concurrency` on `#[process]` — it never capped anything.** The value only
  sized the Redis read buffer, so a handler declared `concurrency = 2` ran ten
  jobs at once (measured: `peak=10` while the boot line announced
  `concurrency=2`). Rather than fix the knob, the decision removes it: nestrs
  targets the container, so a `#[process]` method runs **one job at a time**
  (`WorkerBuilderExt::concurrency(1)`, read buffer of 1 so a waiting job stays
  in Redis where another replica can claim it) and scale comes from replicas —
  the unit the platform already schedules, measures and restarts. A
  `#[process(concurrency = N)]` is refused by name with the replacement
  spelled out, not as an unknown key; a live-Redis e2e pins `peak == 1` while
  still requiring progress; and `tower` drops back to a dev-dependency of
  `nest-rs-redis`.

### Fixed

- **A field-level grant took an entity offline over GraphQL.** `.fields([...])`
  strips a column, and the GraphQL wrapper had to hand the masked value back as
  the operation's own type — which a non-null schema field cannot express, so
  *every* query on that entity failed, including one asking only for granted
  columns. Its HTTP twin served the same rows masked. The mask now follows
  GraphQL's own rule: a stripped column masks to `null` where the field is
  nullable, and where it is not, the selection set decides — an operation that
  **selects** the column is refused (`FORBIDDEN`, names in the `fields`
  extension), one that does not is served. Rows the ability refuses are dropped
  either way, and nothing unmasked ever ships.

- **`nestrs g graphql <feature>` generated code that did not compile**, from two
  independent causes. `#[resolver]` expands to
  `nest_rs_guards::{GraphqlChainCell, GraphqlChainSources,
  run_layered_graphql_chain}`, which sit behind that crate's `graphql` feature —
  and because `nest-rs-guards` is already a dependency of every scaffolded
  workspace, the generator had to enable the *feature*, not add the entry
  (`ensure_features_deps` now widens an existing entry). And over a `g resource`
  port the scaffold called `svc.count()`, a method `CrudService` does not have:
  a resource now takes the `#[crud]` resolver behind `AuthnGuard` +
  `AuthzGuard`, the twin of the HTTP controller `g resource` already writes, and
  its entity gains the `#[expose(graphql)]` flag that makes it a GraphQL object.

- **`AuthzGraphqlModule` was required but never scaffolded.** The generated
  resolver's own comment told the reader to import it, while
  `g resource` / `g auth` wrote `authz/http/` only and no command wrote the
  GraphQL bridge — leaving three providers (`AuthzGraphqlBridge`,
  `GraphqlAuthnGuard`, `LoaderScope`) to be reconstructed from prose.
  `g graphql` now writes `authz/graphql/` when the workspace has a policy to
  enforce, imports it from the adapter's `module.rs`, and lists it at the app's
  composition site.

- **`Bind` / `bind` generic order was inverted throughout the docs.** The real
  signatures put the **action first** (`Bind<Read, UsersService>`,
  `bind::<Read, UsersService>`, `Authorized<Update, PostEntity>`); roughly a
  dozen places wrote the reverse, and `/security/authorization/by-id-binding/`
  stated the rule backwards in prose. Fixed across every page and gated by a new
  `bind-order` check in the docs linter.

- **A panicking event listener destroyed the emitter's request.** The events page
  promises "failure is local"; a panic was contained to the *process*, not the
  listener — it abandoned the dispatch chain mid-way, unwound through `emit` into
  the emitter, and on HTTP took the response with it. The client saw a dropped
  connection rather than a 500, with the emitter's side effects already
  committed, so a retry re-ran them. Any `unwrap()` in a fire-and-forget reaction
  (`email_the_author`, `index_for_search`) was therefore a way to break an
  unrelated write path. Each listener now runs under `catch_unwind`: the panic is
  logged at `error` on `nest_rs::events` with the event type and the panic
  message, and the chain continues.

- **A `Result` reached through a type alias shipped the error struct as a success
  frame over WebSockets.** `#[subscribe_message]` read the return type's last
  path segment to decide whether a handler could fail, so
  `pub type ServiceResult<T> = Result<T, MyError>` read as an ordinary value and
  the `Err` variant was serialized straight into the reply `data` — every field
  of the error, including ones `Display` deliberately withholds, in a frame with
  no `error` key and no server-side `warn`, because nothing knew a failure had
  happened. It compiled without a warning, and only in codebases with typed
  `Serialize` errors: the ones whose errors carry the most detail. The decision
  is now made on the **type** (`ReplyValue`, inherent-impl specialization), so
  however the return is spelled an `Err` becomes the same error frame and the
  same `warn` on `nest_rs::ws`.

- **A per-message WebSocket guard denial was logged under the wrong target.** It
  landed on `nest_rs::layers`, which carries events about the layer *system*; an
  operator tailing `nest_rs::ws=warn` for denials — the filtering every other
  page teaches — saw nothing. Now on `nest_rs::ws`, beside the rest of the
  transport's events.

- **A panicking queue job was dead-lettered in silence.** `CatchPanicLayer`
  contained it correctly — the job failed, the worker survived, the next job ran
  — but it unwound past the per-job span, so the whole field set (`queue`,
  `processor`, `job_id`, `attempt`) and every event were skipped. The only trace
  was the default Rust panic hook on stderr: no target, no fields, no span, and
  nothing at all at the docs' own production filter (`nest_rs::queue=warn`),
  while a deserialization failure on the same worker reported properly. The panic
  is now caught inside the span and reported as
  `job dead-lettered: handler panicked` at `error`. The outcome is unchanged.

- **Listener dispatch order was link order, not declaration order.** The events
  page guarantees "the order their providers appear in `providers = [...]`, then
  the order their methods appear in the `#[listeners]` block". `inventory` hands
  entries back in link order — stable per binary and reshuffled by any change to
  the code, which is the worst shape a guarantee can have: three methods declared
  `first, second, third` dispatched `2, 3, 1`, a second provider's listener
  landed *between* two of the first's, and two listeners ordered deliberately got
  silently rearranged the next time somebody added a third. `#[on_event]` now
  submits its position in its block, `nest-rs-core` seeds a `ProviderOrder` from
  the module walk, and `EventsModule` sorts on the pair.

- **Two controllers in one file could not share a handler name.** `#[routes]`
  emits one module-level type per handler and derived its name from the method
  alone, so `V1Controller::ping` and `V2Controller::ping` collided in a namespace
  neither knew it shared — breaking the layout the versioning page prescribes,
  and `list` / `get` / `create` besides. The symbol is now qualified by the
  controller.

- **A missing `WsModule` panicked the app after it had already mounted the
  gateway.** The connection registry was resolved with an `.expect(...)` at
  mount, so the app compiled, logged `mounted endpoint kind="ws"`, and *then*
  died with a backtrace note — where every other boot-time misconfiguration
  exits cleanly. A gateway now declares the registry as a dependency, so the
  access graph refuses the boot naming both the missing type and the module that
  provides it. (A namespaced gateway self-provided its own registry at that
  point; the live campaign moved ownership to `WsModule` — see *Changed*.)

- **`ThrottlerGuard` could not be wired the way the page describes.** The two
  documented steps — import `ThrottlerModule::for_root(None)`, bind
  `#[use_guards(ThrottlerGuard)]` — failed the boot: `#[use_guards]` puts the
  guard under the access contract, so the *controller's* module owed a provider
  for it, and a dynamic (`for_root`) import contributes only global
  infrastructure and could never satisfy it. `ThrottlerModule` (and its Redis
  twin, through one shared `provide_guard`) now registers the guard alongside the
  store it reads. **Breaking for an app that worked around this** by listing
  `ThrottlerGuard` in `providers`: that is now a duplicate registration and fails
  the boot naming it — remove the line.

- **An attribute-bound layer no module provides was reported as
  `<unnamed dependency>`.** A guard, filter or interceptor is reached by
  `Container::get::<P>` rather than an `#[inject]` field, and the access graph's
  names list covered only the fields — so every layer fell off the end of it and
  printed as a placeholder, *including in the suggested fix*. The framework's
  best wiring diagnostic was unusable for exactly the things wired as `dyn`.
  `#[controller]`, `#[resolver]`, `#[gateway]`, `#[routes]`, `#[messages]` and
  the resolver impl now emit index-aligned labels.

- **GraphQL relations answered `database error` with no `DbErr` and no SQL.** The
  flagship "relations resolve themselves" feature failed wholesale on a wiring
  gap nothing announced: `batch_spawner` fell back to a bare `tokio::spawn` when
  no `dyn GraphqlBatchContext` was registered, and a batch on a fresh task has no
  ambient executor, so `Repo` failed before a single statement reached the
  database. Schema build now warns when loaders are seeded with no batch context,
  naming the binding (`LoaderScope as dyn GraphqlBatchContext`), and
  `Repo::conn`'s missing-executor error is logged at `error` on `nest_rs::orm`
  with every context that installs one — because the wire form of
  `ServiceError::Db` is the constant `database error` and carries nothing an
  operator can act on.

- **A failing health indicator's error was reachable from nowhere.** The probe
  body reports a fixed `"check failed"` — deliberate, since `/health/*` is
  routinely unauthenticated and an `anyhow` chain from a connection check carries
  a DSN or an internal hostname — but the field's own rustdoc and the indicators
  page both promised the stringified error. The three now agree: the detail goes
  to a `warn` on `nest_rs::health`, and a test pins both directions.

- **The skipped-indicator notice fired on every probe.** A linked-but-unreachable
  indicator is a startup fact about the module tree; repeating it per request
  turned a wiring notice into production log volume, and no other discovery seam
  does that. Named once at boot now, at `warn`, matching `nest_rs::queue` and
  `nest_rs::events`; the docs said `debug` and are corrected.

- **The metric export interval was reachable from nowhere.** All three OTel
  signals arrive, but metrics wait ~60 s while traces and logs land immediately —
  so the standard first check (wire a meter, hit the route, look at the
  collector) shows zero metrics for a full minute and reads as a broken pipeline.
  The SDK's default was in neither the env table nor `OpenTelemetryConfig`, so it
  could not be shortened for a local run either. Now
  `metric_interval` / `NESTRS_OPENTELEMETRY__METRIC_INTERVAL_SECS`, dual-path
  like every other field, with the 60 s default named as
  `DEFAULT_METRIC_INTERVAL`.

- **`#[process]` obliged its call site to depend on `nest-rs-worker`.** The
  expansion emitted bare `::nest_rs_worker::` paths, which resolve against the
  *consumer's* extern prelude — so writing a processor needed a crate named
  nowhere in the docs (`nest-rs-worker` appears once in the whole set, as an
  "ambient job context seam"), and the first `cargo check` after
  `nestrs g queue` was `could not find nest_rs_worker`. `nest-rs-queue`
  re-exports it and the macro routes through that, as its own module docs already
  claimed. `nest-rs-queue`'s integration suite declares no `nest-rs-worker`,
  which is what keeps it closed.

- **`async_trait` was re-exported by three surface crates and not by the four
  layer crates.** `nest-rs-http` / `-queue` / `-ws` did; `-interceptors`,
  `-filters`, `-exception-filters` and `-guards` did not, so the one import a
  reader needed most was the one no page could name — and the miss cascades
  (without the attribute every trait method reports a lifetime mismatch, so the
  real cause hides under four unrelated errors). All seven now do.

- **`#[expose(…, graphql)]` required async-graphql features the consumer had to
  discover one error at a time.** The macro re-emits a column's own type into the
  generated `InputObject`, and an entity's columns are `Uuid` and `DateTime*` by
  construction — so a foreign key exposed as an input failed with
  `the trait bound uuid::Uuid: InputType is not satisfied`, pointing at a field
  whose type the developer never chose. `nest-rs-resource` declares `uuid` and
  `chrono` on its optional `async-graphql`, since it is the crate whose macro
  creates the requirement.

- **A controller and a self-mounted endpoint on one path panicked poem instead
  of failing the boot.** The exclusivity rule ran inside each family only —
  `prefix_owner` for controllers, `endpoint_owner` for self-mounts, two maps
  never crossed — so `#[controller(path = "/chat")]` plus
  `#[gateway(path = "/chat")]` passed both checks, logged both mounts as
  successful, and then hit poem's `duplicate path` panic the code's own comment
  promised to catch. One combined check refuses at boot; and the collision
  message names the owners (`ChatGateway`), not just the kinds — "a ws endpoint
  and a ws endpoint both mount there" is unusable with five gateways in play.

- **A gateway binding its own guards was reported as an unguarded self-mount
  edge.** The predicate consulted only the presence of a global guard pool, so
  `#[use_guards(TicketGuard)]` on the gateway — verified working, 401/403 at
  the upgrade — still drew the boot warning, with a hint recommending exactly
  what was already done: a security signal an operator learns to ignore.
  `HttpEndpointMeta` now carries the self-mount's own posture; a gateway with
  no guard at all is still reported.

- **Destructured handler arguments failed to compile on HTTP and GraphQL.**
  `#[routes]` and `#[resolver]` forwarded each argument to the generated
  wrapper by name, and a pattern has none — so the idiomatic poem forms the
  docs print (`Path(name): Path<String>`, `Query(q)`, `Json(body)`) were
  rejected with "must be simple identifiers", while `#[messages]` and
  `#[process]`, which forward by position, accepted them. The wrapper now
  forwards under the one identifier the pattern binds — the method keeps its
  pattern, only the wrapper's parameter list is normalized, and the name comes
  from the pattern because on GraphQL it *is* the SDL argument name. `Valid<T>`
  becomes destructurable (`pub` newtype field — exposing it grants nothing;
  `Authorized<A, E>` stays sealed, that proof guards data access). A pattern
  binding zero or several names keeps a named error, pinned by a trybuild
  snapshot; one suite drives all four transports against a single app.

- **`#[input]` did not derive `JsonSchema`.** `#[routes]` documents every
  `Json<T>` / `Query<T>` argument in the OpenAPI document, so an extractor DTO
  must implement `schemars::JsonSchema` — and the decorator whose whole role
  is absorbing input-DTO boilerplate left that one derive out. The failure was
  an unsatisfied-bound error pointing at `schema_of`, naming neither the
  missing derive nor the DTO. `#[input]` derives it now, and a test pins the
  full derive set so a future edit cannot drop one.

### CLI

- **`nestrs g queue` generated code that did not compile, and the `tracing` half
  hit three generators.** `g queue`, `g schedule` and `g ws` all write a
  `tracing::` call into the handler body while adding only their own `nest-rs-*`
  crate, and a workspace scaffolded by `nestrs new` carries no `tracing` in its
  features crate. `a_skeleton_that_names_a_crate_declares_it` derives the
  requirement from the template text, so a skeleton that starts logging drags its
  dependency along on the same commit.

- **`nestrs g mcp` pinned `rmcp 1.7` while `nest-rs-mcp` builds against 2.2.**
  Two majors in one graph put two `ServerHandler` traits in scope and every
  `#[tool_handler]` method mismatched; pinning harder made it worse. The
  generator's line is now read against the workspace manifest by
  `the_rmcp_pin_matches_the_frameworks_own`, so bumping the framework's `rmcp`
  fails there until the generator follows.

- **`nestrs g ws` omitted `nest-rs-guards`' `ws` feature.** `#[messages]` expands
  to `GuardAsWsMessageCheck`, which that feature gates — and the miss was worse
  than an ordinary one: `cargo check -p features` failed while
  `cargo check --workspace` passed, because a dev-dependency elsewhere in the
  graph unified the feature in.

- **`nestrs g queue` hid its own `#[queue]` marker.** The generator declared it
  in the adapter's private `processor` module and did not re-export it, so
  `push_to::<Q>` — the enqueue path the crate designates as the default — was
  unreachable even from the feature's own service, leaving the untyped
  `push(name, job)` escape hatch as the only way to enqueue. The marker now sits
  at the port beside the payload, where `QueueName`'s own docs say it belongs.

- **`nestrs g queue`'s module imported nothing.** That held only while the
  processor stayed the inert stub; give it the shape the Queue page prescribes and
  the worker died at boot on an access violation. Every adapter module imports
  its port now, as `g http` / `g ws` / `g schedule` already did.

- **`nestrs g ws`'s module omitted `WsModule`**, so following the generator's own
  "Next steps" produced an app that mounted the gateway and then failed to boot.

- **`nestrs g ws <feature>` wrote `path = "/ws"` for every feature**, so the
  second WS adapter collided with the first at boot — right after the
  generator's own "Next steps" said to import it. The path derives from the
  feature name now, as the HTTP twin always did.

- **`nestrs g http <feature>` emitted a route with no posture** — the only one
  of the three route generators (`new`, `g graphql`, `g http`) whose output
  booted straight into the unguarded-routes warning. It writes `#[public]`
  with the same `// SECURITY:` comment as the GraphQL template.

- **A typed WS payload needed a `serde` the generator did not write.**
  `nest_rs_ws` re-exports `serde_json` only, so the first
  `#[derive(serde::Deserialize)]` payload — the messages page's normal case —
  failed to compile until `serde` was added by hand. `g ws` writes it, and the
  install stanza on the WebSockets page explains why it is on the list.

### Documentation

- **The whole `/queue/` section described a pre-`QueueName` API.** Every
  `#[process]` example named its queue with a string — a form the shipped macro
  rejects — while `#[queue(name = …, job = …)]` and `QueueName` appeared in no
  prose page at all, so the only place a reader met the real API was the
  generator output. The producer half was worse because it *compiled*:
  `/queue/producing-jobs/` taught `queue.of::<T>(AUDIO_QUEUE)`, the
  runtime-name escape hatch, and called the string "the only stringly-typed
  coupling" — the exact coupling the type removed. Rewritten across the section
  and gated by a new `queue-name` check in the docs linter.

- **Failed jobs land in `<queue>:dead`, not `<queue>:failed`.** The page tells
  readers to inspect and replay that set themselves, so the key name is the one
  detail an operator actually types. Two neighbours corrected with it: a
  deserialization failure is non-retryable and dead-letters on the first attempt
  rather than burning the budget, and every failed attempt *including the
  terminal one* logs `will retry within the budget` — read the `attempt` field,
  not the message.

- **Three snippets imported `async_trait` from `poem`, which does not export
  it**, and the global-interceptor snippet omitted
  `AppBuilderInterceptorsExt`.

- **Standalone mode loses every generator, not the two the CLI page named**, and
  the landing page's "grow into a workspace when you add apps" was promised in
  one sentence and explained nowhere. Both corrected, with the growth path
  written out as a recipe.

- **Per-section dependency stanzas** on `/queue/`, `/mcp/`, `/graphql/` and
  `/websockets/`. Every section opened with `cargo add <one-crate>`, and in
  several cases that did not compile the page's own first example — the real set
  was discovered one compiler error at a time. The stanzas also give the
  generators a spec to match, rather than leaving the generator and the docs as
  two independent guesses at the same list.

- **The decorators index pointed `#[on_connect]` / `#[on_disconnect]` at
  Messages**; both are documented on Rooms.

- **The trailing-slash trap**, on the controllers page: `#[get("/")]` under
  `#[controller(path = "/greetings")]` serves `/greetings`, and the
  trailing-slash form is a 404 that still carries a global interceptor's headers
  — convincing enough to read as a broken feature. The boot line is
  authoritative.

- **The sources page described a cascade mechanism the code does not have.** It
  said `EnvSource` triggers the `.env` cascade merge on its first `get`,
  writing into `std::env` under a `Once` — the code parses the cascade into a
  crate-internal map and never mutates the process environment on a read;
  `Environment::init` is the only publisher, called on the first line of every
  scaffolded `main` for consumers that only know `std::env::var`. The page's
  conclusion was right for the wrong reason, and its advice sent readers to
  distrust calls with no side effect. Rewritten around the real mechanism,
  three derived claims on other pages corrected, and the *actual* hermeticity
  trap documented: the first parse freezes the working directory and
  `NESTRS_ENV` that chose the files, so a test that moves either afterwards
  resolves against the previous test's cascade. Two new tests pin the one
  claim nothing covered.

- **Twelve snippets used destructured arguments that did not compile** (the
  macro fix above makes the poem-idiom six compile as printed), **and three of
  them were structurally wrong regardless**: `Valid(Json(input))` never holds
  a `Json`, and `Piped(id)` cannot be a pattern (`Piped` carries a
  `PhantomData`, and a public type projection just so a pattern works is not
  worth it) — those read `Valid(input)` and `id: Piped<…>` + `*id` now. Three
  more `#[input]` snippets relied on the `JsonSchema` derive the decorator was
  missing.

- **Two env keys were absent from the reference** — `NESTRS_HTTP__COMPRESSION`
  and `NESTRS_STORAGE__ALLOW_HTTP`, the second security-relevant — plus the
  seven OAuth2 keys of the `authn` namespace. Established by extracting every
  key each `from_env` reads and diffing against the page, across all nine
  namespaces; key counts in prose ("all fifteen keys") are gone, since a count
  rusts at the first added option.

- **Four dead internal anchors repointed**, detected by diffing every
  `](/page/#anchor)` against the built HTML's `id=` set; one had the right
  label on the wrong page. Every internal anchor in the docs now resolves.

- **`/queue/` gains a "One job at a time" section** documenting the
  concurrency decision and its two consequences (head-of-line blocking is per
  queue, no prefetch), and an idempotent-handlers aside stating the
  at-least-once contract with its cause — a starting replica re-runs a peer's
  in-flight jobs, upstream in apalis-redis — and a pointer to the e2e test
  that pins both halves of the replica behaviour.

## [1.1.1] - 2026-07-27

Six findings from the 1.1.0 read-through, each closed with the check that keeps
it closed — the generator defect is now a unit test, the scaffold wording an
integration assertion, and the three documentation classes are greps in
`docs/scripts/lint-docs.mjs`, which gates the whole corpus (the baseline is
empty).

### Fixed

- **`nestrs g migration` names the table, not the migration.** The skeleton's
  `DeriveIden` enum was rendered from the migration name, and `DeriveIden`
  snake-cases the enum straight into the DDL — so `g migration create_widgets`
  created a `create_widgets` table while the entity `g resource widgets` had
  just written read `widget`. `db up` reported success and the first query
  failed. The enum is now derived from the *subject* of the name
  (`create_widgets` → `Widget`, `add_status_to_posts` → `Post`,
  `drop_orgs_table` → `Org`), the emitted comment names the table it writes, and
  the CLI's next-steps print it.

- **The scaffolded `.env.example` says where a test override goes.** It sent
  developers to `.env.local`, which the cascade skips under `NESTRS_ENV=test` by
  design — so a machine-specific database override was silently ignored by
  `nestrs run test e2e`, and the failure named a connection rather than the
  ignored file. It now points at `.env.test.local` and says why.

- **The tutorial no longer promises unauthenticated CRUD.** The index and
  `/tutorial/validation/` still curled a guarded controller with no bearer and
  documented the pre-guard responses; a reader following them verbatim got a
  `401` where the page showed a `201` or the `400` it was teaching. Both now
  carry the token, and the index says guards arrive with the database on page 4
  instead of listing them as a step-8 addition. `/server-timing/` and
  `/rate-limiting/`, which the same check caught, carry it too.

- **Six documentation snippets that could not compile, and taught the wrong
  layer while failing to.** `/security/authorization/public-reads/`,
  `/http/versioning/`, `/security/authorization/response-masking/`,
  `/security/authorization/by-id-binding/` and `/server-timing/` all `?`-ed a
  `CrudService` read directly from a handler: those yield `Result<_, DbErr>`, and
  `DbErr` is no `ResponseError`. The fix is the exemplar's shape rather than a
  `map_err` at the route — a service method returns the **wire type** (as
  `PostsService::create_in_org` already does), so a hand-written handler is a
  one-line delegation and the `Model` conversion stays in the service. The
  by-id pages keep their `Access` → status match: mapping `Found`/`Denied`/
  `Missing` onto 200/403/404 is transport work, and it is what those pages are
  about.

- **Stale `nest-rs* = "1.0"` pins** on `/tutorial/entity/`, `/database/` and
  `/packages/`, one release behind what `nestrs g resource` writes.

## [1.1.0] - 2026-07-26

Fixes from a crash-test of the 1.0.0 release: building an app by following the
documentation end to end, on a pristine `nestrs new` scaffold. A minor rather
than a patch — `nestrs g auth` is new, `g resource` emits a different slice, and
two flags are gone (`g resource --guarded`, `new --template`).

### Added

- **Every `nestrs new` layout ships the same `hello` module** — a service with a
  greeting and one `#[public] GET /` that returns it. Previously only two of the
  four generation paths mounted a route: `nestrs new blog` **inside** a
  workspace produced an app with an empty route table, and both `--template
  empty` variants did too — while the CLI's own next-steps told you to open a
  browser at a URL that answered `404`. A freshly created project has to prove
  it started, and a `404` proves nothing to the developer looking at it.

  Workspace mode writes the greeting as a feature named after the app
  (`crates/features/src/blog/`), because the layout keeps no `service.rs` /
  `controller.rs` in an app crate; standalone writes the same two files under
  `src/`. `nestrs new <name>` now refuses when a feature already owns that name,
  rather than overwriting product code.

- **`nestrs g auth`** — the app-side authn/authz adapter (`Claims`,
  `AuthnGuard`, `AuthzAbility`, `AuthzGuard`, and their modules) that roughly ten
  documentation pages referenced and nothing generated. The framework is
  generic over the principal and the policy, so these types cannot ship in a
  `nest-rs-*` crate; every workspace wrote the same eight files by hand, from
  crate sources, or not at all.

- **`AbilityFactory::define_visitor`** — the anonymous branch of an app's
  policy, consulted by `AbilityGuard` on a `#[public]` route. A DB-backed
  resource anyone may read was previously not expressible: the public branch
  installed an ability built from an *empty* `AbilityBuilder` and never asked
  the app's factory, so `Authorize` answered `403` and `Repo` filtered every
  row — whatever the developer wrote. The new method defaults to granting
  nothing, so an app that does not implement it behaves exactly as before, and
  a route opened with `#[public]` still exposes nothing until a rule is
  written. One correction covers HTTP, GraphQL and the WebSocket upgrade: all
  three run `AbilityGuard::check_http`. `/mcp` deliberately carries no `Public`
  marker and keeps refusing anonymous callers.

  **The reach of `#[public]` grows with this**: the marker now selects which
  half of the policy runs, so reviewing a diff that adds it means reading
  `define_visitor` too. Documented in
  [Public reads](https://nestrs.dev/security/authorization/public-reads/).

  Additive under semver, with one exception worth naming: an app that already
  has an **inherent** `define_visitor` method on its `AuthzAbility` would see the
  inherent one win at every call site, and the trait method silently keep its
  empty default. The name is new, so the risk is close to zero — but it is a
  real shadowing rule, not a rounding error.

- **A malformed rule on a `#[public]` route now fails closed.** The public
  branch used `unwrap_or_default()`, degrading a rule the builder rejected into
  a deny-all ability — indistinguishable, to the caller, from an ordinary empty
  result. It goes through the same `match` as the authenticated branch and
  answers `Denial::internal`.

- **`nestrs new` scaffolds `crates/migrations/` and `crates/seed/`**, with the
  `migrate` binary behind every `nestrs run db …` verb. `nestrs g migration`
  bootstraps them for a workspace scaffolded before this.

- **`AuthError` and `CredentialError` implement poem's `ResponseError`**, so a
  handler can `?`-propagate them as the exception-filter documentation
  describes. `AuthError::Unavailable` keeps its distinct `500`.

### Removed

- **`nestrs new --template`.** With one starter that always serves `/`, the flag
  had one remaining value (`empty`) whose only effect was a project answering
  `404` on its first page. One way to do a thing.

### Changed

- **`nestrs g resource` emits the guarded `#[crud]` form** and scaffolds the
  auth adapter when the workspace has none; **`--guarded` is removed** — it is
  the only shape now. The unguarded slice it used to emit compiled but could
  not serve a single row: `Repo` filters every read by the caller's ambient
  `Ability`, which only an `AbilityGuard` installs, so every route answered
  `500` (missing ability) or read an empty table forever.

- **`Environment::init()` merges the `.env` cascade into `std::env`**, as its
  documentation always said. Without it the scaffold's own
  `NESTRS_LOG` / `NESTRS_LOG_FORMAT` / `NESTRS_LOG_SOURCE_LOCATION` in
  `.env.development` were inert, and a `migrate`-style binary reading
  `std::env::var("NESTRS_SEAORM__URL")` found nothing. It writes through
  `set_var`, so the documented obligation stands: call it at the top of `main`,
  never from a task.

- **`#[crud]`'s error mapping moved into `nest_rs_seaorm::crud_error`.** The
  status mapping is unchanged (409 on a unique violation, 403 on the ability
  re-check's `RecordNotInserted`, 404 on a vanished row); it is now one
  implementation instead of one copy per controller, and it logs the unexpected
  `DbErr` it turns into an empty-bodied 500.

### Fixed

- **A `500` from the authz or ORM path is no longer silent.** `Authorize` logs
  at `error` on `nest_rs::authz` when no ability guard ran, naming the action,
  the subject and the fix; `ServiceError`'s opaque variants log their cause at
  `error` on `nest_rs::orm` when they become a 5xx. Diagnosing one used to mean
  a custom debug handler and reading three crates' sources — at `trace`, a `500`
  produced zero records.

- **`g resource` injects the dependencies the decorators expand to** —
  `schemars` (`#[expose]` derives `JsonSchema`) and `nest-rs-authz` (`#[crud]`
  emits `Authorize<A, E>` parameters). Without them the first `cargo check`
  after generating was a wall of macro-expansion errors, invisible to
  `cargo check` on the scaffold itself.

- **`nestrs run db up` and `db seed` work on a fresh workspace.** Every
  `db.just` recipe named the `migrations` and `seed` crates, and neither
  existed.

- **`nestrs run test unit` and `test e2e` work on a fresh scaffold.** Both
  filter on `binary(e2e)`, and nextest rejects a filterset naming a binary the
  workspace does not have — so every app is now scaffolded with an empty
  `tests/e2e/main.rs`, which the docs already claimed.

- **The scaffolded smoke test compiles.** It called
  `TestAppBuilder::with_test_telemetry`, which lives behind
  `nest-rs-testing`'s optional `opentelemetry` feature. The scaffold imports no
  `OpenTelemetryModule`, so the call is simply gone.

- **The `sea-orm` pin the generator writes is `2.0`**, not the `2.0.0-rc.38`
  release-candidate floor, and its feature list matches what `nest-rs-seaorm`
  itself resolves.

- **Scaffold polish**: the hello route is `#[public]`, so a first run no longer
  greets you with the framework warning about its own template; workspace mode
  no longer writes a `.dockerignore` it ships no `Dockerfile` for; standalone
  mode no longer ships database recipes that need a workspace; and the
  generated `README.md` links resolve outside nestrs.dev.

### Documentation

- The tutorial carries the two guards from the HTTP page onward, and
  `/database/` states plainly that no row crosses the data layer without an
  ability — the previous narrative was not reproducible.
- Tutorial page 1's checkpoint is a `200 Hello World` instead of a `404` it
  taught you to expect, and `/cli/`'s template table is replaced by the one
  starter.
- 18 further corrections: wrong imports (`ServiceError` is in `nest_rs_seaorm`),
  the missing `AbilityGuard` import path, the undocumented `connect_from_env`,
  the two contradictory `Migrator` locations, `PATCH`'s whole-body semantics,
  the exact validation-error body, the scaffolded file tree, and the boot log
  lines.

## [1.0.0] - 2026-07-25

A handful of crates *are* the framework's public surface — their types appear
in signatures the macros emit. Their majors are tied to the nestrs major and
are frozen within 1.x: `poem = "3"`, `sea-orm = "=2.0"`,
`async-graphql = "=7.2.1"`, `rmcp = "2.2"`, `inventory = "0.3"`,
`validator = "0.20"`, `schemars = "1"`. sea-orm and async-graphql are
exact-pinned (not caret) because the ORM bounds and the GraphQL registry
codegen read enough of their surface that even a *minor* can shift generated
code.

### Changed

- **`nest_rs_redis::ConnectionError` is now `RedisError`** (re-exported at the
  crate root). A generic infra-error name collides in an app that imports
  several backends' error types; the house pattern is concern-prefixed
  (`ConfigError`, `StorageError`, `QueueError`), and Redis was the last one out
  of step. Rename the import; the variants and fail-closed semantics are
  unchanged.

- **`DatabaseConfig::retry_serialization_conflicts` is now
  `observe_serialization_conflicts`** (env
  `NESTRS_SEAORM__OBSERVE_SERIALIZATION_CONFLICTS`). The flag never retried
  — it tags a commit-time conflict (`40001` / `40P01` / `1213` / `1205`) as a
  structured `warn` on `nest_rs::orm` so contention is distinguishable from a
  generic commit error. The old name promised a transparency the framework
  deliberately does not offer: replaying a conflict means re-running the whole
  handler, and a handler may already have pushed a job, emitted an event, or
  written an object — none of which roll back with the transaction. Retrying
  stays the service's decision, at a boundary it knows is replayable
  (`nest_rs_seaorm::retry::retry_on_conflict`). Renamed before the freeze
  because a config key is public surface for the whole `1.x` line.

- **`AuthGuard` is now `AuthnGuard`** (`nest_rs_authn::AuthnGuard<S>`). It was
  the only half of the pair not carrying its concern's suffix, so
  `#[use_guards(AuthGuard, AuthzGuard)]` read as if the two guards answered
  different kinds of question. They don't: one establishes *who* (authn), the
  other *what they may do* (authz). Rename the import; nothing else changes.
  `OAuthGuard` and other `OAuth*` names are untouched — that `Auth` is OAuth's.

- **Social providers activate from configuration, not from a per-provider
  module import.** Importing `SocialModule` is now the whole wiring step: it
  owns every registry entry, so it is the module gate, and inside that gate
  each linked provider turns on when its credentials are set. A provider with
  no credentials is inert with one boot `warn` (its routes `404` like an
  unknown key); a *partially* set or invalid one **fails boot naming the
  provider**, so a half-configured login is never silently dropped.
  - `GithubSocialProviderModule` / `GoogleSocialProviderModule` and their
    `Setup` types are **removed**. Delete those imports; pin config the
    ordinary way by providing a `GithubSocialConfig` / `GoogleSocialConfig`
    value, which still wins over the environment.
  - `SocialProviders` is renamed **`SocialRegistry`** — it is the registry, not
    the providers.
  - `SocialProviderEntry` gains `env_namespace` and `build` (normally one
    `resolve_provider` call) and drops `provider_type_id` / `resolve`. A
    third-party provider crate is now two files, `config.rs` + `provider.rs`,
    with no module to write: a social provider is never `#[inject]`ed by type,
    so it has nothing for a module of its own to own.

- **`nestrs new` scaffolds its smoke test into `tests/integration/`** (it
  boots `TestApp` in process, no live infra — so it now runs on every
  `nestrs run test unit` instead of hiding behind the `binary(e2e)` gate).
  The scaffolded `e2e` recipe carries `--no-tests=pass` until the project
  adds a real e2e suite.

- **Capability-only guards are the documented pattern for non-CRUD routes**
  (`authn-authz.md`): a route whose response is not an entity row gates
  through a custom `Guard` checking the ability imperatively, bound via
  `#[use_guards]` — `Authorize<A, S>` would arm response masking against a
  body that is no wire model. Exemplar: `audio`'s `TranscodeGuard`.

- Third-party pins consolidated in `[workspace.dependencies]` (`redis`,
  `clap`, `toml_edit`, `tempfile`, `tower`, `libc`; `tokio-tungstenite` in
  the demo workspace). `nest-rs-redis` names `redis::RedisError` /
  `redis::aio::ConnectionManager` through the `redis` crate directly —
  apalis stays an implementation detail. Dead framework deps dropped from
  the demo apps' manifests; the worker enables the OTel `http` feature it
  actually serves.

### Added

- **`sea_orm` and `rmcp` are re-exported from their surface crates**
  (`nest_rs_seaorm::sea_orm`, `nest_rs_mcp::rmcp`), the way `nest-rs-http`
  already re-exports `poem` and `nest-rs-graphql` re-exports `async_graphql`. A
  consumer no longer carries its own `sea-orm` dependency and hand-mirrors the
  framework's exact `=2.0` pin — the lockstep version travels with the
  framework. (rmcp's `#[tool*]` macros still expand to a crate-relative `rmcp::`
  path, so a crate that *hosts* a tool keeps a direct `rmcp` dependency for that
  expansion; the re-export covers every other use.)

- **`nest_rs_ws::Scoped<T>`** resolves an `#[injectable(scope = request)]`
  provider from inside a WebSocket message handler, opening a fresh request
  scope per inbound message — the same seam the per-message guards already run
  on. This closes the four-transport parity: HTTP, GraphQL and MCP already had
  `Scoped`.

- **`#[wire_default(...)]`** (`nest-rs-resource`) — an auditable opt-in
  placeholder for an unexposed column whose type the response-masking
  reconstruction cannot default on its own (a custom enum, `Uuid`, timestamp,
  `Decimal`). Without it such a column fails the masked round-trip closed (a
  `500`); with it the reconstruction succeeds and the placeholder is stripped by
  the static expose set before the body ships — so it never reaches the wire.
  Sound only for a column no ability rule predicates on, and the macro rejects
  it on an exposed, PK or relation field. This is what lets a strict DB-backed
  enum stand in for a hidden `String` column: an unknown stored value then fails
  to load rather than being silently coerced to a default.

- **Ambient request state now reaches an MCP tool body — `Repo` works on MCP.**
  rmcp dispatches every tool call on its own spawned task, so the request
  scope, ambient executor and ambient ability installed around the endpoint
  never reached a tool. The new `PropagatingHandler` closes that gap: the
  endpoint stashes the state in the HTTP request extensions, rmcp forwards them
  as `http::request::Parts` into the operation's `RequestContext`, and the
  handler re-installs them *inside* the dispatch. A tool method now resolves
  `Scoped<T>` and reads through `Repo` with the caller's row filter applied —
  the same transparency HTTP and GraphQL have, with no filtering written in the
  tool.
  - New `McpToolContext` seam (`nest-rs-mcp`) with the first-party
    `nest_rs_seaorm::McpDataContext` behind seaorm's new `mcp` feature —
    the MCP twin of `WsDataContext`. It installs a **lazy** per-operation
    transaction: a read-only tool opens none, a writing tool commits on success
    and rolls back on error. `AuthzMcpModule` provides it.
  - Without a registered `McpToolContext` a `Repo`-backed tool still fails
    **closed and loud**, never unscoped.
  - `endpoint_with_guard` takes the context as a second argument (the `#[mcp]`
    macro resolves it from the container; hand-written call sites pass `None`).

- **MCP reaches the security sub-layer through the same wiring as GraphQL.**
  Both transports are `EdgePosture::Exempt` and gate in-band, but only
  `/graphql` had the surrounding seams; `/mcp` now has all of them, so the two
  answer identically to one app wiring.
  - **The global guard pool reaches `/mcp`.** With no `dyn McpOperationGuard`
    registered, the endpoint folds the `use_guards_global(...)` chain in-band
    (`FallbackMcpGuard` + `nest_rs_guards::GlobalPoolMcpGuard`, behind guards'
    new `mcp` feature) instead of going straight to deny-all. A global
    `ThrottlerGuard` now rate-limits a tool call — it previously could not.
    The fallback only ever *widens* what the app declared: with no pool (or an
    empty one) `/mcp` stays deny-all, and unlike `/graphql` it carries no
    `Public` marker, so a pooled `AuthnGuard` still refuses an anonymous call.
  - **`McpOperationGuard` gained `capture` + `around`** (both defaulted, so
    existing impls are unaffected): snapshot on the request, install *inside*
    rmcp's spawned dispatch — the same split `McpToolContext` already used for
    the same crossing. `McpAbilityBridge` implements them, so the **guard**
    installs the caller's ambient `Ability` on both transports and a tool body
    is now scoped even when the app registers no `McpToolContext`.
  - **One authn→authz chain.** `nest_rs_authz::run_ability_chain` holds the
    ordering once; each bridge only maps the resulting `Denial` into its own
    transport error. Side effect: an MCP denial keeps its status, so a `429`
    from a throttler in the chain reaches the client as `429` with its
    `Retry-After` instead of a flattened `401`.

- **Three test suites that never ran now run.** `cargo nextest run --workspace`
  builds every member with its *default* features, which silently excluded a
  large part of the framework's own coverage: `nest-rs-authz`'s http / graphql /
  mcp bridge tests compiled away behind `#[cfg(feature = …)]`, and the
  `nest-rs-seaorm` and `nest-rs-redis` e2e targets were skipped outright
  because their `required-features` were unsatisfied. Each crate now carries a
  path-only **self dev-dependency** that turns its own features on for its test
  targets (dev-deps do not propagate, and Cargo strips them from the published
  manifest). The workspace-wide `-E 'binary(e2e)'` step went from 1 test to 21.
  - Enabling them surfaced two real defects, both fixed: the `nest-rs-seaorm`
    e2e harness `expect`ed `NESTRS_SEAORM__URL` instead of defaulting to the
    dev container like its `nest-rs-redis` / `nest-rs-storage` siblings, and its
    shared probe tables were guarded by a per-*process* `OnceCell` while nextest
    runs each test in its own process — so a fresh database raced
    `CREATE TABLE IF NOT EXISTS` against the Postgres catalog. The DDL now
    serializes on a transaction advisory lock.

- **Two macro diagnostics are pinned by compile-fail snapshots.** Arming the
  `#[routes]` response shaper with a type that only *borrows* the
  `Authorize`/`Bind` name now has a `trybuild` fixture, as does a
  `for_root(...)` value that is not `Send` (the bound `#[module]`'s
  construct-once dynamic imports introduced). Both errors were already
  emitted; neither was guarded against silent regression.

- **`Repo::insert_unscoped`** — the write pendant of `Repo::unscoped()`, on
  an explicit connection, for pre-principal provisioning (social login) and
  principal-less system work. The social-login inserts and the slug
  uniqueness probe now route through `Repo`, so "every data access lives in
  `Repo`" holds by construction; each escape documents its bar in rustdoc.

### Fixed

- **A primary-key-less entity no longer panics on the data hot path.** `Repo`'s
  query and mutation paths `expect`ed at least one primary-key column, so a user
  modeling a view or a keyless table hit a mid-request panic — in the layer
  whose written contract is "never panic, return `DbErr`". Both sites now return
  a typed error naming the entity, logged at `error`.

- **`nestrs g mcp` scaffolds compiling code again**: the MCP tool template
  imported `Content`, an rmcp 1.x alias renamed `ContentBlock` in 2.x — the
  generated file could not compile.
- **The configured OpenTelemetry `service.name` now wins**: the SDK's
  `SdkProvidedResourceDetector` always supplies a `service.name` (env override
  or the `unknown_service:*` sentinel) and `with_schema_url` merged it *over*
  the configured attrs; `build_resource` now applies the config after the
  detector merge, with a regression test.
- **`nest-rs-testing` decides `NESTRS_ENV` before any `.env` read**: the
  set-if-absent `NESTRS_ENV=test` default moved from `TestAppBuilder::new`
  into `load_project_env`'s `Once`, so a db-first harness
  (`EphemeralDatabase::create` before `TestApp::builder`) no longer loads
  `.env.local` and skips `.env.test.local`. `nestrs run test e2e` works on
  bare metal again.
- **Macro path hygiene**: `#[hooks]` emitted a bare `::anyhow` path,
  `#[gateway]` a bare `::tracing`, `#[messages]` a bare `::nest_rs_http`, and
  the http/resource macros bare `::poem`/`::serde_json`/`::tracing`/
  `::async_trait` — all now route through their surface crate's re-exports
  (`nest_rs_core::anyhow` is new), so a downstream app without those direct
  deps compiles. Proven by the new `nest-rs-macro-hygiene` witness crate
  (workspace-internal, `publish = false`), which consumes the decorators with
  zero third-party dependencies.
- **`Authorization: basic` (any case) is accepted**: `basic_credentials` now
  matches the scheme case-insensitively per RFC 7235, mirroring
  `bearer_token`.
- **GraphQL/WS guard denials always log at `warn`**: the layered chain
  runners emit the same structural floor as HTTP's `deny_http`, so a custom
  guard that denies silently can no longer create an unobservable denial.
- Assorted robustness: the health endpoints return 500 instead of an empty
  body when the report fails to serialize; the authz predicate downcast,
  password-timing dummy, response-masking defaults, pagination-cursor header
  and conflict-retry exhaustion no longer `expect`/panic on request paths
  (each fails closed or degrades with a logged error); a broken `JobContext`
  is attributed to the new `nest_rs::worker` target instead of
  `nest_rs::queue`.

### Removed

- **`nest_rs_authz::mcp::masked_output`.** It was a one-line delegation to
  `nest_rs_authz::masked_output_ambient` — two public names for one behaviour,
  against *one way to do a thing*. Call `masked_output_ambient` directly; the
  signature and the fail-closed semantics are unchanged.

- **The unfinished offset-pagination surface.** `PageArgs`, the `<Name>Page`
  envelope emitted by `#[expose(..., paginate)]`, the `paginate` flag itself,
  and the `paginate = page` mode of `#[crud]` are all gone. The mode was never
  wired — both transports answered it with a compile error — so the types
  documented a capability the framework refused to generate. Keyset
  (`paginate = cursor`, the default) and `paginate = none` are the whole knob;
  a consumer that genuinely needs page numbers plus a total hand-writes that
  operation on its service. No caller in either workspace was affected.

- **`demo/.env.example`, and the `.env.local` the devcontainer seeded from it.**
  The demo now commits its whole configuration in `.env` + `.env.development`:
  it holds no real secret (its signing key is the dev keypair already committed
  for the test suites, its OAuth credentials are fixtures), so the git-ignored
  half had nothing legitimate to carry. It carried a `<REPLACE-ME>` placeholder
  instead, which the `postCreateCommand` copied into every fresh container and
  which the `auth` app refused to boot on. `git clone` then `nestrs run` now
  works with nothing to prime. Existing clones can delete their `demo/.env.local`
  — and their `demo/.env.test.local`, which pinned `localhost` backend URLs that
  no process inside the devcontainer can reach. The secret-handling pattern is
  unchanged where it belongs: `nestrs new` still scaffolds `.env.example` next
  to a git-ignored `.env.local`.

### Known for the 1.x line

- **`Guard::check_http` sits on the base `Guard` trait**, so `nest-rs-guards`
  depends unconditionally on `poem` and `nest-rs-http` — a queue-only binary
  still compiles the HTTP stack. Build hygiene, with no runtime, security or
  correctness effect; moving it to an extension trait touches every guard impl,
  HTTP dispatch and the boot chain-validation, so it lands in `2.0`.

## [0.5.0] - 2026-07-19

### Changed

- **WS message handlers are transactional.** `WsDataContext` installs the
  same lazy executor per message: a read-only or non-querying message opens
  no transaction, while a writing handler commits on a success reply and
  rolls back on an error reply — a multi-write handler that fails mid-way no
  longer half-persists. Guest connections stay fail-closed (deny-all without
  an ambient ability).
- **Mutating HTTP requests no longer pay `BEGIN`/`ROLLBACK` before guards
  run.** `DbContext` now installs a lazy executor (`Executor::Lazy`): the
  request transaction opens on the **first data-layer touch**, so a
  guard-denied POST — or any mutating request that never queries — opens
  zero transactions and consumes no Postgres transaction slot. Commit /
  rollback semantics, the `MappedError` rollback, and the escaped-executor
  fail-loud check are unchanged.
- **`Creatable::create` is atomic on every executor shape.** On a pool
  executor (a WS message handler, a bare `with_executor`) the insert and its
  SQL scope re-check now run in a local transaction — an out-of-scope create
  surfaces `RecordNotInserted` and persists nothing, instead of relying on
  the HTTP request transaction for the rollback.
- **`ThrottlerStore::hit` is async.** The Redis store awaits its round-trip
  on the request task instead of parking a runtime worker with
  `block_in_place` + `block_on` per rate-limit check (which also panicked on
  a current-thread runtime). Fail-closed behavior on a Redis outage is
  unchanged.
- **Guard chains are validated at boot from declared markers.** `Guard` gains
  `phase()` (authentication / authorization / other) and
  `produced_principal()` / `expected_principal()`. A chain listing authz
  before authn, or pairing an `AuthGuard` whose principal type differs from
  the `AbilityGuard`'s expected actor, now **fails boot with a named error**
  instead of answering 500 on every request; the old name-substring ordering
  heuristic is gone.

- **Response masking is cross-checked at run time.** `#[routes]` arms the
  response shaper by matching the `Authorize`/`Bind` parameter-type name; a
  renamed import (`use Authorize as Az`) used to disarm masking silently.
  Unarmed routes now carry a `MaskProbe`: when a masking extractor runs on a
  route whose shaper is not armed, the request fails closed with a logged
  `500` instead of shipping an unmasked body.
- **`Bind` / GraphQL `bind` no longer echo `DbErr` text to the client.** A
  failed by-id load logs the full error at `error` on `nest_rs::orm` and
  answers with an empty `500` (HTTP) / a generic `INTERNAL_SERVER_ERROR`
  extension (GraphQL), matching the `#[crud]` write mapper.

### Added

- **`nest_rs_authz::masked_reply`** — mask a handler's wire JSON with the
  ambient ability in one call, for surfaces with no automatic response
  shaper (a WS gateway reply, a hand-built payload). Same fail-closed core
  as the HTTP shaper and the GraphQL wrapper; the reference `users` WS
  gateway now uses it instead of ten hand-rolled masking lines.
- **`Creatable::create_from_active`** — insert a *prepared* `ActiveModel`
  through the same audited create path as `Creatable::create` (atomic
  insert + SQL scope re-check), for service methods that stamp server-side
  columns (the token's org id, a status default) before insert. The demo's
  users/posts services now use it instead of raw
  `ActiveModel::insert(&Repo::conn()?)`.

### Removed

- **Reserved cross-transport layer seams that were never invoked.**
  `Interceptor::wrap_graphql`/`wrap_ws` (with `GraphqlNext`/`WsNext`),
  `ExceptionFilter::catch_graphql`/`catch_ws`, and
  `Filter::filter_graphql`/`filter_ws` compiled but no macro or dispatcher
  ever called them — implementing one was a silent no-op. They are removed
  from the trait surfaces (along with the now-empty `graphql`/`ws` features
  of `nest-rs-interceptors`, `nest-rs-exception-filters`, and
  `nest-rs-filters`) until real wiring ships. Guards' cross-transport
  entries are unaffected; a global interceptor/filter still covers GraphQL
  and WS through the HTTP transport edge.

## [0.4.0] - 2026-07-19

### Changed

- **One error format at the HTTP boundary — RFC 9457
  `application/problem+json` everywhere.** Three shapes previously
  coexisted: the NestJS-style `{statusCode, error, message, details}`
  validation body, bare-text framework/service errors, and poem's
  plain-text transport errors (an unmounted-route `404`, a `413`). All
  now render as `ProblemDetails` (`type`/`title`/`status`, optional
  `detail`). Field-level validation errors ride as the RFC-9457
  **extension member** `errors`; `ServiceError`, guard denials
  (401/403/429, `Retry-After` preserved) and pipe rejections all map to
  the same envelope. `HttpTransport` installs a transport-edge boundary
  (`nest_rs_http::normalize_error_response`) that lifts any leftover
  raw plain-text error onto `problem+json` — a `Filter`/`ExceptionFilter`
  mapping (tagged `MappedError`) or a deliberately-typed body is left
  untouched, and internal (`5xx`) detail is dropped so no driver message
  reaches the wire. New `ProblemDetails::from_status` /
  `with_extension`.

### Added

- **The OpenAPI document is complete.** Previously skeletal — no query
  parameters, every path parameter a bare `string`, no security scheme,
  a lone `200` per operation. The generated document now carries: path
  parameters typed from the handler's `Path<T>` (a `Path<Uuid>` id is
  `string`/`format: uuid`), each `Query<T>` payload expanded into one
  query parameter per property (the `#[crud]` list op's `first`/`after`
  cursor is documented), a `bearerAuth` security scheme applied to
  guarded non-`#[public]` routes — including routes covered only by a
  `use_guards_global` pool — and per-route RFC 9457 error responses
  (401/403/404/409/422, each honest to what the route can produce)
  referencing a shared `ProblemDetails` schema. A new
  `NESTRS_OPENAPI__EMIT_DOCUMENT`/`DOCUMENT_PATH` config writes the
  document to disk at boot, the OpenAPI analogue of the GraphQL SDL
  emit, so a committed `openapi.json` stays fresh as a side effect of a
  dev run.

- **`HttpConfig.compression`** negotiates response compression (gzip /
  deflate / brotli / zstd) from each request's `Accept-Encoding` — one
  flag (`NESTRS_HTTP__COMPRESSION` or the pinned struct), off by default
  so a fronting proxy keeps ownership when it has it. A preflight
  (`OPTIONS`, no body) and an already-encoded response are left alone.

- **`Storage::get_stream`** downloads an object as a chunked byte stream
  instead of buffering the whole body ([`get_bytes`] collects), so a
  large media file flows to the client without ever sitting whole in
  process memory — feed it straight into a streamed HTTP body.

- **Streaming and multipart HTTP** are now first-class: poem's `sse`,
  `multipart` and `compression` features are enabled, so a handler can
  return `poem::web::sse::SSE` or a `Body::from_bytes_stream` response,
  or take a `poem::web::Multipart` upload, and `#[routes]` passes each
  through untouched. The demo's `audio` slice exercises all three
  (direct upload, streamed download, an SSE progress feed).

- **`nestrs g migration <name>`** scaffolds a SeaORM migration and
  registers it in **both** `crates/migrations/src/lib.rs` and
  `migrator.rs` — the `migrator.rs` vec is regenerated from the module
  list, so the two registrations can never drift (the one you forget by
  hand is the one that silently never runs).

- **`nestrs g resource --guarded`** scaffolds the hardened `#[crud]` +
  guards form (the `orgs/` shape) instead of the unguarded stub, for a
  workspace that already provides `AuthGuard` / `AuthzGuard` /
  `AuthzHttpModule`.

### Fixed

- **A duplicated concrete provider fails the boot.** Two modules (or a
  seed and a module) registering the same concrete type previously
  warned and silently last-write-wins — a wiring mistake that only
  surfaced as wrong behaviour. It now fails the boot with a named
  `DuplicateProviderError`, uniform with the other wiring checks. Keyed
  providers keep their documented last-write-wins, and `dyn Trait`
  bindings stay the intended override mechanism.

- **A missing `Ctx<T>` replies with a bare 500, not the Rust type.** The
  extractor built the response body from the internal Rust type name;
  that detail now goes to the logs and the client gets a bare 500.

- **A malformed relational rule fails ability construction instead of
  going fail-open.** `PredicateBuilder::related` rejects an invalid
  relation (composite key, or a relation not pointing at the declared
  related entity) with the `Deny` sentinel. In a `cannot(...)` that
  sentinel lowered to `1 = 0` and combined as `grant AND NOT(1 = 0)` —
  i.e. the restriction evaporated (fail-*open*). `AbilityBuilder::build`
  now returns `Result<Ability, MalformedRuleError>` and fails naming the
  faulty rule; the HTTP ability guard denies the request (fail-closed)
  when construction fails. A malformed grant, previously a silent
  deny-all, is surfaced the same way.

- **A scoped/transient provider's missing dependency fails the boot,
  not the first request.** The access graph only flagged *cross-module*
  reaches; a request-scoped or transient provider whose dependency was
  provided by no module at all passed boot and panicked at its first
  `get(...)` resolution — a runtime panic in place of the framework's
  hallmark named boot diagnostic. Lazily-built providers now report the
  names of what they inject, and the access-graph pass fails boot with a
  `MissingDependencyError` naming both the provider and the unmet
  dependency. A dependency provided imperatively (a hand-written
  `impl Module`) or by a lazy factory is still tolerated: the pass
  consults the actual registered set before declaring a dependency unmet.

- **An eagerly-built provider's missing dependency no longer panics
  before the graph check.** The synchronous register phase ran ahead of
  `validate_from_inventory`, so a missing dependency panicked with the
  generated `expect` message and preempted the named `AccessGraphError`.
  Construction now defers the miss to the graph pass, which reports the
  same unified `MissingDependencyError`; a genuine dependency cycle still
  panics with its cycle diagnostic naming the chain.

- **`#[crud]` writes return the right HTTP status.** A generated create
  / update / delete previously mapped every write failure to a blanket
  `500`, so a unique-constraint violation, an out-of-scope create the
  ability re-check rolled back, or a row that vanished mid-request all
  read as internal errors. The generated handlers now map a
  `DbErr` to its status — unique violation → `409`, `RecordNotInserted`
  → `403`, `RecordNotUpdated` / `RecordNotFound` → `404` — and a
  genuinely unexpected error to a `500` with an empty body (the driver
  message no longer leaks). A service with a manual create maps the
  unique violation to `ServiceError::conflict` for the same result.

- **Auto-resolved `has_many` relations are memory-bounded.** An
  `#[expose]`d `has_many` field's dataloader previously loaded *every*
  child of a parent (`.all()` with no `LIMIT`), so a relation with large
  fanout (`Org.posts` over millions of rows) could pull an unbounded
  result set into memory. The generated FK loader now caps its batch
  query at `RELATION_LOAD_CAP × keys` and truncates each parent's bucket
  to `RELATION_LOAD_CAP` (100), logging a `warn` when it does. A relation
  that legitimately exceeds the cap should be a paginated
  `#[field_resolver]`, not an auto-resolved list.

## [0.3.0] - 2026-07-16

### Added

- **Social login with an open provider contract.** The new
  `nest-rs-social` crate makes social login a first-class capability.
  `SocialProvider` is flow-owning — `authorize` / `exchange` default to
  the shared PKCE/CSRF flow, so a standard provider implements only
  `profile`, while a deviating one (Apple's ES256 client secret)
  overrides a step without changing the trait. Ships first-party GitHub
  and Google; a third party publishes their own provider as an
  independent crate through the same seam. Discovery is link-time and
  module-gated: an unimported provider stays inert with a boot warn, and
  a duplicate or disagreeing key fails boot rather than silently
  shadowing a login provider. Identity keys on the provider's stable
  `(provider, subject)` pair, not the email, so a user who changes their
  provider email keeps their account.
- **Keyed providers.** `#[inject(key = "…")]` fields and `provide_keyed`
  let several instances of one concrete type coexist under a
  `ProviderKey`. The access graph validates each keyed dependency
  against the global keyed set at boot, naming both type and key on
  failure. Used by the keyed OAuth clients behind social login.
- **Request-scoped providers inside GraphQL and MCP handlers.**
  `nest_rs_graphql::Scoped<T>` and `nest_rs_mcp::Scoped<T>` resolve an
  `#[injectable(scope = request)]` provider from inside a resolver or
  tool body, falling through to singletons — so both transports share
  the per-request resolution model HTTP already had.
- **Type-safe queue identity.** `#[queue(name = "…", job = …)]` declares
  a `QueueName` unit struct carrying both the wire name and the job
  type. Both sides name the type (`push_to::<Q>`,
  `#[process(queue = Q)]`) and the macro asserts the process method's
  job argument matches, so a typo is a compile error instead of a job
  that silently never drains. The stringly-typed form still works.
- **Redis-backed throttler.** `RedisThrottler` puts the fixed-window
  counter in Redis so N replicas share one budget per client instead of
  N× the limit. The window advances in a single atomic Lua script (one
  round-trip, no check-then-act race) and fails closed on a backend
  outage.
- **Per-argument pipes on every transport.** `Piped<P, T>` / `Valid<T>`
  bind on GraphQL, WebSockets, and queue handlers (value-form carriers in
  `nest-rs-pipes`, stripped by `#[resolver]` / `#[messages]` /
  `#[processor]`); HTTP keeps its extractor forms. A rejection surfaces as
  the transport's native error (GraphQL error, WS error frame, job error).
- **Relational predicate scoping.** `p.related::<R, _>(relation, |r| ...)`
  scopes an entity by a condition on a related entity through a typed
  SeaORM relation — lowered to a semi-join (`IN` subquery / correlated
  `EXISTS`), with boot-time guards on the relation target and key arity.
- **Scalar predicate variants.** `p.ne` / `p.lt` / `p.lte` / `p.gt` /
  `p.gte` (`Cmp`) and `p.is_null` / `p.is_not_null` (`IsNull`).
- **Action-typed authorization proofs.** `Authorized<E, A>` carries the
  action as a type parameter, with `bind_required::<S, A>` as the GraphQL
  subject binder — a `Read` proof no longer passes where an `Update` proof
  is required.
- **Generic client-credentials grant helper** in `nest-rs-authn`.
- **Selective `#[crud]` ops with segregated write traits.**
  `ops = [list, get, delete]` synthesises exactly those; the write half
  lives in opt-in `Creatable` / `Updatable` / `Deletable` traits, so a
  read-only resource declares no placeholder input types.
- **Generated list operations paginate by default**, with a hard
  backstop on page size.
- **`ServiceError` carries real 4xx variants** plus `Internal` — features
  stop redefining plumbing errors.
- **`resolve_unique_slug()`** for soft-deletable entities and a **`now()`**
  timestamp helper in `nest-rs-seaorm`.
- **Actor identity on the request span** — denials are attributable
  without per-site threading.
- **Per-job spans and start/ok/fail events** in the Redis queue
  consumer.
- **`#[non_exhaustive]` on the eight public error enums**, so a new
  variant is no longer a breaking change, and compiler-enforced
  unsafe-freedom via `[workspace.lints] unsafe_code = "forbid"`, opted
  into workspace-wide with three documented exceptions.
- **Bounded WebSocket connection lifetime** (`WsConfig`, default 4h)
  and an OpenAPI enable toggle.
- **`nest-rs-testing` auto-loads the project `.env`** for e2e, so every
  boot sees the same URLs the app does — no duplicated test env file.
- `nestrs run db down [N]` reverts N migrations (default one step).
- `nestrs new` ships a `compose.yml` in the workspace scaffold.

### Changed

- **Minimum supported Rust is now 1.96** (was 1.95), pinned explicitly
  in `rust-toolchain.toml` and the workspace `rust-version`.
- **`nest-rs-macros` is renamed `nest-rs-core-macros`.** Apps consuming
  the framework through the `nest-rs` umbrella are unaffected; a direct
  dependency on the old name must be repointed.
- **`async-graphql` is pinned to `=7.2.1`** (exact, not caret): the
  resolver codegen spells out a public-but-internal registry literal
  that a minor bump can silently change. Guarded by a compile-time
  canary and an SDL snapshot test; the bump procedure lives in the
  crate docs.
- **`ConfigService::var` is renamed `var_name`** — it returns the
  variable's name, not its value, and shadowed the meaning of
  `std::env::var`.
- **`nest-rs-config` no longer mutates the process environment** on the
  live path — it reads an in-crate `.env` map, with the real
  environment winning.
- **Transport dependencies are feature-gated** (interceptors, filters,
  exception-filters, guards) so an HTTP-only app skips the GraphQL and
  WebSocket stacks.
- **Access and create authorization are decided in SQL.**
  `CrudService::access` re-checks the primary key against
  `condition_for(action)` in the database instead of an in-memory
  `Ability::can` — one source of truth with the list filter, and what
  makes relational rules enforceable on the by-id and create paths.
- **GraphQL posture is mandatory and visible.** Every operation declares
  `#[authorize(Action, Entity)]` (class gate + automatic response
  masking) or `#[public]`; an operation without a posture does not
  compile, and an `Authorized<E>` parameter is not accepted as a
  standalone posture.
- **Transfer objects are named by the boundary they cross** — REST
  `Dto`, queue `Command` / `Event`, GraphQL `Input`; entity-derived
  CRUD forms stay bare (`CreateUser`), with file-role placement to
  match.
- **Framework and product split into two Cargo workspaces** (root
  `crates/nest-rs-*` vs `demo/`), the demo consuming the framework by
  relative path.

### Fixed

- **Security: a pre-release audit pass across the framework.** All authz
  denials log at `warn`; a throttler brute-force bypass is closed
  (per-bucket window + route-scoped key); JWT `aud`/`iss` are enforced;
  a failed predicate fail-closes to `Deny` instead of panicking per
  request; OAuth state compares in constant time; submitted values are
  stripped from validation-error responses; masked responses are
  retained by a static expose set.
- **Login separates store outages from credential mismatches.** Every
  `DbErr` on the login path used to map to an invalid-credentials 401,
  hiding outages and locking out returning OAuth users. Store failures
  now surface as `AuthError::Unavailable` (500, logged at `error`),
  kept distinct from the opaque credential rejection.
- Boot fails with a named error on a duplicate controller prefix
  (previously a panic).
- Lifecycle hooks whose provider is unreachable are surfaced at boot
  instead of silently never running.
- `#[crud]` GraphQL operation names derive from the snake_case entity
  name.
- `#[public]` is rejected on WS message handlers; OAuth login input
  hardened.

### Documentation

- Content overhaul: a linear onboarding journey, a request-lifecycle
  page, corrected decorator docs with macro expansion sketches, and a
  new Entities reference page.
- Shipped `STYLE.md`, page templates, and a docs lint gate.

## [0.2.0] - 2026-06-10

### Added

- **CLI generators (`nest-rs-cli`).** New scaffolding binary with
  `nestrs g feature/resource/<transport>` — transactional scaffold core that
  generates files and auto-wires modules, with context detection.
- **`nestrs run` task front door.** Single entry point that forwards to `just`
  recipes, with first-run toolchain bootstrap (installs `just`, `bacon`,
  `cargo-nextest`, binstall-preferred; opt out via `--no-bootstrap` /
  `NESTRS_NO_BOOTSTRAP`).
- **Publish suite.** Exemplar workspace with org-scoped posts spanning REST,
  GraphQL, WebSockets, queue, and MCP apps.

### Changed

- **Unified layer pool.** Guards, pipes, interceptors, filters, and
  exception-filters now resolve through a single deduplicated pool per family
  (execute exactly once per request; broadest scope wins).
- **Apps renamed** and **service-naming conventions** tightened across the
  workspace (`svc` / `<name>_svc` injection naming).

### Fixed

- **Security: hardened authn/authz, transports, the data layer, and the CLI**
  against several edge cases.
- **Security: fail closed on unwired MCP** and **enforce a minimum HS256 secret
  length** at boot.
- Access-log `duration_ms` now rounded to microsecond precision.

### Documentation

- Added the Lifecycle fundamentals page and a dedicated packages page.
- Routed all task examples through `nestrs run`.
- Refined the splash hero / landing page (mobile layout, hello code-tabs demo,
  access-log terminal lines) and slimmed the README toward contributors,
  pointing users to nestrs.dev.

## [0.1.0] - 2026-06-08

Initial public release of the nestrs framework — an opinionated Rust framework
where the developer writes business logic and the framework carries the
cross-cutting concerns (authn, authz, row-level filtering, transactions, edge
validation, discovery, lifecycle).

### Added

- **Composition & DI.** Type-id container with `#[inject]` fields, `#[module]`
  composition, four-phase `App::builder().build()`, singleton/request/transient
  scopes, and a compile-time + boot-time access graph.
- **Request layers.** Guards, pipes, interceptors, filters, and exception
  filters with symmetric scopes (global / controller / handler) and TypeId
  dedup.
- **Transports.** HTTP (`nest-rs-http`), GraphQL (`nest-rs-graphql`),
  WebSockets (`nest-rs-ws`), queue (`nest-rs-queue` + `nest-rs-redis`),
  scheduler (`nest-rs-schedule`), MCP, and OpenAPI (`nest-rs-openapi`).
- **Authn / authz.** `nest-rs-authn` (strategies, `AuthGuard`, `JwtService`)
  and `nest-rs-authz` (abilities, ability guards, response masking) with
  bridges per transport.
- **Data layer.** `nest-rs-seaorm` with transparent ability-scoped `Repo`,
  ambient executor/transaction `task_local!`s, route-model binding, and
  auto-resolved GraphQL relations from `#[expose]`.
- **Supporting crates.** Pipes, events, health, throttler, config,
  opentelemetry, and the `nest-rs` umbrella crate (`use nest_rs::prelude::*`).
- **`nest-rs-*` naming alignment** across directories, packages, and imports;
  framework-owned error types.
- Rust 1.95 / edition 2024; tag-based release CI with the `mold` linker on
  Linux.

[Unreleased]: https://github.com/YV17labs/NestRS/compare/v6.1.0...HEAD
[6.1.0]: https://github.com/YV17labs/NestRS/compare/v6.0.0...v6.1.0
[6.0.0]: https://github.com/YV17labs/NestRS/compare/v5.1.0...v6.0.0
[5.1.0]: https://github.com/YV17labs/NestRS/compare/v5.0.0...v5.1.0
[5.0.0]: https://github.com/YV17labs/NestRS/compare/v4.0.0...v5.0.0
[4.0.0]: https://github.com/YV17labs/NestRS/compare/v3.1.0...v4.0.0
[3.1.0]: https://github.com/YV17labs/NestRS/compare/v3.0.0...v3.1.0
[3.0.0]: https://github.com/YV17labs/NestRS/compare/v2.1.0...v3.0.0
[2.1.0]: https://github.com/YV17labs/NestRS/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/YV17labs/NestRS/compare/v1.3.0...v2.0.0
[1.3.0]: https://github.com/YV17labs/NestRS/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/YV17labs/NestRS/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/YV17labs/NestRS/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/YV17labs/NestRS/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/YV17labs/NestRS/compare/v0.5.0...v1.0.0
[0.5.0]: https://github.com/YV17labs/NestRS/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/YV17labs/NestRS/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/YV17labs/NestRS/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/YV17labs/NestRS/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/YV17labs/NestRS/releases/tag/v0.1.0
