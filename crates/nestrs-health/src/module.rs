use nestrs_core::module;

use crate::service::{HealthCheck, HealthService};

#[module(providers = [HealthService as dyn HealthCheck])]
pub struct HealthModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_default_health_check() {
        let container = HealthModule::register(Container::builder()).build();
        let svc: Option<Arc<dyn HealthCheck>> = container.get_dyn();
        assert!(svc.is_some());
    }
}
