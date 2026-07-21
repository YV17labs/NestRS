use std::time::Duration;

use nest_rs_config::{Config, ConfigError, ConfigService, Environment, Result, config};
use validator::Validate;

const DEFAULT_URL: &str = "redis://127.0.0.1/";

/// Default drain window on shutdown: 30s — comfortably under a typical
/// Kubernetes `terminationGracePeriodSeconds` (30s) so the worker drains
/// cleanly before SIGKILL rather than being force-killed mid-job.
const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

/// Redis connection settings for the queue, settable via `NESTRS_QUEUE__*` or
/// pinned through [`QueueModule::for_root`](crate::QueueModule::for_root). The
/// URL is redacted in `Debug` output — it may embed credentials.
#[config(namespace = "queue")]
#[derive(Clone, Validate)]
pub struct QueueConfig {
    /// The Redis connection URL (e.g. `redis://127.0.0.1/`).
    pub url: String,
    /// How long the worker waits for in-flight jobs to finish after a shutdown
    /// signal before returning anyway. Bounds a hung `#[process]` so SIGTERM
    /// can't block forever until the orchestrator SIGKILLs the pod (losing every
    /// other in-flight job's drain — QUEUE-I5). Read from
    /// `NESTRS_QUEUE__SHUTDOWN_TIMEOUT_SECS`; defaults to 30s.
    pub shutdown_timeout: Duration,
}

impl std::fmt::Debug for QueueConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueConfig")
            .field("url", &"<redacted>")
            .field("shutdown_timeout", &self.shutdown_timeout)
            .finish()
    }
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_string(),
            shutdown_timeout: Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECS),
        }
    }
}

impl Config for QueueConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        let shutdown_timeout = env
            .parse::<u64>("SHUTDOWN_TIMEOUT_SECS")?
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECS));
        Ok(Self {
            url: resolve_url(env.get("URL"), Environment::from_env())?,
            shutdown_timeout,
        })
    }
}

/// Resolve the queue URL from the raw `NESTRS_QUEUE__URL` value and the active
/// profile. Unset or blank falls back to the loopback default **only** in
/// dev/test; in staging/production it aborts boot — a silent
/// `redis://127.0.0.1/` there points the queue at a non-existent local Redis, a
/// fail-open default (REDIS-Q1). Mirrors the DB posture. Pure, so the
/// profile-dependent branch is testable without mutating the process env.
fn resolve_url(raw: Option<String>, environment: Environment) -> Result<String> {
    match raw {
        Some(url) if !url.trim().is_empty() => Ok(url),
        _ => {
            if matches!(environment, Environment::Production | Environment::Staging) {
                return Err(ConfigError::parse(
                    "NESTRS_QUEUE__URL",
                    format!(
                        "must be set in the `{}` environment (no localhost fallback outside dev/test)",
                        environment.as_str()
                    ),
                ));
            }
            Ok(DEFAULT_URL.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_url_targets_local_loopback_redis() {
        assert_eq!(QueueConfig::default().url, "redis://127.0.0.1/");
    }

    #[test]
    fn resolve_url_uses_loopback_default_in_dev_and_test() {
        for env in [Environment::Development, Environment::Test] {
            assert_eq!(
                resolve_url(None, env).expect("dev/test defaults"),
                DEFAULT_URL
            );
            assert_eq!(
                resolve_url(Some("  ".into()), env).expect("blank ⇒ default in dev/test"),
                DEFAULT_URL
            );
        }
    }

    #[test]
    fn resolve_url_aborts_when_unset_in_staging_or_production() {
        // REDIS-Q1: no silent localhost fallback outside dev/test.
        for env in [Environment::Staging, Environment::Production] {
            let err = resolve_url(None, env).expect_err("must abort");
            assert!(
                err.to_string().contains("NESTRS_QUEUE__URL"),
                "the error names the variable: {err}",
            );
            assert!(
                resolve_url(Some(String::new()), env).is_err(),
                "blank also aborts"
            );
        }
    }

    #[test]
    fn shutdown_timeout_defaults_to_30s_and_reads_the_env() {
        // QUEUE-I5: the drain window is configurable and defaults to a
        // K8s-friendly 30s.
        let d = QueueConfig::default();
        assert_eq!(d.shutdown_timeout, Duration::from_secs(30));

        let cfg = QueueConfig::from_env(&ConfigService::with_vars(
            "queue",
            [
                ("NESTRS_QUEUE__URL", "redis://redis:6379"),
                ("NESTRS_QUEUE__SHUTDOWN_TIMEOUT_SECS", "5"),
            ],
        ))
        .expect("ok");
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(5));
    }

    #[test]
    fn resolve_url_accepts_a_set_url_in_every_profile() {
        for env in [
            Environment::Development,
            Environment::Test,
            Environment::Staging,
            Environment::Production,
        ] {
            let url = resolve_url(Some("redis://redis:6379/1".into()), env).expect("set ⇒ ok");
            assert_eq!(url, "redis://redis:6379/1");
        }
    }

    #[test]
    fn from_env_picks_up_a_custom_url() {
        let cfg = QueueConfig::from_env(&ConfigService::with_vars(
            "queue",
            [("NESTRS_QUEUE__URL", "redis://redis.staging:6379/2")],
        ))
        .expect("ok");
        assert_eq!(cfg.url, "redis://redis.staging:6379/2");
    }
}
