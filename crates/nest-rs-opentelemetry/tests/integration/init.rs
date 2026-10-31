//! Covers `src/init.rs` — the subscriber `OpenTelemetry::init_with` installs as
//! the process's global one, which the panic hook `#[nest_rs::main]` installs
//! files through. Process-wide, so the case runs in a child process.

use std::process::{Command, Output};

use nest_rs_core::EnvPrefix;
use nest_rs_core::logging::var;
use nest_rs_opentelemetry::{LogFormat, OpenTelemetry, OpenTelemetryConfig, OpenTelemetryError};

use crate::{console_vars, init_under};

const CHILD_TEST: &str = "init::init_child_process";

/// The variable a child reads its role from.
const CHILD_ROLE: &str = "NEST_RS_OPENTELEMETRY_CHILD_ROLE";

/// A value whose text panics as the subscriber writes it.
struct Unwritable;

impl std::fmt::Display for Unwritable {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("the field's text panicked")
    }
}

#[nest_rs_core::main]
async fn inside_the_subscriber() -> nest_rs_core::anyhow::Result<()> {
    let _otel = OpenTelemetry::init_with(OpenTelemetryConfig {
        log_format: LogFormat::Json,
        log_filter: "info".into(),
        ..OpenTelemetryConfig::default()
    })?;
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

/// This test binary, re-run on the child test alone, to its exit, every one of
/// [`console_vars`] removed.
fn child() -> Output {
    let mut child = Command::new(std::env::current_exe().expect("the test binary"));
    child
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ROLE, "inside-the-subscriber");
    for name in console_vars() {
        child.env_remove(name);
    }
    child.output().expect("the child runs")
}

/// The production shape with an exporter's subscriber: OpenTelemetry's is the
/// global one, and a panic while it is still writing the line that panicked
/// never re-enters it — the hook says it in one line on stderr, and the
/// process exits.
#[test]
fn a_panic_inside_the_subscriber_s_own_dispatch_is_one_line_on_stderr() {
    let output = child();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(101),
        "a panic's status: {stdout} {stderr}"
    );
    let errors = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["level"] == "ERROR")
        .count();
    assert_eq!(errors, 0, "nothing re-entered the subscriber: {stdout}");
    let said: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("nestrs: panicked at "))
        .collect();
    assert_eq!(said.len(), 1, "one line, the hook's: {stderr}");
    assert!(said[0].contains("the field's text panicked"), "{}", said[0]);
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

    let first = OpenTelemetry::init_with(OpenTelemetryConfig::default());
    assert!(first.is_ok(), "{:?}", first.err());
    let second = OpenTelemetry::init_with(OpenTelemetryConfig::default());
    assert!(
        matches!(second, Err(OpenTelemetryError::Init(_))),
        "{:?}",
        second.err()
    );
}

/// The OTLP exporter quotes its request's URL, whose userinfo or query may
/// carry a collector's credential, and the collector's error body, both at
/// `debug`: under a bare `trace` the subscriber `init_with` installs writes
/// neither, and the SDK's own lines stay open.
#[cfg(feature = "otlp")]
#[test]
fn init_with_clamps_the_exporter_s_debug_lines_under_a_bare_trace() {
    let _otel = init_under(
        &[],
        OpenTelemetryConfig {
            log_filter: "trace".into(),
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");

    assert!(!tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::DEBUG));
    assert!(tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::INFO));
    assert!(tracing::event_enabled!(target: "opentelemetry_sdk", tracing::Level::DEBUG));
}

/// A directive naming the exporter replaces its clamp.
#[cfg(feature = "otlp")]
#[test]
fn init_with_writes_the_exporter_s_debug_lines_a_filter_names() {
    let _otel = init_under(
        &[],
        OpenTelemetryConfig {
            log_filter: "info,opentelemetry-otlp=debug".into(),
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");

    assert!(tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::DEBUG));
}

/// The test subscriber clamps as the app's does, whatever filter it reads.
#[cfg(feature = "otlp")]
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn the_test_subscriber_clamps_the_exporter_s_debug_lines_under_a_bare_trace() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(
            nest_rs_core::EnvPrefix::var(nest_rs_core::logging::var::FILTER),
            "trace",
        );
        nest_rs_opentelemetry::__private::init_for_tests();
        Ok(())
    });

    assert!(!tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::DEBUG));
    assert!(tracing::event_enabled!(target: "opentelemetry_sdk", tracing::Level::TRACE));
}

/// A clamp narrows the filter it is given: under `warn`, stricter than the
/// exporter's `info` clamp, the exporter's `info` lines stay closed, and the
/// process's `tracing` and `log` levels are the filter's.
#[cfg(feature = "otlp")]
#[test]
fn init_with_never_opens_what_a_stricter_filter_closes() {
    let _otel = init_under(
        &[],
        OpenTelemetryConfig {
            log_filter: "warn".into(),
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");

    assert!(!tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::INFO));
    assert!(tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::WARN));
    assert_eq!(
        tracing::level_filters::LevelFilter::current(),
        tracing::level_filters::LevelFilter::WARN
    );
    assert_eq!(log::max_level(), log::LevelFilter::Warn);
}

