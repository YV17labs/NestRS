//! The probe endpoints, and the one boot line that says where they ended up.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{Container, Discovery};
use nest_rs_http::{
    HttpConfig, HttpControllerMeta, controller, join_path, normalize_mount_path, routes,
};
use poem::{Response, http::StatusCode};

use crate::indicator::{IndicatorStatus, ProbeKind, ProbeReport};
use crate::service::HealthService;

/// Serves the Kubernetes-style probe endpoints under `/health`, delegating each
/// to [`HealthService`].
#[controller(path = "/health")]
pub(crate) struct HealthController {
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
                target: crate::TARGET,
                %error,
                "health report failed to serialize",
            );
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish()
        }
    }
}

/// Name the paths the probes are **actually** served at, once, at boot, when
/// `HttpConfig::global_prefix` has moved them off the documented ones.
///
/// A probe path is a contract with an orchestrator rather than part of the
/// app's API namespace, so `/health/live` under `global_prefix = "/api/v1"` is
/// not a cosmetic difference: a manifest written from this crate's docs gets a
/// `404`, the kubelet reads `404` as a failed probe, and on a liveness probe
/// that is `CrashLoopBackOff` caused by the framework's own documentation.
///
/// Exempting the mount would be the better answer and it is not this crate's to
/// give: `HttpTransport` nests the fully-assembled tree — controllers,
/// self-mounts and imperative mounts alike — inside the prefix, so nothing a
/// module contributes can land outside it. Making the surprise *loud and
/// exact* is what remains available here, and a knob that could not move the
/// mount would have been a false statement rather than a smaller answer.
///
/// `warn`, not `info`: the operator has to act on it before the next rollout,
/// and it fires only when the paths differ from the documented ones — an app
/// with no prefix is silent.
pub(crate) fn report_prefixed_probe_paths(container: &Container) {
    let Some(prefix) = container
        .get::<HttpConfig>()
        .and_then(|cfg| cfg.global_prefix.clone())
        .map(|raw| normalize_mount_path(&raw))
        .filter(|prefix| prefix != "/")
    else {
        return;
    };

    // Keyed on the provider's `TypeId` rather than on the controller's name or
    // its declared path: both are written in the decorator above, and a boot
    // line that names a path the decorator no longer mounts is worse than none.
    let discovery = Discovery::new(container);
    let Some(meta) = discovery
        .meta::<HttpControllerMeta>()
        .into_iter()
        .find(|d| d.provider_type_id == Some(TypeId::of::<HealthController>()))
        .map(|d| d.meta)
    else {
        return;
    };

    let served: Vec<String> = meta
        .routes
        .iter()
        .map(|route| join_path(&prefix, &join_path(meta.path, route.path)))
        .collect();
    tracing::warn!(
        target: crate::TARGET,
        prefix = prefix.as_str(),
        served = served.join(", ").as_str(),
        declared = meta.path,
        hint = "point the liveness/readiness/startup probes at the served paths",
        "health probes are served under the HTTP global prefix",
    );
}
