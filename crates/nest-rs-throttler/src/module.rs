//! [`ThrottlerModule`] — binds the shared [`InMemoryThrottler`] as the
//! `dyn ThrottlerStore` the [`ThrottlerGuard`](crate::ThrottlerGuard) injects,
//! carrying the default rate limit. Configure at the import site with
//! `ThrottlerModule::for_root(None)`.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::config::ThrottlerConfig;
use crate::rate::Throttle;
use crate::store::{InMemoryThrottler, ThrottlerStore};

pub const DEFAULT_THROTTLE: Throttle = Throttle::per_minute(60);

/// Provides the process-wide [`InMemoryThrottler`]. Wire with
/// `ThrottlerModule::for_root(None)` (env-driven, default
/// `Throttle::per_minute(60)`).
pub struct ThrottlerModule;

impl ThrottlerModule {
    /// Pass `None` to load [`ThrottlerConfig`] from `NESTRS_THROTTLER__*`, or a
    /// [`ThrottlerConfig`] to pin it in code (wins over the environment).
    pub fn for_root(config: impl Into<Option<ThrottlerConfig>>) -> ThrottlerSetup {
        ThrottlerSetup {
            pinned: config.into(),
        }
    }
}

pub struct ThrottlerSetup {
    pinned: Option<ThrottlerConfig>,
}

impl DynamicModule for ThrottlerSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        // Register the store as the `dyn ThrottlerStore` binding the guard
        // injects — a factory output, so the access graph sees it as global
        // infrastructure (the guard's `#[inject] Arc<dyn ThrottlerStore>`
        // resolves). An alternative backend (`RedisThrottlerModule`) supplies
        // the same binding from its own factory; import exactly one.
        builder.provide_factory::<Arc<dyn ThrottlerStore>, _, _>(|container| async move {
            let config = container
                .get::<ThrottlerConfig>()
                .expect("ThrottlerConfig is resolved by ConfigModule::provide_feature");
            let (default, trusted_proxies) = resolve(&config)?;
            Ok(Arc::new(InMemoryThrottler::new(default, trusted_proxies)) as Arc<dyn ThrottlerStore>)
        })
    }
}

/// Resolve a [`ThrottlerConfig`] into the default [`Throttle`] and the parsed
/// trusted-proxy list every [`ThrottlerStore`] backend needs. Shared so the
/// in-memory and Redis modules resolve config identically. A bad IP aborts the
/// boot naming the variable — never a silent skip.
pub fn resolve(config: &ThrottlerConfig) -> anyhow::Result<(Throttle, Vec<IpAddr>)> {
    Ok((throttle_from(config), parse_trusted_proxies(&config.trusted_proxies)?))
}

fn throttle_from(config: &ThrottlerConfig) -> Throttle {
    let limit = config.limit.unwrap_or(DEFAULT_THROTTLE.limit);
    let window = config
        .window_secs
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_THROTTLE.window);
    Throttle::new(limit, window)
}

fn parse_trusted_proxies(raw: &[String]) -> anyhow::Result<Vec<IpAddr>> {
    raw.iter()
        .map(|s| {
            s.parse::<IpAddr>().map_err(|e| {
                anyhow::anyhow!("NESTRS_THROTTLER__TRUSTED_PROXIES: invalid IP `{s}`: {e}")
            })
        })
        .collect()
}
