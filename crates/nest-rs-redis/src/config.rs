//! [`RedisConfig`] — the connection's `#[config]`: how to reach Redis.
//!
//! Namespace `redis`, read off the path like every other config's. The
//! connection is the crate's own subject — every binding folder (`queue/`,
//! `worker/`, `throttler/`) shares it — so it lives at the crate root under the
//! crate's word, `NESTRS_REDIS__*`, and the operator configures the resource
//! they provisioned rather than the capability that happened to ask first.

use std::time::Duration;

use nest_rs_config::{Config, ConfigError, ConfigService, Environment, Namespaced, Result, config};

const DEFAULT_URL: &str = "redis://127.0.0.1/";

/// Default boot budget for reaching Redis: 10s — long enough to ride out a
/// cold DNS lookup or a sidecar still starting, short enough that a
/// misconfigured URL fails the container's startup probe instead of parking it.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Redis settings, settable via `NESTRS_REDIS__*` or pinned through
/// [`RedisModule::for_root`](crate::RedisModule::for_root). The URL is redacted
/// in `Debug` output — it may embed credentials.
#[config(namespace = "redis")]
#[derive(Clone)]
pub struct RedisConfig {
    /// The Redis connection URL (e.g. `redis://127.0.0.1/`).
    pub url: String,
    /// How long boot may spend reaching Redis before failing with a named
    /// error. The client retries an unreachable endpoint indefinitely on its
    /// own, so without a budget a wrong URL parks the process forever with an
    /// empty log — never healthy, never crashed. Read from
    /// `NESTRS_REDIS__CONNECT_TIMEOUT_SECS`; defaults to 10s.
    pub connect_timeout: Duration,
}

impl std::fmt::Debug for RedisConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisConfig")
            .field("url", &"<redacted>")
            .field("connect_timeout", &self.connect_timeout)
            .finish()
    }
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_string(),
            connect_timeout: Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
        }
    }
}

impl Config for RedisConfig {
    /// The loopback URL is a dev convenience, so the *unpinned* baseline drops it
    /// outside dev/test: an unset `NESTRS_REDIS__URL` then fails boot naming the
    /// variable instead of silently pointing every Redis binding at a
    /// non-existent local Redis (REDIS-Q1). It lives here rather than in
    /// `from_env` so it applies only where it is a default, never over a pinned
    /// value.
    fn defaults() -> Self {
        let d = Self::default();
        if matches!(
            Environment::from_env(),
            Environment::Production | Environment::Staging
        ) {
            return Self {
                url: String::new(),
                ..d
            };
        }
        d
    }

    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        // A zero budget would restore the unbounded hang this knob exists to
        // prevent, so it is rejected rather than silently normalized.
        let connect_timeout = match env.parse::<u64>("CONNECT_TIMEOUT_SECS")? {
            Some(0) => {
                return Err(ConfigError::parse(
                    env.var_name("CONNECT_TIMEOUT_SECS"),
                    "must be at least 1 second — a zero budget cannot bound the connect",
                ));
            }
            Some(secs) => Duration::from_secs(secs),
            None => base.connect_timeout,
        };
        Ok(Self {
            url: resolve_url(env.get("URL").or(Some(base.url)), Environment::from_env())?,
            connect_timeout,
        })
    }
}

/// Resolve the Redis URL from the raw `NESTRS_REDIS__URL` value and the active
/// profile. Unset or blank falls back to the loopback default **only** in
/// dev/test; in staging/production it aborts boot — a silent
/// `redis://127.0.0.1/` there points the queue and the rate limiter at a
/// non-existent local Redis, a fail-open default (REDIS-Q1). Mirrors the DB
/// posture. Pure, so the profile-dependent branch is testable without mutating
/// the process env.
fn resolve_url(raw: Option<String>, environment: Environment) -> Result<String> {
    match raw {
        Some(url) if !url.trim().is_empty() => Ok(url),
        _ => {
            if matches!(environment, Environment::Production | Environment::Staging) {
                return Err(ConfigError::parse(
                    nest_rs_config::var_name(RedisConfig::NAMESPACE, "URL"),
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
        assert_eq!(RedisConfig::default().url, "redis://127.0.0.1/");
    }

    #[test]
    fn the_namespace_is_the_crate_word() {
        // The connection is the crate's subject, shared by every binding — so
        // the operator sees the resource they provisioned, not the capability
        // that first asked for it.
        assert_eq!(RedisConfig::NAMESPACE, "redis");
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        let pinned = RedisConfig {
            url: "redis://pinned:6379/".into(),
            connect_timeout: Duration::from_secs(7),
        };
        let cfg = RedisConfig::from_env(
            &ConfigService::with_vars("redis", [("URL", "redis://from-env:6379/")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(
            cfg.url, "redis://from-env:6379/",
            "the env outranks the pin"
        );
        assert_eq!(
            cfg.connect_timeout,
            Duration::from_secs(7),
            "and the untouched pin survives",
        );
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
                err.to_string()
                    .contains(&nest_rs_config::var_name("redis", "URL")),
                "the error names the variable: {err}",
            );
            assert!(
                resolve_url(Some(String::new()), env).is_err(),
                "blank also aborts"
            );
        }
    }

    // C6: the connect budget is the knob that turns an unreachable backend from
    // a silent forever-hang into a named boot failure — so it must be
    // configurable, and a zero must not quietly restore the hang.
    #[test]
    fn connect_timeout_defaults_to_10s_and_reads_the_env() {
        assert_eq!(
            RedisConfig::default().connect_timeout,
            Duration::from_secs(10)
        );

        let cfg = RedisConfig::from_env(
            &ConfigService::with_vars(
                "redis",
                [("URL", "redis://redis:6379"), ("CONNECT_TIMEOUT_SECS", "3")],
            ),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.connect_timeout, Duration::from_secs(3));
    }

    #[test]
    fn connect_timeout_of_zero_is_rejected_by_name() {
        let err = RedisConfig::from_env(
            &ConfigService::with_vars(
                "redis",
                [("URL", "redis://redis:6379"), ("CONNECT_TIMEOUT_SECS", "0")],
            ),
            Default::default(),
        )
        .expect_err("zero must abort boot");
        assert!(
            err.to_string().contains("CONNECT_TIMEOUT_SECS"),
            "the error names the variable: {err}",
        );
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
        let cfg = RedisConfig::from_env(
            &ConfigService::with_vars("redis", [("URL", "redis://redis.staging:6379/2")]),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.url, "redis://redis.staging:6379/2");
    }
}
