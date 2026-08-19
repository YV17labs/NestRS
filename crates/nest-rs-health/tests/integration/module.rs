use std::sync::Arc;

use nest_rs_core::{Container, Module};
use nest_rs_health::{HealthModule, HealthService};

#[test]
fn registers_health_service() {
    let container = HealthModule::register(Container::builder()).build();
    let svc: Option<Arc<HealthService>> = container.get();
    assert!(svc.is_some());
}

/// The `for_root` seam, booted — the composition witness *Shipping a new
/// capability* asks every seam for. It proves what `registers_health_service`
/// cannot: that the pinned ceilings survive the boot and reach the container a
/// probe reads them from.
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

        // Pinning configures; it never replaces the wiring the bare import
        // brings, so the probes answer exactly as they do without it.
        app.http()
            .get("/health/ready")
            .send()
            .await
            .assert_status_is_ok();
    }
}
