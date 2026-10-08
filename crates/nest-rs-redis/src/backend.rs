//! [`BACKEND`] — what the Redis backend declares to the queue port: its
//! `messaging.system` and the optional capabilities it honours.

use nest_rs_queue::{Capabilities, Capability, QueueBackend};

/// The Redis backend, as the queue port reads it: `redis` is its
/// `messaging.system`, and it declares every optional capability the port
/// names, each kept by the scripts of [`crate::queue`] in the keys
/// [`crate::layout`] tabulates.
///
/// - **Delayed delivery.** A job held back waits in the queue's `due` set,
///   due at an instant on Redis's clock, and a worker draining the queue files
///   it on the stream within a second of it falling due. A retry is the same
///   filing — the port's record for the next attempt — so a worker never holds
///   a permit, or a shutdown, while a job waits.
/// - **Unique jobs.** The push claims the key for its job in the script that
///   files it, and the job lets go of it when it ends or is cancelled. At most
///   once over pushes, never a lock.
/// - **Cancellation.** A job still waiting — undelivered on the stream, or held
///   back — is removed with everything it held; one a delivery runs is not.
/// - **Throttle.** A fixed window per queue, counted in Redis before a read
///   asks for jobs: the window opens with its first start and ends when its key
///   expires, and a full one holds the replica's reads for the method until
///   then, so the backlog waits on the queue.
/// - **Checkpoints.** A field per job, written only by the delivery holding
///   the job, and removed when the job ends.
///
/// A capability joins this list one by one, with the keys that keep it and the
/// e2e that proves it.
pub(crate) static BACKEND: QueueBackend = QueueBackend::new(
    "redis",
    Capabilities::NONE
        .with(Capability::DelayedPush)
        .with(Capability::UniquePush)
        .with(Capability::Cancellation)
        .with(Capability::Throttle)
        .with(Capability::Checkpoint),
);

#[cfg(test)]
mod tests {
    use super::*;

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
