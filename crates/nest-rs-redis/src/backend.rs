//! [`BACKEND`] — what the Redis backend declares to the queue port: its
//! `messaging.system` and the optional capabilities it honours — and the clock
//! it holds a job back on.

use std::time::{SystemTime, UNIX_EPOCH};

use nest_rs_queue::{Capabilities, Capability, QueueBackend};

/// The Redis backend, as the queue port reads it: `redis` is its
/// `messaging.system`, and it declares every optional capability the port
/// names, each kept in keys of its own beside apalis's queue storage
/// ([`crate::layout`]).
///
/// - **Delayed delivery.** A delayed push is filed on apalis's schedule rather
///   than its queue, through apalis's own `schedule_request`, and a due record
///   moves onto the queue on the next scan of whichever side runs one: every
///   worker scans its queue's schedule each second, and a producer holding
///   delayed records of its own scans until they are all due. A retry is the
///   same filing — the port's record for the next attempt, due once the backoff
///   has passed — so a worker never holds a permit, or a shutdown, while a job
///   waits.
/// - **Unique jobs.** The push claims `…:unique:<key>` for the job in one script
///   before it files it, and the job lets go of it when it settles or is
///   cancelled. At most once over pushes, never a lock: see [`crate::layout`] for
///   how long a claim whose job vanished holds out.
/// - **Cancellation.** A tombstone beside the job, written only while no
///   attempt holds its lease; the delivery that meets it acknowledges the job
///   without running it.
/// - **Throttle.** A fixed window per queue, counted in Redis by the step that
///   admits an attempt: the window opens with its first start and ends when its
///   key expires, and an attempt over the limit is handed back for when it ends.
/// - **Checkpoints.** One key per job, read once per delivery and cleared at the
///   job's terminal outcome.
///
/// What every backend owes regardless — the retry budget the port counts, one
/// transaction per attempt, a method's `concurrency` — is honoured.
///
/// A capability joins this list with the keys that keep it and the e2e that
/// proves it, one by one — never as `Capabilities::ALL`, so a capability the port
/// adds later is not claimed before this crate honours it.
pub(crate) static BACKEND: QueueBackend = QueueBackend::new(
    "redis",
    Capabilities::NONE
        .with(Capability::DelayedPush)
        .with(Capability::UniquePush)
        .with(Capability::Cancellation)
        .with(Capability::Throttle)
        .with(Capability::Checkpoint),
);

/// The second apalis files a record held back until `due` under: apalis keeps
/// its schedule in whole seconds, and this rounds up, so a job never becomes
/// available before the instant it was held back until. An instant before the
/// epoch is due at once.
pub(crate) fn due_second(due: SystemTime) -> i64 {
    let second = due
        .duration_since(UNIX_EPOCH)
        .map(|since| {
            since
                .as_secs()
                .saturating_add(u64::from(since.subsec_nanos() > 0))
        })
        .unwrap_or_default();
    i64::try_from(second).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// apalis schedules on whole seconds, and a due time shortened by the
    /// rounding would run a job before the instant it was held back until.
    #[test]
    fn a_record_is_due_on_the_second_its_delay_ends_or_after_it() {
        let at = |millis: u64| UNIX_EPOCH + Duration::from_millis(millis);
        assert_eq!(due_second(at(10_000)), 10);
        assert_eq!(due_second(at(10_001)), 11);
        assert_eq!(due_second(at(10_999)), 11);
        assert_eq!(due_second(UNIX_EPOCH - Duration::from_secs(5)), 0);
    }

    /// The capabilities this backend keeps, each with the keys and the e2e that
    /// honour it — and, the port's enum being non-exhaustive, none claimed for
    /// it by default.
    #[test]
    fn the_backend_declares_what_it_keeps_and_nothing_else() {
        let declared: Vec<Capability> = BACKEND.capabilities().iter().collect();
        assert_eq!(
            declared,
            [
                Capability::DelayedPush,
                Capability::UniquePush,
                Capability::Cancellation,
                Capability::Throttle,
                Capability::Checkpoint,
            ]
        );
    }
}
