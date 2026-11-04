//! [`RedisConfig`] — the connection's `#[config]`: how to reach Redis, under
//! `<PREFIX>_REDIS__*`.

use std::time::Duration;

use nest_rs_config::{
    Bound, ClientTls, ConfigError, ConfigService, DurationBounds, Field, Floor, Result, config,
};
use nest_rs_core::Secret;

/// Default boot budget for reaching Redis: 10s.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// The connect budget's range, and why. The ceiling also keeps the socket's
/// liveness under what the kernel accepts, which `connection.rs` asserts at
/// compile time.
const CONNECT_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::AboveZero(
        "the budget bounds the boot's connect and every command after it, and a zero one gives \
         up before the first attempt and fails every command at once",
    ),
    Bound::new(
        60 * 60,
        "the budget bounds the boot's wait for Redis and every command a caller waits on, \
         and past an hour it bounds neither — a Redis silent that long is gone rather than \
         slow, and a boot that waits longer is a parked process",
    ),
);

/// Redis settings, read by [`RedisModule`](crate::RedisModule) from
/// `<PREFIX>_REDIS__*` over a base
/// [`RedisModule::for_root`](crate::RedisModule::for_root) may pin.
///
/// No profile names a Redis: the boot refuses an unset `<PREFIX>_REDIS__URL`
/// by name in every one, and the deployment — or the profile's `.env` file,
/// as `nestrs new` writes it — names the server to dial.
#[config(namespace = "redis")]
#[derive(Clone, Debug)]
pub struct RedisConfig {
    /// The Redis connection URL, whose scheme declares the topology:
    /// `redis://valkey:6379/2` one server;
    /// `redis-sentinel://s1:26379/2?node=s2:26379&sentinelServiceName=orders`
    /// the primary the sentinels name (`sentinelUsername` and
    /// `sentinelPassword` for sentinels holding users); and
    /// `redis-cluster://n1:6379/2?node=n2:6379` a Cluster. Each `rediss…://`
    /// form connects over TLS. A [`Secret`], since it may carry a password.
    #[config(with = url)]
    pub url: Secret<String>,
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
    #[config(bounds = CONNECT_TIMEOUT)]
    pub connect_timeout: Duration,
    /// What a `rediss://` URL trusts and presents: nothing set trusts the
    /// system's authorities and presents no certificate. Read from
    /// `<PREFIX>_REDIS__TLS_CA_CERT`, `<PREFIX>_REDIS__TLS_CERT` and
    /// `<PREFIX>_REDIS__TLS_KEY`, each with its `_FILE` spelling — see
    /// [`ClientTls`]. A certificate is presented to a Redis that requires one,
    /// its default once TLS is on (`tls-auth-clients yes`).
    pub tls: ClientTls,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: Secret::default(),
            connect_timeout: Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS),
            tls: ClientTls::default(),
        }
    }
}

/// The deployment's URL, else the base's — a pin — refusing a blank one under
/// the spelling that set it, or the variable of the instance `env` reads when
/// neither holds one.
fn url(env: &ConfigService, field: Field, base: Secret<String>) -> Result<Secret<String>> {
    match env.setting(field.key())? {
        Some(setting) if setting.value.trim().is_empty() => {
            Err(setting.refuse("is blank — it must hold the Redis URL"))
        }
        Some(setting) => Ok(Secret::new(setting.value)),
        None if base.expose_secret().trim().is_empty() => Err(ConfigError::unset(
            env.namespace(),
            field.key(),
            "the URL is deployment data, which no default holds in any profile, and a config \
             pinned in code reads it from the deployment, not `.env`",
        )),
        None => Ok(base),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::{Config, Environment, Namespaced};

    use super::*;

    #[test]
    fn the_struct_default_holds_no_url() {
        assert!(
            RedisConfig::default().url.expose_secret().is_empty(),
            "an address is deployment data, never a default"
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
    fn the_default_names_no_redis() {
        assert!(
            RedisConfig::default().url.expose_secret().is_empty(),
            "an address is the profile file's, never the framework's"
        );
    }

    #[test]
    fn debug_never_prints_the_url() {
        let config = RedisConfig {
            url: "rediss://app:s3cret@valkey:6379/2".into(),
            ..RedisConfig::default()
        };
        let printed = format!("{config:?}");
        assert!(!printed.contains("s3cret"), "{printed}");
        assert!(printed.contains("url: <redacted>"), "{printed}");
    }

    #[test]
    fn an_unset_url_is_refused_in_every_profile_whether_pinned_or_not() {
        for ((profile, namespace), base) in ["development", "test", "staging", "production", ""]
            .into_iter()
            .flat_map(|profile| [(profile, "redis"), (profile, "redis__cache")])
            .flat_map(|read| {
                [
                    (read, None),
                    (
                        read,
                        Some(RedisConfig {
                            connect_timeout: Duration::from_secs(7),
                            ..RedisConfig::default()
                        }),
                    ),
                ]
            })
        {
            let pinned = base.is_some();
            let refused = in_profile(profile, || {
                RedisConfig::from_env(
                    &ConfigService::with_vars(namespace, []),
                    base.unwrap_or_default(),
                )
            })
            .expect_err("nothing names a Redis to dial")
            .to_string();
            assert!(
                refused.contains(&format!(
                    "{}: must be set, inline or through {} — ",
                    nest_rs_config::var_name(namespace, "URL"),
                    nest_rs_config::var_name(namespace, "URL_FILE"),
                )),
                "the refusal names both spellings of the instance's variable: {profile:?}, \
                 {namespace}, pinned {pinned}: {refused}"
            );
            assert!(
                refused.contains(
                    "no default holds in any profile, and a config pinned in code reads it from \
                     the deployment, not `.env`"
                ),
                "a `.env` setting the URL is no answer to a pin: {profile:?}: {refused}"
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
            cfg.url.expose_secret(),
            "redis://from-env:6379/",
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
            RedisConfig {
                url: "rediss://valkey.local".into(),
                ..RedisConfig::default()
            },
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
                url: "rediss://valkey.local".into(),
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
                    RedisConfig::default(),
                )
            })
            .expect("set ⇒ ok");
            assert_eq!(
                cfg.url.expose_secret(),
                "redis://redis:6379/1",
                "{profile:?}"
            );
        }
    }

    #[test]
    fn from_env_picks_up_a_custom_url() {
        let cfg = RedisConfig::from_env(
            &ConfigService::with_vars("redis", [("URL", "redis://redis.staging:6379/2")]),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.url.expose_secret(), "redis://redis.staging:6379/2");
    }
}
