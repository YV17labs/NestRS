//! [`RedisOccurrenceLock`] — the schedule port's occurrence lock over the shared
//! [`RedisConnection`]: one atomic `SET … NX PX` per occurrence.

use async_trait::async_trait;
use nest_rs_schedule::{Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError};

use crate::RedisConnection;
use crate::layout::millis;

/// Every key this binding writes: `nestrs:schedule:claims:<token>`, one per
/// occurrence a `replicas = "one"` job has claimed, `<token>` being the port's
/// verbatim (`features:NotificationsTasks:purge_expired:1789002000000`).
///
/// `nestrs:<concern>:<structure>[:<member>]`, like every key the framework
/// writes. The concern is the tail of [`nest_rs_schedule::TARGET`] — the crate
/// that owns the concern, never `redis`, because an operator looking at Redis is
/// looking for the scheduler's keys. The structure is read off the port rather
/// than chosen: [`OccurrenceLock`] calls the act `claim` and asks `claimed`. So
/// `nestrs:schedule:claims:features:*` names one crate's claims, and
/// `nestrs:schedule:claims:features:NotificationsTasks:purge_expired:*` one job's.
///
/// The prefix is fixed, not the deployment's: `NESTRS_ENV_PREFIX` renames the
/// developer's variables, while a key is the framework's own machinery, and two
/// deployments sharing one Redis are separated by the logical database in the
/// connection URL.
const CLAIMS: &str = "nestrs:schedule:claims";

/// The key the occurrence `token` is claimed under. The token is the port's and
/// is never parsed here: its `:` are the port's levels, which is what makes a
/// `SCAN` by crate, by host or by job possible.
fn claim_key(token: &str) -> String {
    format!("{CLAIMS}:{token}")
}

/// Claims each occurrence of a job declared `replicas = "one"` in Redis, so one
/// replica of the deployment fires it.
///
/// A claim is one `SET <claim> <holder> NX PX <hold>`, and an overrun question one
/// `EXISTS <claim>` — two commands on `nestrs:schedule:claims:*`, which with the
/// connection's own `PING` (and `SELECT`, when the URL names a database) is the
/// whole of what a Redis ACL has to allow the schedule. Atomic, so two replicas
/// reaching one occurrence at the same instant cannot both win; expiring, so the
/// key lasts as long as the port's hold and no longer. Each runs over the
/// multiplexed connection and answers or fails within the connect budget, like
/// every command a caller waits on — and a failure skips the occurrence, which is
/// the port's fail-closed rule rather than this binding's choice: an occurrence
/// is fired **at most once**, and one whose claim Redis could not answer is not
/// fired at all.
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
    async fn claim(&self, occurrence: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError> {
        // `OK` when this call created the key, nil when a replica already held it;
        // read as Redis's own reply, so an answer that is neither never fires.
        let answer: redis::Value = redis::cmd("SET")
            .arg(claim_key(&occurrence.token))
            .arg(format!("{}/{}", self.holder, occurrence.run))
            .arg("NX")
            .arg("PX")
            // `PX 0` is an error to Redis, and a hold is never meant to be none.
            .arg(millis(occurrence.hold))
            .query_async(&mut self.conn.clone())
            .await
            .map_err(OccurrenceLockError::new)?;
        match answer {
            redis::Value::Okay => Ok(OccurrenceClaim::Claimed),
            redis::Value::Nil => Ok(OccurrenceClaim::ClaimedElsewhere),
            other => Err(OccurrenceLockError::new(format!(
                "the claim's `SET … NX` answered {other:?}, which is neither `OK` nor nil"
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
}

/// What a claim records about the process that wrote it, before the run's trace
/// id — for an operator reading the key during an incident, never for the claim,
/// which decides on the key's existence alone. `HOSTNAME` is the platform's (a
/// container's, a pod's), not a framework variable.
#[expect(
    clippy::disallowed_methods,
    reason = "HOSTNAME is the platform's variable, not a framework setting the env prefix renames"
)]
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
        assert_eq!(
            claim_key("features:AudioTasks:sweep:1789000000000"),
            "nestrs:schedule:claims:features:AudioTasks:sweep:1789000000000"
        );
    }
}
