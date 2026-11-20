//! Covers `src/module.rs`: the wiring the docs page shows boots — a bare import
//! per instance, `for_root` pinning one — every module importing an instance
//! shares its one policy, and a provider injecting a policy no module builds is
//! refused naming the import.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{MissingDependencyError, injectable, instance, module};
use nest_rs_resilience::{ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy};
use nest_rs_testing::HeadlessApp;

use crate::{held, quiet};

#[instance]
struct Upstream;

#[instance]
struct Carrier;

#[injectable]
struct PreviewService {
    #[inject]
    upstream: Arc<ResiliencePolicy<Upstream>>,
}

#[injectable]
struct TrackingService {
    #[inject]
    upstream: Arc<ResiliencePolicy<Upstream>>,
    #[inject]
    carrier: Arc<ResiliencePolicy<Carrier>>,
}

#[module(imports = [ResilienceModule<Upstream>], providers = [PreviewService])]
struct PreviewsModule;

#[module(
    imports = [
        ResilienceModule<Upstream>,
        ResilienceModule::for_root((Carrier, ResilienceConfig {
            timeout: Duration::from_millis(100),
            ..quiet()
        })),
    ],
    providers = [TrackingService],
)]
struct TrackingModule;

#[module(imports = [PreviewsModule, TrackingModule])]
struct AppModule;

async fn booted() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<AppModule>()
        .build_headless()
        .await
        .expect("the documented wiring boots")
}

#[tokio::test]
async fn every_module_importing_an_instance_shares_its_one_policy() {
    let app = booted().await;
    let previews = app.container().get::<PreviewService>().expect("provided");
    let tracking = app.container().get::<TrackingService>().expect("provided");
    assert!(
        Arc::ptr_eq(&previews.upstream, &tracking.upstream),
        "one breaker for every caller of the dependency",
    );
    assert!(Arc::ptr_eq(&tracking.upstream, &held::<Upstream>(&app)));
}

#[tokio::test(start_paused = true)]
async fn a_pinned_instance_runs_on_its_pin_and_a_bare_one_on_the_defaults() {
    let app = booted().await;
    let tracking = app.container().get::<TrackingService>().expect("provided");

    let given_up = tracking
        .carrier
        .run(|_| std::future::pending::<Result<(), std::io::Error>>())
        .await;
    assert!(
        matches!(
            given_up,
            Err(ResilienceError::TimedOut { timeout, .. }) if timeout == Duration::from_millis(100)
        ),
        "{given_up:?}"
    );

    let started = tokio::time::Instant::now();
    let given_up = tracking
        .upstream
        .run(|_| std::future::pending::<Result<(), std::io::Error>>())
        .await;
    assert!(
        matches!(given_up, Err(ResilienceError::TimedOut { .. })),
        "{given_up:?}"
    );
    assert_eq!(
        started.elapsed(),
        ResilienceConfig::default().timeout,
        "the bare import reads the defaults"
    );
}

#[injectable]
struct Unwired {
    #[inject]
    _upstream: Arc<ResiliencePolicy<Upstream>>,
}

#[module(providers = [Unwired])]
struct NoPolicyModule;

#[tokio::test]
async fn a_policy_no_module_builds_is_refused_naming_its_import() {
    let Err(refused) = HeadlessApp::builder()
        .module::<NoPolicyModule>()
        .build_headless()
        .await
    else {
        panic!("a policy nothing builds must not boot");
    };
    let missing = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("a named unmet dependency, got: {refused:#}"));
    assert!(
        missing.remedy.as_deref().is_some_and(|remedy| {
            remedy.contains("`nest_rs::resilience::ResilienceModule<Upstream>`")
        }),
        "{missing}"
    );
}

#[module(imports = [ResilienceModule::for_root(ResilienceConfig {
    backoff_first: Duration::from_secs(10),
    ..ResilienceConfig::default()
})])]
struct NeverWaitedModule;

#[tokio::test]
async fn a_combination_that_cannot_work_refuses_the_boot_naming_its_variables() {
    let Err(refused) = HeadlessApp::builder()
        .module::<NeverWaitedModule>()
        .build_headless()
        .await
    else {
        panic!("a first wait above the ceiling must not boot");
    };
    let said = format!("{refused:#}");
    assert!(
        said.contains(&nest_rs_config::var_name("resilience", "BACKOFF_FIRST_MS"))
            && said.contains(&nest_rs_config::var_name(
                "resilience",
                "BACKOFF_CEILING_MS"
            )),
        "{said}"
    );
}
