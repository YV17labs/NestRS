//! The import seam for the DB readiness bridge: add [`DatabaseHealthModule`]
//! alongside `nest_rs_health::HealthModule` in an app's `#[module(imports =
//! [...])]` and the framework gates `GET /health/ready` (and `/startup`) on a
//! round-trip to the database via [`DbHealthIndicator`].

use nest_rs_core::module;

use super::DbHealthIndicator;

#[module(providers = [DbHealthIndicator])]
pub struct DatabaseHealthModule;
