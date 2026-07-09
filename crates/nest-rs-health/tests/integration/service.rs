use nest_rs_health::{HealthService, IndicatorStatus, ProbeKind};

#[tokio::test]
async fn unbound_service_reports_up_with_empty_details() {
    // An app that mounts `HealthModule` but has not yet reached
    // `OnApplicationBootstrap` (e.g. a probe racing the wire-up) gets an
    // empty `up`: the framework prefers "permissive while warming" over a
    // flapping 503.
    let svc = HealthService::default();
    for kind in [
        ProbeKind::Liveness,
        ProbeKind::Readiness,
        ProbeKind::Startup,
    ] {
        let report = svc.probe(kind).await;
        assert_eq!(report.status, IndicatorStatus::Up);
        assert!(
            report.details.is_empty(),
            "{kind:?} reports must be empty when unbound"
        );
    }
}
