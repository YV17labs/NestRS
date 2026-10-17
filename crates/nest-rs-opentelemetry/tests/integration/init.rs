//! Covers `src/init.rs` — the subscriber `OpenTelemetry::init_with` installs as
//! the process's global one, which the panic hook `#[nest_rs::main]` installs
//! files through. Process-wide, so the case runs in a child process.

use std::process::{Command, Output};

use nest_rs_core::panic::FIELD;
use nest_rs_opentelemetry::{LogFormat, OpenTelemetry, OpenTelemetryConfig, OpenTelemetryError};

const CHILD_TEST: &str = "init::init_child_process";

/// The variable a child reads its role from.
const CHILD_ROLE: &str = "NEST_RS_OPENTELEMETRY_CHILD_ROLE";

/// The hook's one sentence for a panic it files itself.
const NO_UNIT: &str = "panicked where no unit of work contains it";

/// A value whose text panics as the subscriber writes it.
struct Unwritable;

impl std::fmt::Display for Unwritable {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("the field's text panicked")
    }
}

#[nest_rs_core::main]
async fn inside_the_subscriber() -> nest_rs_core::anyhow::Result<()> {
    let _otel = OpenTelemetry::init_with(
        OpenTelemetryConfig::new("reentrant")
            .with_log_format(LogFormat::Json)
            .with_log_filter("info"),
    )?;
    tracing::info!(target: "nest_rs::fixture", value = %Unwritable, "never written");
    Ok(())
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the parent test hands the child its role through the environment"
)]
fn init_child_process() {
    if std::env::var(CHILD_ROLE).is_ok() {
        let _ = inside_the_subscriber();
    }
}

/// This test binary, re-run on the child test alone, to its exit.
fn child() -> Output {
    Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ROLE, "inside-the-subscriber")
        .output()
        .expect("the child runs")
}

/// The production shape with an exporter's subscriber: OpenTelemetry's is the
/// global one, and the panic hook files through it while it is still writing
/// the line that panicked — one event, the hook's, and the process exits.
#[test]
fn a_panic_inside_the_subscriber_s_own_dispatch_is_one_event_of_the_hook() {
    let output = child();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        output.status.code(),
        Some(101),
        "a panic's status: {stdout}"
    );
    let errors: Vec<serde_json::Value> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["level"] == "ERROR")
        .collect();
    assert_eq!(errors.len(), 1, "one error, the hook's: {stdout}");
    assert_eq!(errors[0]["fields"]["message"], NO_UNIT, "{}", errors[0]);
    assert_eq!(
        errors[0]["fields"][FIELD], "the field's text panicked",
        "{}",
        errors[0]
    );
}

/// A `log` logger that keeps every record to itself.
struct KeptLogger;

impl log::Log for KeptLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        false
    }

    fn log(&self, _: &log::Record<'_>) {}

    fn flush(&self) {}
}

static KEPT_LOGGER: KeptLogger = KeptLogger;

/// A `log` logger set before `init` keeps the `log` records, and `init`
/// installs its subscriber all the same; a second `init` keeps the first's.
#[test]
fn init_installs_its_subscriber_beside_a_log_logger_set_before() {
    log::set_logger(&KEPT_LOGGER).expect("the process's first `log` logger");

    let first = OpenTelemetry::init_with(OpenTelemetryConfig::new("log-logger"));
    assert!(first.is_ok(), "{:?}", first.err());
    let second = OpenTelemetry::init_with(OpenTelemetryConfig::new("log-logger"));
    assert!(
        matches!(second, Err(OpenTelemetryError::Init(_))),
        "{:?}",
        second.err()
    );
}