/// A filter naming one target leaves every clamped library closed.
#[cfg(feature = "otlp")]
#[test]
fn init_with_a_filter_naming_one_target_writes_no_clamped_library() {
    let _otel = init_under(
        &[],
        OpenTelemetryConfig {
            log_filter: "billing=debug".into(),
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");

    assert!(tracing::event_enabled!(target: "billing::service", tracing::Level::DEBUG));
    assert!(!tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::ERROR));
}

/// The test subscriber's own default, `warn`, is stricter than the exporter's
/// clamp, and stays the process's level.
#[cfg(feature = "otlp")]
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn the_test_subscriber_s_default_never_opens_a_clamped_library() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        nest_rs_opentelemetry::__private::init_for_tests();
        Ok(())
    });

    assert!(!tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::INFO));
    assert!(tracing::event_enabled!(target: "opentelemetry-otlp", tracing::Level::WARN));
    assert_eq!(
        tracing::level_filters::LevelFilter::current(),
        tracing::level_filters::LevelFilter::WARN
    );
}

/// A config pinned in code is the base the console's variables overlay, field
/// by field, as from `init()`: the deployment raises a pinned filter.
#[test]
fn init_with_lays_the_console_s_variables_over_a_pinned_config() {
    let filter = EnvPrefix::var(var::FILTER);
    let _otel = init_under(
        &[(filter.as_str(), "debug")],
        OpenTelemetryConfig {
            log_filter: "warn".into(),
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");

    assert_eq!(
        tracing::level_filters::LevelFilter::current(),
        tracing::level_filters::LevelFilter::DEBUG,
        "the deployment's filter, over the one pinned in code"
    );
    assert!(tracing::event_enabled!(target: "billing::service", tracing::Level::DEBUG));
}

/// The boot line says why a signal exports nothing: its selector switched it
/// off, or nothing names an endpoint for it.
#[cfg(feature = "otlp")]
#[test]
fn the_boot_line_tells_a_signal_switched_off_from_one_with_no_endpoint() {
    let logs = nest_rs_testing::LogCapture::install();
    let otel = init_under(
        &[
            (
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "http://127.0.0.1:4318/v1/traces",
            ),
            (
                "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
                "http://127.0.0.1:4318/v1/metrics",
            ),
            ("OTEL_TRACES_EXPORTER", "none"),
        ],
        OpenTelemetryConfig::default(),
    )
    .expect("the process's first subscriber");

    let event = logs.expect_one(nest_rs_opentelemetry::TARGET, "OpenTelemetry initialised");
    assert_eq!(
        event.field("traces").as_deref(),
        Some("off (OTEL_TRACES_EXPORTER=none)")
    );
    assert_eq!(
        event.field("metrics").as_deref(),
        Some("http://127.0.0.1:4318/v1/metrics")
    );
    assert_eq!(event.field("logs").as_deref(), Some("off (no endpoint)"));
    drop(otel);
}

#[nest_rs_core::main]
async fn initialised_under_main() -> nest_rs_core::anyhow::Result<()> {
    let _otel = OpenTelemetry::init()?;
    Ok(())
}

/// `init` takes no argument: the service is the binary `#[nest_rs::main]`
/// started, read off its manifest.
#[test]
fn init_names_the_service_after_the_binary_main_started() {
    let logs = nest_rs_testing::LogCapture::install();

    initialised_under_main().expect("the process's first subscriber");

    let event = logs.expect_one(nest_rs_opentelemetry::TARGET, "OpenTelemetry initialised");
    assert_eq!(
        event.field("service").as_deref(),
        Some(env!("CARGO_PKG_NAME"))
    );
}

/// A binary built without the `otlp` feature builds no exporter, so a selector
/// naming one is refused naming the variable and the feature, never the value,
/// where a deployment would believe a signal ships; `none` boots.
#[cfg(not(feature = "otlp"))]
#[test]
fn without_the_otlp_feature_a_selector_naming_an_exporter_is_refused_and_none_boots() {
    for (variable, value) in [
        ("OTEL_TRACES_EXPORTER", "OTLP"),
        ("OTEL_METRICS_EXPORTER", "zipkin"),
        ("OTEL_LOGS_EXPORTER", "none,console"),
    ] {
        let Err(refused) = init_under(&[(variable, value)], OpenTelemetryConfig::default()) else {
            panic!("{variable}={value} must not boot a binary built without the otlp feature");
        };
        assert!(
            matches!(refused, OpenTelemetryError::Config(_)),
            "{refused:?}"
        );
        let said = refused.to_string();
        assert!(
            said.contains(variable) && said.contains("without the otlp feature"),
            "{said}"
        );
        assert!(!said.contains(value), "{said}");
    }

    let _otel = init_under(
        &[
            ("OTEL_TRACES_EXPORTER", "none"),
            ("OTEL_METRICS_EXPORTER", " None "),
            ("OTEL_LOGS_EXPORTER", "none,none"),
        ],
        OpenTelemetryConfig::default(),
    )
    .expect("none is what a binary built without the otlp feature exports");
}
