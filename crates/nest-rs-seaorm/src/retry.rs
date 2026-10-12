//! Bounded retry for transient transaction conflicts.
//!
//! A `DatabaseTransaction` is aborted past a conflict and cannot be replayed:
//! wrap the whole programmatic transaction in [`retry_on_conflict`].

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nest_rs_core::{Backoff, process_seed};
use sea_orm::{DbErr, RuntimeErr, SqlxError};

/// SQLSTATE markers a transient conflict surfaces under across the
/// supported backends.
const RETRYABLE_SQLSTATES: &[&str] = &[
    "40001", // PG / MySQL — serialization failure
    "40P01", // PG — deadlock detected
    "1213",  // MySQL — deadlock
    "1205",  // SQL Server — deadlock victim
];

/// SQLSTATE markers a **connection** fault surfaces under: the server closed
/// the session, or refused to open one.
///
/// `08007` (`transaction_resolution_unknown`) is absent on purpose: that
/// transaction may have committed.
const CONNECTION_SQLSTATES: &[&str] = &[
    "08000", // PG — connection exception
    "08001", // PG — sqlclient unable to establish sqlconnection
    "08003", // PG — connection does not exist
    "08004", // PG — sqlserver rejected establishment of sqlconnection
    "08006", // PG — connection failure
    "08P01", // PG — protocol violation
    "57P01", // PG — admin shutdown (`pg_terminate_backend`, a restart)
    "57P02", // PG — crash shutdown
    "57P03", // PG — cannot connect now (the server is still starting)
];

/// Hard ceiling for the public retry budget, whatever the caller passes.
pub const MAX_RETRY_ATTEMPTS: usize = 32;

/// Default bounded retry budget for [`retry_on_conflict`]: 3 attempts.
pub const DEFAULT_RETRY_ATTEMPTS: usize = 3;

/// Initial backoff before the first retry — 5 ms, doubled each retry
/// (5 ms → 10 ms → 20 ms), each within a fifth either way.
pub const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(5);

/// Per-sleep ceiling before jitter, however many attempts have failed.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// How many [`retry_on_conflict`] calls this process made: two transactions
/// that conflicted with each other retry apart, not at one instant into the
/// same conflict.
static CALLS: AtomicU64 = AtomicU64::new(0);

/// Whether `err` is a transient conflict worth retrying, matched on the typed
/// SQLSTATE, never the message text.
pub fn is_retryable_conflict(err: &DbErr) -> bool {
    // Widening this to `is_transient_failure` would replay a `COMMIT` whose
    // outcome is unknown (e2e `lazy::a_commit_whose_outcome_is_unknown_stays_deterministic`).
    let sqlx_err = match err {
        DbErr::Query(RuntimeErr::SqlxError(e)) | DbErr::Exec(RuntimeErr::SqlxError(e)) => e,
        _ => return false,
    };
    let Some(db_err) = sqlx_err.as_database_error() else {
        return false;
    };
    matches!(
        db_err.code().as_deref(),
        Some(code) if RETRYABLE_SQLSTATES.contains(&code)
    )
}

/// Whether a **statement** failure inside an open transaction is one a fresh
/// attempt could clear — a conflict, or the connection going away.
///
/// Never use it at `COMMIT`, where a lost connection may mean the commit landed:
/// the commit site keeps [`is_retryable_conflict`].
pub fn is_transient_failure(err: &DbErr) -> bool {
    if is_retryable_conflict(err) {
        return true;
    }
    match err {
        DbErr::ConnectionAcquire(_) => true,
        // Narrowed to sqlx: sea-orm's rusqlite driver maps every error, a
        // constraint violation included, to `DbErr::Conn`.
        DbErr::Conn(RuntimeErr::SqlxError(_)) => true,
        DbErr::Exec(RuntimeErr::SqlxError(e)) | DbErr::Query(RuntimeErr::SqlxError(e)) => {
            match &**e {
                SqlxError::Io(_)
                | SqlxError::PoolTimedOut
                | SqlxError::PoolClosed
                | SqlxError::WorkerCrashed => true,
                other => other.as_database_error().is_some_and(|db| {
                    matches!(db.code().as_deref(), Some(code) if CONNECTION_SQLSTATES.contains(&code))
                }),
            }
        }
        _ => false,
    }
}

