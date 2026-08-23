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

/// Consumer settings, settable via `NESTRS_REDIS__WORKER__*` or pinned through
/// [`RedisWorkerModule::for_root`](crate::RedisWorkerModule::for_root).
#[config(namespace = "redis__worker")]
#[derive(Clone, Debug)]
pub struct RedisWorkerConfig {
    /// How long the worker waits for in-flight jobs to finish after a shutdown
    /// signal before returning anyway. Bounds a hung `#[process]` so SIGTERM
    /// can't block forever until the orchestrator SIGKILLs the pod (losing every
    /// other in-flight job's drain — QUEUE-I5). Read from
    /// `NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS`; defaults to 30s.
    pub shutdown_timeout: Duration,
}

impl Default for RedisWorkerConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout: Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECS),
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
        })
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
}
