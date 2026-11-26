//! A decorated indicator of an `#[indicators]` block, attempted again within
//! the one run its probe makes, and refused: its probe down, the others up.

use std::sync::Arc;

use http::StatusCode;
use nest_rs_core::{injectable, module};
use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{HeadlessApp, LogCapture, TestApp};
use serde_json::Value;

use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing, retrying_once};

#[injectable]
struct UpstreamIndicator {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[indicators]
impl UpstreamIndicator {
    #[readiness]
    #[resilient(upstream, idempotent)]
    async fn upstream_reachable(&self) -> Result<(), ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(())
    }
}

#[module(
    imports = [HealthModule, ResilienceModule::for_root(retrying_once())],
    providers = [UpstreamIndicator, Attempts],
)]
struct DecoratedHealthModule;

#[tokio::test(start_paused = true)]
async fn an_indicator_failing_once_answers_up_on_its_retry() {
    let app = HeadlessApp::builder()
        .module::<DecoratedHealthModule>()
        .build_headless()
        .await
        .expect("a decorated indicator boots");
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");
    let ready = health.probe(ProbeKind::Readiness).await;

    assert_eq!(ready.status, IndicatorStatus::Up, "{:?}", ready.details);
    assert!(ready.details.contains_key("upstream_reachable"));
    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
}

#[injectable]
struct DependencyIndicator {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
}

#[indicators]
impl DependencyIndicator {
    #[readiness]
    #[resilient(upstream, timeout = "100ms")]
    async fn dependency_reachable(&self) -> Result<(), ResilienceError<UpstreamDown>> {
        self.dependency.call().await
    }
}

#[module(
    imports = [HealthModule, ResilienceModule::for_root(refusing())],
    providers = [DependencyIndicator, Dependency],
)]
struct RefusedHealthModule;

/// What the probe at `path` answers: its status and its report.
async fn probed(app: &TestApp, path: &str) -> (StatusCode, Value) {
    let answered = app.http().get(path).send().await;
    let status = answered.0.status();
    (status, answered.json().await.value().deserialize::<Value>())
}

/// The `503` here is the probe's own, which an orchestrator reads: a
/// refused indicator is down, its refusal in the log and never in the
/// body, and the probe after the refusal lifted is up.
#[tokio::test(start_paused = true)]
async fn an_indicator_its_policy_refuses_answers_down_on_its_probe_alone() {
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<RefusedHealthModule>()
            .await
            .expect("a decorated indicator boots");
        let indicator = app
            .container()
            .get::<DependencyIndicator>()
            .expect("provided");
        let (upstream, dependency) = refusing.arrange(app.container());
        let mut events = upstream.events();

        let (status, ready) = probed(&app, "/health/ready").await;
        let told = crate::told(&mut events);
        assert_eq!(dependency.called(), refusing.sent());
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{ready}");
        assert_eq!(ready["status"], "down", "{ready}");
        assert_eq!(ready["error"]["dependency_reachable"]["status"], "down");
        let (status, live) = probed(&app, "/health/live").await;
        assert_eq!(status, StatusCode::OK, "{live}");
        assert_eq!(live["status"], "up", "{live}");

        let failed = logs.expect_one(nest_rs_health::TARGET, "health indicator failed");
        assert_eq!(failed.level, "warn", "{failed:?}");
        assert_eq!(
            failed.field("indicator").as_deref(),
            Some("dependency_reachable")
        );
        assert_eq!(
            failed.field("kind").as_deref(),
            Some(ProbeKind::Readiness.as_str())
        );
        refusing.told(&told, "DependencyIndicator", "dependency_reachable");
        let chain = refusing.chain(indicator.dependency_reachable().await);
        assert_eq!(
            failed.field("error").as_deref(),
            Some(&*chain),
            "{failed:?}"
        );
        assert!(
            !ready.to_string().contains(&chain),
            "the report says the indicator is down, never why: {ready}"
        );

        refusing.lift(&upstream, &dependency);
        let (status, ready) = probed(&app, "/health/ready").await;
        assert_eq!(status, StatusCode::OK, "{ready}");
        assert_eq!(ready["info"]["dependency_reachable"]["status"], "up");
    }
}
