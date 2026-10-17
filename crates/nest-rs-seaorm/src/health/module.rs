use nest_rs_core::module;

use super::SeaOrmHealthIndicator;

/// Import seam for the DB readiness bridge — registers [`SeaOrmHealthIndicator`] so
/// `/health/ready` and `/startup` gate on a database round-trip.
#[module(providers = [SeaOrmHealthIndicator])]
pub struct SeaOrmHealthModule;
