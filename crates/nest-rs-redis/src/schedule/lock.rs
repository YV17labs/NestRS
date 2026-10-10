//! [`RedisOccurrenceLock`] — the schedule port's occurrence lock over the shared
//! [`RedisConnection`]: one atomic `SET … NX PX` per occurrence.

use async_trait::async_trait;
use nest_rs_schedule::{Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError};

use crate::RedisConnection;
use crate::millis::millis;

/// Every key this binding writes: `nestrs:schedule:claims:<token>`, one per
/// occurrence a `replicas = "one"` job has claimed, `<token>` being the port's
/// verbatim (`features:NotificationsTasks:purge_expired:1789002000000`). The
/// concern is the tail of [`nest_rs_schedule::TARGET`], never `redis`.
pub(crate) const CLAIMS: &str = "nestrs:schedule:claims";

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
/// cannot both win; expiring with the port's hold. A failure skips the
/// occurrence: it is fired **at most once**.
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

    #[test]
    fn every_key_is_a_level_of_the_schedule() {
        crate::testing::assert_keys_of(nest_rs_schedule::TARGET, &[CLAIMS]);
    }

    /// The port's token follows the structure verbatim, its own levels
    /// included.
    #[test]
    fn a_claim_carries_the_token_verbatim() {
        assert_eq!(
            claim_key("features:AudioTasks:sweep:1789000000000"),
            "nestrs:schedule:claims:features:AudioTasks:sweep:1789000000000"
        );
    }
}
