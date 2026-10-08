//! The custom-prefix contract, exercised end to end.
//!
//! The prefix is frozen on first use, so each test owns a process and sets
//! `NESTRS_ENV_PREFIX` before anything reads a name; no `NESTRS_*` name may
//! resolve anything.

use nest_rs_config::{
    Config, ConfigModule, ConfigService, Environment, Namespaced, config, var_name,
};
use nest_rs_core::{App, EnvPrefix, module};

#[config(namespace = "widget")]
#[derive(Clone)]
struct WidgetConfig {
    port: u16,
    label: String,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            label: "default".to_owned(),
        }
    }
}

impl Config for WidgetConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            port: env.parse("PORT")?.unwrap_or(base.port),
            label: env.get("LABEL")?.unwrap_or(base.label),
        })
    }
}

#[module(imports = [ConfigModule::for_root(), ConfigModule::for_feature::<WidgetConfig>()])]
struct WidgetModule;

#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn the_declared_prefix_replaces_nestrs_everywhere() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "ACME");

        assert_eq!(EnvPrefix::current(), "ACME");
        assert_eq!(EnvPrefix::var("LOG"), "ACME_LOG");
        assert_eq!(Environment::var_name(), "ACME_ENV");
        assert_eq!(var_name("seaorm", "URL"), "ACME_SEAORM__URL");
        assert_eq!(
            ConfigService::for_namespace(WidgetConfig::NAMESPACE).var_name("PORT"),
            "ACME_WIDGET__PORT",
        );
        Ok(())
    });
}

/// Only a boot proves the factory `ConfigModule` queues reads the prefixed name.
// Not `#[tokio::test]`: `figment::Jail` is sync and owns the scope.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_booted_app_resolves_its_config_from_the_declared_prefix() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "ACME");
        jail.set_env("ACME_WIDGET__PORT", "8443");
        jail.set_env("NESTRS_WIDGET__LABEL", "from-the-old-prefix");

        let app = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(async { App::builder().module::<WidgetModule>().build().await })
            .expect("the app boots with the config feature wired");

        let config = app
            .container()
            .get::<WidgetConfig>()
            .expect("ConfigModule::for_feature registers the config for injection");

        assert_eq!(config.port, 8443, "ACME_WIDGET__PORT must reach the field");
        assert_eq!(
            config.label, "default",
            "a NESTRS_-prefixed variable must be inert once the deployment names its own prefix",
        );
        Ok(())
    });
}

/// `<PREFIX>_ENV` is read before any `ConfigService` exists.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn the_active_environment_is_read_from_the_declared_prefix() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "ACME");
        jail.set_env("ACME_ENV", "production");
        jail.set_env("NESTRS_ENV", "staging");
        assert_eq!(
            Environment::from_env(),
            Environment::Production,
            "ACME_ENV decides; NESTRS_ENV is just another variable now",
        );
        Ok(())
    });
}

/// A prefix written into the cascade aborts, through a plain config read — all
/// a `migrate` or `seed` binary does.
#[test]
#[should_panic(expected = "is `ACME` in the `.env` cascade")]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_prefix_written_into_the_cascade_aborts_without_environment_init() {
    figment::Jail::expect_with(|jail| {
        // Pinned, not inherited: a shell already exporting `ACME` would agree
        // with the file and the abort would not fire.
        jail.set_env(EnvPrefix::VAR, "FIXTURE");
        jail.create_file(
            ".env",
            &format!(
                "{}=ACME\n{}=9001\n",
                EnvPrefix::VAR,
                nest_rs_config::var_name("widget", "PORT"),
            ),
        )?;
        let _ = ConfigService::for_namespace(WidgetConfig::NAMESPACE).get("PORT");
        Ok(())
    });
}

/// And through `load_cascade`, which bypasses the memoized map.
#[test]
#[should_panic(expected = "is `ACME` in the `.env` cascade")]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_prefix_written_into_the_cascade_aborts_through_load_cascade() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "FIXTURE");
        jail.create_file(".env", &format!("{}=ACME\n", EnvPrefix::VAR))?;
        nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Development);
        Ok(())
    });
}

/// A malformed prefix aborts on the first read rather than degrading to `NESTRS`.
#[test]
#[should_panic(expected = "must start with an uppercase ASCII letter")]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_malformed_prefix_aborts_on_first_read() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "acme");
        let _ = EnvPrefix::current();
        Ok(())
    });
}
