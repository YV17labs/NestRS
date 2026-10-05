//! [`Scripts`] — every Lua script the Redis queue runs, one per transition.
//!
//! **A transition is one script**, so what it writes lands whole or not at all
//! and replicates whole. **Every write a delivery makes is fenced** on its
//! pending entry: the script first reads the entry's owner and delivery count
//! (`XPENDING`) and writes nothing unless both are still what this worker
//! received, because `XACK` and `XCLAIM` ignore who owns an entry. A reclaim
//! bumps the count; a renewal (`JUSTID`) does not. **Redis's clock decides
//! when a job is due** (`TIME`, in milliseconds), never a host's.
//!
//! A script never echoes an argument into an error: a record is a payload.
//! Each is sent as `EVALSHA`, loaded once, and never inside a pipeline, where a
//! script Redis has not cached could not be loaded and sent again.

use std::sync::LazyLock;

use redis::Script;

/// Every script's first line: its writes are replicated as effects, not the
/// script — which Redis 6.2 needs before a write that follows a command whose
/// answer varies (`TIME`, `XPENDING`, `XINFO`), when a deployment turned
/// `lua-replicate-commands` off. Later servers do it always, and take the call
/// as a no-op.
macro_rules! effects {
    () => {
        "redis.replicate_commands()
"
    };
}

/// `now()`: Redis's clock, in milliseconds.
macro_rules! now {
    () => {
        "local function now()
  local t = redis.call('TIME')
  return tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
end
"
    };
}

/// `pending(entry)`: the entry's line in the group's pending list, `nil` when
/// it has none — the group gone with it included.
///
/// `KEYS[1]` is the stream, `ARGV[1]` the group.
macro_rules! pending {
    () => {
        "local function pending(entry)
  local line = redis.pcall('XPENDING', KEYS[1], ARGV[1], entry, entry, 1)
  if line.err then
    if string.sub(line.err, 1, 7) == 'NOGROUP' then
      return nil
    end
    error(line.err)
  end
  return line[1]
end
"
    };
}

/// `holds(entry, count)`: whether this worker still holds the delivery it
/// received as `entry`, delivered `count` times then.
///
/// `ARGV[2]` is this worker's consumer name.
macro_rules! holds {
    () => {
        "local function holds(entry, count)
  local line = pending(entry)
  return line ~= nil and line[2] == ARGV[2] and line[4] == tonumber(count)
end
"
    };
}

/// `forget(job)`: let go of every record of a job that ended, but the entry
/// it ran from — its place in `jobs`, its unique key if it still holds it, its
/// deferral and its checkpoint.
///
/// `KEYS` from 2: entries, due, delayed, unique, claims, deferred, checkpoints.
macro_rules! forget {
    () => {
        "local function forget(job)
  redis.call('HDEL', KEYS[2], job)
  local key = redis.call('HGET', KEYS[6], job)
  if key then
    if redis.call('HGET', KEYS[5], key) == job then
      redis.call('HDEL', KEYS[5], key)
    end
    redis.call('HDEL', KEYS[6], job)
  end
  redis.call('HDEL', KEYS[7], job)
  redis.call('HDEL', KEYS[8], job)
end
"
    };
}

/// `file(job, record, after)`: file a job's record on the stream, or held back
/// until `after` milliseconds from now, carrying its first deferral.
///
/// `KEYS` from 1: jobs, entries, due, delayed, …, deferred (7).
macro_rules! file {
    () => {
        "local function file(job, record, after)
  if after > 0 then
    redis.call('ZADD', KEYS[3], now() + after, job)
    redis.call('HSET', KEYS[4], job, record)
    redis.call('HDEL', KEYS[2], job)
    return
  end
  local deferred = redis.call('HGET', KEYS[7], job)
  local entry
  if deferred then
    entry = redis.call('XADD', KEYS[1], '*', 'job', job, 'record', record, 'deferred', deferred)
  else
    entry = redis.call('XADD', KEYS[1], '*', 'job', job, 'record', record)
  end
  redis.call('HSET', KEYS[2], job, entry)
end
"
    };
}

