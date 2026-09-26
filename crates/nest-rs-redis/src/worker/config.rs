//! [`RedisWorkerConfig`] — the consumer binding's own settings. Namespace
//! `redis__worker`, read off the path like every other config's: the crate's
//! word, then the binding folder's, so `NESTRS_REDIS__WORKER__*` names the type
//! and the file that parse it.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Result, config};

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

/// How many heartbeats fit in the orphan threshold.
const HEARTBEATS_PER_THRESHOLD: u32 = 10;

/// The slowest a replica beats, whatever the threshold: apalis records a
/// heartbeat to the second, so a faster one would record nothing new.
const MIN_HEARTBEAT: Duration = Duration::from_secs(1);

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
    /// every tenth of this (at least once a second), so a live one is never
    /// taken for dead by a slow answer or two. Shorter recovers a crashed
    /// replica's jobs sooner; the delivery lease keeps a job a live replica still
    /// runs from running twice either way. Read from
    /// `NESTRS_REDIS__WORKER__ORPHAN_AFTER_SECS`, at least 5; defaults to 300s.
    pub orphan_after: Duration,
    /// How long a running attempt's claim on its job outlives its last renewal.
    /// The claim is renewed every third of this while the attempt runs, and a
    /// second delivery of the same job waits it out rather than running beside
    /// it. After a crash the job runs again once the claim lapses, so this is also
    /// the longest a crashed replica's job waits for a restarted one. Read from
    /// `NESTRS_REDIS__WORKER__LEASE_SECS`, at least 1; defaults to 30s.
    pub lease: Duration,
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
        }
    }
}

impl Config for RedisWorkerConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            shutdown_timeout: env
                .parse::<u64>("SHUTDOWN_TIMEOUT_SECS")?
                .map(Duration::from_secs)
                .unwrap_or(base.shutdown_timeout),
            orphan_after: seconds_at_least(
                env,
                "ORPHAN_AFTER_SECS",
                MIN_ORPHAN_AFTER_SECS,
                "five heartbeats of one second each — anything shorter reads a slow answer as a death",
            )?
            .unwrap_or(base.orphan_after),
            lease: seconds_at_least(
                env,
                "LEASE_SECS",
                1,
                "a lease renewed every third of it needs at least a second to renew in",
            )?
            .unwrap_or(base.lease),
        })
    }
}

/// A whole number of seconds read from `key`, refused below `floor` with `why`
/// — naming the spelling that supplied it, so a value given as a file is
/// refused under `_FILE`.
fn seconds_at_least(
    env: &ConfigService,
    key: &str,
    floor: u64,
    why: &str,
) -> Result<Option<Duration>> {
    let Some(setting) = env.setting(key)? else {
        return Ok(None);
    };
    match setting.parse::<u64>()? {
        secs if secs < floor => Err(setting.refuse(format!("must be at least {floor} — {why}"))),
        secs => Ok(Some(Duration::from_secs(secs))),
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

    /// A threshold that would read one slow answer as a death, and a lease with
    /// no time to renew in, are refused naming the variable — never clamped in
    /// silence.
    #[test]
    fn a_threshold_or_a_lease_too_short_to_hold_is_refused_naming_the_variable() {
        for (key, value) in [("ORPHAN_AFTER_SECS", "4"), ("LEASE_SECS", "0")] {
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
}
