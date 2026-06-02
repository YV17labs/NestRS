use nestrs_core::module;

use crate::controller::HealthController;
use crate::service::{HealthCheck, HealthService};

#[module(
    providers = [
        HealthService as dyn HealthCheck,
        HealthController,
    ],
)]
pub struct HealthModule;
