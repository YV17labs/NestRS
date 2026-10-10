//! [`ServerTimingConfig`] — whether responses carry `Server-Timing`, from
//! `<PREFIX>_SERVER_TIMING__*` or pinned through
//! [`ServerTimingModule::for_root`](crate::ServerTimingModule::for_root).

use nest_rs_config::{Config, ConfigService, Environment, Result, config};

/// Whether responses carry `Server-Timing`, settable via
/// `<PREFIX>_SERVER_TIMING__ENABLED` or pinned through
/// [`ServerTimingModule::for_root`](crate::ServerTimingModule::for_root).
#[config(namespace = "server_timing")]
#[derive(Clone, Debug, Default)]
pub struct ServerTimingConfig {
    /// Stamp each response with the time its backends took.
    ///
    /// The header reaches every client, so `Default` keeps it off — a config
    /// pinned over `..Default::default()` stamps nothing in any profile — and
    /// only a development or test profile's [`Config::defaults`] turns it on.
    /// Enabling it outside such a profile is honoured and logged at `warn`.
    pub enabled: bool,
}

impl Config for ServerTimingConfig {
    // Not in `from_env`: overlaying it would rewrite a pinned `enabled: true`.
    fn defaults() -> Self {
        Self {
            enabled: dev_profile(Environment::from_env()),
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let environment = Environment::from_env();
        let enabled = env.flag("ENABLED", base.enabled)?;
        if enabled && !dev_profile(environment) {
            tracing::warn!(
                target: crate::TARGET,
                environment = environment.as_str(),
                "Server-Timing is enabled outside a dev profile",
            );
        }
        Ok(Self { enabled })
    }
}

fn dev_profile(environment: Environment) -> bool {
    !matches!(environment, Environment::Production | Environment::Staging)
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use super::*;

    const ENABLED_OUTSIDE_DEV: &str = "Server-Timing is enabled outside a dev profile";

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
    fn the_struct_default_stamps_nothing() {
        assert!(!ServerTimingConfig::default().enabled);
    }

    #[test]
    fn the_unpinned_default_follows_the_profile() {
        for (profile, enabled) in [
            ("development", true),
            ("test", true),
            ("staging", false),
            ("production", false),
        ] {
            assert_eq!(
                in_profile(profile, ServerTimingConfig::defaults).enabled,
                enabled,
                "{profile}",
            );
        }
    }

    #[test]
    fn a_pin_over_the_struct_default_stays_off_in_development() {
        let cfg = in_profile("development", || {
            ServerTimingConfig::from_env(
                &ConfigService::with_vars("server_timing", []),
                ServerTimingConfig::default(),
            )
        })
        .expect("ok");
        assert!(!cfg.enabled, "a pin opens the header only by saying so");
    }

    #[test]
    fn enabled_reads_boolean_spellings() {
        let off = ConfigService::with_vars("server_timing", [("ENABLED", "off")]);
        let cfg =
            ServerTimingConfig::from_env(&off, ServerTimingConfig { enabled: true }).expect("ok");
        assert!(!cfg.enabled, "`off` overrides a pinned `true`");

        let on = ConfigService::with_vars("server_timing", [("ENABLED", "1")]);
        let cfg = ServerTimingConfig::from_env(&on, ServerTimingConfig::default()).expect("ok");
        assert!(cfg.enabled);
    }

    #[test]
    fn enabled_rejects_unparseable_value_naming_the_var() {
        let service = ConfigService::with_vars("server_timing", [("ENABLED", "maybe")]);
        let err = ServerTimingConfig::from_env(&service, ServerTimingConfig::default())
            .expect_err("a non-boolean must fail, never silently default");
        assert!(
            matches!(err, nest_rs_config::ConfigError::Parse { ref var, .. } if *var == nest_rs_config::var_name("server_timing", "ENABLED")),
            "the error must name the offending variable: {err}",
        );
    }

    #[test]
    fn enabled_outside_a_dev_profile_is_honoured_and_reported() {
        in_profile("production", || {
            let logs = nest_rs_testing::LogCapture::install();
            let cfg = ServerTimingConfig::from_env(
                &ConfigService::with_vars("server_timing", [("ENABLED", "true")]),
                ServerTimingConfig::default(),
            )
            .expect("an explicit `true` is honoured, not overridden");
            assert!(cfg.enabled, "the deployment's choice stands");

            let event = logs.expect_one(crate::TARGET, ENABLED_OUTSIDE_DEV);
            assert_eq!(event.level, "warn");
            assert_eq!(event.field("environment").as_deref(), Some("production"));
        });
    }

    #[test]
    fn enabled_in_development_is_silent() {
        in_profile("development", || {
            let logs = nest_rs_testing::LogCapture::install();
            let cfg = ServerTimingConfig::from_env(
                &ConfigService::with_vars("server_timing", [("ENABLED", "true")]),
                ServerTimingConfig::default(),
            )
            .expect("ok");
            assert!(cfg.enabled);
            logs.expect_none(crate::TARGET, ENABLED_OUTSIDE_DEV);
        });
    }

    #[test]
    fn off_outside_a_dev_profile_is_silent() {
        in_profile("production", || {
            let logs = nest_rs_testing::LogCapture::install();
            let _ = ServerTimingConfig::from_env(
                &ConfigService::with_vars("server_timing", []),
                ServerTimingConfig::defaults(),
            )
            .expect("ok");
            logs.expect_none(crate::TARGET, ENABLED_OUTSIDE_DEV);
        });
    }
}
