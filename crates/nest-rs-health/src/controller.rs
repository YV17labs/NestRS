use std::sync::Arc;

use nest_rs_http::{controller, routes};
use poem::{Response, http::StatusCode};

use crate::indicator::{IndicatorStatus, ProbeKind, ProbeReport};
use crate::service::HealthService;

/// Serves the Kubernetes-style probe endpoints under `/health`, delegating each
/// to [`HealthService`].
#[controller(path = "/health")]
pub struct HealthController {
    #[inject]
    svc: Arc<HealthService>,
}

#[routes]
impl HealthController {
    #[get("/live")]
    #[public]
    async fn live(&self) -> Response {
        respond(self.svc.probe(ProbeKind::Liveness).await)
    }

    #[get("/ready")]
    #[public]
    async fn ready(&self) -> Response {
        respond(self.svc.probe(ProbeKind::Readiness).await)
    }

    #[get("/startup")]
    #[public]
    async fn startup(&self) -> Response {
        respond(self.svc.probe(ProbeKind::Startup).await)
    }
}

fn respond(report: ProbeReport) -> Response {
    let status = match report.status {
        IndicatorStatus::Up => StatusCode::OK,
        IndicatorStatus::Down => StatusCode::SERVICE_UNAVAILABLE,
    };
    // A report that fails to serialize (out-of-memory territory — the shape is
    // plain strings/maps) must not ship a silent empty 200: an orchestrator
    // would read that as healthy. Fail loud with a 500 instead.
    match serde_json::to_vec(&report) {
        Ok(body) => Response::builder()
            .status(status)
            .content_type("application/json")
            .body(body),
        Err(error) => {
            tracing::error!(
                target: "nest_rs::health",
                %error,
                "health report failed to serialize",
            );
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish()
        }
    }
}
