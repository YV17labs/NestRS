//! Liveness/readiness probes — [`HealthModule`] mounts `GET /health/*` on the HTTP transport.

mod controller;
mod module;
mod service;

pub use controller::HealthController;
pub use module::HealthModule;
pub use service::{HealthCheck, HealthService};
