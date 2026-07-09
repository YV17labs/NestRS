use std::sync::Arc;

use nest_rs_core::{Container, Module};
use nest_rs_health::{HealthModule, HealthService};

#[test]
fn registers_health_service() {
    let container = HealthModule::register(Container::builder()).build();
    let svc: Option<Arc<HealthService>> = container.get();
    assert!(svc.is_some());
}
