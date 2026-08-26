//! Liveness/readiness/startup probes for nestrs apps.
//!
//! Importing [`HealthModule`] mounts three routes on the HTTP transport —
//! `GET /health/live`, `GET /health/ready`, `GET /health/startup`. Each route
//! runs every [`HealthIndicator`] registered for its [`ProbeKind`] against the
//! assembled container and returns `200` with a JSON body when all are `up`,
//! `503` when any is `down`.
//!
//! **Those paths are the mounted ones, not necessarily the served ones.** A
//! probe route is an ordinary controller, so it sits under
//! `HttpConfig::global_prefix` like every other: an app with
//! `NESTRS_HTTP__GLOBAL_PREFIX=/api/v1` serves `GET /api/v1/health/live`, and a
//! Kubernetes manifest written from the unqualified path above gets a `404` —
//! which the kubelet scores as a failed probe. Exempting the mount is not this
//! crate's to give (the transport nests the whole assembled tree, self-mounts
//! included, inside the prefix), so what it does instead is refuse to let the
//! difference be silent: a prefixed app logs one `warn` at boot naming the
//! exact paths its probes answer on. Point the manifest at those.
//!
//! **Both ceilings on a probe are configurable** — see [`HealthConfig`]. The
//! indicators run concurrently under a per-indicator ceiling and a probe-wide
//! deadline, both defaulting inside Kubernetes' own `timeoutSeconds` default of
//! one second, so a slow check answers `503` with a log line rather than
//! silently outliving the kubelet's deadline.
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

mod config;
mod controller;
mod indicator;
mod module;
mod service;

pub use config::HealthConfig;
/// The kernel's, re-exported so `#[indicators]`' expansion reaches it through
/// this crate rather than across to a sibling — the form `framework.md`
/// sanctions for a `*-macros` crate.
#[doc(hidden)]
pub use nest_rs_core::unresolved_host;
// `IndicatorFuture` and `IndicatorRun` are exported because `HealthIndicator`
// is public and `run` is a public field of that type: `indicator` is a private
// module, so without these the field's type is unnameable by a caller and
// renders unlinked on docs.rs. They are the export contract of a public field,
// not a surface anyone is expected to write.
pub use indicator::{
    HealthIndicator, IndicatorFuture, IndicatorReport, IndicatorRun, IndicatorStatus, ProbeKind,
    ProbeReport,
};
pub use module::{HealthModule, HealthSetup};
pub use nest_rs_health_macros::indicators;
pub use service::HealthService;
