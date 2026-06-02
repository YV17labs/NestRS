use std::sync::Arc;

use nestrs_core::{Container, Module};
use nestrs_health::{HealthCheck, HealthModule};

#[test]
fn registers_default_health_check() {
    let container = HealthModule::register(Container::builder()).build();
    let svc: Option<Arc<dyn HealthCheck>> = container.get_dyn();
    assert!(svc.is_some());
}
