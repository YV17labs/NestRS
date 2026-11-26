//! A decorated indicator of an `#[indicators]` block, attempted again within
//! the one run its probe makes, and refused on each of the three probes: its
//! probe down, the others up.

use std::sync::Arc;

use http::StatusCode;
use nest_rs_core::{Container, Module, injectable, module};
use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{HeadlessApp, LogCapture, TestApp};
use serde_json::Value;

use super::{Attempts, Dependency, Refusable, Refusing, UpstreamDown, refusing, retrying_once};

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
    async fn dependency_reachable(&self) -> Refusable {
        self.dependency.call().await
    }
}

#[module(
    imports = [HealthModule, ResilienceModule::for_root(refusing())],
    providers = [DependencyIndicator, Dependency],
)]
struct RefusedHealthModule;

#[injectable]
struct LivenessIndicator {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
}

#[indicators]
impl LivenessIndicator {
    #[liveness]
    #[resilient(upstream, timeout = "100ms")]
    async fn dependency_alive(&self) -> Refusable {
        self.dependency.call().await
    }
}

#[module(
    imports = [HealthModule, ResilienceModule::for_root(refusing())],
    providers = [LivenessIndicator, Dependency],
)]
struct RefusedLivenessModule;

#[injectable]
struct StartupIndicator {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
}

#[indicators]
impl StartupIndicator {
    #[startup]
    #[resilient(upstream, timeout = "100ms")]
    async fn dependency_started(&self) -> Refusable {
        self.dependency.call().await
    }
}

#[module(
    imports = [HealthModule, ResilienceModule::for_root(refusing())],
    providers = [StartupIndicator, Dependency],
)]
struct RefusedStartupModule;

/// What the probe at `path` answers: its status and its report.
async fn probed(app: &TestApp, path: &str) -> (StatusCode, Value) {
    let answered = app.http().get(path).send().await;
    let status = answered.0.status();
    (status, answered.json().await.value().deserialize::<Value>())
}

/// Each probe and the path an orchestrator reads it at.
const PROBES: [(ProbeKind, &str); 3] = [
    (ProbeKind::Liveness, "/health/live"),
    (ProbeKind::Readiness, "/health/ready"),
    (ProbeKind::Startup, "/health/startup"),
];

/// The decorated indicator `M` declares on the probe `kind`, `indicator` of
/// `host`, refused each way: the `503` is the probe's own, which an
/// orchestrator reads — a refused indicator is down, its refusal in the log
/// and never in the body, the other probes up — and the probe after the
/// refusal lifted is up. `called` is the indicator called as its own caller
/// calls it.
async fn an_indicator_refused_answers_down_on_its_probe_alone<M: Module + 'static>(
    kind: ProbeKind,
    (host, indicator): (&'static str, &'static str),
    called: impl AsyncFn(&Container) -> Refusable,
) {
    let (_, path) = PROBES
        .into_iter()
        .find(|(probe, _)| *probe == kind)
        .expect("every probe has its path");
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<M>()
            .await
            .expect("a decorated indicator boots");
        let mut refused = refusing.arrange(app.container()).await;
        let mut events = refused.upstream.events();

        let (status, report) = probed(&app, path).await;
        let told = crate::told(&mut events);
        assert_eq!(refused.dependency.called(), refusing.sent());
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{report}");
        assert_eq!(report["status"], "down", "{report}");
        assert_eq!(report["error"][indicator]["status"], "down");
        for (other, other_path) in PROBES {
            if other != kind {
                let (status, report) = probed(&app, other_path).await;
                assert_eq!(status, StatusCode::OK, "{other_path}: {report}");
                assert_eq!(report["status"], "up", "{other_path}: {report}");
            }
        }

        let failed = logs.expect_one(nest_rs_health::TARGET, "health indicator failed");
        assert_eq!(failed.level, "warn", "{failed:?}");
        assert_eq!(failed.field("indicator").as_deref(), Some(indicator));
        assert_eq!(failed.field("kind").as_deref(), Some(kind.as_str()));
        refusing.told(&told, host, indicator);
        let chain = refusing.chain(called(app.container()).await);
        assert_eq!(
            failed.field("error").as_deref(),
            Some(&*chain),
            "{failed:?}"
        );
        assert!(
            !report.to_string().contains(&chain),
            "the report says the indicator is down, never why: {report}"
        );

        refused.lift().await;
        let (status, report) = probed(&app, path).await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(report["info"][indicator]["status"], "up");
    }
}

/// A refused readiness check takes the replica out of traffic alone.
#[tokio::test(start_paused = true)]
async fn an_indicator_its_policy_refuses_answers_down_on_its_probe_alone() {
    an_indicator_refused_answers_down_on_its_probe_alone::<RefusedHealthModule>(
        ProbeKind::Readiness,
        ("DependencyIndicator", "dependency_reachable"),
        async |container| {
            indicator_of::<DependencyIndicator>(container)
                .dependency_reachable()
                .await
        },
    )
    .await;
}

/// A refused liveness check fails the liveness probe: the orchestrator
/// restarts the process for its dependency's refusal, which is why the docs
/// decorate readiness checks, never liveness ones.
#[tokio::test(start_paused = true)]
async fn a_liveness_indicator_its_policy_refuses_fails_the_liveness_probe() {
    an_indicator_refused_answers_down_on_its_probe_alone::<RefusedLivenessModule>(
        ProbeKind::Liveness,
        ("LivenessIndicator", "dependency_alive"),
        async |container| {
            indicator_of::<LivenessIndicator>(container)
                .dependency_alive()
                .await
        },
    )
    .await;
}

/// A refused startup check holds the startup probe down, and the others up.
#[tokio::test(start_paused = true)]
async fn a_startup_indicator_its_policy_refuses_answers_down_on_the_startup_probe_alone() {
    an_indicator_refused_answers_down_on_its_probe_alone::<RefusedStartupModule>(
        ProbeKind::Startup,
        ("StartupIndicator", "dependency_started"),
        async |container| {
            indicator_of::<StartupIndicator>(container)
                .dependency_started()
                .await
        },
    )
    .await;
}

/// The host of indicators `H` `container` holds.
fn indicator_of<H: Send + Sync + 'static>(container: &Container) -> Arc<H> {
    container.get::<H>().expect("provided")
}
