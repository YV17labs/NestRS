//! [`BACKEND`] — what the Redis backend declares to the queue port: its
//! `messaging.system` and the optional capabilities it honours — the clock it
//! holds a job back on, and the apalis context every record it files carries.

use std::time::{SystemTime, UNIX_EPOCH};

use apalis::prelude::WorkerId;
use apalis_redis::RedisContext;
use nest_rs_queue::{Capabilities, Capability, QueueBackend};
use serde::de::Error as _;

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

/// The field apalis-redis 0.7's [`RedisContext`] keeps its attempt cap under.
/// The field is private and has no setter: its serde form is the one public way
/// to set it, so the name is pinned by a test against the apalis the workspace
/// builds with.
const ATTEMPT_CAP: &str = "max_attempts";

/// The field it keeps the worker a record was fetched by under — the in-flight
/// set apalis's `reschedule` takes the record out of.
const FETCHED_BY: &str = "lock_by";

/// The apalis context every record this backend files carries — a push, a
/// delayed push, a retry's next attempt, a job handed back: apalis's own attempt
/// cap lifted past any count a job can reach, and, for a record a worker files
/// back, the worker it was fetched by.
///
/// **apalis never ends a job on its own.** apalis-redis 0.7 counts every
/// delivery of a record — a retry, a throttle's deferral, a hand-back alike —
/// and a record answered with a plain error once that count has reached the
/// context's cap, five by default, is killed onto apalis's dead set, with none
/// of the port's events and no line of the framework's. A plain error is what a
/// hand-back Redis refused answers, precisely so the job is delivered again, so
/// the cap would bury the jobs the guard delays most. The port's budget is the
/// only one a job has: it ends when the port dead-letters it, as apalis's
/// `Abort`, whatever apalis counted.
///
/// Fails only when apalis's serde form no longer names these fields — which the
/// test below pins, so an apalis upgrade that renames them fails the build's
/// tests rather than a job.
pub(crate) fn uncapped_context(
    fetched_by: Option<&str>,
) -> Result<RedisContext, serde_json::Error> {
    let mut form = serde_json::to_value(RedisContext::default())?;
    let fetched_by = serde_json::to_value(fetched_by.map(WorkerId::new))?;
    for (field, value) in [
        (ATTEMPT_CAP, serde_json::Value::from(usize::MAX)),
        (FETCHED_BY, fetched_by),
    ] {
        let Some(slot) = form.get_mut(field) else {
            return Err(serde_json::Error::custom(format!(
                "apalis-redis's RedisContext no longer serializes a `{field}` field, which every \
                 record this backend files sets"
            )));
        };
        *slot = value;
    }
    serde_json::from_value(form)
}

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

    /// apalis's context keeps its attempt cap and its fetching worker under the
    /// names this crate sets, beside nothing else — and the cap it lifts is
    /// apalis's five. An apalis whose serde form moved fails here, not in a job
    /// buried on its fifth delivery.
    #[test]
    fn apalis_keeps_the_attempt_cap_and_the_fetching_worker_where_the_backend_sets_them() {
        let form = serde_json::to_value(RedisContext::default()).expect("serializes");
        let mut fields: Vec<&str> = form
            .as_object()
            .expect("a struct serializes as an object")
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(fields, [FETCHED_BY, ATTEMPT_CAP, "run_at"]);
        assert_eq!(
            form[ATTEMPT_CAP], 5,
            "apalis's own cap, which the docs name"
        );
        assert!(form[FETCHED_BY].is_null());
    }

    /// Every record filed carries a cap no count reaches — through apalis's own
    /// JSON codec, as the record is stored and read back — and a record filed
    /// back names the worker that fetched it, so `reschedule` finds its
    /// in-flight set.
    #[test]
    fn every_record_the_backend_files_carries_a_cap_no_count_reaches() {
        use apalis::prelude::Request;

        let pushed = uncapped_context(None).expect("apalis's form holds both fields");
        let record = Request::new_with_ctx(serde_json::json!({ "clip": 1 }), pushed);
        let stored = serde_json::to_vec(&record).expect("encodes");
        let read: Request<serde_json::Value, RedisContext> =
            serde_json::from_slice(&stored).expect("decodes as apalis does");
        let context = serde_json::to_value(&read.parts.context).expect("serializes");
        assert_eq!(context[ATTEMPT_CAP], serde_json::Value::from(usize::MAX));
        assert!(context[FETCHED_BY].is_null());

        let handed_back = uncapped_context(Some("host:01")).expect("apalis's form holds both");
        let context = serde_json::to_value(&handed_back).expect("serializes");
        assert_eq!(context[ATTEMPT_CAP], serde_json::Value::from(usize::MAX));
        assert_eq!(
            context[FETCHED_BY],
            serde_json::to_value(WorkerId::new("host:01")).expect("serializes"),
        );
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
