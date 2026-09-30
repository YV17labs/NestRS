//! [`RedisWorkerConfig`] — the consumer binding's own settings. Namespace
//! `redis__worker`, read off the path like every other config's: the crate's
//! word, then the binding folder's, so `NESTRS_REDIS__WORKER__*` names the type
//! and the file that parse it.
//!
//! **Every duration has a floor and a ceiling**, and a value outside them fails
//! the boot naming the variable — whichever side set it, since a value out of
//! bounds breaks the worker the same way from code as from the environment. Two
//! ceilings are another setting's: the lease is at most half the orphan
//! threshold and the poll at most the whole of it, and a pair that breaks one is
//! refused naming both variables.
//!
//! **No value accepted reaches apalis in a form it panics on.** apalis-redis
//! 0.7.4 turns the orphan threshold into a `chrono` duration with
//! `from_std(..).unwrap()` on every poll and subtracts it from the current
//! instant, which panics too once the result falls outside `chrono`'s range —
//! from a threshold of about 262,000 years, thirteen digits of seconds that
//! nothing refused before it had a ceiling. A day is nowhere near it, and every
//! other duration apalis reads — the poll, the heartbeat, its scan of the
//! schedule — arms a `futures-timer` delay, which saturates rather than panics.
//! Pinned by the tests below.

use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigService, DurationBounds, DurationUnit, Floor, Result, config,
};

/// Default drain window on shutdown: 30s, Kubernetes' default
/// `terminationGracePeriodSeconds`. Attempts run for all but its last five
/// seconds, and what still runs then is handed back in those, which takes
/// milliseconds; the queue documentation still asks for a grace period above
/// the window, so SIGKILL never cuts the reserve short.
const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

/// Default orphan threshold: five minutes, apalis's own — ten of the heartbeats a
/// replica proves it is alive with.
const DEFAULT_ORPHAN_AFTER_SECS: u64 = 300;

/// Default lease: thirty seconds, renewed every ten while the attempt runs.
const DEFAULT_LEASE_SECS: u64 = 30;

/// Default poll: a tenth of a second, apalis's own.
const DEFAULT_POLL_INTERVAL_MS: u64 = 100;

/// How many heartbeats fit in the orphan threshold.
const HEARTBEATS_PER_THRESHOLD: u32 = 10;

/// The fastest a replica beats, whatever the threshold: apalis records a
/// heartbeat to the second, so a faster one would record nothing new.
const MIN_HEARTBEAT: Duration = Duration::from_secs(1);

/// The drain window's bounds, the variable that sets it, and why.
const SHUTDOWN_TIMEOUT: DurationBounds = DurationBounds {
    key: "SHUTDOWN_TIMEOUT_SECS",
    field: "RedisWorkerConfig::shutdown_timeout",
    unit: DurationUnit::Seconds,
    least: Floor::Units(Bound {
        count: 1,
        why: "the drain keeps half its window, up to five seconds, to hand interrupted jobs \
              back, and with none a job running at shutdown stays in flight until a peer's sweep \
              takes it, the orphan threshold later",
    }),
    most: Some(Bound {
        count: 60 * 60,
        why: "a stopping replica holds its rollout for as long as it drains, and a job still \
              running an hour into a shutdown is one to hand back and resume from its \
              checkpoint, not to wait out",
    }),
};

/// The orphan threshold's bounds, the variable that sets it, and why.
const ORPHAN_AFTER: DurationBounds = DurationBounds {
    key: "ORPHAN_AFTER_SECS",
    field: "RedisWorkerConfig::orphan_after",
    unit: DurationUnit::Seconds,
    least: Floor::Units(Bound {
        count: 5,
        why: "five heartbeats of one second each — anything shorter reads a slow answer as a death",
    }),
    most: Some(Bound {
        count: 24 * 60 * 60,
        why: "the threshold is how long a crashed replica's jobs wait for a peer to take them, \
              and past a day that wait is a typo rather than a choice",
    }),
};

/// The lease's floor, the variable that sets it, and why. Its ceiling is half
/// the orphan threshold, checked once both are read.
const LEASE: DurationBounds = DurationBounds {
    key: "LEASE_SECS",
    field: "RedisWorkerConfig::lease",
    unit: DurationUnit::Seconds,
    least: Floor::Units(Bound {
        count: 1,
        why: "a lease renewed every third of it needs at least a second to renew in",
    }),
    most: None,
};

