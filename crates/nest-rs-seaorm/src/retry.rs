//! Bounded retry for transient transaction conflicts.
//!
//! Postgres' serializable isolation surfaces a conflicting transaction as
//! `40001` (`serialization_failure`); a deadlock as `40P01`. MySQL surfaces
//! a deadlock as `1213`; SQL Server as `1205`. The conventional answer is
//! "retry the whole transaction", because SeaORM's transaction handle is
//! already aborted past a conflict — the same `DatabaseTransaction` cannot
//! be replayed. This module is the **reusable primitive** a service can
//! wrap around a programmatic transaction boundary; the [`DbContext`]
//! interceptor consults [`is_retryable_conflict`] to tag the conflict for
//! observability when the `retry_serialization_conflicts` config is on.
//!
//! [`DbContext`]: crate::DbContext
//! [`is_retryable_conflict`]: crate::retry::is_retryable_conflict

use std::time::Duration;

use sea_orm::{DbErr, RuntimeErr};

/// SQLSTATE markers a transient conflict surfaces under across the
/// supported backends. Matched against the typed `sqlx::Error::Database`'s
/// `code()` so a digit substring appearing in a message — a port number,
/// byte offset, row id, timestamp — does not get misclassified as a
/// conflict and retried.
const RETRYABLE_SQLSTATES: &[&str] = &[
    "40001", // PG / MySQL — serialization failure
    "40P01", // PG — deadlock detected
    "1213",  // MySQL — deadlock
    "1205",  // SQL Server — deadlock victim
];

/// Hard ceiling for the public retry budget. A misconfigured `usize::MAX`
/// would otherwise hot-spin on a persistent conflict; the cap halts the
/// retry loop at 32 attempts (an exponential backoff that already exceeds
/// the lifetime of any reasonable request) regardless of the input.
pub const MAX_RETRY_ATTEMPTS: usize = 32;

/// Default bounded retry budget for [`retry_on_conflict`]: 3 attempts.
pub const DEFAULT_RETRY_ATTEMPTS: usize = 3;

/// Initial backoff before the first retry — 5 ms, doubled each retry
/// (5 ms → 10 ms → 20 ms). Small enough that a contended-but-quick conflict
/// is invisible; jitter-free because the contention is single-process.
pub const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(5);

/// Per-sleep ceiling: a single retry sleep never exceeds 30 s, regardless
/// of how many attempts have already failed. Without this cap, exponential
/// doubling at attempt 25 already exceeds 23 hours — well past any
/// reasonable request lifetime, and long enough that captured `Arc`s
/// (transaction handles, services) stay pinned for hours after the real
/// work has been abandoned. 30 s preserves coverage for genuinely
/// transient conflicts while keeping the retry loop bounded.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Whether `err` is a transient conflict worth retrying. Matches the
/// SQLSTATE returned by `sqlx`'s typed `DatabaseError::code()` — never the
/// formatted error string, so a digit substring in a message (a port, a
/// row id, a timestamp) cannot trigger a false retry.
pub fn is_retryable_conflict(err: &DbErr) -> bool {
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

/// Run `op` up to `attempts` times, sleeping `initial_backoff << attempt`
/// between tries when the previous attempt failed with a conflict
/// recognized by [`is_retryable_conflict`]. A non-retryable error returns
/// immediately; the last error after exhausting the budget is returned.
///
/// `attempts` is clamped to `[1, MAX_RETRY_ATTEMPTS]` so a caller passing
/// `0` still runs the operation once and a caller passing `usize::MAX`
/// does not hot-spin against a persistent conflict.
///
/// `op` is `FnMut` because the closure re-runs from scratch on each
/// attempt — a service that owns its transaction boundary can re-open it
/// inside `op` and replay the work against a fresh snapshot.
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
    let mut last_err: Option<DbErr> = None;
    for attempt in 0..attempts {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !is_retryable_conflict(&err) {
                    return Err(err);
                }
                tracing::warn!(
                    target: "nest_rs::orm",
                    attempt = attempt + 1,
                    attempts,
                    error = %err,
                    "transaction conflict — retrying",
                );
                last_err = Some(err);
                if attempt + 1 < attempts {
                    tokio::time::sleep(backoff_for(initial_backoff, attempt)).await;
                }
            }
        }
    }
    Err(last_err.expect("retry budget exhausted ⇒ at least one error observed"))
}

