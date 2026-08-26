//! The import seam for the DB readiness bridge: add [`SeaOrmHealthModule`]
//! alongside `nest_rs_health::HealthModule` in an app's `#[module(imports =
//! [...])]` and the framework gates `GET /health/ready` (and `/startup`) on a
//! round-trip to the database via [`SeaOrmHealthIndicator`].

use nest_rs_core::module;

use super::SeaOrmHealthIndicator;

/// Import seam for the DB readiness bridge — registers [`SeaOrmHealthIndicator`] so
/// `/health/ready` and `/startup` gate on a database round-trip.
#[module(providers = [SeaOrmHealthIndicator])]
pub struct SeaOrmHealthModule;
