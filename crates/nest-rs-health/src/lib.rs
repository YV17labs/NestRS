//! Liveness/readiness/startup probes for nestrs apps.
//!
//! Importing [`HealthModule`] mounts three routes on the HTTP transport
//! (`GET /health/live`, `GET /health/ready`, `GET /health/startup`). Each
//! route runs every [`HealthIndicator`] registered for its [`ProbeKind`]
//! against the assembled container and returns `200` with a JSON body when
//! all are `up`, `503` when any is `down`.
//!
//! Indicators are declared with the `#[indicators]` decorator on an
//! `#[injectable]` provider's `impl` block — see the [`indicators`] macro
//! re-export below. Discovery is link-time (via the `inventory` crate) and
//! module-gated by
//! [`ReachableProviders`](::nest_rs_core::ReachableProviders), so an indicator
//! whose provider lives in an unimported module compiles in but does not
//! fire.

mod controller;
mod indicator;
mod module;
mod service;

pub use controller::HealthController;
pub use indicator::{
    HealthIndicator, IndicatorReport, IndicatorStatus, ProbeKind, ProbeReport,
};
pub use module::HealthModule;
pub use nest_rs_health_macros::indicators;
pub use service::HealthService;
