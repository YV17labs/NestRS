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
        ran: Mutex<Vec<String>>,
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
                report.details.keys().map(ToString::to_string).collect();
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

/// An indicator a module attaches to the container — a resource the framework
/// opens, once per instance — beside one `#[indicators]` submits to the
/// link-time registry: one probe runs both, and one name check covers both.
mod attached_by_a_module {
    use nest_rs_core::{App, Container, ContainerBuilder, Module, Registering, injectable, module};
    use nest_rs_health::{
        HealthModule, HealthService, IndicatorEntry, IndicatorFuture, IndicatorStatus, ProbeKind,
        indicators,
    };

    fn answers(_: &Container) -> IndicatorFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    /// What a resource's health binding is: a module attaching its entry.
    struct PoolHealthModule;

    impl Module for PoolHealthModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide_meta(IndicatorEntry::new::<Self>(
                "pool",
                ProbeKind::Readiness,
                answers,
            ))
        }
    }

    #[injectable]
    #[derive(Default)]
    struct Upstream;

    #[indicators]
    impl Upstream {
        #[readiness]
        async fn upstream(&self) {}
    }

    #[module(imports = [HealthModule, PoolHealthModule], providers = [Upstream])]
    struct BothKinds;

    #[tokio::test]
    async fn an_attached_indicator_and_a_decorated_one_run_in_one_probe() {
        let app = App::builder()
            .module::<BothKinds>()
            .build()
            .await
            .unwrap_or_else(|refused| panic!("boots: {refused:#}"));
        let health = app
            .container()
            .get::<HealthService>()
            .expect("HealthModule provides it");

        let ready = health.probe(ProbeKind::Readiness).await;
        assert_eq!(ready.status, IndicatorStatus::Up);
        assert_eq!(
            ready
                .details
                .keys()
                .map(|key| &**key)
                .collect::<Vec<&str>>(),
            ["pool", "upstream"],
            "{ready:#?}"
        );
        assert!(
            health.probe(ProbeKind::Startup).await.details.is_empty(),
            "each entry answers its own probe alone"
        );
    }

    /// A resource's binding claiming the name an app's own method claims.
    struct ClashingHealthModule;

    impl Module for ClashingHealthModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide_meta(IndicatorEntry::new::<Self>(
                "cache",
                ProbeKind::Readiness,
                answers,
            ))
        }
    }

    #[injectable]
    #[derive(Default)]
    struct HandWrittenCache;

    #[indicators]
    impl HandWrittenCache {
        #[readiness]
        async fn cache(&self) {}
    }

    #[module(
        imports = [HealthModule, ClashingHealthModule],
        providers = [HandWrittenCache],
    )]
    struct Clashing;

    #[tokio::test]
    async fn an_attached_and_a_decorated_indicator_on_one_name_fail_the_boot_naming_both() {
        let Err(refused) = App::builder().module::<Clashing>().build().await else {
            panic!("two verdicts under one key must not boot");
        };
        let sentence = format!("{refused:#}");
        for named in ["\"cache\"", "ClashingHealthModule", module_path!()] {
            assert!(
                sentence.contains(named),
                "the refusal names {named}: {sentence}"
            );
        }
    }
}
