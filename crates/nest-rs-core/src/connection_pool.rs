//! The connection pools drivers open, observed by whatever exports their
//! connections: a driver registers its pool, and the observability stack reads
//! every one still open when its metric reader exports.
//!
//! The registry holds each pool weakly, so what an app opened goes with it.

use std::sync::{Arc, Mutex, PoisonError, Weak};

/// A connection pool a driver opened, read as the database conventions'
/// `db.client.connection.*` instruments describe one.
pub trait ConnectionPool: Send + Sync + 'static {
    /// Its name, unique in the process: `server.address:server.port/db.namespace`,
    /// as the conventions build one for a pool that has none of its own.
    fn name(&self) -> &str;

    /// Its connections now; `None` once it closed.
    fn connections(&self) -> Option<PoolConnections>;

    /// How many times a caller gave up waiting for one of its connections.
    fn timeouts(&self) -> u64;
}

/// A pool's connections at one instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolConnections {
    idle: u64,
    used: u64,
    max: u64,
    idle_min: u64,
}

impl PoolConnections {
    /// `idle` connections open and waiting, `used` handed out, of the `max` the
    /// pool opens, keeping `idle_min` open when idle.
    pub const fn new(idle: u64, used: u64, max: u64, idle_min: u64) -> Self {
        Self {
            idle,
            used,
            max,
            idle_min,
        }
    }

    /// Open and waiting to be used.
    pub const fn idle(&self) -> u64 {
        self.idle
    }

    /// Open and handed out.
    pub const fn used(&self) -> u64 {
        self.used
    }

    /// The most the pool opens.
    pub const fn max(&self) -> u64 {
        self.max
    }

    /// The fewest idle connections the pool keeps open.
    pub const fn idle_min(&self) -> u64 {
        self.idle_min
    }
}

static POOLS: Mutex<Vec<Weak<dyn ConnectionPool>>> = Mutex::new(Vec::new());

/// Observe `pool` for as long as something else holds it.
pub fn observe<P: ConnectionPool>(pool: &Arc<P>) {
    let observed: Weak<dyn ConnectionPool> = Arc::downgrade(pool) as Weak<P>;
    let mut pools = POOLS.lock().unwrap_or_else(PoisonError::into_inner);
    pools.retain(|held| held.strong_count() > 0);
    pools.push(observed);
}

/// `connection_pool` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use super::{Arc, ConnectionPool, POOLS, PoisonError, Weak};

    /// Every pool observed and still held, for the observability stack's
    /// instruments.
    pub fn observed_pools() -> Vec<Arc<dyn ConnectionPool>> {
        let mut pools = POOLS.lock().unwrap_or_else(PoisonError::into_inner);
        pools.retain(|held| held.strong_count() > 0);
        pools.iter().filter_map(Weak::upgrade).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::__private::observed_pools;
    use super::*;

    struct Probe(&'static str);

    impl ConnectionPool for Probe {
        fn name(&self) -> &str {
            self.0
        }

        fn connections(&self) -> Option<PoolConnections> {
            Some(PoolConnections::new(2, 1, 10, 0))
        }

        fn timeouts(&self) -> u64 {
            3
        }
    }

    fn names() -> Vec<String> {
        observed_pools()
            .iter()
            .map(|pool| pool.name().to_owned())
            .collect()
    }

    #[test]
    fn a_pool_is_observed_while_held_and_forgotten_once_dropped() {
        let held = Arc::new(Probe("db.internal:5432/kept"));
        let dropped = Arc::new(Probe("db.internal:5432/dropped"));
        observe(&held);
        observe(&dropped);
        assert!(names().contains(&"db.internal:5432/dropped".to_owned()));

        drop(dropped);

        let names = names();
        assert!(
            names.contains(&"db.internal:5432/kept".to_owned()),
            "{names:?}"
        );
        assert!(
            !names.contains(&"db.internal:5432/dropped".to_owned()),
            "{names:?}"
        );
        let kept = observed_pools()
            .into_iter()
            .find(|pool| pool.name() == "db.internal:5432/kept")
            .expect("the held pool");
        let connections = kept.connections().expect("open");
        assert_eq!(
            (
                connections.idle(),
                connections.used(),
                connections.max(),
                connections.idle_min(),
                kept.timeouts(),
            ),
            (2, 1, 10, 0, 3),
        );
    }
}
