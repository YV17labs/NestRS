//! [`RedisWorkerConfig`] — the consumer binding's own settings. Namespace
//! `redis__worker`, read off the path like every other config's: the crate's
//! word, then the binding folder's, so `NESTRS_REDIS__WORKER__*` names the type
//! and the file that parse it.

use std::time::Duration;

use nest_rs_config::{Config, ConfigError, ConfigService, Result, config};

/// Default drain window on shutdown: 30s — comfortably under a typical
/// Kubernetes `terminationGracePeriodSeconds` (30s) so the worker drains
/// cleanly before SIGKILL rather than being force-killed mid-job.
const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

/// Default orphan threshold: five minutes, apalis's own — ten of the heartbeats a
/// replica proves it is alive with.
const DEFAULT_ORPHAN_AFTER_SECS: u64 = 300;

/// The shortest orphan threshold accepted: five of the one-second heartbeats a
/// replica beats at least, so one slow answer never reads as a death.
const MIN_ORPHAN_AFTER_SECS: u64 = 5;

/// Default lease: thirty seconds, renewed every ten while the attempt runs.
const DEFAULT_LEASE_SECS: u64 = 30;

/// Default poll: a tenth of a second, apalis's own.
const DEFAULT_POLL_INTERVAL_MS: u64 = 100;

/// How many heartbeats fit in the orphan threshold.
const HEARTBEATS_PER_THRESHOLD: u32 = 10;

/// The fastest a replica beats, whatever the threshold: apalis records a
/// heartbeat to the second, so a faster one would record nothing new.
const MIN_HEARTBEAT: Duration = Duration::from_secs(1);

/// The shortest orphan threshold accepted, the variable that sets it, and why.
const ORPHAN_AFTER_FLOOR: Floor = Floor {
    key: "ORPHAN_AFTER_SECS",
    field: "orphan_after",
    unit: Unit::Seconds,
    least: MIN_ORPHAN_AFTER_SECS,
    why: "five heartbeats of one second each — anything shorter reads a slow answer as a death",
};

/// The shortest lease accepted, the variable that sets it, and why.
const LEASE_FLOOR: Floor = Floor {
    key: "LEASE_SECS",
    field: "lease",
    unit: Unit::Seconds,
    least: 1,
    why: "a lease renewed every third of it needs at least a second to renew in",
};

/// The shortest poll accepted, the variable that sets it, and why: every poll
/// costs Redis a fetch and a sweep of silent peers per method per replica, jobs
/// or none, so ten milliseconds already spends up to two hundred scripts a
/// second on a method with nothing to do.
const POLL_INTERVAL_FLOOR: Floor = Floor {
    key: "POLL_INTERVAL_MS",
    field: "poll_interval",
    unit: Unit::Millis,
    least: 10,
    why: "every poll costs Redis a fetch and a sweep per method per replica, jobs or none — up \
          to two hundred scripts a second at the floor",
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
    /// job with it. Read from `NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS`;
    /// defaults to 30s.
    pub shutdown_timeout: Duration,
    /// How long a replica may go without proving it is alive before the others
    /// take the jobs it was running and run them again. A replica proves it
    /// every tenth of this (never more often than once a second), so a live one
    /// is never taken for dead by a slow answer or two. Shorter recovers a
    /// crashed replica's jobs sooner; the delivery lease keeps a job a live
    /// replica still runs from running twice either way. Read from
    /// `NESTRS_REDIS__WORKER__ORPHAN_AFTER_SECS`, at least 5; defaults to 300s.
    pub orphan_after: Duration,
    /// How long a running attempt's claim on its job outlives its last renewal.
    /// The claim is renewed every third of this while the attempt runs, and a
    /// second delivery of the same job waits it out rather than running beside
    /// it. After a crash the job runs again once the claim lapses, so this is also
    /// the longest a crashed replica's job waits for a restarted one. Read from
    /// `NESTRS_REDIS__WORKER__LEASE_SECS`, at least 1; defaults to 30s.
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
        let config = Self {
            shutdown_timeout: env
                .parse::<u64>("SHUTDOWN_TIMEOUT_SECS")?
                .map(Duration::from_secs)
                .unwrap_or(base.shutdown_timeout),
            orphan_after: ORPHAN_AFTER_FLOOR.read(env, base.orphan_after)?,
            lease: LEASE_FLOOR.read(env, base.lease)?,
            poll_interval: POLL_INTERVAL_FLOOR.read(env, base.poll_interval)?,
        };
        // apalis sweeps silent peers' jobs back onto the queue on the poll, not
        // on a clock of its own: a poll past the orphan threshold would leave a
        // crashed replica's jobs waiting past it — and one of hours, a typo
        // away, would never fetch at all while the worker says it started.
        if config.poll_interval > config.orphan_after {
            return Err(ConfigError::parse(
                env.var_name(POLL_INTERVAL_FLOOR.key),
                format!(
                    "the poll is {:?}, longer than the orphan threshold of {:?} ({}) — apalis \
                     sweeps a silent replica's jobs back onto the queue on the poll, so a longer \
                     one leaves them waiting past the threshold",
                    config.poll_interval,
                    config.orphan_after,
                    env.var_name(ORPHAN_AFTER_FLOOR.key),
                ),
            ));
        }
        Ok(config)
    }
}