/// The poll's floor, the variable that sets it, and why: every poll costs Redis
/// a fetch and a sweep of silent peers per method per replica, jobs or none, so
/// ten milliseconds already spends up to two hundred scripts a second on a
/// method with nothing to do. Its ceiling is the orphan threshold, checked once
/// both are read.
const POLL_INTERVAL: DurationBounds = DurationBounds {
    key: "POLL_INTERVAL_MS",
    field: "RedisWorkerConfig::poll_interval",
    unit: DurationUnit::Millis,
    least: Floor::Units(Bound {
        count: 10,
        why: "every poll costs Redis a fetch and a sweep per method per replica, jobs or none — \
              up to two hundred scripts a second at the floor",
    }),
    most: None,
};

/// Consumer settings, settable via `NESTRS_REDIS__WORKER__*` or pinned through
/// [`RedisWorkerModule::for_root`](crate::RedisWorkerModule::for_root).
#[config(namespace = "redis__worker")]
#[derive(Clone, Debug)]
pub struct RedisWorkerConfig {
    /// How long the worker lets running attempts finish after a shutdown signal.
    /// An attempt still running when the window closes is interrupted and its
    /// job handed back to the queue for another replica, inside the window — so
    /// SIGTERM never blocks past it, and the orchestrator's SIGKILL never takes a
    /// job with it. Read from `NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS`, at
    /// least 1 and at most 3600 (an hour); defaults to 30s.
    pub shutdown_timeout: Duration,
    /// How long a replica may go without proving it is alive before the others
    /// take the jobs it was running and run them again. A replica proves it
    /// every tenth of this (never more often than once a second), so a live one
    /// is never taken for dead by a slow answer or two. Shorter recovers a
    /// crashed replica's jobs sooner; the delivery lease keeps a job a live
    /// replica still runs from running twice either way. Read from
    /// `NESTRS_REDIS__WORKER__ORPHAN_AFTER_SECS`, at least 5 and at most 86400
    /// (a day); defaults to 300s.
    pub orphan_after: Duration,
    /// How long a running attempt's claim on its job outlives its last renewal.
    /// The claim is renewed every third of this while the attempt runs, and a
    /// second delivery of the same job waits it out rather than running beside
    /// it. After a crash the job runs again once the claim lapses, so this is also
    /// the longest a crashed replica's job waits for a restarted one. Read from
    /// `NESTRS_REDIS__WORKER__LEASE_SECS`, at least 1 and at most half the orphan
    /// threshold; defaults to 30s.
    pub lease: Duration,
    /// How often each method of a replica asks Redis for jobs while one of its
    /// permits is free. Each ask takes up to the method's `concurrency` jobs, so
    /// a method whose jobs are short starts at most `concurrency` of them per
    /// interval on one replica: a shorter interval raises that ceiling, and costs
    /// Redis a fetch and a sweep of silent peers every interval per method per
    /// replica, busy or idle. The sweep is what hands a crashed replica's jobs
    /// to the others, so the poll is at most the orphan threshold. Read from
    /// `NESTRS_REDIS__WORKER__POLL_INTERVAL_MS`, at least 10; defaults to 100 ms.
    pub poll_interval: Duration,
}

impl RedisWorkerConfig {
    /// How often a replica proves to its peers that it is alive: a tenth of the
    /// orphan threshold, and never more often than once a second.
    pub(crate) fn heartbeat(&self) -> Duration {
        (self.orphan_after / HEARTBEATS_PER_THRESHOLD).max(MIN_HEARTBEAT)
    }

    /// How long after a delivery answers its acknowledgement may still be on
    /// its way to Redis: a poll, then a heartbeat.
    ///
    /// apalis-redis 0.7 acknowledges from the loop that also fetches, beats and
    /// sweeps, one call at a time, so an answer waits behind whatever that loop
    /// is doing and the answers queued before it. A loop further behind than a
    /// heartbeat has let its own heartbeat slip as well — a replica that far
    /// behind is one its peers are on their way to sweeping, and a job it held
    /// that is redelivered by a sweep is what a settled mark's usual span
    /// already covers.
    pub(crate) fn acknowledged_within(&self) -> Duration {
        self.poll_interval.saturating_add(self.heartbeat())
    }
}

impl Default for RedisWorkerConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout: Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECS),
            orphan_after: Duration::from_secs(DEFAULT_ORPHAN_AFTER_SECS),
            lease: Duration::from_secs(DEFAULT_LEASE_SECS),
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
        }
    }
}

