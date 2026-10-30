//! [`ThrottlerModule`] — the port's own seam: the policy
//! ([`ThrottlerConfig`], `<PREFIX>_THROTTLER__*`), the [`ThrottlerGuard`] that
//! applies it, and the in-process [`InMemoryThrottler`] as the default
//! `dyn ThrottlerStore`, which a vendor binding imported beside it supersedes.

use std::any::TypeId;

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, MissingDependencyError};

use crate::config::ThrottlerConfig;
use crate::guard::ThrottlerGuard;
use crate::store::{InMemoryThrottler, ThrottlerStore};
use crate::throttle::Throttle;

/// The throttler port: policy, guard, and the in-process default store. Wire
/// with `ThrottlerModule::for_root(None)` (env-driven, default
/// `Throttle::per_minute(60)`); add a vendor binding beside it to share the
/// counters across instances.
pub struct ThrottlerModule;

impl ThrottlerModule {
    /// Pass `None` to load [`ThrottlerConfig`] from `<PREFIX>_THROTTLER__*`, or a
    /// [`ThrottlerConfig`] to pin as the base those variables overlay, per field.
    pub fn for_root(config: impl Into<Option<ThrottlerConfig>>) -> ThrottlerSetup {
        ThrottlerSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`ThrottlerModule::for_root`].
pub struct ThrottlerSetup {
    pinned: Option<ThrottlerConfig>,
}

impl DynamicModule for ThrottlerSetup {
    fn module() -> TypeId {
        TypeId::of::<ThrottlerModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
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
                     `ThrottlerModule::for_root` binds the in-process default"
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
}

/// Resolve a [`ThrottlerConfig`] into the default [`Throttle`] the guard applies
/// to routes that pin none.
fn resolve(config: &ThrottlerConfig) -> Throttle {
    Throttle::new(config.limit, config.window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::throttle::DEFAULT_THROTTLE;

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
