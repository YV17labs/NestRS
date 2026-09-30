//! [`RedisOccurrenceLock`] — the schedule port's occurrence lock over the shared
//! [`RedisConnection`]: one atomic script per claim, taking the occurrence's
//! claim and the job's run lease together.

use async_trait::async_trait;
use nest_rs_schedule::{Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError};
use redis::Script;

use crate::RedisConnection;
use crate::layout::millis;

/// Every occurrence claim this binding writes: `nestrs:schedule:claims:<token>`,
/// one per occurrence a `replicas = "one"` job has claimed, `<token>` being the
/// port's verbatim
/// (`features:notifications:schedule:tasks:NotificationsTasks:purge_expired:1789002000000`).
///
/// `nestrs:<concern>:<structure>[:<member>]`, like every key the framework
/// writes. The concern is the tail of [`nest_rs_schedule::TARGET`] — the crate
/// that owns the concern, never `redis`, because an operator looking at Redis is
/// looking for the scheduler's keys. The structure is read off the port rather
/// than chosen: [`OccurrenceLock`] calls the act `claim` and asks `claimed`. So
/// `nestrs:schedule:claims:features:notifications:*` names one module's claims,
/// and `nestrs:schedule:claims:*:NotificationsTasks:purge_expired:*` one job's.
///
/// The prefix is fixed, not the deployment's: `NESTRS_ENV_PREFIX` renames the
/// developer's variables, while a key is the framework's own machinery, and two
/// deployments sharing one Redis are separated by the logical database in the
/// connection URL.
const CLAIMS: &str = "nestrs:schedule:claims";

/// Every run lease this binding writes: `nestrs:schedule:leases:<job>`, one per
/// job a replica is running, `<job>` being the port's identity verbatim. The
/// structure is the port's word again — [`Occurrence::lease`], which the worker's
/// job lease (`nestrs:queue:<queue>:leases:<job_id>`) already spells the same.
const LEASES: &str = "nestrs:schedule:leases";

/// The key the occurrence `token` is claimed under. The token is the port's and
/// is never parsed here: its `:` are the port's levels, which is what makes a
/// `SCAN` by module, by host or by job possible.
fn claim_key(token: &str) -> String {
    format!("{CLAIMS}:{token}")
}

/// The key the run lease of `job` is held under.
fn lease_key(job: &str) -> String {
    format!("{LEASES}:{job}")
}

/// Claim the occurrence and take the job's run lease, or neither — the port's
/// order of questions in one script, so no replica sees the claim without the
/// lease or the lease without the claim.
///
/// `KEYS`: the lease, the claim. `ARGV`: the holder, the claim's hold in
/// milliseconds, the lease's. Replies `0` claimed, `1` claimed elsewhere, `2`
/// running elsewhere.
const CLAIM: &str = r"
if redis.call('EXISTS', KEYS[1]) == 1 then
  return 2
end
if not redis.call('SET', KEYS[2], ARGV[1], 'NX', 'PX', ARGV[2]) then
  return 1
end
redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[3])
return 0
";

/// Extend the lease while its holder still holds it: `1` renewed, `0` lost.
const RENEW: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
";

/// Drop the lease if its holder still holds it — never a lease another run took
/// once this one's lapsed.
const RELEASE: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
";

/// Claims each occurrence of a job declared `replicas = "one"` in Redis, and
/// holds the job's run while one fires, so one replica of the deployment fires
/// each occurrence and no two run the job at once.
///
/// A claim is one Lua script, sent as `EVALSHA` with a `SCRIPT LOAD` the first
/// time Redis has not cached it: a lease another run holds answers *running
/// elsewhere*; otherwise a `SET <claim> <holder> NX PX <hold>` decides, and the
/// one that creates the key also sets the lease. Atomic, so two replicas reaching
/// one occurrence at the same instant cannot both win, and none sees a claim
/// without its lease; expiring, so a claim lasts as long as the port's hold and
/// a lease whose holder stopped renewing lapses. Every command runs over the
/// multiplexed connection and answers or fails within the connect budget, like
/// every command a caller waits on — and a failure skips the occurrence, which
/// is the port's fail-closed rule rather than this binding's choice: an
/// occurrence is fired **at most once**, and one whose claim Redis could not
/// answer is not fired at all.
#[derive(Clone)]
pub struct RedisOccurrenceLock {
    conn: RedisConnection,
    holder: String,
    claim: Script,
    renew: Script,
    release: Script,
}

