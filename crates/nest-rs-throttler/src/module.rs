//! [`ThrottlerModule`] — provides the shared [`InMemoryThrottler`] carrying the
//! default rate limit. Configure at the import site with
//! `ThrottlerModule::for_root(None)`.

use std::net::IpAddr;
use std::time::Duration;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::config::ThrottlerConfig;
use crate::store::InMemoryThrottler;
use crate::rate::Throttle;

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
        builder.provide_factory::<InMemoryThrottler, _, _>(|container| async move {
            let config = container
                .get::<ThrottlerConfig>()
                .expect("ThrottlerConfig is resolved by ConfigModule::provide_feature");
            let trusted_proxies = parse_trusted_proxies(&config.trusted_proxies)?;
            Ok(InMemoryThrottler::new(
                throttle_from(&config),
                trusted_proxies,
            ))
        })
    }
}

fn throttle_from(config: &ThrottlerConfig) -> Throttle {
    let limit = config.limit.unwrap_or(DEFAULT_THROTTLE.limit);
    let window = config
        .window_secs
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_THROTTLE.window);
    Throttle::new(limit, window)
}

// A bad IP aborts the boot naming the variable — never a silent skip.
fn parse_trusted_proxies(raw: &[String]) -> anyhow::Result<Vec<IpAddr>> {
    raw.iter()
        .map(|s| {
            s.parse::<IpAddr>().map_err(|e| {
                anyhow::anyhow!("NESTRS_THROTTLER__TRUSTED_PROXIES: invalid IP `{s}`: {e}")
            })
        })
        .collect()
}