/// File a push's jobs, unless one of them carries a unique key another job
/// holds — then nothing is filed, and the answer is that job's position (from
/// 0) and the holder's id. `0` once every job is filed.
///
/// `KEYS`: jobs, entries, due, delayed, unique, claims, deferred.
/// `ARGV`: how long the jobs are held back, in milliseconds (`0`: due now),
/// then `job`, `record`, `unique key` (`''`: none) for each job.
const PUSH: &str = concat!(
    effects!(),
    now!(),
    file!(),
    "local after = tonumber(ARGV[1])
for at = 2, #ARGV, 3 do
  local key = ARGV[at + 2]
  if key ~= '' then
    local holder = redis.call('HGET', KEYS[5], key)
    if holder then
      return {(at - 2) / 3, holder}
    end
  end
end
for at = 2, #ARGV, 3 do
  local job, key = ARGV[at], ARGV[at + 2]
  file(job, ARGV[at + 1], after)
  if key ~= '' then
    redis.call('HSET', KEYS[5], key, job)
    redis.call('HSET', KEYS[6], job, key)
  end
end
return 0
"
);

/// Cancel a job that has not started: `1` when it was waiting — on the
/// stream, undelivered, or held back — and now never runs; `0` when it is
/// running, ended, or unknown, touching nothing.
///
/// `KEYS`: jobs, entries, due, delayed, unique, claims, deferred, checkpoints.
/// `ARGV`: the group, the job's id (`''`: the job holding the unique key),
/// the unique key (`''` beside an id).
const CANCEL: &str = concat!(
    effects!(),
    pending!(),
    forget!(),
    "local job = ARGV[2]
if job == '' then
  job = redis.call('HGET', KEYS[5], ARGV[3])
  if not job then
    return 0
  end
end
local entry = redis.call('HGET', KEYS[2], job)
if entry then
  if pending(entry) then
    return 0
  end
  redis.call('XDEL', KEYS[1], entry)
elseif redis.call('ZREM', KEYS[3], job) == 1 then
  redis.call('HDEL', KEYS[4], job)
else
  return 0
end
forget(job)
return 1
"
);

