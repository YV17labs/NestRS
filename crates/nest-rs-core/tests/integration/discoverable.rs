//! Covers `src/discoverable.rs` — an inert host said at the level its cause
//! earns, read off the composition.

use nest_rs_core::{App, module, target};
use nest_rs_testing::LogCapture;
use nestrs_hooks_probe::HooksProbe;

#[module]
struct WithoutTheProbeModule;

/// The probe is a dependency like the framework's own crates: the cause is the
/// module this app does not import, whatever the crate is called.
#[tokio::test]
async fn a_dependencys_hooks_host_this_app_does_not_import_is_said_at_debug() {
    let logs = LogCapture::install();
    let app = App::builder()
        .module::<WithoutTheProbeModule>()
        .build()
        .await
        .expect("an empty module boots");
    app.init().await.expect("the init phases drain");

    let host = std::any::type_name::<HooksProbe>();
    let probe: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| {
            event.target == target::LIFECYCLE
                && event
                    .field("origin")
                    .is_some_and(|origin| host.starts_with(&origin))
        })
        .collect();
    assert_eq!(probe.len(), 1, "one line for the probe's hook: {probe:#?}");
    assert_eq!(probe[0].level, "debug");
    assert_eq!(
        probe[0].message,
        "skipped lifecycle hook: not this app's to run"
    );
    assert_eq!(probe[0].field("cause").as_deref(), Some("dependency"));
    assert!(
        probe[0]
            .field("hint")
            .is_some_and(|hint| hint.contains("`HooksProbeModule`")),
        "{:?}",
        probe[0].fields,
    );
}
