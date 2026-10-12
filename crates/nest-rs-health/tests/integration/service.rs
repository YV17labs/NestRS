//! Covers `src/service.rs`: a probe with nothing bound, and an indicator that
//! unwinds — down, said once, the probe answering for the others.
//!
//! `inventory` is process-wide; `Unwinding` is reachable only from this file's
//! `UnwindingModule`, so the sibling suites' probes never run it.

use nest_rs_core::{injectable, module};
use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators};
use nest_rs_testing::{LogCapture, TestApp};

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

#[injectable]
#[derive(Default)]
struct Unwinding;

#[indicators]
impl Unwinding {
    #[startup]
    async fn warmed_up(&self) -> Result<(), std::io::Error> {
        panic!("the cache client unwound")
    }

    #[startup]
    async fn migrated(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[module(imports = [HealthModule], providers = [Unwinding])]
struct UnwindingModule;

/// An indicator that unwinds is a check that failed: it reads `down` with the
/// opaque reason, the probe still answers — `503`, its sibling heard — and the
/// panic is filed once at `error`, never in the body.
#[tokio::test]
async fn an_indicator_that_panics_reads_down_and_the_probe_answers() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<UnwindingModule>()
        .await
        .expect("a module with a decorated indicator host boots");

    let response = app.http().get("/health/startup").send().await.0;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .into_string()
        .await
        .expect("the probe answers with a body");

    assert_eq!(status, 503, "{body}");
    let report: serde_json::Value = serde_json::from_str(&body).expect("a JSON report");
    assert_eq!(report["details"]["warmed_up"]["status"], "down", "{body}");
    assert_eq!(
        report["details"]["warmed_up"]["error"], "check failed",
        "{body}"
    );
    assert_eq!(report["details"]["migrated"]["status"], "up", "{body}");
    assert!(!body.contains("unwound"), "{body}");

    let said = logs.expect_one(nest_rs_health::TARGET, "health indicator panicked");
    assert_eq!(said.level, "error");
    assert_eq!(said.field("indicator").as_deref(), Some("warmed_up"));
    assert_eq!(
        said.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the cache client unwound"),
    );
}
