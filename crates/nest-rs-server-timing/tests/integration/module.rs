//! Covers `src/module.rs` — what each profile makes of the bare import, and
//! the two openings outside development: the variable and the pin.

use nest_rs_core::module;
use nest_rs_server_timing::{ServerTimingConfig, ServerTimingModule};
use nest_rs_testing::LogCapture;

use crate::{BareModule, ProbeController, in_profile, probe_timing};

#[test]
fn the_bare_import_stamps_nothing_outside_development() {
    for profile in ["production", "staging"] {
        assert_eq!(
            in_profile(profile, &[], probe_timing::<BareModule>),
            None,
            "a {profile} process sends no backend timings unless it says so",
        );
    }
}

#[test]
fn the_bare_import_stamps_every_answer_in_development() {
    assert!(in_profile("development", &[], probe_timing::<BareModule>).is_some());
}

#[test]
fn the_variable_opens_it_in_production() {
    let enabled = [(nest_rs_config::var_name("server_timing", "ENABLED"), "true")];
    assert!(
        in_profile("production", &enabled, probe_timing::<BareModule>).is_some(),
        "the deployment's one visible line turns it on",
    );
}

#[module(
    imports = [ServerTimingModule::for_root(ServerTimingConfig { enabled: true })],
    providers = [ProbeController],
)]
struct PinnedModule;

#[test]
fn a_pin_opens_it_in_production_and_the_boot_says_so() {
    let (timing, logs) = in_profile("production", &[], || async {
        let logs = LogCapture::install();
        (probe_timing::<PinnedModule>().await, logs)
    });
    assert!(timing.is_some(), "the pin is the opening, written in code");
    let event = logs.expect_one(
        nest_rs_server_timing::TARGET,
        "Server-Timing is enabled outside a dev profile",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("environment").as_deref(), Some("production"));
}

#[test]
fn the_variable_closes_a_pin() {
    let disabled = [(
        nest_rs_config::var_name("server_timing", "ENABLED"),
        "false",
    )];
    assert_eq!(
        in_profile("development", &disabled, probe_timing::<PinnedModule>),
        None,
        "the deployment outranks the pin, field by field",
    );
}
