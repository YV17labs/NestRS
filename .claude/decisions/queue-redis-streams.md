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