impl Config for RedisWorkerConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let shutdown_timeout = SHUTDOWN_TIMEOUT.read(env, base.shutdown_timeout)?;
        let orphan_after = ORPHAN_AFTER.read(env, base.orphan_after)?;
        let lease = LEASE.read(env, base.lease)?;
        let poll_interval = POLL_INTERVAL.read(env, base.poll_interval)?;
        let threshold = env.var_name(ORPHAN_AFTER.key);
        // A peer takes a crashed replica's jobs once it has missed its
        // heartbeats for the threshold — at least four fifths of it after the
        // crash, a heartbeat being at most a fifth — and the delivery that
        // meets a lease still held hands the job back until it lapses. At half
        // the threshold, a lease renewed until the crash has lapsed first.
        if lease.value > orphan_after.value / 2 {
            return Err(lease.refuse(format!(
                "the lease is {:?}, more than half the orphan threshold of {:?} ({threshold}) — a \
                 peer takes a crashed replica's jobs once the threshold has passed, and a lease \
                 still held then hands each back until it lapses; at half the threshold it has \
                 always lapsed first",
                lease.value, orphan_after.value,
            )));
        }
        // apalis sweeps silent peers' jobs back onto the queue on the poll, not
        // on a clock of its own: a poll past the orphan threshold would leave a
        // crashed replica's jobs waiting past it — and one of hours, a typo
        // away, would never fetch at all while the worker says it started.
        if poll_interval.value > orphan_after.value {
            return Err(poll_interval.refuse(format!(
                "the poll is {:?}, longer than the orphan threshold of {:?} ({threshold}) — apalis \
                 sweeps a silent replica's jobs back onto the queue on the poll, so a longer one \
                 leaves them waiting past the threshold",
                poll_interval.value, orphan_after.value,
            )));
        }
        Ok(Self {
            shutdown_timeout: shutdown_timeout.value,
            orphan_after: orphan_after.value,
            lease: lease.value,
            poll_interval: poll_interval.value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_config::Namespaced;

    /// The config read from `vars` over `base`, as the boot reads it.
    fn read(vars: &[(&str, &str)], base: RedisWorkerConfig) -> Result<RedisWorkerConfig> {
        RedisWorkerConfig::from_env(
            &ConfigService::with_vars("redis__worker", vars.iter().copied()),
            base,
        )
    }

    /// The refusal of `vars` over the defaults, as its sentence.
    fn refused(vars: &[(&str, &str)]) -> String {
        read(vars, RedisWorkerConfig::default())
            .expect_err("refused")
            .to_string()
    }

    /// The refusal of `pinned`, with no variable set, as its sentence.
    fn refused_pinned(pinned: RedisWorkerConfig) -> String {
        read(&[], pinned).expect_err("refused").to_string()
    }

    fn var(key: &str) -> String {
        nest_rs_config::var_name("redis__worker", key)
    }

    #[test]
    fn the_namespace_is_the_crate_word_then_the_binding_word() {
        assert_eq!(RedisWorkerConfig::NAMESPACE, "redis__worker");
    }

    #[test]
    fn shutdown_timeout_defaults_to_30s_and_reads_the_env() {
        // QUEUE-I5: the drain window is configurable and defaults to a
        // K8s-friendly 30s.
        assert_eq!(
            RedisWorkerConfig::default().shutdown_timeout,
            Duration::from_secs(30)
        );
        let cfg = read(&[("SHUTDOWN_TIMEOUT_SECS", "5")], Default::default()).expect("ok");
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(5));
    }

    /// The liveness settings read the env over the base, and the heartbeat is
    /// several beats inside the threshold at any value accepted.
    #[test]
    fn the_orphan_threshold_and_the_lease_read_the_env_and_the_heartbeat_follows() {
        let cfg = read(
            &[("ORPHAN_AFTER_SECS", "60"), ("LEASE_SECS", "4")],
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.orphan_after, Duration::from_secs(60));
        assert_eq!(cfg.lease, Duration::from_secs(4));
        assert_eq!(cfg.heartbeat(), Duration::from_secs(6));

        let Floor::Units(least) = ORPHAN_AFTER.least else {
            panic!("the orphan threshold's floor is a count of seconds");
        };
        let floor = RedisWorkerConfig {
            orphan_after: Duration::from_secs(least.count),
            lease: Duration::from_secs(1),
            ..Default::default()
        };
        assert_eq!(floor.heartbeat(), MIN_HEARTBEAT);
        assert!(floor.orphan_after >= floor.heartbeat() * 5);
        assert_eq!(
            RedisWorkerConfig::default().heartbeat(),
            Duration::from_secs(30)
        );
    }

    /// The poll reads the env over the base, in milliseconds, and defaults to
    /// apalis's own tenth of a second.
    #[test]
    fn the_poll_defaults_to_a_tenth_of_a_second_and_reads_the_env_in_milliseconds() {
        assert_eq!(
            RedisWorkerConfig::default().poll_interval,
            Duration::from_millis(100)
        );
        let cfg = read(&[("POLL_INTERVAL_MS", "25")], Default::default()).expect("ok");
        assert_eq!(cfg.poll_interval, Duration::from_millis(25));
    }

    /// A drain with no window, a threshold that would read one slow answer as a
    /// death, a lease with no time to renew in, and a poll that would spend
    /// Redis on nothing, are refused naming the variable — never clamped in
    /// silence.
    #[test]
    fn a_value_below_its_floor_is_refused_naming_the_variable() {
        for (key, value) in [
            ("SHUTDOWN_TIMEOUT_SECS", "0"),
            ("ORPHAN_AFTER_SECS", "4"),
            ("LEASE_SECS", "0"),
            ("POLL_INTERVAL_MS", "9"),
        ] {
            let refused = refused(&[(key, value)]);
            assert!(refused.contains(&var(key)), "{refused}");
            assert!(refused.contains("must be at least"), "{refused}");
        }
    }

    /// A drain past an hour and a threshold past a day are refused naming the
    /// variable, and the ceiling itself is accepted.
    #[test]
    fn a_value_above_its_ceiling_is_refused_naming_the_variable() {
        for (key, value) in [
            ("SHUTDOWN_TIMEOUT_SECS", "3601"),
            ("ORPHAN_AFTER_SECS", "86401"),
        ] {
            let refused = refused(&[(key, value)]);
            assert!(refused.contains(&var(key)), "{refused}");
            assert!(refused.contains("must be at most"), "{refused}");
        }
        let ceiling = read(
            &[
                ("SHUTDOWN_TIMEOUT_SECS", "3600"),
                ("ORPHAN_AFTER_SECS", "86400"),
            ],
            Default::default(),
        )
        .expect("the ceilings themselves are accepted");
        assert_eq!(ceiling.shutdown_timeout, Duration::from_secs(60 * 60));
        assert_eq!(ceiling.orphan_after, Duration::from_secs(24 * 60 * 60));
    }

    /// A lease longer than half the orphan threshold is refused naming both
    /// variables, whichever of the two moved: a lease set too long, and a
    /// threshold set too short for the default lease. Half the threshold is
    /// accepted.
    #[test]
    fn a_lease_longer_than_half_the_orphan_threshold_is_refused_naming_both() {
        for refused in [
            refused(&[("LEASE_SECS", "151")]),
            refused(&[("ORPHAN_AFTER_SECS", "59")]),
            refused_pinned(RedisWorkerConfig {
                lease: Duration::from_millis(150_001),
                ..Default::default()
            }),
        ] {
            for key in ["LEASE_SECS", "ORPHAN_AFTER_SECS"] {
                assert!(refused.contains(&var(key)), "{refused}");
            }
            assert!(
                refused.contains("more than half the orphan threshold"),
                "{refused}"
            );
        }

        let half = read(&[("LEASE_SECS", "150")], Default::default())
            .expect("a lease of half the threshold is accepted");
        assert_eq!(half.lease * 2, half.orphan_after);
    }

    /// A poll longer than the orphan threshold — the sweep that hands a crashed
    /// replica's jobs over runs on it — is refused naming both variables,
    /// pinned or from the environment; one equal to it is accepted.
    #[test]
    fn a_poll_longer_than_the_orphan_threshold_is_refused_naming_both() {
        for refused in [
            refused(&[("ORPHAN_AFTER_SECS", "60"), ("POLL_INTERVAL_MS", "60001")]),
            refused_pinned(RedisWorkerConfig {
                poll_interval: Duration::from_secs(301),
                ..Default::default()
            }),
        ] {
            for key in ["POLL_INTERVAL_MS", "ORPHAN_AFTER_SECS"] {
                assert!(refused.contains(&var(key)), "{refused}");
            }
            assert!(
                refused.contains("longer than the orphan threshold"),
                "{refused}"
            );
        }

        let equal = read(
            &[("ORPHAN_AFTER_SECS", "60"), ("POLL_INTERVAL_MS", "60000")],
            Default::default(),
        )
        .expect("a poll as long as the threshold is accepted");
        assert_eq!(equal.poll_interval, equal.orphan_after);
    }

    /// A bound holds for a value pinned in code as it does for the
    /// environment's: the refusal names the field and the variable that would
    /// override it, and the bound itself is accepted.
    #[test]
    fn a_value_pinned_outside_its_bounds_is_refused_naming_the_field_and_the_variable() {
        for (pinned, field, key) in [
            (
                RedisWorkerConfig {
                    shutdown_timeout: Duration::from_millis(999),
                    ..Default::default()
                },
                "shutdown_timeout",
                "SHUTDOWN_TIMEOUT_SECS",
            ),
            (
                RedisWorkerConfig {
                    shutdown_timeout: Duration::from_secs(60 * 60) + Duration::from_nanos(1),
                    ..Default::default()
                },
                "shutdown_timeout",
                "SHUTDOWN_TIMEOUT_SECS",
            ),
            (
                RedisWorkerConfig {
                    poll_interval: Duration::from_millis(9),
                    ..Default::default()
                },
                "poll_interval",
                "POLL_INTERVAL_MS",
            ),
            (
                RedisWorkerConfig {
                    orphan_after: Duration::from_secs(4),
                    lease: Duration::from_secs(1),
                    poll_interval: Duration::from_millis(10),
                    ..Default::default()
                },
                "orphan_after",
                "ORPHAN_AFTER_SECS",
            ),
            (
                RedisWorkerConfig {
                    orphan_after: Duration::from_secs(24 * 60 * 60 + 1),
                    ..Default::default()
                },
                "orphan_after",
                "ORPHAN_AFTER_SECS",
            ),
            (
                RedisWorkerConfig {
                    lease: Duration::from_millis(999),
                    ..Default::default()
                },
                "lease",
                "LEASE_SECS",
            ),
        ] {
            let refused = refused_pinned(pinned);
            assert!(
                refused.contains(&format!("`RedisWorkerConfig::{field}` set in code")),
                "{refused}"
            );
            assert!(refused.contains(&var(key)), "{refused}");
        }

        for accepted in [
            RedisWorkerConfig {
                shutdown_timeout: Duration::from_secs(1),
                poll_interval: Duration::from_millis(10),
                orphan_after: Duration::from_secs(5),
                lease: Duration::from_secs(1),
            },
            RedisWorkerConfig {
                shutdown_timeout: Duration::from_secs(60 * 60),
                poll_interval: Duration::from_secs(24 * 60 * 60),
                orphan_after: Duration::from_secs(24 * 60 * 60),
                lease: Duration::from_secs(12 * 60 * 60),
            },
        ] {
            let read = read(&[], accepted.clone()).expect("the bounds themselves are accepted");
            assert_eq!(read.shutdown_timeout, accepted.shutdown_timeout);
            assert_eq!(read.orphan_after, accepted.orphan_after);
            assert_eq!(read.lease, accepted.lease);
            assert_eq!(read.poll_interval, accepted.poll_interval);
        }
    }

    /// The one path from this config into apalis that can panic is the orphan
    /// threshold: on every poll apalis-redis 0.7.4 evaluates
    /// `Utc::now() - chrono::Duration::from_std(reenqueue_orphaned_after).unwrap()`,
    /// where the conversion unwraps and the subtraction is
    /// `checked_sub_signed(..).expect(..)`. Evaluated here without either
    /// panic, with the longest threshold accepted both answer — while a
    /// threshold of thirteen digits, which nothing refused before the ceiling,
    /// fails the subtraction apalis would have panicked on.
    #[test]
    fn apalis_sweeps_under_the_longest_orphan_threshold_accepted_without_a_panic() {
        let longest = ORPHAN_AFTER.most.as_ref().expect("a ceiling").count;
        let accepted = read(
            &[("ORPHAN_AFTER_SECS", &longest.to_string())],
            Default::default(),
        )
        .expect("the longest threshold is accepted");
        let swept = |threshold: Duration| {
            chrono::Duration::from_std(threshold)
                .ok()
                .and_then(|threshold| chrono::Utc::now().checked_sub_signed(threshold))
        };
        assert!(
            swept(accepted.orphan_after).is_some(),
            "apalis's sweep answers under the longest threshold accepted"
        );

        let thirteen_digits = 1_000_000_000_000_u64 * 10;
        assert!(
            swept(Duration::from_secs(thirteen_digits)).is_none(),
            "a threshold of about 317,000 years panics apalis's sweep"
        );
        let refused = refused(&[("ORPHAN_AFTER_SECS", &thirteen_digits.to_string())]);
        assert!(refused.contains("must be at most"), "{refused}");
    }
}
