//! [`RedisConfig`] — the connection's `#[config]`: how to reach Redis, under
//! `<PREFIX>_REDIS__*`.

use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, DurationBounds, Environment, Floor, Namespaced,
    Result, config,
};

use crate::RedisTls;

const DEFAULT_URL: &str = "redis://127.0.0.1/";

/// Default boot budget for reaching Redis: 10s.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// The connect budget's range, the variable that sets it, and why. The ceiling
/// also keeps the socket's liveness under what the kernel accepts, which
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
    /// The Redis connection URL, whose scheme declares the topology:
    /// `redis://valkey:6379/2` one server;
    /// `redis-sentinel://s1:26379/2?node=s2:26379&sentinelServiceName=orders`
    /// the primary the sentinels name (`sentinelUsername` and
    /// `sentinelPassword` for sentinels holding users); and
    /// `redis-cluster://n1:6379/2?node=n2:6379` a Cluster. Each `rediss…://`
    /// form connects over TLS.
    pub url: String,
    /// How long boot may spend reaching Redis before failing with a named
    /// error, and afterwards the most any command a caller waits on may take
    /// from end to end — past it the command fails as a timeout. Read from
    /// `<PREFIX>_REDIS__CONNECT_TIMEOUT_SECS`, whole seconds from 1 to 3600 —
    /// refused outside, and in code anything above zero up to an hour; defaults
    /// to 10s. The boot refuses a budget at or past a net waiting on the
    /// connection — each binding declares its port's, as does the
    /// authentication guard when its strategy injects the connection — and the
    /// queue binding a lease the port's renewal cannot fit in
    /// ([`lease_fits_renewal`](nest_rs_queue::lease_fits_renewal)).
    pub connect_timeout: Duration,
    /// What a `rediss://` URL trusts and presents: nothing set trusts the
    /// system's authorities and presents no certificate. Read from `<PREFIX>_REDIS__TLS_*` — see
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
    /// The *unpinned* baseline drops the loopback URL outside dev/test, so an
    /// unset `<PREFIX>_REDIS__URL` fails boot naming the variable; here rather
    /// than in `from_env`, so it never overrides a pinned value.
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
/// dev/test; in staging/production it aborts boot.
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
