//! The subscriber `init` installs, `Metrics`' provider, and the OTLP
//! exporters' client.
//! nextest runs every test in a process of its own, so a test that initialises
//! OpenTelemetry never does it for another.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod init;
#[cfg(feature = "otlp")]
mod linker;
#[cfg(feature = "otlp")]
mod metrics;
#[cfg(feature = "otlp")]
mod module;
#[cfg(feature = "otlp")]
mod otlp;
#[cfg(feature = "otlp")]
mod process_metrics;
#[cfg(feature = "otlp")]
mod sampler;
#[cfg(feature = "otlp")]
mod unit_durations;
#[cfg(feature = "otlp")]
mod unit_records;
#[cfg(feature = "otlp")]
mod unit_spans;

use std::process::{Command, Output};

use nest_rs_core::EnvPrefix;
use nest_rs_core::logging::var;
use nest_rs_opentelemetry::{OpenTelemetry, OpenTelemetryConfig, OpenTelemetryError};

/// The variable a child reads its role from.
const CHILD_ROLE: &str = "NEST_RS_OPENTELEMETRY_CHILD_ROLE";

/// The role a parent test started this process in, if one did.
#[expect(
    clippy::disallowed_methods,
    reason = "the parent test hands the child its role through the environment"
)]
fn child_role() -> Option<String> {
    std::env::var(CHILD_ROLE).ok()
}

/// This test binary, re-run on the test `child` alone in `role`, with `vars`
/// set, to its exit, every one of [`console_vars`] removed.
fn child_running(child: &str, role: &str, vars: &[(&str, &str)]) -> Output {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args(["--exact", child, "--nocapture"])
        .env(CHILD_ROLE, role);
    for name in console_vars() {
        command.env_remove(name);
    }
    for (name, value) in vars {
        command.env(name, value);
    }
    command.output().expect("the child runs")
}

/// Every variable the console reads, which a test clears so that it reads what
/// it pinned and set, whatever the shell running it exports.
fn console_vars() -> [String; 3] {
    [
        "RUST_LOG".to_owned(),
        EnvPrefix::var(var::FORMAT),
        EnvPrefix::var(var::SOURCE_LOCATION),
    ]
}

/// `config` initialised with `vars` set over the process's environment, every
/// one of [`console_vars`] blanked first.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn init_under(
    vars: &[(&str, &str)],
    config: OpenTelemetryConfig,
) -> Result<OpenTelemetry, OpenTelemetryError> {
    let mut init = None;
    figment::Jail::expect_with(|jail| {
        for name in console_vars() {
            jail.set_env(name, "");
        }
        for (key, value) in vars {
            jail.set_env(key, value);
        }
        init = Some(OpenTelemetry::init_with(config));
        Ok(())
    });
    init.expect("the jail ran")
}

/// What refuses `config` at boot under `vars`, a configuration error, as said.
fn refused_under(vars: &[(&str, &str)], config: OpenTelemetryConfig) -> String {
    let Err(refused) = init_under(vars, config) else {
        panic!("{vars:?} must not boot");
    };
    assert!(
        matches!(refused, OpenTelemetryError::Config(_)),
        "{refused:?}"
    );
    refused.to_string()
}