impl RedisOccurrenceLock {
    /// A lock over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self {
            conn,
            holder: holder(),
            claim: Script::new(CLAIM),
            renew: Script::new(RENEW),
            release: Script::new(RELEASE),
        }
    }

    /// What a claim and a lease record about who holds them: this process, then
    /// the run — the tick's trace id, so an operator reading a key during an
    /// incident finds the trace of the run behind it. Unique per run, which is
    /// what lets a renewal and a release touch only the lease their run took.
    fn held_by(&self, occurrence: &Occurrence) -> String {
        format!("{}/{}", self.holder, occurrence.run)
    }
}

#[async_trait]
impl OccurrenceLock for RedisOccurrenceLock {
    async fn claim(&self, occurrence: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError> {
        let answer: i64 = self
            .claim
            .key(lease_key(&occurrence.job))
            .key(claim_key(&occurrence.token))
            .arg(self.held_by(occurrence))
            // `PX 0` is an error to Redis, and neither is ever meant to be none.
            .arg(millis(occurrence.hold))
            .arg(millis(occurrence.lease))
            .invoke_async(&mut self.conn.clone())
            .await
            .map_err(OccurrenceLockError::new)?;
        match answer {
            0 => Ok(OccurrenceClaim::Claimed),
            1 => Ok(OccurrenceClaim::ClaimedElsewhere),
            2 => Ok(OccurrenceClaim::RunningElsewhere),
            other => Err(OccurrenceLockError::new(format!(
                "the claim script answered {other}, which is none of its replies"
            ))),
        }
    }

    async fn claimed(&self, token: &str) -> Result<bool, OccurrenceLockError> {
        redis::cmd("EXISTS")
            .arg(claim_key(token))
            .query_async(&mut self.conn.clone())
            .await
            .map_err(OccurrenceLockError::new)
    }

    async fn renew(&self, occurrence: &Occurrence) -> Result<bool, OccurrenceLockError> {
        let renewed: i64 = self
            .renew
            .key(lease_key(&occurrence.job))
            .arg(self.held_by(occurrence))
            .arg(millis(occurrence.lease))
            .invoke_async(&mut self.conn.clone())
            .await
            .map_err(OccurrenceLockError::new)?;
        Ok(renewed == 1)
    }

    async fn release(&self, occurrence: &Occurrence) -> Result<(), OccurrenceLockError> {
        self.release
            .key(lease_key(&occurrence.job))
            .arg(self.held_by(occurrence))
            .invoke_async::<i64>(&mut self.conn.clone())
            .await
            .map(drop)
            .map_err(OccurrenceLockError::new)
    }
}

/// What the key's value records about the process that claimed it — for an
/// operator reading the key during an incident, never for the claim, which
/// decides on the key's existence alone. `HOSTNAME` is the platform's (a
/// container's, a pod's), not a framework variable.
fn holder() -> String {
    match std::env::var("HOSTNAME") {
        Ok(host) if !host.is_empty() => format!("{host}:{}", std::process::id()),
        _ => format!("pid:{}", std::process::id()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The concern is read off the owning crate's span target rather than
    /// chosen, so renaming the target moves the keys — or fails here — and the
    /// port's strings follow each structure verbatim, their own levels included.
    #[test]
    fn the_keys_name_the_concern_its_crate_emits_on_and_carry_the_port_strings_verbatim() {
        let concern = nest_rs_schedule::TARGET
            .strip_prefix("nest_rs::")
            .expect("a framework target");
        assert_eq!(
            CLAIMS.split(':').collect::<Vec<_>>(),
            ["nestrs", concern, "claims"],
        );
        assert_eq!(
            LEASES.split(':').collect::<Vec<_>>(),
            ["nestrs", concern, "leases"],
        );
        let job = "features:audio:schedule:tasks:AudioTasks:sweep";
        assert_eq!(
            lease_key(job),
            "nestrs:schedule:leases:features:audio:schedule:tasks:AudioTasks:sweep"
        );
        assert_eq!(
            claim_key(&format!("{job}:1789000000000")),
            "nestrs:schedule:claims:features:audio:schedule:tasks:AudioTasks:sweep:1789000000000"
        );
    }
}
