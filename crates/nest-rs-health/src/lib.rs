//! Liveness/readiness/startup probes for nestrs apps.
//!
//! Importing [`HealthModule`] mounts three routes on the HTTP transport —
//! `GET /health/live`, `GET /health/ready`, `GET /health/startup`. Each route
//! runs every [`HealthIndicator`] registered for its [`ProbeKind`] against the
//! assembled container and returns `200` with a JSON body when all are `up`,
//! `503` when any is `down`.
//!
//! **The routes sit under `HttpConfig::global_prefix`**: with
//! `<PREFIX>_HTTP__GLOBAL_PREFIX=/api/v1` a probe answers on
//! `GET /api/v1/health/live`, and a prefixed app logs one `warn` at boot naming
//! the exact paths. Point the Kubernetes manifest at those.
//!
//! Indicators run concurrently under a per-indicator ceiling and a probe-wide
//! deadline ([`HealthConfig`]), both inside Kubernetes' one-second
//! `timeoutSeconds` default.
//!
//! Indicators are declared with [`indicators`] on an `#[injectable]` provider's
//! `impl` block; one whose provider lives in an unimported module does not run.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — health indicators and their verdicts.
pub const TARGET: &str = "nest_rs::health";

mod config;
mod controller;
mod indicator;
mod module;
mod service;

pub use config::HealthConfig;
// `IndicatorFuture` and `IndicatorRun` name the type of the public field
// `HealthIndicator::run`.
pub use indicator::{
    HealthIndicator, IndicatorFuture, IndicatorReport, IndicatorRun, IndicatorStatus, ProbeKind,
    ProbeReport,
};
pub use module::{HealthModule, HealthSetup};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use nest_rs_core::unresolved_host;
}

/// Orchestrator on a provider's `impl` block: each `#[liveness]`,
/// `#[readiness]` or `#[startup]` method in it is a [`HealthIndicator`] that
/// probe runs.
///
/// ```
/// # use anyhow::Context as _;
/// # use nest_rs_core::{App, injectable, module};
/// # use nest_rs_health::{HealthIndicator, HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators};
/// #[injectable]
/// #[derive(Default)]
/// pub struct AppHealth;
///
/// #[indicators]
/// impl AppHealth {
///     #[readiness]
///     async fn db_ping(&self) -> anyhow::Result<()> {
///         Ok(())
///     }
///
///     #[liveness]
///     async fn process_responsive(&self) {}
/// }
/// # #[module(imports = [HealthModule], providers = [AppHealth])]
/// # struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = App::builder().module::<AppModule>().build().await?;
/// # app.init().await?;
/// # let health = app.container().get::<HealthService>().context("HealthModule provides it")?;
///
/// let ready = health.probe(ProbeKind::Readiness).await;
/// assert_eq!(ready.status, IndicatorStatus::Up);
/// assert!(ready.details.contains_key("db_ping"));
///
/// let declared = nest_rs_core::inventory::iter::<HealthIndicator>()
///     .find(|indicator| indicator.name == "db_ping")
///     .map(|indicator| indicator.kind);
/// assert_eq!(declared, Some(ProbeKind::Readiness));
/// # Ok(())
/// # }
/// ```
pub use nest_rs_health_macros::indicators;
pub use service::HealthService;
