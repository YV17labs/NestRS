use nestrs_health::{HealthCheck, HealthService};

#[tokio::test]
async fn default_service_reports_live_ready_and_started() {
    let svc = HealthService;
    assert!(svc.is_live().await);
    assert!(svc.is_ready().await);
    assert!(svc.is_started().await);
}
