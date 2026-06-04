use std::sync::Arc;

use nestrs_http::{controller, routes};
use poem::{Response, http::StatusCode};

use crate::service::HealthCheck;

#[controller(path = "/health")]
pub struct HealthController {
    #[inject]
    svc: Arc<dyn HealthCheck>,
}

#[routes]
impl HealthController {
    #[get("/live")]
    async fn live(&self) -> Response {
        if self.svc.is_live().await {
            Response::builder().status(StatusCode::OK).body("ok")
        } else {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body("dead")
        }
    }

    #[get("/ready")]
    async fn ready(&self) -> Response {
        if self.svc.is_ready().await {
            Response::builder().status(StatusCode::OK).body("ready")
        } else {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body("not ready")
        }
    }

    #[get("/startup")]
    async fn startup(&self) -> Response {
        if self.svc.is_started().await {
            Response::builder().status(StatusCode::OK).body("started")
        } else {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body("starting")
        }
    }
}
