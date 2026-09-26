//! [`RedisOccurrenceLock`] — the schedule port's occurrence lock over the shared
//! [`RedisConnection`]: one atomic `SET … NX PX` per occurrence.

use std::time::Duration;

use async_trait::async_trait;
use nest_rs_schedule::{OccurrenceLock, OccurrenceLockError};

use crate::RedisConnection;

/// Every key this binding writes: `nestrs:schedule:claims:<occurrence>`, one per
/// occurrence a `replicas = "one"` job has claimed, `<occurrence>` being the
/// port's token verbatim (`AudioTasks:sweep:1789000000000`).
///
/// `nestrs:<concern>:<structure>[:<member>]`, like every key the framework
/// writes. The concern is the tail of [`nest_rs_schedule::TARGET`] — the crate
/// that owns the concern, never `redis`, because an operator looking at Redis is
/// looking for the scheduler's keys. `claims` is the structure level, read off the
/// port rather than chosen: [`OccurrenceLock`] calls the act `claim` and asks
/// `claimed`. So `nestrs:schedule:claims:AudioTasks:*` names one host's claims,
/// and `nestrs:schedule:claims:AudioTasks:sweep:*` one job's.
///
/// The prefix is fixed, not the deployment's: `NESTRS_ENV_PREFIX` renames the
/// developer's variables, while a key is the framework's own machinery, and two
/// deployments sharing one Redis are separated by the logical database in the
/// connection URL.
const CLAIMS: &str = "nestrs:schedule:claims";

/// The key `occurrence` is claimed under. The token is the port's and is never
/// parsed here: its `:` are the port's levels, a provider's, a method's and an
/// instant's, which is what makes a `SCAN` by host or by job possible.
fn claim_key(occurrence: &str) -> String {
    format!("{CLAIMS}:{occurrence}")
}

/// Claims each occurrence of a job declared `replicas = "one"` in Redis, so one
/// replica of the deployment fires it.
///
/// A claim is one `SET <key> <holder> NX PX <hold>`. Atomic, so two replicas
/// reaching one occurrence at the same instant cannot both win; expiring, so the
/// key lasts as long as the port's hold and no longer. It runs over the
/// multiplexed connection and answers or fails within the connect budget, like
/// every command a caller waits on — and a failure skips the occurrence, which
/// is the port's fail-closed rule rather than this binding's choice: an
/// occurrence is fired **at most once**, and one whose claim Redis could not
/// answer is not fired at all.
#[derive(Clone)]
pub struct RedisOccurrenceLock {
    conn: RedisConnection,
    holder: String,
}

impl RedisOccurrenceLock {
    /// A lock over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self {
            conn,
            holder: holder(),
        }
    }
}

#[async_trait]
impl OccurrenceLock for RedisOccurrenceLock {
    async fn claim(&self, occurrence: &str, hold: Duration) -> Result<bool, OccurrenceLockError> {
        // `PX 0` is an error to Redis, and a hold is never meant to be none.
        let hold_ms = u64::try_from(hold.as_millis()).unwrap_or(u64::MAX).max(1);
        let mut conn = self.conn.clone();
        // `OK` when this call created the key, nil when a replica already held it.
        let created: Option<String> = redis::cmd("SET")
            .arg(claim_key(occurrence))
            .arg(&self.holder)
            .arg("NX")
            .arg("PX")
            .arg(hold_ms)
            .query_async(&mut conn)
            .await
            .map_err(OccurrenceLockError::new)?;
        Ok(created.is_some())
    }

    async fn claimed(&self, occurrence: &str) -> Result<bool, OccurrenceLockError> {
        let mut conn = self.conn.clone();
        redis::cmd("EXISTS")
            .arg(claim_key(occurrence))
            .query_async(&mut conn)
            .await
            .map_err(OccurrenceLockError::new)
    }
}

/// What the key's value records about who claimed it — for an operator reading
/// the key during an incident, never for the lock, which decides on the key's
/// existence alone. `HOSTNAME` is the platform's (a container's, a pod's), not a
/// framework variable.
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
    /// chosen, so renaming the target moves the key — or fails here — and the
    /// port's token follows the structure verbatim, its own levels included.
    #[test]
    fn the_claims_name_the_concern_its_crate_emits_on_and_carry_the_token_verbatim() {
        let concern = nest_rs_schedule::TARGET
            .strip_prefix("nest_rs::")
            .expect("a framework target");
        assert_eq!(
            CLAIMS.split(':').collect::<Vec<_>>(),
            ["nestrs", concern, "claims"],
        );
        let occurrence = "AudioTasks:sweep:1789000000000";
        assert_eq!(
            claim_key(occurrence),
            "nestrs:schedule:claims:AudioTasks:sweep:1789000000000"
        );
    }
}
