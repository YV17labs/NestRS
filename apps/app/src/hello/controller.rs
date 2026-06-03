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
    async fn hello(&self) -> String {
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
        assert!(HelloController::dependencies().is_empty());
        assert!(
            HelloController::injected().contains(&TypeId::of::<HelloService>()),
            "the controller's injected HelloService is recorded for the access graph",
        );
    }
}
