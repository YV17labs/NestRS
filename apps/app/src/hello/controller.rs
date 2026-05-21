use std::sync::Arc;

use nestrs_core::{controller, routes};

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