/// The whole-number unit a duration's variable is written in.
#[derive(Clone, Copy)]
enum Unit {
    Seconds,
    Millis,
}

impl Unit {
    fn duration(self, count: u64) -> Duration {
        match self {
            Self::Seconds => Duration::from_secs(count),
            Self::Millis => Duration::from_millis(count),
        }
    }
}

/// The shortest value a duration setting accepts, and what refuses a shorter
/// one — the same whichever side set it, since a value below it breaks the
/// worker the same way from code as from the environment.
struct Floor {
    /// The variable's key in the namespace.
    key: &'static str,
    /// The field the value is pinned through in code.
    field: &'static str,
    unit: Unit,
    /// The floor, in `unit`s.
    least: u64,
    /// Why nothing shorter holds.
    why: &'static str,
}

impl Floor {
    /// The setting's value — the variable's when it is set, `base` when it is
    /// not — refused below the floor, never clamped in silence: under the
    /// spelling that supplied it, so a value given as a file is refused under
    /// `_FILE`, or, for a value pinned in code, under the variable that would
    /// override it, naming the field.
    fn read(&self, env: &ConfigService, base: Duration) -> Result<Duration> {
        let least = self.unit.duration(self.least);
        match env.setting(self.key)? {
            Some(setting) => {
                let value = self.unit.duration(setting.parse::<u64>()?);
                if value < least {
                    return Err(
                        setting.refuse(format!("must be at least {} — {}", self.least, self.why))
                    );
                }
                Ok(value)
            }
            None if base < least => Err(ConfigError::parse(
                env.var_name(self.key),
                format!(
                    "is not set, and `RedisWorkerConfig::{}` pinned in code is {base:?}, below the \
                     {least:?} it must be at least — {}",
                    self.field, self.why
                ),
            )),
            None => Ok(base),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_config::Namespaced;

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
        let cfg = RedisWorkerConfig::from_env(
            &ConfigService::with_vars("redis__worker", [("SHUTDOWN_TIMEOUT_SECS", "5")]),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(5));
    }

    /// The liveness settings read the env over the base, and the heartbeat is
    /// several beats inside the threshold at any value accepted.
    #[test]
    fn the_orphan_threshold_and_the_lease_read_the_env_and_the_heartbeat_follows() {
        let cfg = RedisWorkerConfig::from_env(
            &ConfigService::with_vars(
                "redis__worker",
                [("ORPHAN_AFTER_SECS", "60"), ("LEASE_SECS", "4")],
            ),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.orphan_after, Duration::from_secs(60));
        assert_eq!(cfg.lease, Duration::from_secs(4));
        assert_eq!(cfg.heartbeat(), Duration::from_secs(6));

        let floor = RedisWorkerConfig {
            orphan_after: Duration::from_secs(MIN_ORPHAN_AFTER_SECS),
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
        let cfg = RedisWorkerConfig::from_env(
            &ConfigService::with_vars("redis__worker", [("POLL_INTERVAL_MS", "25")]),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.poll_interval, Duration::from_millis(25));
    }

    /// A threshold that would read one slow answer as a death, a lease with no
    /// time to renew in, and a poll that would spend Redis on nothing, are
    /// refused naming the variable — never clamped in silence.
    #[test]
    fn a_threshold_a_lease_or_a_poll_too_short_to_hold_is_refused_naming_the_variable() {
        for (key, value) in [
            ("ORPHAN_AFTER_SECS", "4"),
            ("LEASE_SECS", "0"),
            ("POLL_INTERVAL_MS", "9"),
        ] {
            let refused = RedisWorkerConfig::from_env(
                &ConfigService::with_vars("redis__worker", [(key, value)]),
                Default::default(),
            )
            .expect_err("refused")
            .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("redis__worker", key)),
                "{refused}"
            );
            assert!(refused.contains("must be at least"), "{refused}");
        }
    }

