use nestrs_core::module;

use crate::hello::controller::HelloController;
use crate::hello::service::HelloService;

#[module(providers = [HelloService, HelloController])]
pub struct HelloModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_hello_service() {
        let container = HelloModule::register(Container::builder()).build();
        let svc: Option<Arc<HelloService>> = container.get();
        assert!(svc.is_some());
    }
}