/// Take up to `ARGV[4]` deliveries whose lease lapsed — pending for at least
/// `ARGV[3]` milliseconds without a renewal — for this worker, from one page of
/// at most `ARGV[6]` pending entries after `ARGV[5]`: `{taken, vanished, next}`,
/// each taken one as `{entry, delivery count, fields}`, and `next` where the
/// next look starts — the last entry read, or `-` once the page ran past the
/// list's end. An entry deleted under its pending line — by hand, since no
/// script deletes one still pending — is let go and named in `vanished`: Redis
/// 6.2 answers it as a nil, later servers drop it from the pending list and
/// answer nothing.
///
/// `KEYS`: jobs. `ARGV`: the group, this worker, the lease, the most to take,
/// where the page starts (`-`, or `(` and an entry), the page's length.
const RECLAIM: &str = concat!(
    effects!(),
    r"
local lines = redis.pcall('XPENDING', KEYS[1], ARGV[1], ARGV[5], '+', ARGV[6])
if lines.err then
  if string.sub(lines.err, 1, 7) == 'NOGROUP' then
    return {{}, {}, '-'}
  end
  return lines
end
local lease, most = tonumber(ARGV[3]), tonumber(ARGV[4])
local taken, vanished, last = {}, {}, nil
for _, line in ipairs(lines) do
  if #taken == most then
    break
  end
  last = line[1]
  if line[3] >= lease then
    local entry = redis.call('XCLAIM', KEYS[1], ARGV[1], ARGV[2], ARGV[3], line[1])[1]
    if entry then
      taken[#taken + 1] = {entry[1], line[4] + 1, entry[2]}
    else
      if entry == false then
        redis.call('XACK', KEYS[1], ARGV[1], line[1])
      end
      vanished[#vanished + 1] = line[1]
    end
  end
end
if last == nil or (#lines < tonumber(ARGV[6]) and last == lines[#lines][1]) then
  return {taken, vanished, '-'}
end
return {taken, vanished, last}
",
);

/// Renew each delivery this worker still holds, without counting a delivery:
/// one `1` (held), `0` (lost) or `2` (its entry deleted under it, which later
/// servers drop from the pending list as they claim it) per delivery, in order.
///
/// `KEYS`: jobs. `ARGV`: the group, this worker, then `entry`, `count` for
/// each delivery.
const RENEW: &str = concat!(
    effects!(),
    pending!(),
    holds!(),
    "local held = {}
for at = 3, #ARGV, 2 do
  if holds(ARGV[at], ARGV[at + 1]) then
    local claimed = redis.call('XCLAIM', KEYS[1], ARGV[1], ARGV[2], 0, ARGV[at], 'JUSTID')
    if #claimed == 0 then
      held[#held + 1] = 2
    else
      held[#held + 1] = 1
    end
  else
    held[#held + 1] = 0
  end
end
return held
"
);

/// End one delivery this worker still holds, as the port's disposition says:
/// `1` when written, `0` when the delivery is no longer this worker's and
/// nothing was.
///
/// `KEYS`: jobs, entries, due, delayed, unique, claims, deferred,
/// checkpoints, dead.
/// `ARGV`: the group, this worker, the entry, its delivery count, the job,
/// the way (`complete`, `retry`, `defer`, `requeue`, `dead`), the wait before
/// the record filed is due in milliseconds, the record, the dead letter's
/// reason, the most dead letters kept, how long one is kept in milliseconds.
const SETTLE: &str = concat!(
    effects!(),
    now!(),
    pending!(),
    holds!(),
    forget!(),
    file!(),
    "if not holds(ARGV[3], ARGV[4]) then
  return 0
end
redis.call('XACK', KEYS[1], ARGV[1], ARGV[3])
redis.call('XDEL', KEYS[1], ARGV[3])
local job, way = ARGV[5], ARGV[6]
if way == 'complete' then
  forget(job)
elseif way == 'dead' then
  redis.call('XADD', KEYS[9], 'MAXLEN', '~', ARGV[10], '*',
    'job', job, 'record', ARGV[8], 'reason', ARGV[9])
  redis.call('XTRIM', KEYS[9], 'MINID', '~', string.format('%.0f', now() - tonumber(ARGV[11])))
  forget(job)
elseif way == 'retry' then
  redis.call('HDEL', KEYS[7], job)
  file(job, ARGV[8], tonumber(ARGV[7]))
elseif way == 'defer' then
  if not redis.call('HGET', KEYS[7], job) then
    redis.call('HSET', KEYS[7], job, now())
  end
  file(job, ARGV[8], tonumber(ARGV[7]))
elseif way == 'requeue' then
  file(job, ARGV[8], 0)
else
  error('a disposition this script does not know')
end
return 1
"
);

/// File up to `ARGV[1]` held-back jobs that fell due, trim the dead letters
/// older than `ARGV[2]` milliseconds — a dead letter's age is kept here too,
/// not only when another job dies — and answer how long until the next held
/// job is due in milliseconds: `0` when more are due already, `-1` when none
/// is held back.
///
/// `KEYS`: jobs, entries, due, delayed, unique, claims, deferred, dead.
const PROMOTE: &str = concat!(
    effects!(),
    now!(),
    file!(),
    "local at = now()
redis.call('XTRIM', KEYS[8], 'MINID', '~', string.format('%.0f', at - tonumber(ARGV[2])))
local due = redis.call('ZRANGEBYSCORE', KEYS[3], '-inf', at, 'LIMIT', 0, ARGV[1])
for _, job in ipairs(due) do
  local record = redis.call('HGET', KEYS[4], job)
  redis.call('ZREM', KEYS[3], job)
  if record then
    redis.call('HDEL', KEYS[4], job)
    file(job, record, 0)
  end
end
if #due == tonumber(ARGV[1]) then
  return 0
end
local next = redis.call('ZRANGE', KEYS[3], 0, 0, 'WITHSCORES')
if #next == 0 then
  return -1
end
return math.max(0, tonumber(next[2]) - at)
"
);

/// Count up to `ARGV[3]` starts into the throttle window — opened for
/// `ARGV[2]` milliseconds by its first start — within its limit `ARGV[1]`:
/// `{starts counted, 0}`, or `{0, milliseconds until the window ends}` when it
/// is full. A window with no end — its key persisted by hand — is given one.
///
/// `KEYS`: throttle.
const ADMIT: &str = concat!(
    effects!(),
    r"
local used = tonumber(redis.call('GET', KEYS[1]) or '0')
local left = redis.call('PTTL', KEYS[1])
if left < 0 then
  used, left = 0, tonumber(ARGV[2])
end
local take = math.min(tonumber(ARGV[1]) - used, tonumber(ARGV[3]))
if take <= 0 then
  return {0, math.max(1, left)}
end
redis.call('SET', KEYS[1], used + take, 'PX', left)
return {take, 0}
",
);

/// Give back up to `ARGV[1]` starts a receive counted and did not use, while
/// their window lasts.
///
/// `KEYS`: throttle.
const RELEASE: &str = concat!(
    effects!(),
    r"
local used = tonumber(redis.call('GET', KEYS[1]) or '0')
local left = redis.call('PTTL', KEYS[1])
local back = math.min(used, tonumber(ARGV[1]))
if back > 0 and left > 0 then
  redis.call('SET', KEYS[1], used - back, 'PX', left)
end
return back
",
);

/// Save a job's checkpoint while this worker still holds the delivery running
/// it: `1` when written, `0` when not. The script that ends the job lets it go.
///
/// `KEYS`: jobs, checkpoints. `ARGV`: the group, this worker, the entry, its
/// delivery count, the job, the state.
const CHECKPOINT: &str = concat!(
    effects!(),
    pending!(),
    holds!(),
    "if not holds(ARGV[3], ARGV[4]) then
  return 0
end
redis.call('HSET', KEYS[2], ARGV[5], ARGV[6])
return 1
"
);

/// Remove every consumer of the group holding no delivery and silent for more
/// than `ARGV[2]` milliseconds — what a replica that stopped without leaving
/// leaves behind. Answers how many went.
///
/// `KEYS`: jobs. `ARGV`: the group, the silence.
const SWEEP: &str = concat!(
    effects!(),
    r"
local consumers = redis.pcall('XINFO', 'CONSUMERS', KEYS[1], ARGV[1])
if consumers.err then
  return 0
end
local gone = 0
for _, consumer in ipairs(consumers) do
  local field = {}
  for at = 1, #consumer, 2 do
    field[consumer[at]] = consumer[at + 1]
  end
  if field['pending'] == 0 and field['idle'] > tonumber(ARGV[2]) then
    redis.call('XGROUP', 'DELCONSUMER', KEYS[1], ARGV[1], field['name'])
    gone = gone + 1
  end
end
return gone
",
);

/// Remove this worker from the group, unless it still holds a delivery:
/// `1` when it left.
///
/// `KEYS`: jobs. `ARGV`: the group, this worker.
const LEAVE: &str = concat!(
    effects!(),
    r"
local held = redis.pcall('XPENDING', KEYS[1], ARGV[1], '-', '+', 1, ARGV[2])
if held.err or #held > 0 then
  return 0
end
redis.call('XGROUP', 'DELCONSUMER', KEYS[1], ARGV[1], ARGV[2])
return 1
",
);

/// Every script, built once per process: building one hashes its source.
pub(crate) struct Scripts {
    pub(crate) push: Script,
    pub(crate) cancel: Script,
    pub(crate) reclaim: Script,
    pub(crate) renew: Script,
    pub(crate) settle: Script,
    pub(crate) promote: Script,
    pub(crate) admit: Script,
    pub(crate) release: Script,
    pub(crate) checkpoint: Script,
    pub(crate) sweep: Script,
    pub(crate) leave: Script,
}

/// The scripts, shared by every producer and consumer of the process.
pub(crate) static SCRIPTS: LazyLock<Scripts> = LazyLock::new(|| Scripts {
    push: Script::new(PUSH),
    cancel: Script::new(CANCEL),
    reclaim: Script::new(RECLAIM),
    renew: Script::new(RENEW),
    settle: Script::new(SETTLE),
    promote: Script::new(PROMOTE),
    admit: Script::new(ADMIT),
    release: Script::new(RELEASE),
    checkpoint: Script::new(CHECKPOINT),
    sweep: Script::new(SWEEP),
    leave: Script::new(LEAVE),
});

impl Scripts {
    /// Every script a worker runs, to load before its first delivery.
    pub(crate) fn consumer(&self) -> [&Script; 9] {
        [
            &self.reclaim,
            &self.renew,
            &self.settle,
            &self.promote,
            &self.admit,
            &self.release,
            &self.checkpoint,
            &self.sweep,
            &self.leave,
        ]
    }
}