    /// A poll longer than the orphan threshold — the sweep that hands a crashed
    /// replica's jobs over runs on it — is refused naming both variables,
    /// pinned or from the environment; one equal to it is accepted.
    #[test]
    fn a_poll_longer_than_the_orphan_threshold_is_refused_naming_both() {
        let from_env = RedisWorkerConfig::from_env(
            &ConfigService::with_vars(
                "redis__worker",
                [("ORPHAN_AFTER_SECS", "60"), ("POLL_INTERVAL_MS", "60001")],
            ),
            Default::default(),
        )
        .expect_err("refused")
        .to_string();
        let pinned = RedisWorkerConfig::from_env(
            &ConfigService::with_vars("redis__worker", Vec::<(&str, &str)>::new()),
            RedisWorkerConfig {
                poll_interval: Duration::from_secs(301),
                ..Default::default()
            },
        )
        .expect_err("refused")
        .to_string();
        for refused in [from_env, pinned] {
            for key in ["POLL_INTERVAL_MS", "ORPHAN_AFTER_SECS"] {
                assert!(
                    refused.contains(&nest_rs_config::var_name("redis__worker", key)),
                    "{refused}"
                );
            }
            assert!(
                refused.contains("longer than the orphan threshold"),
                "{refused}"
            );
        }

        let equal = RedisWorkerConfig::from_env(
            &ConfigService::with_vars(
                "redis__worker",
                [("ORPHAN_AFTER_SECS", "60"), ("POLL_INTERVAL_MS", "60000")],
            ),
            Default::default(),
        )
        .expect("a poll as long as the threshold is accepted");
        assert_eq!(equal.poll_interval, equal.orphan_after);
    }

    /// A floor holds for a value pinned in code as it does for the
    /// environment's: the refusal names the field and the variable that would
    /// override it, and the floor itself is accepted.
    #[test]
    fn a_value_pinned_below_its_floor_is_refused_naming_the_field_and_the_variable() {
        for (pinned, field, key) in [
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
            let refused = RedisWorkerConfig::from_env(
                &ConfigService::with_vars("redis__worker", Vec::<(&str, &str)>::new()),
                pinned,
            )
            .expect_err("refused")
            .to_string();
            assert!(
                refused.contains(&format!("`RedisWorkerConfig::{field}` pinned in code")),
                "{refused}"
            );
            assert!(
                refused.contains(&nest_rs_config::var_name("redis__worker", key)),
                "{refused}"
            );
        }

        let floor = RedisWorkerConfig {
            poll_interval: Duration::from_millis(10),
            orphan_after: Duration::from_secs(MIN_ORPHAN_AFTER_SECS),
            lease: Duration::from_secs(1),
            ..Default::default()
        };
        let accepted = RedisWorkerConfig::from_env(
            &ConfigService::with_vars("redis__worker", Vec::<(&str, &str)>::new()),
            floor.clone(),
        )
        .expect("the floor itself is accepted");
        assert_eq!(accepted.poll_interval, floor.poll_interval);
    }
}
