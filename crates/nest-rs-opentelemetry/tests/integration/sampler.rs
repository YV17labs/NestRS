//! Covers `src/sampler.rs` as `init` installs it: a health probe that starts a
//! trace starts it unsampled, its duration still counted, and one a trusted
//! proxy continued keeps its parent's decision.

use nest_rs_core::{Correlation, module};
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule, HttpSetup};
use nest_rs_testing::TestApp;

use crate::otlp::{Collector, Received, metric_exports, trace_exports, trusting};

/// The probe a kubelet calls most, as `HealthModule` serves it.
const LIVENESS: &str = "/health/live";

/// The specification's own example, sampled.
const SAMPLED_PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

fn behind_loopback() -> HttpSetup {
    HttpModule::for_root(HttpConfig {
        trusted_proxies: vec!["127.0.0.1".parse().expect("a proxy entry")],
        ..HttpConfig::default()
    })
}

#[module(imports = [behind_loopback(), HealthModule])]
struct Probed;

/// What the collector read once `call` ran against the booted app, the guard
/// flushing as it drops; a unit of the probe package's opened beside it shows
/// the export ran.
fn exported_after(call: impl AsyncFnOnce(&TestApp)) -> (Vec<Received>, Vec<Received>) {
    let collector = Collector::serving();
    let otel = trusting(&collector, &[("OTEL_LOGS_EXPORTER", "none")]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    runtime.block_on(async {
        let app = TestApp::for_module::<Probed>()
            .await
            .expect("the probes boot");
        call(&app).await;
    });
    drop(runtime);
    drop(nestrs_unit_probe::open(&Correlation::minted(None)));
    drop(otel);
    (trace_exports(&collector), metric_exports(&collector))
}

#[test]
fn a_probe_starting_a_trace_exports_no_span_and_still_counts_its_duration() {
    let (traces, metrics) = exported_after(async |app| {
        app.http().get(LIVENESS).send().await.assert_status_is_ok();
    });

    assert!(
        traces
            .iter()
            .any(|export| export.holds(nestrs_unit_probe::WORK.name())),
        "the export ran: {traces:?}",
    );
    assert!(
        !traces.iter().any(|export| export.holds(LIVENESS)),
        "the probe's trace started unsampled: {traces:?}",
    );
    assert!(
        metrics.iter().any(|export| {
            export.holds("http.server.request.duration") && export.holds(LIVENESS)
        }),
        "the probe's duration was counted: {metrics:?}",
    );
}

#[test]
fn a_probe_a_trusted_proxy_continued_sampled_is_exported() {
    let (traces, _) = exported_after(async |app| {
        app.http()
            .get(LIVENESS)
            .peer_addr("127.0.0.1:40000".parse().expect("a socket address"))
            .header("traceparent", SAMPLED_PARENT)
            .send()
            .await
            .assert_status_is_ok();
    });

    assert!(
        traces.iter().any(|export| export.holds(LIVENESS)),
        "the parent's decision stood: {traces:?}",
    );
}
