//! `OpenTelemetryModule`'s boot contract, and the OTLP exporters' client.
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
mod metrics;
mod module;
#[cfg(feature = "otlp")]
mod otlp;

use nest_rs_core::EnvPrefix;
use nest_rs_core::logging::var;
use nest_rs_opentelemetry::{OpenTelemetry, OpenTelemetryConfig, OpenTelemetryError};

/// `config` initialised with `vars` set over the process's environment, every
/// variable the console reads blanked first: a test reads what it pinned and
/// set, whatever the shell running it exports.
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
        for name in [
            EnvPrefix::var(var::FILTER),
            "RUST_LOG".to_owned(),
            EnvPrefix::var(var::FORMAT),
            EnvPrefix::var(var::SOURCE_LOCATION),
        ] {
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
