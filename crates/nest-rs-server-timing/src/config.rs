//! [`ServerTimingConfig`] — whether responses carry `Server-Timing`, from
//! `<PREFIX>_SERVER_TIMING__*` or pinned through
//! [`ServerTimingModule::for_root`](crate::ServerTimingModule::for_root).

use nest_rs_config::{Environment, Surface, config};

/// Whether responses carry `Server-Timing`, settable via
/// `<PREFIX>_SERVER_TIMING__ENABLED` or pinned through
/// [`ServerTimingModule::for_root`](crate::ServerTimingModule::for_root).
#[config(namespace = "server_timing")]
#[derive(Clone, Debug, Default)]
pub struct ServerTimingConfig {
    /// Stamp each response with the time its backends took.
    ///
    /// The header reaches every client, so `Default` leaves it to the profile:
    /// on under a declared development or test profile alone. Opening it
    /// outside one, [`Surface::Open`] or `true` from the environment, is
    /// honoured and logged at `warn`.
    pub enabled: Surface,
}

impl ServerTimingConfig {
    /// One `warn` when the header is on outside a development or test profile:
    /// honoured, since the deployment chose it, and said, since every client
    /// then reads the backends' timings.
    pub(crate) fn warn_when_open(&self) {
        let environment = Environment::from_env();
        if environment.is_deployed() && self.enabled.is_open() {
            tracing::warn!(
                target: crate::TARGET,
                environment = environment.as_str(),
                "Server-Timing is enabled outside a dev profile",
            );
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use nest_rs_config::{Config, ConfigService};

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
    fn the_struct_default_follows_the_profile() {
        for (profile, enabled) in [
            ("development", true),
            ("test", true),
            ("staging", false),
            ("production", false),
            ("", false),
        ] {
            assert_eq!(
                in_profile(profile, || ServerTimingConfig::default().enabled.is_open()),
                enabled,
                "{profile:?}",
            );
        }
    }

    #[test]
    fn enabled_reads_boolean_spellings() {
        let off = ConfigService::with_vars("server_timing", [("ENABLED", "off")]);
        let cfg = ServerTimingConfig::from_env(
            &off,
            ServerTimingConfig {
                enabled: Surface::Open,
            },
        )
        .expect("ok");
        assert_eq!(
            cfg.enabled,
            Surface::Closed,
            "`off` overrides a pinned opening"
        );

        let on = ConfigService::with_vars("server_timing", [("ENABLED", "1")]);
        let cfg = ServerTimingConfig::from_env(&on, ServerTimingConfig::default()).expect("ok");
        assert_eq!(cfg.enabled, Surface::Open);
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
            assert!(cfg.enabled.is_open(), "the deployment's choice stands");
            cfg.warn_when_open();

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
            assert!(cfg.enabled.is_open());
            cfg.warn_when_open();
            logs.expect_none(crate::TARGET, ENABLED_OUTSIDE_DEV);
        });
    }

    #[test]
    fn off_outside_a_dev_profile_is_silent() {
        in_profile("production", || {
            let logs = nest_rs_testing::LogCapture::install();
            ServerTimingConfig::from_env(
                &ConfigService::with_vars("server_timing", []),
                ServerTimingConfig::default(),
            )
            .expect("ok")
            .warn_when_open();
            logs.expect_none(crate::TARGET, ENABLED_OUTSIDE_DEV);
        });
    }
}
