use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_health::{HealthModule, HealthService};

#[test]
fn registers_health_service() {
    let container = Container::builder().import::<HealthModule>().build();
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
        HealthModule::for_root(
            HealthConfig::default()
                .with_indicator_timeout(Duration::from_millis(120))
                .with_probe_deadline(Duration::from_millis(250)),
        )
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
