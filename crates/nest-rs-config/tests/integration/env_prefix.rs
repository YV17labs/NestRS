//! The custom-prefix contract, exercised end to end.
//!
//! This whole binary runs under `env_prefix!("ACME")` (declared in `main.rs`),
//! which is the only way to test it honestly: the prefix is a link-time
//! property of the binary, so a per-test override would prove something the
//! framework does not offer. Every assertion below therefore reads a variable
//! an `ACME` deployment would actually export, and **no `NESTRS_*` name may
//! resolve anything here** — a half-applied rename is exactly the failure this
//! suite exists to catch.

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
fn the_declared_prefix_replaces_nestrs_everywhere() {
    assert_eq!(EnvPrefix::current(), "ACME");
    assert_eq!(EnvPrefix::var("LOG"), "ACME_LOG");
    assert_eq!(Environment::var_name(), "ACME_ENV");
    assert_eq!(var_name("database", "URL"), "ACME_DATABASE__URL");
    assert_eq!(
        ConfigService::for_namespace(WidgetConfig::NAMESPACE).var_name("PORT"),
        "ACME_WIDGET__PORT",
    );
}

/// The composition: the documented wiring booted, and what the caller gets back
/// asserted. Compiling proves the name is *built* from the prefix; only a boot
/// proves the factory `ConfigModule` queues reads the same name.
// Not `#[tokio::test]`: `figment::Jail` is sync and owns the scope, so the
// runtime is built inside it rather than around it.
#[test]
#[allow(clippy::result_large_err)] // figment::Jail's fixed closure signature
fn a_booted_app_resolves_its_config_from_the_declared_prefix() {
    figment::Jail::expect_with(|jail| {
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
            "a NESTRS_-prefixed variable must be inert once the app declares its own prefix",
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
