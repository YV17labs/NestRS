//! An app reading any config registers the active environment, with no
//! import of its own, and says once when the process declared none; a test's
//! override or seed of the environment stands in for it; the default
//! instance's config resolves through its `Config::resolve`; a config that
//! fails to read fails the boot with its own refusal, however it is loaded.

use std::sync::Arc;

use nest_rs_config::{
    Config, ConfigError, ConfigModule, ConfigService, Environment, Result, config, var_name,
};
use nest_rs_core::{
    App, AppBuilder, Collecting, ContainerBuilder, FactoryFailedError, Module, Registering,
    Unnamed, injectable, module,
};
use nest_rs_testing::LogCapture;

const UNDECLARED: &str = "no environment declared; running as production";

/// Boot `KilnsRead` with `<PREFIX>_ENV` holding `declared` (`""` declares
/// none), returning the environment it registered and what the boot filed.
fn boot_declaring(declared: &str) -> (Environment, LogCapture) {
    boot_handed(declared, |app| app)
}

/// [`boot_declaring`] of the app `hand` hands values to before it boots.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn boot_handed(declared: &str, hand: fn(AppBuilder) -> AppBuilder) -> (Environment, LogCapture) {
    let mut booted = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(Environment::var_name(), declared);
        let logs = LogCapture::install();
        let app = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(async { hand(App::builder().module::<KilnsRead>()).build().await })
            .expect("the readers boot");
        let environment = *app
            .container()
            .get::<Environment>()
            .expect("the first config read registers the active environment");
        booted = Some((environment, logs));
        Ok(())
    });
    booted.expect("the jailed boot ran")
}

#[test]
fn an_undeclared_process_registers_production_and_says_so_once() {
    let (environment, logs) = boot_declaring("");
    assert_eq!(environment, Environment::Production);
    let event = logs.expect_one(nest_rs_config::TARGET, UNDECLARED);
    assert_eq!(event.level, "info");
    assert_eq!(event.field("declared").as_deref(), Some("false"));
    assert_eq!(event.field("environment").as_deref(), Some("production"));
}

#[test]
fn a_declared_environment_boots_silently() {
    for declared in ["production", "development", "test", "staging"] {
        let (environment, logs) = boot_declaring(declared);
        assert_eq!(environment.as_str(), declared);
        logs.expect_none(nest_rs_config::TARGET, UNDECLARED);
    }
}

#[test]
fn a_test_s_override_or_seed_of_the_environment_stands_in_for_the_one_a_config_load_registers() {
    let hands: [fn(AppBuilder) -> AppBuilder; 3] = [
        |app| app.override_value(Environment::Development),
        |app| app.override_arc(Arc::new(Environment::Development)),
        |app| app.provide(Environment::Development),
    ];
    for hand in hands {
        let (environment, logs) = boot_handed("", hand);
        assert_eq!(environment, Environment::Development);
        logs.expect_none(nest_rs_config::TARGET, UNDECLARED);
    }
}

#[config(namespace = "kiln", from_env = "manual")]
#[derive(Clone, Debug, Default)]
struct KilnConfig {
    heat: Option<u16>,
}

impl Config for KilnConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            heat: env.parse("HEAT")?.or(base.heat),
        })
    }

    fn resolve(pinned: Option<Self>) -> Result<Self> {
        let pinned = pinned.and_then(|pinned| pinned.heat).unwrap_or_default();
        Ok(Self {
            heat: Some(1000 + pinned),
        })
    }
}

#[module]
struct Kilns;

/// What reads the kiln's config, and so loads it.
#[injectable]
struct Kiln {
    #[inject]
    _config: Arc<KilnConfig>,
}

#[module(providers = [Kiln])]
struct KilnRead;

#[config(namespace = "kiln_vent")]
#[derive(Clone, Debug, Default)]
struct KilnVentConfig {
    open: bool,
}

/// A second reader of a second config, so two loads share one environment.
#[injectable]
struct KilnVent {
    #[inject]
    _config: Arc<KilnVentConfig>,
}

#[module(imports = [KilnRead], providers = [KilnVent])]
struct KilnsRead;

#[module(imports = [ConfigModule::setup::<Kilns, KilnConfig, Unnamed>(KilnConfig { heat: Some(7) })])]
struct KilnPinned;

fn heat<M: Module + 'static>() -> Option<u16> {
    let app = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("test runtime")
        .block_on(async { App::builder().module::<M>().build().await })
        .expect("the kiln boots");
    app.container()
        .get::<KilnConfig>()
        .and_then(|kiln| kiln.heat)
}

#[test]
fn the_boot_resolves_the_default_instance_through_its_config_s_resolve() {
    assert_eq!(heat::<KilnRead>(), Some(1000));
    assert_eq!(heat::<KilnPinned>(), Some(1007));
}

#[config(namespace = "kiln_door")]
#[derive(Clone, Debug, Default)]
struct KilnDoorConfig {
    width: Option<u16>,
}

/// A module reading the door's config on its bare import, written by hand as
/// a binding is.
struct KilnDoorModule;

impl Module for KilnDoorModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<Unnamed, KilnDoorConfig>(builder)
    }
}

#[module(imports = [KilnDoorModule])]
struct KilnDoorRead;

#[module]
struct KilnDoors;

#[module(imports = [ConfigModule::setup::<KilnDoors, KilnDoorConfig, Unnamed>(KilnDoorConfig { width: Some(3) })])]
struct KilnDoorPinned;

/// What reads the door's config, and so loads it.
#[injectable]
struct KilnDoor {
    #[inject]
    _config: Arc<KilnDoorConfig>,
}

#[module(providers = [KilnDoor])]
struct KilnDoorInjected;

/// The refusal `M`'s boot fails with while the door's width does not parse.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn refused<M: Module + 'static>() -> anyhow::Error {
    let mut refused = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(var_name("kiln_door", "WIDTH"), "wide");
        let booted = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(async { App::builder().module::<M>().build().await });
        refused = booted.err();
        Ok(())
    });
    refused.expect("a width that does not parse fails the boot")
}

#[test]
fn a_config_that_fails_to_read_fails_the_boot_with_its_own_refusal_however_it_is_loaded() {
    let variable = var_name("kiln_door", "WIDTH");
    for (loaded, refused) in [
        ("a bare import", refused::<KilnDoorRead>()),
        ("a pin", refused::<KilnDoorPinned>()),
        ("an injection", refused::<KilnDoorInjected>()),
    ] {
        assert!(
            refused
                .to_string()
                .starts_with(&format!("invalid value for {variable}: ")),
            "{loaded}: the variable leads: {refused:#}"
        );
        assert!(
            refused.downcast_ref::<FactoryFailedError>().is_none(),
            "{loaded}: no factory wraps a read: {refused:#}"
        );
        assert!(
            matches!(
                refused.downcast_ref::<ConfigError>(),
                Some(ConfigError::Parse { var, .. }) if *var == variable
            ),
            "{loaded}: the boot's error is the config's own: {refused:#}"
        );
    }
}
