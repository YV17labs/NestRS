use std::sync::Arc;

use nest_rs_http::{controller, routes};
use poem::{Response, http::StatusCode};

use crate::indicator::{IndicatorStatus, ProbeKind, ProbeReport};
use crate::service::HealthService;

#[controller(path = "/health")]
pub struct HealthController {
    #[inject]
    svc: Arc<HealthService>,
}

#[routes]
impl HealthController {
    #[get("/live")]
    async fn live(&self) -> Response {
        respond(self.svc.probe(ProbeKind::Liveness).await)
    }

    #[get("/ready")]
    async fn ready(&self) -> Response {
        respond(self.svc.probe(ProbeKind::Readiness).await)
    }

    #[get("/startup")]
    async fn startup(&self) -> Response {
        respond(self.svc.probe(ProbeKind::Startup).await)
    }
}

fn respond(report: ProbeReport) -> Response {
    let status = match report.status {
        IndicatorStatus::Up => StatusCode::OK,
        IndicatorStatus::Down => StatusCode::SERVICE_UNAVAILABLE,
    };
    Response::builder()
        .status(status)
        .content_type("application/json")
        .body(serde_json::to_vec(&report).unwrap_or_default())
}
