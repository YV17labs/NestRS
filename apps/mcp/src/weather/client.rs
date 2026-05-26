//! Provisioning of the weather feature's outbound HTTP client.
//!
//! The Open-Meteo client and its config are infrastructure the weather feature
//! owns, so they are installed here — in a module — rather than seeded in
//! `main` (the framework rule that `main` holds only
//! `App::builder().module::<…>()`). Both land in the **collect phase**, before
//! any provider is built: `WeatherConfig` synchronously (it is infallible), the
//! `reqwest::Client` through a factory (its `build()` is fallible, so the
//! factory's `Result` aborts boot cleanly on error). `OpenMeteoClient` then
//! injects both as ordinary providers — they are global infrastructure, so the
//! access-graph check sees them reachable from any module.

use std::time::Duration;

use nestrs_core::{ContainerBuilder, Module};

use crate::weather::config::WeatherConfig;

/// Owns the weather HTTP client and its config. Imported by [`WeatherModule`](crate::weather::WeatherModule).
pub(in crate::weather) struct WeatherClientModule;

impl Module for WeatherClientModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            // Sync + infallible, so provided directly — and thus visible to the
            // client factory queued just below, which runs after collect.
            .provide(WeatherConfig::from_env())
            .provide_factory::<reqwest::Client, _, _>(|container| async move {
                let cfg = container
                    .get::<WeatherConfig>()
                    .expect("WeatherConfig is provided in collect, before factories run");
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_millis(cfg.request_timeout_ms))
                    .user_agent(concat!("nestrs-mcp/", env!("CARGO_PKG_VERSION")))
                    .build()?;
                Ok(client)
            })
    }
}
