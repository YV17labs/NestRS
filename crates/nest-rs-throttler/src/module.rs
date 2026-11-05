//! [`ThrottlerModule`] — the port's own seam: the policy
//! ([`ThrottlerConfig`], `<PREFIX>_THROTTLER__*`), the [`ThrottlerGuard`] that
//! applies it, and the in-process [`InMemoryThrottler`] as the default
//! `dyn ThrottlerStore`, which a vendor binding the app imports supersedes.

use std::sync::Arc;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{
    Collecting, ContainerBuilder, MissingDependencyError, Module, Registering, Unnamed,
};

use crate::config::ThrottlerConfig;
use crate::guard::{CountingStore, ThrottlerGuard};
use crate::store::{InMemoryThrottler, ThrottlerStore};
use crate::throttle::Throttle;

/// One sentence for the three things a guard nothing built can miss: a host
/// reaches the guard itself, a pool or a hand-listed guard what it injects.
const IMPORT_REMEDY: &str = "Import `nest_rs::throttler::ThrottlerModule` where the guard is \
                             bound, in the adapter whose host binds it or at the root whose \
                             `GlobalGuards` lists it: it builds the guard over the port's policy \
                             and store, so no module lists the guard by hand.";

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(std::any::TypeId::of::<ThrottlerGuard>, IMPORT_REMEDY)
}

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(std::any::TypeId::of::<Throttle>, IMPORT_REMEDY)
}

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(std::any::TypeId::of::<CountingStore>, IMPORT_REMEDY)
}

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(
        std::any::TypeId::of::<Arc<dyn ThrottlerStore>>,
        "Import `nest_rs::throttler::ThrottlerModule`, which binds the in-process \
         `dyn ThrottlerStore` a vendor binding supersedes.",
    )
}

/// The throttler port: policy, guard, and the in-process default store. An
/// adapter whose host binds the guard imports it bare, which reads
/// `<PREFIX>_THROTTLER__*` (default `Throttle::per_minute(60)`); a vendor binding
/// at the app's root shares the counters across instances.
pub struct ThrottlerModule;

impl ThrottlerModule {
    /// Pin `config` as the base `<PREFIX>_THROTTLER__*` overlays, per field;
    /// the bare import reads those variables over [`ThrottlerConfig`]'s
    /// defaults.
    pub fn for_root(config: ThrottlerConfig) -> ThrottlerSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import [`ThrottlerModule::for_root`] returns.
pub type ThrottlerSetup = ConfigSetup<ThrottlerModule, ThrottlerConfig>;

impl Module for ThrottlerModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature::<Unnamed, ThrottlerConfig>(builder);
        // An *ordinary* factory, so a vendor binding's declared one supersedes it
        // wherever the two fall in `imports`.
        let builder = builder.provide_factory::<Arc<dyn ThrottlerStore>, _, _>(|_| async {
            Ok(Arc::new(InMemoryThrottler::new()) as Arc<dyn ThrottlerStore>)
        });
        // A provider of its own, so a guard built by any other path fails the boot
        // naming `Throttle` rather than running 60/minute in silence.
        let builder = builder.provide_factory_after::<Throttle, (ThrottlerConfig,), _, _>(
            |container| async move {
                let config = ConfigModule::resolved::<ThrottlerConfig>(&container)?;
                Ok(resolve(&config))
            },
        );
        // After the store, whose vendor factory may wait on a connection of its own.
        builder.provide_factory_after::<
            ThrottlerGuard,
            (Arc<dyn ThrottlerStore>, Throttle, ThrottlerConfig),
            _,
            _,
        >(|container| async move {
            let default = container.get::<Throttle>().ok_or(MissingDependencyError::new(
                "ThrottlerModule",
                "ThrottlerGuard",
                "Throttle",
            ))?;
            let store = container.get_dyn::<dyn ThrottlerStore>().ok_or_else(|| {
                anyhow::anyhow!(
                    "ThrottlerGuard needs a `dyn ThrottlerStore` binding — \
                     `ThrottlerModule` binds the in-process default"
                )
            })?;
            let config = ConfigModule::resolved::<ThrottlerConfig>(&container)?;
            let pseudonym_key = config
                .pseudonym_key
                .as_ref()
                .map(|key| key.expose_secret().as_str());
            ThrottlerGuard::new(store, *default, pseudonym_key)
        })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

/// Resolve a [`ThrottlerConfig`] into the default [`Throttle`] the guard applies
/// to routes that pin none.
fn resolve(config: &ThrottlerConfig) -> Throttle {
    Throttle::new(config.limit, config.window)
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, injectable, module};

    use super::*;
    use crate::throttle::DEFAULT_THROTTLE;

    #[expect(
        dead_code,
        reason = "the guard is declared for the container to resolve, never read"
    )]
    #[injectable]
    struct BindsTheGuard {
        #[inject]
        guard: Arc<ThrottlerGuard>,
    }

    #[module(providers = [BindsTheGuard])]
    struct AdapterWithoutThePort;

    #[tokio::test]
    async fn a_host_reaching_the_guard_with_no_port_imported_is_told_the_import() {
        let Err(refused) = App::builder()
            .module::<AdapterWithoutThePort>()
            .build()
            .await
        else {
            panic!("a guard nothing builds must not boot");
        };
        let missing = refused
            .downcast_ref::<MissingDependencyError>()
            .unwrap_or_else(|| panic!("a named unmet dependency, got: {refused:#}"));
        assert!(
            missing
                .remedy
                .is_some_and(|remedy| remedy.contains("`nest_rs::throttler::ThrottlerModule`")),
            "{missing}"
        );
    }

    #[test]
    fn the_default_config_is_the_ports_default_throttle() {
        let t = resolve(&ThrottlerConfig {
            limit: 5,
            ..ThrottlerConfig::default()
        });
        assert_eq!(t.limit(), 5);
        assert_eq!(t.window(), DEFAULT_THROTTLE.window());
        let default = resolve(&ThrottlerConfig::default());
        assert_eq!(
            (default.limit(), default.window()),
            (DEFAULT_THROTTLE.limit(), DEFAULT_THROTTLE.window())
        );
    }
}
