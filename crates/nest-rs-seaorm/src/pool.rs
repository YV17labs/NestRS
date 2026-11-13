//! The app's pool as the database conventions read a connection pool — its
//! connections by state, its bounds and the callers it timed out — observed
//! through `nest_rs_core::connection_pool`.
//!
//! sqlx tells no waiter's count and no acquire's duration, so the pool reports
//! neither `pending_requests` nor `wait_time`, `create_time` or `use_time`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use nest_rs_core::connection_pool::{self, ConnectionPool, PoolConnections};
use sea_orm::sqlx::PgPool;
use sea_orm::{ConnAcquireErr, DatabaseConnection, DatabaseConnectionType, DbErr};

/// The callers that gave up waiting for a connection. `Repo` holds one pool
/// per binary, so the process's count is its pool's.
static ACQUIRE_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

/// One pool, named as the conventions name a pool that has none of its own.
pub(crate) struct SeaOrmPool {
    pool: PgPool,
    name: String,
}

impl SeaOrmPool {
    /// `db`'s pool, observed for as long as the handle returned lives; `None`
    /// for a connection holding no Postgres pool.
    pub(crate) fn observed(db: &DatabaseConnection) -> Option<Arc<Self>> {
        let DatabaseConnectionType::SqlxPostgresPoolConnection(_) = db.inner else {
            return None;
        };
        let pool = db.get_postgres_connection_pool().clone();
        let options = pool.connect_options();
        let server = format!("{}:{}", options.get_host(), options.get_port());
        let name = match options.get_database() {
            Some(database) => format!("{server}/{database}"),
            None => server,
        };
        let observed = Arc::new(Self { pool, name });
        connection_pool::observe(&observed);
        Some(observed)
    }
}

/// Count `err` where it is a caller that gave up waiting for a connection.
pub(crate) fn count_timeout(err: &DbErr) {
    if matches!(err, DbErr::ConnectionAcquire(ConnAcquireErr::Timeout)) {
        ACQUIRE_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    }
}

impl ConnectionPool for SeaOrmPool {
    fn name(&self) -> &str {
        &self.name
    }

    fn connections(&self) -> Option<PoolConnections> {
        if self.pool.is_closed() {
            return None;
        }
        let open = u64::from(self.pool.size());
        let idle = u64::try_from(self.pool.num_idle())
            .unwrap_or(u64::MAX)
            .min(open);
        let options = self.pool.options();
        Some(PoolConnections::new(
            idle,
            open.saturating_sub(idle),
            u64::from(options.get_max_connections()),
            u64::from(options.get_min_connections()),
        ))
    }

    fn timeouts(&self) -> u64 {
        ACQUIRE_TIMEOUTS.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_giving_up_on_a_connection_is_counted_and_nothing_else_is() {
        let before = ACQUIRE_TIMEOUTS.load(Ordering::Relaxed);
        count_timeout(&DbErr::ConnectionAcquire(ConnAcquireErr::ConnectionClosed));
        count_timeout(&DbErr::RecordNotFound("posts".to_owned()));
        assert_eq!(ACQUIRE_TIMEOUTS.load(Ordering::Relaxed), before);

        count_timeout(&DbErr::ConnectionAcquire(ConnAcquireErr::Timeout));
        assert_eq!(ACQUIRE_TIMEOUTS.load(Ordering::Relaxed), before + 1);
    }
}
