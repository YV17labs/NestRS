//! [`RedisConfig`] — the connection's `#[config]`: how to reach Redis, under
//! `<PREFIX>_REDIS__*`.

use std::time::Duration;

use nest_rs_config::{
    Bound, ClientTls, Config, ConfigError, ConfigService, DurationBounds, Environment, Floor,
    Result, config,
};

/// The Redis a declared development or test profile's [`Config::defaults`]
/// dials, never `Default`'s.
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
///
/// `Default` is the production-safe value: no URL, so the boot refuses an
/// unset `<PREFIX>_REDIS__URL` by name. A declared development or test
/// profile's [`Config::defaults`] adds the plaintext loopback
/// `redis://127.0.0.1/`; a config pinned over `..Default::default()` gets it in
/// no profile, and one pinned over `..RedisConfig::defaults()` gets the
/// profile's.
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
    /// queue binding (feature `queue`) a lease the port's renewal cannot fit
    /// in, by the queue port's `lease_fits_renewal`.
    pub connect_timeout: Duration,
    /// What a `rediss://` URL trusts and presents: nothing set trusts the
    /// system's authorities and presents no certificate. Read from
    /// `<PREFIX>_REDIS__TLS_CA_CERT`, `<PREFIX>_REDIS__TLS_CERT` and
    /// `<PREFIX>_REDIS__TLS_KEY`, each with its `_FILE` spelling — see
    /// [`ClientTls`]. A certificate is presented to a Redis that requires one,
    /// its default once TLS is on (`tls-auth-clients yes`).
    pub tls: ClientTls,
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
            url: String::new(),
            connect_timeout: Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
            tls: ClientTls::default(),
        }
    }
}

impl Config for RedisConfig {
    /// In a declared development or test profile, the loopback over
    /// `Default`. Here rather than in `from_env`, so it never rewrites a
    /// pinned struct.
    fn defaults() -> Self {
        let safe = Self::default();
        if Environment::from_env().is_deployed() {
            return safe;
        }
        Self {
            url: DEFAULT_URL.to_owned(),
            ..safe
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let connect_timeout = CONNECT_TIMEOUT.read(env, base.connect_timeout)?.value;
        Ok(Self {
            url: resolve_url(env, base.url)?,
            connect_timeout,
            tls: ClientTls::from_env(env, base.tls)?,
        })
    }
}

/// The deployment's URL, else the base's — a pin, or the profile's loopback —
/// refusing a blank one under the spelling that set it, or the variable of the
/// instance `env` reads when neither holds one.
fn resolve_url(env: &ConfigService, base: String) -> Result<String> {
    match env.setting("URL")? {
        Some(setting) if setting.value.trim().is_empty() => {
            Err(setting.refuse("is blank — it must hold the Redis URL"))
        }
        Some(setting) => Ok(setting.value),
        None if base.trim().is_empty() => Err(ConfigError::unset(
            env.namespace(),
            "URL",
            "`RedisConfig::defaults()` holds the loopback in a declared development or test \
             profile alone, `RedisConfig::default()` never, and a config pinned in code reads \
             the URL from the deployment, not `.env`",
        )),
        None => Ok(base),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::Namespaced;

    use super::*;

    #[test]
    fn the_struct_default_holds_no_url() {
        assert!(
            RedisConfig::default().url.is_empty(),
            "the loopback is a development convenience, opened by the profile alone"
        );
    }

    /// `profile` declared on the process for `read`, `""` declaring none.
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn in_profile<T>(profile: &str, read: impl FnOnce() -> T) -> T {
        let mut out = None;
        figment::Jail::expect_with(|jail| {
            jail.set_env(Environment::var_name(), profile);
            out = Some(read());
            Ok(())
        });
        out.expect("the jailed read ran")
    }

    #[test]
    fn the_loopback_comes_with_a_declared_development_or_test_profile_alone() {
        for (profile, url) in [
            ("development", DEFAULT_URL),
            ("test", DEFAULT_URL),
            ("staging", ""),
            ("production", ""),
            ("", ""),
        ] {
            let d = in_profile(profile, <RedisConfig as Config>::defaults);
            assert_eq!(d.url, url, "{profile:?}");
            assert_eq!(
                d.connect_timeout,
                RedisConfig::default().connect_timeout,
                "{profile:?}"
            );
        }
    }

    #[test]
    fn a_pin_over_the_struct_default_without_a_url_is_refused_in_every_profile() {
        for (profile, namespace) in ["development", "test", "staging", "production", ""]
            .into_iter()
            .flat_map(|profile| [(profile, "redis"), (profile, "redis__cache")])
        {
            let refused = in_profile(profile, || {
                RedisConfig::from_env(
                    &ConfigService::with_vars(namespace, []),
                    RedisConfig {
                        connect_timeout: Duration::from_secs(7),
                        ..RedisConfig::default()
                    },
                )
            })
            .expect_err("a pin over `Default` holds no URL to dial")
            .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name(namespace, "URL")),
                "{profile:?}, {namespace}: {refused}"
            );
            assert!(
                refused.contains(
                    "a config pinned in code reads the URL from the deployment, not `.env`"
                ),
                "a `.env` setting the URL is no answer to a pin: {profile:?}: {refused}"
            );
        }
    }

    #[test]
    fn a_pin_over_the_profiles_defaults_dials_the_loopback_in_development() {
        let cfg = in_profile("development", || {
            RedisConfig::from_env(
                &ConfigService::with_vars("redis", []),
                RedisConfig {
                    connect_timeout: Duration::from_secs(7),
                    ..RedisConfig::defaults()
                },
            )
        })
        .expect("the profile's loopback");
        assert_eq!(cfg.url, DEFAULT_URL);
    }

    #[test]
    fn an_undeclared_environment_requires_the_url() {
        for namespace in ["redis", "redis__cache"] {
            let refused = in_profile("", || {
                RedisConfig::from_env(
                    &ConfigService::with_vars(namespace, []),
                    RedisConfig::defaults(),
                )
            })
            .expect_err("a process that declares no profile runs as production")
            .to_string();
            assert!(
                refused.contains(&format!(
                    "{}: must be set, inline or through {} — ",
                    nest_rs_config::var_name(namespace, "URL"),
                    nest_rs_config::var_name(namespace, "URL_FILE"),
                )),
                "the refusal names both spellings of the instance's variable: {refused}"
            );
        }
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
    fn a_blank_url_is_refused_under_the_spelling_that_set_it() {
        let refused = RedisConfig::from_env(
            &ConfigService::with_vars("redis", [("URL", "  ")]),
            RedisConfig {
                url: "redis://pinned:6379/".into(),
                ..RedisConfig::default()
            },
        )
        .expect_err("a blank URL dials nothing")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("redis", "URL"))
                && refused.contains("is blank"),
            "{refused}"
        );
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
    fn a_set_url_is_taken_in_every_profile() {
        for profile in ["development", "test", "staging", "production", ""] {
            let cfg = in_profile(profile, || {
                RedisConfig::from_env(
                    &ConfigService::with_vars("redis", [("URL", "redis://redis:6379/1")]),
                    RedisConfig::defaults(),
                )
            })
            .expect("set ⇒ ok");
            assert_eq!(cfg.url, "redis://redis:6379/1", "{profile:?}");
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
