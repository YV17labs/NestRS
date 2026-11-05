//! The probe endpoints, and the one boot line that says where they ended up.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{Container, Discovery};
use nest_rs_http::__private::{join_path, normalize_mount_path};
use nest_rs_http::{HttpConfig, HttpControllerMeta, controller, routes};

use crate::indicator::{ProbeKind, ProbeReport};
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
    #[api(summary = "Liveness probe: whether the process should keep running", error(503 = ProbeReport))]
    async fn live(&self) -> ProbeReport {
        self.svc.probe(ProbeKind::Liveness).await
    }

    #[get("/ready")]
    #[public]
    #[api(summary = "Readiness probe: whether the process can take traffic", error(503 = ProbeReport))]
    async fn ready(&self) -> ProbeReport {
        self.svc.probe(ProbeKind::Readiness).await
    }

    #[get("/startup")]
    #[public]
    #[api(summary = "Startup probe: whether the process has finished booting", error(503 = ProbeReport))]
    async fn startup(&self) -> ProbeReport {
        self.svc.probe(ProbeKind::Startup).await
    }
}

/// Name the paths the probes are **actually** served at, once, at boot, when
/// `HttpConfig::global_prefix` has moved them off the documented ones: a
/// manifest pointing at `/health/live` would get a `404`, a failed probe.
pub(crate) fn report_prefixed_probe_paths(container: &Container) {
    let Some(prefix) = container
        .get::<HttpConfig>()
        .and_then(|cfg| cfg.global_prefix.clone())
        .map(|raw| normalize_mount_path(&raw))
        .filter(|prefix| prefix != "/")
    else {
        return;
    };

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
        .routes()
        .iter()
        .map(|route| join_path(&prefix, &join_path(meta.path(), route.path())))
        .collect();
    tracing::warn!(
        target: crate::TARGET,
        prefix = prefix.as_str(),
        served = served.join(", ").as_str(),
        declared = meta.path(),
        hint = "point the liveness/readiness/startup probes at the served paths",
        "health probes are served under the HTTP global prefix",
    );
}
