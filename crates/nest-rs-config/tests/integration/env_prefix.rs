//! The custom-prefix contract, exercised end to end.
//!
//! The prefix is read from the process environment and frozen on first use, so
//! each test below owns a process (see the suite root) and sets
//! `NESTRS_ENV_PREFIX` before anything reads a name. Every assertion then reads
//! a variable an `ACME` deployment would actually export, and **no `NESTRS_*`
//! name may resolve anything** — a half-applied rename is exactly the failure
//! this suite exists to catch.

use nest_rs_config::{
    Config, ConfigModule, ConfigService, Environment, Namespaced, config, var_name,
};
use nest_rs_core::{App, EnvPrefix, module};

/// A feature config shaped like every other one in the framework: a namespace,
/// a `from_env` overlay, a validated default.
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
            label: env.get("LABEL").unwrap_or(base.label),
        })
    }
}

#[module(imports = [ConfigModule::for_root(), ConfigModule::for_feature::<WidgetConfig>()])]
struct WidgetModule;

#[test]
#[allow(clippy::result_large_err)] // figment::Jail's fixed closure signature
fn the_declared_prefix_replaces_nestrs_everywhere() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "ACME");

        assert_eq!(EnvPrefix::current(), "ACME");
        assert_eq!(EnvPrefix::var("LOG"), "ACME_LOG");
        assert_eq!(Environment::var_name(), "ACME_ENV");
        assert_eq!(var_name("database", "URL"), "ACME_DATABASE__URL");
        assert_eq!(
            ConfigService::for_namespace(WidgetConfig::NAMESPACE).var_name("PORT"),
            "ACME_WIDGET__PORT",
        );
        Ok(())
    });
}

/// The composition: the documented wiring booted, and what the caller gets back
/// asserted. Reading a name proves it is *built* from the prefix; only a boot
/// proves the factory `ConfigModule` queues reads the same name.
// Not `#[tokio::test]`: `figment::Jail` is sync and owns the scope, so the
// runtime is built inside it rather than around it.
#[test]
#[allow(clippy::result_large_err)]
fn a_booted_app_resolves_its_config_from_the_declared_prefix() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "ACME");
        jail.set_env("ACME_WIDGET__PORT", "8443");
        // The old name must be inert, not a second way in: a deployment that
        // still exports it has renamed nothing, and a value silently winning
        // here would hide that.
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

/// `<PREFIX>_ENV` selects the `.env` cascade, and it is read before any
/// `ConfigService` exists — the one variable a rename is most likely to miss.
#[test]
#[allow(clippy::result_large_err)]
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

/// A prefix written into the cascade renamed nothing — it could not have
/// selected the very files it was read from. That must abort, not pass.
///
/// Through a plain config read, which is all a one-shot tool does: the
/// generated `migrate` and `seed` binaries never call `Environment::init`, so a
/// guard hanging off boot would not cover them.
#[test]
#[should_panic(expected = "is `ACME` in the `.env` cascade")]
#[allow(clippy::result_large_err)]
fn a_prefix_written_into_the_cascade_aborts_without_environment_init() {
    figment::Jail::expect_with(|jail| {
        jail.create_file(".env", "NESTRS_ENV_PREFIX=ACME\nNESTRS_WIDGET__PORT=9001\n")?;
        let _ = ConfigService::for_namespace(WidgetConfig::NAMESPACE).get("PORT");
        Ok(())
    });
}

/// And through `load_cascade`, the other entry into the same files — the one
/// `nest_rs_testing::load_project_env` takes. It parses and *publishes* without
/// going through the memoized map, so a guard on that map would leave every e2e
/// harness uncovered.
#[test]
#[should_panic(expected = "is `ACME` in the `.env` cascade")]
#[allow(clippy::result_large_err)]
fn a_prefix_written_into_the_cascade_aborts_through_load_cascade() {
    figment::Jail::expect_with(|jail| {
        jail.create_file(".env", "NESTRS_ENV_PREFIX=ACME\n")?;
        nest_rs_config::load_cascade(std::path::Path::new("."), Environment::Development);
        Ok(())
    });
}

/// A malformed value resolves names no operator wrote, so it aborts on the
/// first read rather than degrading to `NESTRS` — which would be just as wrong,
/// and silent.
#[test]
#[should_panic(expected = "must start with an uppercase ASCII letter")]
#[allow(clippy::result_large_err)]
fn a_malformed_prefix_aborts_on_first_read() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "acme");
        let _ = EnvPrefix::current();
        Ok(())
    });
}
