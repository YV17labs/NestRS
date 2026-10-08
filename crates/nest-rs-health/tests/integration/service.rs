use nest_rs_health::{HealthService, IndicatorStatus, ProbeKind};

#[tokio::test]
async fn unbound_service_reports_up_with_empty_details() {
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
