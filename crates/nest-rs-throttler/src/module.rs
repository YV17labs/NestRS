//! [`ThrottlerModule`] — the port's own seam. `ThrottlerModule::for_root(cfg)`
//! resolves the policy ([`ThrottlerConfig`], `NESTRS_THROTTLER__*`), registers
//! the [`ThrottlerGuard`] that applies it, and binds the in-process
//! [`InMemoryThrottler`] as the default `dyn ThrottlerStore` — an *ordinary*
//! factory, so a vendor binding imported beside it (`nest_rs::redis::RedisThrottlerModule`)
//! supersedes the store wherever it sits in `imports`, and the app removes no
//! line to move its counters off-process.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::config::ThrottlerConfig;
use crate::guard::ThrottlerGuard;
use crate::store::{InMemoryThrottler, ThrottlerStore};
use crate::throttle::{DEFAULT_THROTTLE, Throttle};

/// The throttler port: policy, guard, and the in-process default store. Wire
/// with `ThrottlerModule::for_root(None)` (env-driven, default
/// `Throttle::per_minute(60)`); add a vendor binding beside it to share the
/// counters across instances.
pub struct ThrottlerModule;

impl ThrottlerModule {
    /// Pass `None` to load [`ThrottlerConfig`] from `NESTRS_THROTTLER__*`, or a
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
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        // The default store, as an *ordinary* factory: a vendor binding's
        // declared factory for the same `Arc<dyn ThrottlerStore>` supersedes it
        // wherever the two fall in `imports`, and two vendor bindings contest
        // (`BACKEND_REMEDY`). A factory output, so the guard's
        // `#[inject] Arc<dyn ThrottlerStore>` resolves as global infrastructure.
        let builder = builder.provide_factory::<Arc<dyn ThrottlerStore>, _, _>(|_| async {
            Ok(Arc::new(InMemoryThrottler::new()) as Arc<dyn ThrottlerStore>)
        });
        // The resolved policy, as a provider of its own: the guard injects it,
        // so a guard built by any other path (`providers = [ThrottlerGuard]`
        // with no `for_root`) fails the boot naming `Throttle` rather than
        // running 60/minute in silence. Queued in the same `collect` as the
        // config it reads, hence after it.
        let builder = builder.provide_factory::<Throttle, _, _>(|container| async move {
            let config = container
                .get::<ThrottlerConfig>()
                .expect("ThrottlerConfig is resolved by ConfigModule::provide_feature");
            Ok(resolve(&config))
        });
        // The guard reads the store, and the store is itself a factory output —
        // one that may wait on a connection of its own (a vendor binding).
        // Declared after it, so the drain runs this last however the seams were
        // queued; without it the guard ran first whenever the store's factory
        // was deferred, and failed naming a binding that was about to exist.
        builder.provide_factory_after::<ThrottlerGuard, Arc<dyn ThrottlerStore>, _, _>(
            |container| async move {
                let default = container
                    .get::<Throttle>()
                    .expect("the policy is queued by this same collect, before the guard");
                let store = container.get_dyn::<dyn ThrottlerStore>().ok_or_else(|| {
                    anyhow::anyhow!(
                        "ThrottlerGuard needs a `dyn ThrottlerStore` binding — \
                         `ThrottlerModule::for_root` binds the in-process default"
                    )
                })?;
                Ok(ThrottlerGuard::new(store, *default))
            },
        )
    }
}

/// What the boot tells you when two vendor bindings both bound the store.
/// Shared with every store adapter (`nest-rs-redis` today, a third party's
/// tomorrow — it is part of the store contract) so the two halves of the rule
/// cannot drift.
pub const BACKEND_REMEDY: &str = "Import exactly one throttler store binding beside \
                                  `ThrottlerModule::for_root`: `nest_rs::redis::RedisThrottlerModule` \
                                  shares the counters across instances; with none, they stay \
                                  in this process.";

/// Resolve a [`ThrottlerConfig`] into the default [`Throttle`] the guard applies
/// to routes that pin none.
fn resolve(config: &ThrottlerConfig) -> Throttle {
    let limit = config.limit.unwrap_or(DEFAULT_THROTTLE.limit);
    let window = config
        .window_secs
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_THROTTLE.window);
    Throttle::new(limit, window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_falls_back_to_the_port_default_per_field() {
        let cfg = ThrottlerConfig {
            limit: Some(5),
            window_secs: None,
        };
        let t = resolve(&cfg);
        assert_eq!(t.limit, 5);
        assert_eq!(t.window, DEFAULT_THROTTLE.window);
    }
}