/// Saturating exponential backoff, capped at [`MAX_BACKOFF`]. `1u32 <<
/// attempt` overflows at `attempt >= 32` (UB in debug, wraps to `0` in
/// release — which would hot-spin); a `saturating_mul` against a
/// `saturating_pow` multiplier holds the doubling, and the final
/// `min(MAX_BACKOFF)` keeps a single retry sleep bounded so captured
/// `Arc`s do not stay pinned for hours.
fn backoff_for(initial: Duration, attempt: usize) -> Duration {
    let multiplier = 2u32.saturating_pow(attempt as u32);
    initial.saturating_mul(multiplier).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::RuntimeErr;

    fn db_err(msg: &str) -> DbErr {
        DbErr::Exec(RuntimeErr::Internal(msg.into()))
    }

    // A minimal `sqlx::error::DatabaseError` stub so we can construct a
    // typed `DbErr::Exec(RuntimeErr::SqlxError(...))` carrying a chosen
    // SQLSTATE — the only honest way to verify the positive path now
    // that `is_retryable_conflict` matches against typed codes, not
    // Display substrings.
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
        let sqlx_err = sea_orm::sqlx::Error::database(stub);
        DbErr::Exec(RuntimeErr::SqlxError(std::sync::Arc::new(sqlx_err)))
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
        // The whole point of the typed match: a `DbErr::Exec(Internal(_))`
        // whose message contains "40001" is NOT a serialization conflict —
        // the previous substring match would have falsely retried it.
        let err = db_err("error returned from database: 40001: could not serialize access");
        assert!(!is_retryable_conflict(&err));
    }

    #[test]
    fn rejects_textual_serialization_phrase_without_typed_sqlstate() {
        // Same rationale — a free-form Internal message is not a typed
        // database error and must not classify as a conflict.
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
        // Bug 7: a connection log mentioning port 40001, a row id 1213, a
        // byte offset 1205 would all have falsely classified as a
        // conflict under the substring match. The typed path drops them.
        let err = db_err("connection refused at 127.0.0.1:40001");
        assert!(!is_retryable_conflict(&err));
        let err = db_err("processing row id=1213 took 1205ms");
        assert!(!is_retryable_conflict(&err));
    }

    #[tokio::test]
    async fn retries_until_success_within_budget() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<&str, DbErr> = retry_on_conflict(3, Duration::from_millis(1), || async {
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
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<(), DbErr> = retry_on_conflict(2, Duration::from_millis(1), || async {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(sqlx_db_err(Some("40001"), "could not serialize access"))
        })
        .await;
        assert!(matches!(result, Err(DbErr::Exec(_))));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
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
        // A misconfigured `0` budget would otherwise loop never — clamp
        // to a single attempt so the operation still runs.
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let _ = retry_on_conflict(0, Duration::from_millis(1), || async {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, DbErr>(())
        })
        .await;
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn excessive_attempts_clamp_to_ceiling() {
        // Bug 6: `attempts = 33` would have overflowed `1u32 << attempt`
        // at the 32nd iteration (UB in debug, wraps to `0` in release).
        // The clamp halts the loop at `MAX_RETRY_ATTEMPTS` and the
        // saturating backoff keeps the sleep finite. We pass a
        // non-retryable error to short-circuit on the first attempt — the
        // point is to verify the entry-point does not panic / loop on a
        // huge budget, not to actually iterate to the cap.
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

    #[test]
    fn backoff_saturates_past_shift_overflow() {
        // The arithmetic that overflowed: `1u32 << 32` is UB in debug.
        // `backoff_for` must hold past 32 instead of panicking or wrapping
        // to zero (which would hot-spin the retry loop).
        let base = Duration::from_millis(5);
        // Sanity: small attempts double cleanly below the per-sleep cap.
        assert_eq!(backoff_for(base, 0), Duration::from_millis(5));
        assert_eq!(backoff_for(base, 3), Duration::from_millis(40));
        // The overflow boundary: this must not panic.
        let big = backoff_for(base, 32);
        assert!(
            big > Duration::ZERO,
            "shift cap must not wrap to zero — that would hot-spin the retry loop",
        );
        // Past the doubling point where exponential growth would exceed
        // 30 s, the per-sleep cap [`MAX_BACKOFF`] takes over and every
        // further attempt produces the same bounded sleep.
        assert_eq!(
            big, MAX_BACKOFF,
            "large attempts cap at MAX_BACKOFF, not at base * u32::MAX",
        );
        let huge = backoff_for(base, 1_000);
        let bigger = backoff_for(base, MAX_RETRY_ATTEMPTS);
        assert_eq!(huge, MAX_BACKOFF);
        assert_eq!(bigger, MAX_BACKOFF);
    }

    #[test]
    fn backoff_never_exceeds_max_backoff() {
        // Y3: per-sleep ceiling. Without the cap, doubling at attempt 25
        // already exceeds 23 hours and at attempt 31 reaches years —
        // pinning captured `Arc`s long after the request is dead. Every
        // call must stay `<= MAX_BACKOFF`.
        let base = Duration::from_millis(5);
        for attempt in 0..=MAX_RETRY_ATTEMPTS {
            let b = backoff_for(base, attempt);
            assert!(
                b <= MAX_BACKOFF,
                "attempt {attempt}: backoff {b:?} exceeds MAX_BACKOFF {MAX_BACKOFF:?}",
            );
        }
        // The specific attempt the bug report named: 31 doublings would
        // saturate `Duration` without the cap — assert it stays bounded.
        assert!(backoff_for(base, 31) <= MAX_BACKOFF);
        // And the absurd-input boundary: a usize::MAX-style attempt count
        // (clamped elsewhere, but the function itself must still hold).
        assert!(backoff_for(base, 1_000_000) <= MAX_BACKOFF);
    }
}
