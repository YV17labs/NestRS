use std::sync::Arc;

use nest_rs_http::{controller, routes};
use poem::web::Json;

use crate::dto::HelloDto;
use crate::service::HelloService;

#[controller(path = "/")]
pub struct HelloController {
    #[inject]
    svc: Arc<HelloService>,
}

#[routes]
impl HelloController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    #[get("/hello")]
    async fn hello(&self) -> Json<HelloDto> {
        Json(HelloDto {
            message: self.svc.greeting(),
        })
    }
}
