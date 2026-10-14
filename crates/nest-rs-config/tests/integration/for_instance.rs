//! Covers `src/for_instance.rs` — a module opened once per instance reads one
//! namespace per instance, and pins each instance's config apart.

use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_config::{
    Config, ConfigModule, ConfigService, ConfigSetup, ForInstance, Result, config, var_name,
};
use nest_rs_core::{
    App, Collecting, ContainerBuilder, Instance, Instanced, Module, Registering, Unnamed, instance,
    module,
};

#[config(namespace = "shelf")]
#[derive(Clone, Debug, Default)]
struct ShelfConfig {
    #[validate(range(min = 1))]
    slots: Option<u16>,
}

impl Config for ShelfConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            slots: env.parse("SLOTS")?.or(base.slots),
        })
    }
}

/// A module opened once per instance, written by hand as a binding is.
struct ShelfModule<I = Unnamed>(PhantomData<fn() -> I>);

impl<I: Instance> Module for ShelfModule<I> {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(ForInstance::<I, ShelfConfig>::default(), builder)
    }
}

impl ShelfModule {
    fn for_root<I: Instance>(
        config: impl Into<ForInstance<I, ShelfConfig>>,
    ) -> ConfigSetup<ShelfModule<I>, ShelfConfig, I> {
        ConfigModule::setup(config)
    }
}

#[instance]
struct Cache;

fn pinned(slots: u16) -> ShelfConfig {
    ShelfConfig { slots: Some(slots) }
}

#[module(imports = [ShelfModule::for_root(None), ShelfModule::for_root((Cache, None))])]
struct BothRead;

#[module(imports = [ShelfModule::for_root(pinned(2)), ShelfModule::for_root((Cache, pinned(3)))])]
struct BothPinned;

#[module(imports = [ShelfModule::for_root((Cache, pinned(3))), ShelfModule::for_root((Cache, pinned(4)))])]
struct CachePinnedTwice;

#[module(imports = [ShelfModule::<Cache>])]
struct CacheBare;

/// Boot `M` inside a jail holding `vars`, answering the app or its refusal.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn boot<M: Module + 'static>(vars: &[(String, &str)]) -> std::result::Result<App, String> {
    let mut booted = None;
    figment::Jail::expect_with(|jail| {
        for (name, value) in vars {
            jail.set_env(name, value);
        }
        let app = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(async { App::builder().module::<M>().build().await })
            .map_err(|error| format!("{error:#}"));
        booted = Some(app);
        Ok(())
    });
    booted.expect("the jailed boot ran")
}

fn slots<T: std::borrow::Borrow<ShelfConfig>>(held: Option<Arc<T>>) -> Option<u16> {
    held.and_then(|held| (*held).borrow().slots)
}

#[test]
fn each_instance_reads_its_own_namespace_vendor_first() {
    let app = boot::<BothRead>(&[
        (var_name("shelf", "SLOTS"), "5"),
        (var_name("shelf__cache", "SLOTS"), "7"),
    ])
    .expect("both instances boot");
    let container = app.container();
    assert_eq!(slots(container.get::<ShelfConfig>()), Some(5));
    assert_eq!(
        slots(container.get::<Instanced<Cache, ShelfConfig>>()),
        Some(7)
    );
}

#[test]
fn a_typed_import_reads_its_instance_bare() {
    let app = boot::<CacheBare>(&[(var_name("shelf__cache", "SLOTS"), "9")])
        .expect("the bare instance boots");
    assert_eq!(
        slots(app.container().get::<Instanced<Cache, ShelfConfig>>()),
        Some(9)
    );
    assert!(app.container().get::<ShelfConfig>().is_none());
}

#[test]
fn a_validation_refusal_names_the_namespace_the_instance_read() {
    let refused = boot::<BothRead>(&[(var_name("shelf__cache", "SLOTS"), "0")])
        .err()
        .expect("zero slots is refused");
    assert!(
        refused.contains("configuration validation failed for 'shelf__cache'"),
        "{refused}"
    );
}

#[test]
fn one_pin_per_instance_boots_both() {
    let app = boot::<BothPinned>(&[]).expect("a pin per instance is no contest");
    let container = app.container();
    assert_eq!(slots(container.get::<ShelfConfig>()), Some(2));
    assert_eq!(
        slots(container.get::<Instanced<Cache, ShelfConfig>>()),
        Some(3)
    );
}

#[test]
fn two_pins_for_one_instance_contest() {
    let refused = boot::<CachePinnedTwice>(&[])
        .err()
        .expect("two pins of one instance fail the boot");
    assert!(refused.contains("contested declaration"), "{refused}");
    assert!(
        refused.contains(
            "Instanced<integration::for_instance::Cache, integration::for_instance::ShelfConfig>"
        ),
        "the key names its instance: {refused}"
    );
}

/// The marker's own declaration is told from a twin by its type, not its path,
/// which leaves out the function it is declared in.
#[test]
fn a_marker_declared_inside_a_function_reads_its_own_namespace() {
    #[instance]
    struct Local;

    #[module(imports = [ShelfModule::for_root((Local, None))])]
    struct LocalRead;

    let app = boot::<LocalRead>(&[(var_name("shelf__local", "SLOTS"), "4")])
        .expect("a marker is no twin of itself");
    assert_eq!(
        slots(app.container().get::<Instanced<Local, ShelfConfig>>()),
        Some(4)
    );
}
