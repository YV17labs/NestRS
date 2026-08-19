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

#![warn(missing_docs)]

/// This crate's span target — Health indicators and their verdicts.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::health";

mod controller;
mod indicator;
mod module;
mod service;

pub use controller::HealthController;
pub use indicator::{HealthIndicator, IndicatorReport, IndicatorStatus, ProbeKind, ProbeReport};
pub use module::HealthModule;
pub use nest_rs_health_macros::indicators;
pub use service::HealthService;
