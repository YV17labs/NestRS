//! [`RedisQueueConfig`] — the Redis queue binding's own settings, under
//! `<PREFIX>_REDIS__QUEUE__*`. The drain window is the port's (`<PREFIX>_QUEUE__*`).

use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};

/// Default lease: thirty seconds, renewed every ten while the attempt runs.
const DEFAULT_LEASE_SECS: u64 = 30;

/// The lease's bounds, the variable that sets it, and why.
pub(crate) const LEASE: DurationBounds = DurationBounds::secs(
    "LEASE_SECS",
    "RedisQueueConfig::lease",
    Floor::Units(Bound {
        count: 1,
        why: "a lease is renewed every third of it, and a renewal is a round trip to Redis",
    }),
    Bound {
        count: 60 * 60,
        why: "a crashed replica's jobs wait out their lease before another worker runs them, \
              and past an hour that wait is a typo rather than a choice",
    },
);

/// The Redis queue binding's settings, set through `<PREFIX>_REDIS__QUEUE__*`
/// or pinned through
/// [`RedisQueueModule::for_root`](crate::RedisQueueModule::for_root).
#[config(namespace = "redis__queue")]
#[derive(Clone, Debug)]
pub struct RedisQueueConfig {
    /// How long a delivery holds its job without a renewal. The worker renews
    /// it every third of this while the attempt runs; once it lapses, the job
    /// goes to the next worker that asks, and the attempt still running, if
    /// any, is cut. So it is also the longest a crashed replica's job waits
    /// before another runs it. Read from `<PREFIX>_REDIS__QUEUE__LEASE_SECS`,
    /// at least 1 and at most 3600; defaults to 30s. More than one and a half
    /// times the connection's budget, or the boot is refused: a renewal sent a
    /// third into the lease may wait out the whole budget.
    pub lease: Duration,
}

impl Default for RedisQueueConfig {
    fn default() -> Self {
        Self {
            lease: Duration::from_secs(DEFAULT_LEASE_SECS),
        }
    }
}

impl Config for RedisQueueConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            lease: LEASE.read(env, base.lease)?.value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_config::Namespaced;

    fn read(vars: &[(&str, &str)], base: RedisQueueConfig) -> Result<RedisQueueConfig> {
        RedisQueueConfig::from_env(
            &ConfigService::with_vars("redis__queue", vars.iter().copied()),
            base,
        )
    }

    #[test]
    fn the_namespace_is_the_crate_word_then_the_binding_word() {
        assert_eq!(RedisQueueConfig::NAMESPACE, "redis__queue");
    }

    #[test]
    fn the_lease_defaults_to_30s_and_reads_the_env() {
        assert_eq!(RedisQueueConfig::default().lease, Duration::from_secs(30));
        let cfg = read(&[("LEASE_SECS", "4")], RedisQueueConfig::default()).expect("ok");
        assert_eq!(cfg.lease, Duration::from_secs(4));
    }

    #[test]
    fn a_lease_outside_its_bounds_is_refused_naming_the_variable() {
        let var = nest_rs_config::var_name("redis__queue", "LEASE_SECS");
        for value in ["0", "3601"] {
            let refused = read(&[("LEASE_SECS", value)], RedisQueueConfig::default())
                .expect_err("refused")
                .to_string();
            assert!(refused.contains(&var), "{refused}");
        }
        let pinned = RedisQueueConfig {
            lease: Duration::from_millis(999),
        };
        let refused = read(&[], pinned).expect_err("refused").to_string();
        assert!(refused.contains("RedisQueueConfig::lease"), "{refused}");
    }
}
