# The Redis queue is nestrs's own, on Redis Streams

7.0 drops apalis. apalis-redis 0.7.4 stops compiling on Rust 1.100 (the
never-type fallback, rust-lang/rust#148922), its maintainer ships only 1.0
release candidates whose dirty-exit defect (apalis-redis#103) stalls every
surviving worker, and oxana was tried and abandoned on a dead-peer sweep race.
Half of the 7.0 adapter existed to work around apalis: a lifted attempt cap
written into its private record, a startup sweep that took live peers' jobs, an
acknowledgement dropped at drain, a fetch that lost jobs past Lua's `unpack`
limit, whole-second scheduling on the host clock, and a delivery guard (lease,
settled mark) filtering the duplicates all of that produced. nestrs now writes
the transport on `redis` directly, and none of those keys remain.

**Streams with one consumer group per queue, not lists.** `XREADGROUP` delivers
an entry, records its owner and starts its idle clock in one command, so a
worker dying between fetch and lease cannot happen; `XAUTOCLAIM` recovers what a
dead owner held and counts the redelivery. Lists (BullMQ, Asynq) build that
lease by hand, and their recurring incidents are stale-owner races in exactly
that code (BullMQ lock mismatches, Sidekiq super_fetch #4611, River #1302). A
lease with a delivery count is also the shape of the next members of the family:
JetStream (AckWait, `+WPI`, NAK, MaxDeliver) and Kafka share groups (acquisition
lock, delivery count, release/reject).

**Every transition is one script, fenced on the pending entry.** `XACK` and
`XCLAIM … 0` ignore who owns an entry, so a renewal or an outcome first reads
the entry's owner and delivery count with `XPENDING` and writes nothing unless
both are still what this worker fetched; a reclaim bumps the count, a renewal
(`JUSTID`) does not. The outcome, its give-back and its hand-back are one script,
so the old "give the start back first" ordering has nothing left to order.
Fencing is what fixed the incidents above (BullMQ's lock token, Oban's
`attempted_at`).

**No delivery guard, no settled mark, no time-to-live net.** A settled job is
acknowledged and deleted in the script that settles it, so it cannot be
delivered again; a second runner exists only when a lease lapses under a live
worker (a freeze or a partition past `lease`), and the fence stops its outcome
from landing. Every record is removed by the transition that ends its job, and a
script replicates whole, so no record outlives its job and none needs an expiry
to bound it. Delivery stays at least once.

**One queue, one hash slot.** Keys are `nestrs:queue:{<queue>}:<structure>`: the
braces put a queue's keys on one Redis Cluster node, so the layout never has to
break for Cluster; per-job facts are fields of per-queue hashes, so a script
names every key it touches. Cluster itself waits for a caller (an issue): the
connection is not a cluster client.

**Redis's clock decides when a job is due**, in milliseconds (`TIME` inside the
script), never a host's. **The floor stays Redis 6.2**: nothing here needs a
later command, and every newer one (`XACKDEL` 8.2, `XREADGROUP CLAIM` 8.4,
`XNACK` 8.8) is absent from every Valkey release, so they can only ever be
optimisations. **A blocking fetch has a connection of its own**: on the shared
multiplexed socket it would stall every caller.

**6.x jobs are drained, never moved.** The 7.0 layout under apalis never
shipped, so nothing migrates from it. 6.x structures are apalis-0.6 lists and
sets that cannot be renamed into a stream: the worker still refuses to start
beside them, read-only, and the upgrade path is to drain them with 6.x workers.

**Status (2026-10-03): decided, not landed.** The code still runs apalis, so
`queue.md`, the docs and the 6.x upgrade guide describe the code until the
rewrite lands and updates them in the same change.

## 2026-10-05 — the facts checked again, and the contract the family shares

**Why apalis goes, with its sources.**

- **Rust 1.100 refuses apalis-redis 0.7.4.** rust-lang/rust#155499 ("stabilize
  never type", milestone 1.100.0, tracked by #148922) makes the never-type
  fallback `!` in every edition, and `storage.rs` lines 733, 756, 777, 841 and 847
  await a script or command whose result type only that fallback chose
  (`!: FromRedisValue`). `cargo +beta check` on 2026-10-04 fails there and
  nowhere else in the queue.
- **The ecosystem is pre-release and has no Kafka.** On 2026-10-04 every apalis
  1.0 crate was a release candidate: apalis and apalis-core 1.0.0-rc.10,
  apalis-redis 1.0.0-rc.10, apalis-nats 0.1.0-rc.5 (no stable release). No
  Kafka backend exists: `apalis-kafka` is not on crates.io, none of the 22
  repositories of github.com/apalis-dev is one, and the one mention is a feature
  request (apalis-dev/apalis#718).
- **#103 is fixed; what made us leave is not.** apalis-redis#103 (in-flight sets
  created as hashes and swept as sets, which wedged every surviving worker after
  a dirty exit, reported again as #108) was fixed by #104 and first shipped in
  1.0.0-rc.10 (2026-09-29). In rc.10's code the register_worker guard of #76 is
  still there: a worker re-registering within its threshold errors and its
  stream ends (`lua/register_worker.lua` lines 15-20 at tag 82d76cf). `unpack`
  is still there too, and worse: `lua/fetch_next.lua` promotes every due
  scheduled job with an unbounded `zrangebyscore` and unpacks the lot, so 8,000
  due jobs stall every fetch (lines 26-38). apalis-redis#17 is not about EVAL: it
  sends EVALSHA, but rebuilds the script and its hash on every call and never
  loads it ahead.
- **We want more drivers.** NATS JetStream and Kafka share groups come later,
  with a page on writing one. That page only means something over a contract
  nestrs owns, so the port owns it, and this entry projects it on both.

**Corrections to the entry above.** 6.x ran apalis-redis 0.7.4, never 0.6 (both
v6.0.0 and v6.1.0 lock it), so 6.x structures are apalis-0.7 lists and sets. Past
Lua's `unpack` limit it was apalis's orphan sweep that lost jobs (it pops before
it fails); the fetch only stalled. The public oxana race (pragmaplatform/oxana#157)
was fixed in 2.1.7 on 2026-09-14; 2.3.0 still resurrects a process judged dead by
one read on the sweeper's clock, in round trips no fence covers, which is the
class this design's fence closes. Valkey 9.2.0-rc1 ships `XACKDEL` and `XDELEX`,
so "absent from every Valkey release" holds for its GA lines only. `TIME` is
Redis's wall clock: after a failover the new primary's clock judges every lease
and due instant, so Redis hosts keep NTP. `XNACK`'s `SILENT` lowers and
`RETRYCOUNT` sets the delivery count, so neither may ever be used on an entry a
fence reads. Recovery is not `XAUTOCLAIM`: on 6.2 it returns a pending entry whose
record was deleted as a bare nil with no id, which no script can acknowledge, so
a bounded `XPENDING` page names the ids and `XCLAIM` takes them.

**The port runs the consume loop; a backend is a `JobConsumer`.** Permits per
method, receiving only for free permits, one task per delivery, renewal, the
in-process wait of a backend that cannot file a record due later, the drain and
its window are the same on every backend, so `nest-rs-queue` runs them in its
`QueueWorker` transport. A backend implements `prepare`, `receive`, `renew`,
`settle`, `maintain`, `close` and an optional `checkpoint`, and ends a delivery
with one `Disposition`: `Complete`, `Retry`, `Defer`, `Requeue` or `DeadLetter`.
Every write a delivery makes is fenced where the backend can fence, and a fenced
write that did not land answers `Lost`.

**Two budgets, never one.** `retries` counts attempts that answered, carried in
the envelope as today. A delivery that ended without an answer — its worker
killed, frozen, or cut off from Redis past the lease — is counted by the
record's own delivery count, which every re-filing (`Retry`, `Defer`, `Requeue`)
resets, and the third such delivery dead-letters the job (`STALL_LIMIT`).
Counting starts per job instead (the 2026-10-04 draft) dead-lettered a
`retries = 0` job that never ran on five paths: an admission whose reply was
lost, a kill after admission, a drain whose hand-back missed Redis, a settle
whose reply was lost, a lease lapsing under a live worker. BullMQ splits the same
way (`attemptsMade` against `attempts`, `stalledCounter` against
`maxStalledCount`). Since the stream's pending list records what a fetch
delivered, no queue command needs to wait on socket liveness any more: every one
is bounded by the connection's budget.

**A lost lease cancels its attempt.** When the fence answers `Lost`, or no
renewal was confirmed for a whole lease, the port drops the attempt (its line
files `cancelled`) and requeues it fenced: the holder that reclaimed it runs it,
and the reclaim costs one stall, not the job.

**A later backend declares what it lacks; it never claims what it has.** The 7.0
contract guarantees fenced transitions, a renewable lease, a delivery count per
record, re-filing on `Retry`, `Defer` and `Requeue`, and a record filed due later
with `DelayedPush`. A backend that cannot keep one declares the shortfall on its
`QueueBackend` (`Unfenced`, `NoRenew`, `ApproximateCount`), so no 7.0 driver ever
changes behaviour. What the port does for each: `Unfenced`, `Held` means "sent",
only the local lease deadline protects, and `Checkpoint` is refused; `NoRenew`,
`retries > 0` is refused without `DelayedPush`. A `Disposition` variant added
after 7.0 reaches only a backend declaring the capability that names it.

**The family, on paper.**

| Contract | Redis Streams (7.0) | NATS JetStream ≥ 2.12 | Kafka share groups ≥ 4.2 |
|---|---|---|---|
| receive, lease, count | `XREADGROUP >` on a connection of its own; the pending list's owner, idle time and count | pull fetch with `expires = wait`; lease = AckWait; count = NumDelivered | ShareFetch, `record_limit`, one share consumer per permit; lease = acquisition lock; DeliveryCount, approximate |
| renew | fenced `XCLAIM … 0 JUSTID` | `+WPI`, unfenced | `RENEW` (4.2, explicit mode); librdkafka has none: `NoRenew` |
| Complete | fenced `XACK` + `XDEL` | `+ACK` (double ack) | `ACCEPT` |
| Retry | next record filed due later, then acknowledged, one script | next record re-published (2.12 schedule, producer's clock), then `+ACK` | the port waits holding the lock (`RENEW`), then re-produce + `ACCEPT` |
| Defer, Requeue | same record re-filed, then acknowledged | re-published, then `+ACK` (a NAK would count) | re-produced, then `ACCEPT` (`RELEASE` would count) |
| DeadLetter | the dead stream and the acknowledgement, one script | dead subject, then `+TERM <reason>`: twice at most | own dead topic, then `REJECT` (KIP-1191's broker DLQ targets 4.4) |
| fencing | atomic, in Lua | none: a stale ack lands on the current holder (nats-server#4786, open): `Unfenced` | native per member (`InvalidRecordStateException`) |
| broker cap | none | MaxDeliver set to −1 | `share.delivery.count.limit` 2-10 (25 max): refused at boot when `retries` and `STALL_LIMIT` cannot fit |
| Rust client | `redis` 1.7 | async-nats 0.50 | none supports share groups (librdkafka 2.15's share consumer is a C-only preview) |
| wiring | `RedisQueueModule`, `<PREFIX>_REDIS__QUEUE__*`, group `workers` | `NatsQueueModule`, `<PREFIX>_NATS__QUEUE__*`, one durable consumer per queue on a work-queue stream | `KafkaQueueModule`, `<PREFIX>_KAFKA__QUEUE__*`, one share group per queue |

**The rest, decided with the owner.**

- `QueueModule` attaches the port's `QueueWorker` and owns its drain window
  (`<PREFIX>_QUEUE__SHUTDOWN_TIMEOUT_SECS`); `RedisQueueModule` binds the
  producer and the consumer. `RedisWorkerModule` and `<PREFIX>_REDIS__WORKER__*`
  go; the 6.1 variable left set is said once at boot, naming its replacement.
- The client is `redis` (redis-rs) 1.7: the Rust client redis.io documents,
  28 million downloads in 90 days against 2.2 million for fred, whose last
  release is 2025-02-27, and the one apalis-redis, bullmq-official (pinned
  `=1.7.1`), sidekiq-rs, loco and rsmq build on. 0.32 crossed the freshness bar,
  and its types are in `nest-rs-redis`'s API, so 7.0 is the major that moves it.
- Dead letters live in a stream of their own, bounded by age and length, the job
  kept with the value-free reason its line rendered, and the docs print the one
  command that files a dead letter back, run by a test.
- The worker refuses to start beside a 6.x job that is waiting, in flight or past
  its due instant (no 6.x worker drains it); one only held for later is a `warn`
  naming the count and the latest due instant, so a 30-day delay no longer holds
  a 7.0 queue for 30 days.
- "Settle" is one concept in nestrs: an attempt's outcome made final in the
  store that holds it — its writes in the database (`JobSettlement`), then its
  delivery in the queue (`JobConsumer::settle`, OpenTelemetry's word).
- At-least-once covers what Redis persisted and replicated. A deployment runs
  `noeviction` or a `volatile-*` policy, never `allkeys-*`; only the throttle's
  window carries an expiry.
- The oldest Redis and Valkey the docs claim is the oldest `just test` passed on
  at the release, recorded with the versions in the changelog — no server matrix
  job (removed by 20786bae).
- Third-party drivers run the port's behaviour kit (`nest-rs-testing`, feature
  `queue`), the same cases the Redis driver runs.

**Prior art read, nothing copied.** No code was copied, so no notice is owed.
What it taught: a per-delivery token (BullMQ #1575); renewing through the drain
(BullMQ #2258, #2259); a unique key outliving its job silently drops later jobs
(BullMQ #4311, #4768); a blocking read wedged after a reconnect (BullMQ #4484);
recovery by process rather than by delivery takes live peers' jobs (Asynq #90,
#170; Sidekiq super_fetch #4611, never root-caused, #5435 boot sweeps); one
undecodable record wedges recovery (Asynq #1140); a stale settlement resurrects a
finished job (Oban #1496, fixed by fencing on `attempted_at` in 2.24.1; River
#1302, fixed by #1373 in 0.48.0). Licences: BullMQ and Asynq MIT, Oban Apache-2.0,
River MPL-2.0, Sidekiq LGPL-3.0 and commercial, apalis MIT. bullmq-official, an
MIT Rust port, was weighed and refused: it is a list design with exact pins and no
Cluster.

Amended the same day by the owner: 7.0 is a new version and builds nothing for
6.x. The read-only 6.x detector goes with its tests and its upgrade procedure;
the upgrade guide says to drain each queue with its 6.x workers before 7.0
workers take over, and 7.0 reads no 6.x key.

## 2026-10-05 — landed, and what the code taught

The rewrite landed as planned, with these choices the plan left open:

- **Reclaim happens in a read, for free permits only.** A worker takes a lapsed
  lease over only when it asks for jobs, and only as many as it can start: one
  taken in an upkeep with no permit to run it would lapse again, unrenewed, and
  spend a stall of the job's.
- **The group starts at the stream's first entry** (`XGROUP CREATE … 0
  MKSTREAM`), so jobs pushed before any worker started are read; a read meeting
  `NOGROUP` — the stream deleted by hand or flushed — makes it again.
- **A worker's consumer name is a UUID v7**, it leaves each group when it stops
  holding nothing, and a consumer holding nothing and silent for an hour is
  swept, so a crashed replica leaves no consumer behind for long.
- **The throttle sends `GET`, `PTTL` and `SET` only**, and a deferral reads with
  `HGET`: each command a rule allows must be one a role sends, so the ACL lines
  are exact, which the suite proves through `MONITOR`.
- **The binding's consumer reads two factory outputs** — the connection and its
  config — so `nest-rs-core` gained
  `ContainerBuilder::provide_declared_factory_after_both`; `after` took one type.
- **A dead letter is filed back by one `EVAL` the delivery page prints**, run by
  a test as printed.

## 2026-10-05 — corrections to the entries above

- **Lists do not build the lease by hand.** BullMQ and Asynq take the lease in
  the same script as the list move; what Streams give is the pending list's
  owner and delivery count kept by Redis itself, which the fence reads. River
  #1302 is a Postgres rescuer's stale snapshot, and Sidekiq #4611 was closed
  without a root cause: neither is a list's lease race.
- **Fencing fixed one of the incidents cited, not all.** River #1302 was fixed by
  fencing (PR #1373); BullMQ #2258 by extending locks through the close (PR
  #2259); Sidekiq #4611 never was. Oban's `attempted_at` fence (2.24.1) fixed
  oban#1496, which the prior art lists.
- **A 6.1 variable left set is not said at boot.** The owner's amendment of the
  same day builds nothing for 6.x, so
  `<PREFIX>_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS` is read by nothing, in
  silence — a namespace no config claims may be another binary's — and both
  upgrade pages say to set `<PREFIX>_QUEUE__SHUTDOWN_TIMEOUT_SECS`.
- **The floor the docs claim is the oldest the suite passed on**: Redis 6.2.24
  and Valkey 9.1.2 at this landing, with 8.6.3 in the dev container.