/// Run `op` up to `attempts` times, sleeping `initial_backoff << attempt`
/// between tries — jittered within a fifth either way, from a seed of its own
/// per call — when the previous attempt failed with a conflict recognized by
/// [`is_retryable_conflict`]. A non-retryable error returns immediately; the
/// last error after exhausting the budget is returned.
///
/// `attempts` is clamped to `[1, MAX_RETRY_ATTEMPTS]` so a caller passing
/// `0` still runs the operation once and a caller passing `usize::MAX`
/// does not hot-spin against a persistent conflict.
///
/// `op` re-runs from scratch on each attempt: open the transaction inside it.
pub async fn retry_on_conflict<F, Fut, T>(
    attempts: usize,
    initial_backoff: Duration,
    mut op: F,
) -> Result<T, DbErr>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, DbErr>>,
{
    let attempts = attempts.clamp(1, MAX_RETRY_ATTEMPTS);
    let seed = process_seed() ^ CALLS.fetch_add(1, Ordering::Relaxed);
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !is_retryable_conflict(&err) {
                    return Err(err);
                }
                attempt += 1;
                if attempt >= attempts {
                    tracing::warn!(
                        target: crate::target::ORM,
                        attempt,
                        attempts,
                        error = %nest_rs_core::error_message(&err),
                        "transaction conflict — retry budget exhausted",
                    );
                    return Err(err);
                }
                tracing::warn!(
                    target: crate::target::ORM,
                    attempt,
                    attempts,
                    error = %nest_rs_core::error_message(&err),
                    "transaction conflict — retrying",
                );
                tokio::time::sleep(backoff_for(initial_backoff, attempt, seed)).await;
            }
        }
    }
}

