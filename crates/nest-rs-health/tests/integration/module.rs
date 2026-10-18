use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_health::{HealthConfig, HealthModule, HealthService};

#[test]
fn registers_health_service() {
    let container = Container::builder()
        .provide(HealthConfig::default())
        .import::<HealthModule>()
        .build()
        .expect("the container builds");
    let svc: Option<Arc<HealthService>> = container.get();
    assert!(svc.is_some());
}

/// The `for_root` seam, booted.
mod for_root {
    use nest_rs_core::module;
    use nest_rs_health::{HealthConfig, HealthModule};
    use nest_rs_testing::TestApp;
    use std::time::Duration;

    fn pinned_health() -> nest_rs_health::HealthSetup {
        HealthModule::for_root(HealthConfig {
            indicator_timeout_ms: 120,
            probe_deadline_ms: 250,
        })
    }

    #[module(imports = [pinned_health()])]
    struct PinnedHealthApp;

    #[tokio::test]
    async fn for_root_pins_both_ceilings_and_still_mounts_the_probes() {
        let app = TestApp::for_module::<PinnedHealthApp>()
            .await
            .expect("the pinned wiring boots");

        let config = app
            .container()
            .get::<HealthConfig>()
            .expect("for_root resolves the config into the container");
        assert_eq!(config.indicator_timeout(), Duration::from_millis(120));
        assert_eq!(config.probe_deadline(), Duration::from_millis(250));

        app.http()
            .get("/health/ready")
            .send()
            .await
            .assert_status_is_ok();
    }
}

/// The probe an init hook runs: the wiring step filled the registry before the
/// first hook, so the hook's probe runs the app's indicators.
mod probed_from_an_init_hook {
    use std::sync::{Arc, Mutex};

    use nest_rs_core::{App, hooks, injectable, module};
    use nest_rs_health::{HealthModule, HealthService, ProbeKind, indicators};

    #[injectable]
    #[derive(Default)]
    struct Warmup;

    #[indicators]
    impl Warmup {
        #[readiness]
        async fn cache_warm(&self) -> Result<(), std::io::Error> {
            Ok(())
        }
    }

    /// What the hook saw: the indicator names its probe ran.
    #[injectable]
    #[derive(Default)]
    struct ProbedAtInit {
        ran: Mutex<Vec<&'static str>>,
    }

    #[injectable]
    struct ReadinessGate {
        #[inject]
        svc: Arc<HealthService>,
        #[inject]
        seen: Arc<ProbedAtInit>,
    }

    #[hooks]
    impl ReadinessGate {
        #[on_module_init]
        async fn wait_until_ready(&self) {
            let report = self.svc.probe(ProbeKind::Readiness).await;
            *self.seen.ran.lock().expect("only this hook writes it") =
                report.details.keys().copied().collect();
        }
    }

    #[module(
        imports = [HealthModule],
        providers = [Warmup, ProbedAtInit, ReadinessGate],
    )]
    struct GatedModule;

    #[tokio::test]
    async fn a_probe_from_an_init_hook_runs_the_reachable_indicators() {
        let app = App::builder()
            .module::<GatedModule>()
            .build()
            .await
            .expect("boots");
        app.init().await.expect("the init hooks run");

        let seen = app
            .container()
            .get::<ProbedAtInit>()
            .expect("ProbedAtInit is provided");
        assert_eq!(
            *seen.ran.lock().expect("the hook is done"),
            ["cache_warm"],
            "the probe ran the app's indicator rather than answering an empty `up`",
        );
    }
}

/// The service holds the assembled container to run indicators against; the
/// app's end hands it back, so the singletons it holds drop with the app.
mod dropped_at_the_end {
    use std::sync::atomic::{AtomicBool, Ordering};

    use nest_rs_core::{App, injectable, module};
    use nest_rs_health::HealthModule;

    static DROPPED: AtomicBool = AtomicBool::new(false);

    #[injectable]
    #[derive(Default)]
    struct HoldsASocket;

    impl Drop for HoldsASocket {
        fn drop(&mut self) {
            DROPPED.store(true, Ordering::SeqCst);
        }
    }

    #[module(imports = [HealthModule], providers = [HoldsASocket])]
    struct ProbedModule;

    #[tokio::test]
    async fn a_provider_beside_the_health_service_is_dropped_when_the_app_closes() {
        let app = App::builder()
            .module::<ProbedModule>()
            .build()
            .await
            .expect("boots");
        app.init().await.expect("the init hooks run");

        app.close().await;

        assert!(
            DROPPED.load(Ordering::SeqCst),
            "the health service no longer keeps the container, and every singleton in it, alive"
        );
    }
}
