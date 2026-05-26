use std::sync::Arc;

use nestrs_http::{controller, routes};

use crate::hello::service::HelloService;

#[controller(path = "/")]
pub struct HelloController {
    #[inject]
    svc: Arc<HelloService>,
}

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> &'static str {
        self.svc.greeting()
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nestrs_core::Discoverable;

    use super::HelloController;
    use crate::hello::service::HelloService;

    #[test]
    fn controller_declares_its_injected_dependency_for_the_access_graph() {
        // A controller is built at mount time, so `dependencies` (register
        // ordering) is empty; `#[routes]` bridges its `#[inject]` keys into
        // `injected` (via the inherent fn `#[controller]` emits) so the
        // access-graph check governs it too.
        assert!(HelloController::dependencies().is_empty());
        assert!(
            HelloController::injected().contains(&TypeId::of::<HelloService>()),
            "the controller's injected HelloService is recorded for the access graph",
        );
    }
}