/// The wait after failed attempt `failed` (1-based): `initial` doubling up to
/// [`MAX_BACKOFF`], jittered from `seed` (`nest_rs_core::Backoff`).
fn backoff_for(initial: Duration, failed: usize, seed: u64) -> Duration {
    let failed =
        NonZeroU32::new(u32::try_from(failed).unwrap_or(u32::MAX)).unwrap_or(NonZeroU32::MIN);
    Backoff::new(initial, MAX_BACKOFF).wait(failed, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::RuntimeErr;

    fn db_err(msg: &str) -> DbErr {
        DbErr::Exec(RuntimeErr::Internal(msg.into()))
    }

    #[derive(Debug)]
    struct FakeDbError {
        code: Option<String>,
        msg: String,
    }

    impl std::fmt::Display for FakeDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.msg)
        }
    }

    impl std::error::Error for FakeDbError {}

    impl sea_orm::sqlx::error::DatabaseError for FakeDbError {
        fn message(&self) -> &str {
            &self.msg
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            self.code.as_deref().map(std::borrow::Cow::Borrowed)
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sea_orm::sqlx::error::ErrorKind {
            sea_orm::sqlx::error::ErrorKind::Other
        }
    }

    fn sqlx_db_err(code: Option<&str>, msg: &str) -> DbErr {
        let stub = FakeDbError {
            code: code.map(str::to_owned),
            msg: msg.to_owned(),
        };
        let sqlx_err = sea_orm::SqlxError::database(stub);
        DbErr::Exec(RuntimeErr::SqlxError(std::sync::Arc::new(sqlx_err)))
    }

    #[test]
    fn a_statement_that_lost_its_connection_is_transient() {
        for code in ["08006", "08003", "57P01", "57P02", "57P03"] {
            let err = sqlx_db_err(Some(code), "connection went away");
            assert!(
                is_transient_failure(&err),
                "SQLSTATE {code} closes the session, and Postgres rolls the \
                 transaction back on close — nothing landed",
            );
            assert!(
                !is_retryable_conflict(&err),
                "and it is not a conflict, so the commit site still refuses it: {code}",
            );
        }
    }

    #[test]
    fn a_commit_whose_outcome_is_unknown_is_not_transient() {
        let err = sqlx_db_err(Some("08007"), "transaction resolution unknown");
        assert!(!is_transient_failure(&err));
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn a_pool_that_never_handed_out_a_connection_is_transient() {
        let err = DbErr::ConnectionAcquire(sea_orm::ConnAcquireErr::Timeout);
        assert!(
            is_transient_failure(&err),
            "no connection means no statement reached the database",
        );
        assert!(!is_retryable_conflict(&err), "and it is not a conflict");
    }

    #[test]
    fn a_conflict_is_transient_too() {
        let err = sqlx_db_err(Some("40001"), "could not serialize access");
        assert!(is_transient_failure(&err));
        assert!(is_retryable_conflict(&err));
    }

    #[test]
    fn a_constraint_violation_is_neither() {
        let err = sqlx_db_err(Some("23505"), "unique violation");
        assert!(
            !is_transient_failure(&err),
            "a re-run hits the same row, and the retry budget is spent replaying \
             every side effect the job body has outside the transaction",
        );
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn recognizes_pg_serialization_failure_by_typed_sqlstate() {
        let err = sqlx_db_err(Some("40001"), "could not serialize access");
        assert!(is_retryable_conflict(&err));
    }

    #[test]
    fn recognizes_pg_deadlock_by_typed_sqlstate() {
        let err = sqlx_db_err(Some("40P01"), "deadlock detected");
        assert!(is_retryable_conflict(&err));
    }

    #[test]
    fn recognizes_mysql_deadlock_by_typed_sqlstate() {
        let err = sqlx_db_err(Some("1213"), "deadlock found");
        assert!(is_retryable_conflict(&err));
    }

    #[test]
    fn recognizes_sql_server_deadlock_by_typed_sqlstate() {
        let err = sqlx_db_err(Some("1205"), "transaction was deadlocked");
        assert!(is_retryable_conflict(&err));
    }

    #[test]
    fn rejects_typed_db_err_with_unrelated_sqlstate() {
        let err = sqlx_db_err(Some("23505"), "unique violation");
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn rejects_typed_db_err_without_sqlstate() {
        let err = sqlx_db_err(None, "could not serialize access");
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn rejects_internal_runtime_errors_even_when_message_contains_sqlstate() {
        let err = db_err("error returned from database: 40001: could not serialize access");
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn rejects_textual_serialization_phrase_without_typed_sqlstate() {
        let err = db_err("could not serialize access due to concurrent update");
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn rejects_unique_constraint_violation() {
        let err = db_err("23505 unique constraint violated");
        assert!(
            !is_retryable_conflict(&err),
            "a unique violation must NOT be retried — that loops forever",
        );
    }

    #[test]
    fn rejects_record_not_found() {
        let err = DbErr::RecordNotFound("widget".into());
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn rejects_substring_false_positives() {
        let err = db_err("connection refused at 127.0.0.1:40001");
        assert!(!is_retryable_conflict(&err));
        let err = db_err("processing row id=1213 took 1205ms");
        assert!(!is_retryable_conflict(&err));
    }

    #[tokio::test]
    async fn retries_until_success_within_budget() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<&str, DbErr> =
            retry_on_conflict(3, Duration::from_millis(1), || async {
                let n = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < 2 {
                    Err(sqlx_db_err(Some("40001"), "could not serialize access"))
                } else {
                    Ok("ok")
                }
            })
            .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn surfaces_last_error_when_budget_exhausted() {
        let logs = nest_rs_testing::LogCapture::install();
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<(), DbErr> = retry_on_conflict(2, Duration::from_millis(1), || async {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(sqlx_db_err(Some("40001"), "could not serialize access"))
        })
        .await;
        assert!(matches!(result, Err(DbErr::Exec(_))));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);

        let retried = logs.expect_one(crate::target::ORM, "transaction conflict — retrying");
        assert_eq!(retried.level, "warn");
        assert_eq!(retried.field("attempt").as_deref(), Some("1"));
        assert_eq!(retried.field("attempts").as_deref(), Some("2"));

        let exhausted = logs.expect_one(
            crate::target::ORM,
            "transaction conflict — retry budget exhausted",
        );
        assert_eq!(exhausted.level, "warn");
        assert_eq!(exhausted.field("attempt").as_deref(), Some("2"));
        assert!(
            exhausted
                .field("error")
                .is_some_and(|e| e.contains("database error 40001")),
            "the last error, by its SQLSTATE, is what an operator needs, got {:?}",
            exhausted.fields,
        );
    }

    #[tokio::test]
    async fn returns_immediately_on_non_retryable() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<(), DbErr> = retry_on_conflict(3, Duration::from_millis(1), || async {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(DbErr::RecordNotFound("widget".into()))
        })
        .await;
        assert!(matches!(result, Err(DbErr::RecordNotFound(_))));
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "no retry on a non-conflict error",
        );
    }

    #[tokio::test]
    async fn zero_attempts_clamps_to_one() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        retry_on_conflict(0, Duration::from_millis(1), || async {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, DbErr>(())
        })
        .await
        .expect("the one attempt succeeds");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn excessive_attempts_clamp_to_ceiling() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<(), DbErr> =
            retry_on_conflict(usize::MAX, Duration::from_millis(1), || async {
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err::<(), _>(DbErr::RecordNotFound("non-retryable".into()))
            })
            .await;
        assert!(matches!(result, Err(DbErr::RecordNotFound(_))));
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "non-retryable error short-circuits regardless of the (clamped) budget",
        );
    }

    /// Within a fifth of `min(MAX_BACKOFF, base · 2^(failed − 1))`.
    fn assert_on_the_curve(base: Duration, failed: usize, wait: Duration) {
        let curve = (1..failed).fold(base, |wait, _| wait.saturating_mul(2).min(MAX_BACKOFF));
        assert!(
            wait >= curve.mul_f64(0.8) && wait <= curve.mul_f64(1.2),
            "after {failed} failed: {wait:?} against {curve:?}",
        );
    }

    #[test]
    fn each_wait_doubles_within_a_fifth_up_to_the_ceiling() {
        let base = Duration::from_millis(5);
        for failed in (1..=MAX_RETRY_ATTEMPTS).chain([1_000, usize::MAX]) {
            assert_on_the_curve(base, failed.min(64), backoff_for(base, failed, 7));
        }
        assert!(
            backoff_for(base, usize::MAX, 7) > Duration::ZERO,
            "a ceiling never wraps to zero — that would hot-spin the retry loop",
        );
    }

    #[test]
    fn two_calls_with_their_own_seeds_retry_apart() {
        let base = Duration::from_millis(5);
        let waits: std::collections::BTreeSet<Duration> = (0..32)
            .map(|call| backoff_for(base, 1, process_seed() ^ call))
            .collect();
        assert!(waits.len() > 1, "{waits:?}");
        for wait in waits {
            assert_on_the_curve(base, 1, wait);
        }
        assert_eq!(backoff_for(base, 3, 11), backoff_for(base, 3, 11));
    }

    #[tokio::test(start_paused = true)]
    async fn calls_that_conflicted_together_retry_apart() {
        // A whole second, so each jittered wait lands on the timer's millisecond.
        let initial = Duration::from_secs(1);
        let started = tokio::time::Instant::now();
        let mut calls = tokio::task::JoinSet::new();
        // Eight, not two: two seeds of their own still draw the same wait one
        // time in 401.
        for _ in 0..8 {
            let mut conflicted = false;
            calls.spawn(retry_on_conflict(2, initial, move || {
                let first = !std::mem::replace(&mut conflicted, true);
                async move {
                    if first {
                        Err(sqlx_db_err(Some("40001"), "could not serialize access"))
                    } else {
                        Ok(tokio::time::Instant::now())
                    }
                }
            }));
        }
        let mut retried_at = std::collections::BTreeSet::new();
        while let Some(call) = calls.join_next().await {
            let retried = call.expect("the call ran").expect("the retry committed");
            assert_on_the_curve(initial, 1, retried - started);
            retried_at.insert(retried);
        }
        assert!(
            retried_at.len() > 1,
            "every call retried at one instant: {retried_at:?}"
        );
    }
}
