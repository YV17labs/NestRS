//! [`RedisConfig`] — the connection's `#[config]`: how to reach Redis.
//!
//! Namespace `redis`, read off the path like every other config's. The
//! connection is the crate's own subject — every binding folder (`queue/`,
//! `worker/`, `throttler/`) shares it — so it lives at the crate root under the
//! crate's word, `<PREFIX>_REDIS__*`, and the operator configures the resource
//! they provisioned rather than the capability that happened to ask first.

use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, DurationBounds, Environment, Floor, Namespaced,
    Result, config,
};

use crate::RedisTls;

const DEFAULT_URL: &str = "redis://127.0.0.1/";

/// Default boot budget for reaching Redis: 10s — long enough to ride out a
/// cold DNS lookup or a sidecar still starting, short enough that a
/// misconfigured URL fails the container's startup probe instead of parking it.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// The connect budget's range, the variable that sets it, and why. The floor is
/// above zero rather than a whole second: the budget also bounds every command,
/// and a sub-second one set in code is a fail-fast choice, not a mistake.
///
/// The ceiling is an hour, and it is one knob's ceiling on purpose — the budget
/// bounds the boot's wait, every command a caller waits on, and the socket's
/// liveness, and a second budget for any one of them would be a second answer to
/// one question. Past an hour none of them is bounding anything: a Redis that
/// has not answered a command in an hour is gone rather than slow, and a boot
/// that waits longer is the parked process the budget exists to end. It also
/// keeps the liveness the budget sets under what the kernel accepts — a
/// keepalive idle past 32 767 s failed every dial with `EINVAL` — which
/// `connection.rs` asserts at compile time.
pub(crate) const CONNECT_TIMEOUT: DurationBounds = DurationBounds::secs(
    "CONNECT_TIMEOUT_SECS",
    "RedisConfig::connect_timeout",
    Floor::AboveZero(
        "the budget bounds the boot's connect and every command after it, and a zero one gives \
         up before the first attempt and fails every command at once",
    ),
    Bound {
        count: 60 * 60,
        why: "the budget bounds the boot's wait for Redis and every command a caller waits on, \
              and past an hour it bounds neither — a Redis silent that long is gone rather than \
              slow, and a boot that waits longer is a parked process",
    },
);

/// Redis settings, settable via `<PREFIX>_REDIS__*` or pinned through
/// [`RedisModule::for_root`](crate::RedisModule::for_root). The URL and the
/// private key are redacted in `Debug` output — the URL may embed credentials.
#[config(namespace = "redis")]
#[derive(Clone)]
pub struct RedisConfig {
    /// The Redis connection URL (e.g. `redis://127.0.0.1/`); `rediss://`
    /// connects over TLS.
    pub url: String,
    /// How long boot may spend reaching Redis before failing with a named
    /// error, and afterwards the most any command a caller waits on may take
    /// from end to end — past it the command fails as a timeout. The client
    /// retries an unreachable endpoint on its own, so without a budget a wrong
    /// URL parks the process with an empty log — never healthy, never crashed —
    /// and an outage holds every caller. Read from
    /// `<PREFIX>_REDIS__CONNECT_TIMEOUT_SECS`, whole seconds from 1 to 3600 —
    /// refused outside, and in code anything above zero up to an hour; defaults
    /// to 10s.
    pub connect_timeout: Duration,
    /// What a `rediss://` URL trusts and presents: nothing set trusts the
    /// authorities of Mozilla's root program compiled into the client and
    /// presents no certificate. Read from `<PREFIX>_REDIS__TLS_*` — see
    /// [`RedisTls`].
    pub tls: RedisTls,
}

impl std::fmt::Debug for RedisConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisConfig")
            .field("url", &"<redacted>")
            .field("connect_timeout", &self.connect_timeout)
            .field("tls", &self.tls)
            .finish()
    }
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_string(),
            connect_timeout: Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
            tls: RedisTls::default(),
        }
    }
}

impl Config for RedisConfig {
    /// The loopback URL is a dev convenience, so the *unpinned* baseline drops it
    /// outside dev/test: an unset `<PREFIX>_REDIS__URL` then fails boot naming the
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
        let connect_timeout = CONNECT_TIMEOUT.read(env, base.connect_timeout)?.value;
        Ok(Self {
            url: resolve_url(env.get("URL")?.or(Some(base.url)), Environment::from_env())?,
            connect_timeout,
            tls: RedisTls::from_env(env, base.tls)?,
        })
    }
}

/// Resolve the Redis URL from the raw `<PREFIX>_REDIS__URL` value and the active
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
                        "must be set, inline or through {}, in the `{}` environment (no localhost \
                         fallback outside dev/test)",
                        nest_rs_config::var_name(RedisConfig::NAMESPACE, "URL_FILE"),
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
            ..RedisConfig::default()
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

    /// A zero budget given as a file is refused under the `_FILE` spelling
    /// that supplied it.
    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test tears down its temp file best-effort"
    )]
    fn a_zero_connect_timeout_from_a_file_names_its_file_variable() {
        let path = std::env::temp_dir().join(format!(
            "nest-rs-redis-connect-timeout-{}",
            std::process::id()
        ));
        std::fs::write(&path, "0\n").expect("write the fixture");
        let refused = RedisConfig::from_env(
            &ConfigService::with_vars(
                "redis",
                [(
                    "CONNECT_TIMEOUT_SECS_FILE",
                    path.to_str().expect("a UTF-8 path"),
                )],
            ),
            RedisConfig::default(),
        );
        let _ = std::fs::remove_file(&path);
        let err = refused.expect_err("a zero budget is refused").to_string();
        assert!(
            err.contains(&format!(
                "{}:",
                nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS_FILE")
            )),
            "{err}"
        );
    }

    /// A budget pinned in code is held to the floor the variable is: a zero one
    /// resolved and then gave up before its first attempt, blaming the URL.
    #[test]
    fn a_zero_connect_timeout_pinned_in_code_is_refused_naming_the_field() {
        let err = RedisConfig::from_env(
            &ConfigService::with_vars("redis", []),
            RedisConfig {
                connect_timeout: Duration::ZERO,
                ..RedisConfig::default()
            },
        )
        .expect_err("a pinned zero budget is refused")
        .to_string();
        assert!(
            err.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"))
                && err.contains("`RedisConfig::connect_timeout` set in code is 0ns"),
            "{err}"
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
