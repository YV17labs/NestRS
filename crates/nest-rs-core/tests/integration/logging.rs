//! Covers `src/logging.rs` — the console settings the kernel's subscriber reads
//! at boot. They are read from the process environment once per process, so
//! each case runs in a [`ChildProcess`] of its own.

use std::time::Duration;

use nest_rs_core::logging::var;
use nest_rs_core::{App, EnvPrefix, module};

use crate::ChildProcess;

const CHILD_TEST: &str = "logging::logging_child_process";

/// What a child prints once its boot returned an app.
const BOOTED: &str = "LOGGING-CHILD BOOTED";

/// What a child prints, before the error's chain, once its boot refused.
const REFUSED: &str = "LOGGING-CHILD REFUSED";

#[module]
struct Bare;

#[test]
fn logging_child_process() {
    if crate::child_role().as_deref() == Some("boot") {
        boot();
    }
}

#[expect(
    clippy::print_stdout,
    reason = "the child's stdout is the protocol its parent test reads"
)]
fn boot() {
    match App::new::<Bare>() {
        Ok(_) => println!("{BOOTED}"),
        Err(error) => println!("{REFUSED}: {error:#}"),
    }
}

/// The three settings, each valid, with `overrides` written over them.
fn settings(overrides: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut set = vec![
        (var::FILTER, "info"),
        (var::FORMAT, "text"),
        (var::SOURCE_LOCATION, "false"),
    ];
    for (tail, value) in overrides {
        if let Some(slot) = set.iter_mut().find(|(name, _)| name == tail) {
            slot.1 = value;
        }
    }
    set.into_iter()
        .map(|(tail, value)| (EnvPrefix::var(tail), value.to_owned()))
        .collect()
}

/// What the child's boot said under `overrides`.
fn booted_under(overrides: &[(&str, &str)]) -> String {
    let mut child = ChildProcess::spawn_with(CHILD_TEST, "boot", &settings(overrides));
    let (code, printed) = child.exit_within(Duration::from_secs(10));
    assert_eq!(code, Some(0), "the child ran: {printed:#?}");
    printed
        .into_iter()
        .find(|line| line.starts_with(BOOTED) || line.starts_with(REFUSED))
        .unwrap_or_else(|| panic!("the child said how its boot went"))
}

#[test]
fn every_setting_valid_boots() {
    assert_eq!(booted_under(&[]), BOOTED);
}

#[test]
fn a_log_format_that_does_not_parse_stops_the_boot_naming_the_variable_and_not_the_value() {
    let said = booted_under(&[(var::FORMAT, "jsno")]);

    assert!(said.starts_with(REFUSED), "{said}");
    assert!(said.contains(&EnvPrefix::var(var::FORMAT)), "{said}");
    assert!(said.contains("must be `text` or `json`"), "{said}");
    assert!(!said.contains("jsno"), "the value is never quoted: {said}");
}

#[test]
fn a_source_location_that_is_not_a_boolean_stops_the_boot_naming_the_variable() {
    let said = booted_under(&[(var::SOURCE_LOCATION, "ture")]);

    assert!(said.starts_with(REFUSED), "{said}");
    assert!(
        said.contains(&EnvPrefix::var(var::SOURCE_LOCATION)),
        "{said}"
    );
    assert!(!said.contains("ture"), "the value is never quoted: {said}");
}

#[test]
fn a_filter_that_does_not_parse_stops_the_boot_without_quoting_the_directive() {
    let said = booted_under(&[(var::FILTER, "billing=notalevel")]);

    assert!(said.starts_with(REFUSED), "{said}");
    assert!(said.contains(&EnvPrefix::var(var::FILTER)), "{said}");
    assert!(
        !said.contains("notalevel"),
        "the value is never quoted: {said}"
    );
    assert!(
        !said.contains("billing"),
        "the value is never quoted: {said}"
    );
}
