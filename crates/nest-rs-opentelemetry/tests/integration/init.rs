//! Covers `src/init.rs` — the subscriber `OpenTelemetry::init_with` installs as
//! the process's global one, which the panic hook `#[nest_rs::main]` installs
//! files through, and which `App::run` tells from the kernel's console under an
//! OTLP endpoint. Process-wide, so the panic's case runs in a child process.

use std::process::{Command, Output};

use nest_rs_config::Environment;
use nest_rs_core::logging::LogFormat;
use nest_rs_core::{App, UnservedExportError, module};
use nest_rs_opentelemetry::{OpenTelemetry, OpenTelemetryConfig, OpenTelemetryError};

use crate::{console_vars, init_under, refused_under};

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

/// The role whose child writes the boot line on a text console.
const BOOT_LINE: &str = "boot-line";

/// The role whose child writes the boot line with its format left to the
/// profile.
const BOOT_LINE_BY_PROFILE: &str = "boot-line-by-profile";

/// The boot line of a console set to `log_format`, under the variables the
/// parent set.
fn boot_line(log_format: LogFormat) {
    let otel = OpenTelemetry::init_with(OpenTelemetryConfig {
        log_format,
        log_filter: "info".into(),
        ..OpenTelemetryConfig::default()
    })
    .expect("the child's subscriber");
    drop(otel);
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the parent test hands the child its role through the environment"
)]
fn init_child_process() {
    match std::env::var(CHILD_ROLE).as_deref() {
        Ok(BOOT_LINE) => boot_line(LogFormat::Text),
        Ok(BOOT_LINE_BY_PROFILE) => boot_line(LogFormat::ByProfile),
        Ok(_) => {
            let _ = inside_the_subscriber();
        }
        Err(_) => {}
    }
}

/// This test binary, re-run on the child test alone in `role`, with `vars`
/// set, to its exit, every one of [`console_vars`] removed.
fn child_in(role: &str, vars: &[(&str, &str)]) -> Output {
    let mut child = Command::new(std::env::current_exe().expect("the test binary"));
    child
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ROLE, role);
    for name in console_vars() {
        child.env_remove(name);
    }
    for (name, value) in vars {
        child.env(name, value);
    }
    child.output().expect("the child runs")
}

fn child() -> Output {
    child_in("inside-the-subscriber", &[])
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

/// A config pinned in code is the base the console's variables overlay, field
/// by field, as from `init()`: the deployment raises a pinned filter.
#[test]
fn init_with_lays_the_console_s_variables_over_a_pinned_config() {
    let _otel = init_under(
        &[("RUST_LOG", "debug")],
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

/// The boot line names the sampler as the specification does, whatever chose
/// it, and what chose it beside: the ratio pinned in code, parent-based…
#[cfg(feature = "otlp")]
#[test]
fn the_boot_line_names_a_pinned_ratio_s_sampler_and_where_it_came_from() {
    let logs = nest_rs_testing::LogCapture::install();
    let otel = init_under(
        &[],
        OpenTelemetryConfig {
            trace_sample_ratio: 0.25,
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");

    let event = logs.expect_one(nest_rs_opentelemetry::TARGET, "OpenTelemetry initialised");
    assert_eq!(
        event.field("sampler").as_deref(),
        Some("parentbased_traceidratio")
    );
    assert_eq!(
        event.field("sampler_source").as_deref(),
        Some("trace_sample_ratio")
    );
    assert_eq!(event.field("sample_ratio").as_deref(), Some("0.25"));
    drop(otel);
}

/// …or the specification's variable, whose sampler reads no ratio here.
#[cfg(feature = "otlp")]
#[test]
fn the_boot_line_names_the_sampler_the_variable_chose_and_no_ratio_it_ignores() {
    let logs = nest_rs_testing::LogCapture::install();
    let otel = init_under(
        &[("OTEL_TRACES_SAMPLER", "always_on")],
        OpenTelemetryConfig {
            trace_sample_ratio: 0.25,
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");

    let event = logs.expect_one(nest_rs_opentelemetry::TARGET, "OpenTelemetry initialised");
    assert_eq!(event.field("sampler").as_deref(), Some("always_on"));
    assert_eq!(
        event.field("sampler_source").as_deref(),
        Some("OTEL_TRACES_SAMPLER")
    );
    assert_eq!(event.field("sample_ratio"), None, "{:?}", event.fields);
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
        let said = refused_under(&[(variable, value)], OpenTelemetryConfig::default());
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

/// `OTEL_CONFIG_FILE` names a file the Rust SDK cannot read: a deployment
/// believing it configures the telemetry is refused at boot, the variable
/// named, its path never quoted.
#[test]
fn otel_config_file_is_refused_naming_it() {
    let said = refused_under(
        &[("OTEL_CONFIG_FILE", "/etc/nestrs/otel-config.yaml")],
        OpenTelemetryConfig::default(),
    );
    assert!(said.contains("OTEL_CONFIG_FILE"), "{said}");
    assert!(!said.contains("otel-config.yaml"), "{said}");
}

/// `OTEL_PROPAGATORS` naming a format the framework does not speak is refused
/// at boot, naming the variable and what it accepts, never the value.
#[test]
fn otel_propagators_naming_a_format_the_framework_does_not_speak_is_refused() {
    let said = refused_under(
        &[("OTEL_PROPAGATORS", "tracecontext,baggage,b3multi")],
        OpenTelemetryConfig::default(),
    );
    assert!(
        said.contains("OTEL_PROPAGATORS") && said.contains("tracecontext and baggage"),
        "{said}"
    );
    assert!(!said.contains("b3multi"), "{said}");
}

/// `key=value` pairs of a logfmt line, as an operator's filter reads them: a
/// value runs to the next space unless it is quoted.
fn logfmt(line: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut rest = line.trim();
    while let Some(at) = rest.find('=') {
        let key = rest[..at].rsplit(' ').next().unwrap_or_default().to_owned();
        let after = &rest[at + 1..];
        let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    other => value.push(other),
                }
            }
            (value, &quoted[end..])
        } else {
            let end = after.find(' ').unwrap_or(after.len());
            (after[..end].to_owned(), &after[end..])
        };
        pairs.push((key, value));
        rest = next.trim_start();
    }
    pairs
}

/// The boot line's per-signal state reads whole to a logfmt filter, its
/// spaces inside the value: `traces="off (OTEL_TRACES_EXPORTER=none)"`. The
/// child's stdout is a pipe, as a reader's is, so the text console writes no
/// colour escape for the reader to strip, `NO_COLOR` unset (empty, which
/// no-color.org reads as unset) or not.
#[cfg(feature = "otlp")]
#[test]
fn the_boot_line_s_signal_states_read_whole_as_logfmt() {
    let output = child_in(
        BOOT_LINE,
        &[
            ("NO_COLOR", ""),
            ("OTEL_TRACES_EXPORTER", "none"),
            (
                "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
                "http://127.0.0.1:9/v1/metrics",
            ),
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        !stdout.contains('\u{1b}'),
        "a console piped to a reader writes no colour escape: {stdout:?}"
    );
    let line = stdout
        .lines()
        .find(|line| line.contains("OpenTelemetry initialised"))
        .unwrap_or_else(|| panic!("the boot line: {stdout}"));
    let pairs = logfmt(line);
    let field = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("{name} in {line}"))
    };
    assert_eq!(field("traces"), "off (OTEL_TRACES_EXPORTER=none)", "{line}");
    assert_eq!(field("metrics"), "http://127.0.0.1:9/v1/metrics", "{line}");
    assert_eq!(field("logs"), "off (no endpoint)", "{line}");
    assert_eq!(field("mode"), "otlp", "{line}");
}

/// What a deployment exporting a signal over plain HTTP off this machine says.
#[cfg(feature = "otlp")]
const EXPORTS_IN_PLAINTEXT: &str = "a signal is exported over plain HTTP outside a dev profile: its \
     telemetry travels unencrypted";

/// The lines saying a plaintext export, in a process under `profile` exporting
/// to `endpoint`.
#[cfg(feature = "otlp")]
fn plaintext_said_under(profile: &str, endpoint: &str) -> Vec<nest_rs_testing::CapturedEvent> {
    let logs = nest_rs_testing::LogCapture::install();
    let otel = init_under(
        &[
            (Environment::var_name().as_str(), profile),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint),
        ],
        OpenTelemetryConfig::default(),
    )
    .expect("the process's first subscriber");
    // Nothing answers there, and a final flush would wait its bound.
    std::mem::forget(otel);
    logs.find(nest_rs_opentelemetry::TARGET, EXPORTS_IN_PLAINTEXT)
}

/// An `http://` endpoint declares the export in plaintext; off this machine, a
/// deployment says it once per signal, naming each signal's endpoint.
#[cfg(feature = "otlp")]
#[test]
fn a_deployment_exporting_in_plaintext_off_this_machine_says_it_per_signal() {
    let mut said: Vec<(String, String)> =
        plaintext_said_under("production", "http://192.0.2.1:4318")
            .into_iter()
            .map(|line| {
                assert_eq!(line.level, "warn");
                assert_eq!(line.field("environment").as_deref(), Some("production"));
                (
                    line.field("signal").expect("its signal"),
                    line.field("endpoint").expect("its endpoint"),
                )
            })
            .collect();
    said.sort();
    assert_eq!(
        said,
        [
            ("logs", "http://192.0.2.1:4318/v1/logs"),
            ("metrics", "http://192.0.2.1:4318/v1/metrics"),
            ("traces", "http://192.0.2.1:4318/v1/traces"),
        ]
        .map(|(signal, endpoint)| (signal.to_owned(), endpoint.to_owned()))
    );
}

/// A development or test profile's collector is no opening.
#[cfg(feature = "otlp")]
#[test]
fn a_development_profile_exporting_in_plaintext_says_nothing() {
    let said = plaintext_said_under("development", "http://192.0.2.1:4318");
    assert!(said.is_empty(), "{said:#?}");
}

/// A collector beside the app, the common plaintext shape, crosses no network.
#[cfg(feature = "otlp")]
#[test]
fn a_deployment_exporting_to_its_loopback_says_nothing() {
    let said = plaintext_said_under("production", "http://127.0.0.1:9");
    assert!(said.is_empty(), "{said:#?}");
}

/// The boot line a child in `role` writes under the variables `set`.
fn boot_line_in(role: &str, set: &[(&str, &str)]) -> String {
    let output = child_in(role, set);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{set:?}: {stdout}");
    stdout
        .lines()
        .find(|line| line.contains("OpenTelemetry initialised"))
        .unwrap_or_else(|| panic!("the boot line under {set:?}: {stdout}"))
        .to_owned()
}

/// The boot line a child in `role` writes under `profile`, `""` declaring none.
fn boot_line_under(role: &str, profile: &str) -> String {
    boot_line_in(role, &[(Environment::var_name().as_str(), profile)])
}

/// `log_format` says what the console writes, in the words a written format
/// takes, never the `ByProfile` a format left to the profile holds: the line
/// itself is the proof, written in the format it names.
#[test]
fn the_boot_line_says_the_format_the_console_writes() {
    for profile in ["development", "test"] {
        let line = boot_line_under(BOOT_LINE_BY_PROFILE, profile);
        assert!(
            serde_json::from_str::<serde_json::Value>(&line).is_err(),
            "{profile:?}: {line}"
        );
        assert!(line.contains(" log_format=\"text\""), "{profile:?}: {line}");
    }
    for profile in ["staging", "production", ""] {
        let line = boot_line_under(BOOT_LINE_BY_PROFILE, profile);
        let event: serde_json::Value = serde_json::from_str(&line)
            .unwrap_or_else(|_| panic!("a JSON line under {profile:?}: {line}"));
        assert_eq!(event["fields"]["log_format"], "json", "{profile:?}: {line}");
    }
    let written = boot_line_under(BOOT_LINE, "production");
    assert!(written.contains(" log_format=\"text\""), "{written}");
}

/// A binary built without the `otlp` feature exports nothing, so an endpoint
/// set for a signal its selector leaves on refuses the boot, naming the
/// variable and the feature; `none` lets it boot.
#[cfg(not(feature = "otlp"))]
#[test]
fn without_the_otlp_feature_an_endpoint_is_refused_and_none_boots() {
    let said = refused_under(
        &[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318")],
        OpenTelemetryConfig::default(),
    );
    assert!(
        said.contains("OTEL_EXPORTER_OTLP_ENDPOINT") && said.contains("otlp feature"),
        "{said}"
    );
    assert!(!said.contains("collector:4318"), "{said}");

    let _otel = init_under(
        &[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
            ("OTEL_TRACES_EXPORTER", "none"),
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
        OpenTelemetryConfig::default(),
    )
    .expect("every signal switched off exports nothing, as the binary does");
}

/// Every variable `App::run`'s export check reads, blanked so a test reads the
/// endpoint it set alone, whatever the shell running it exports.
const EXPORT_VARS: [&str; 7] = [
    "OTEL_SDK_DISABLED",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
    "OTEL_TRACES_EXPORTER",
    "OTEL_METRICS_EXPORTER",
    "OTEL_LOGS_EXPORTER",
];

#[module]
struct ServingNothing;

/// `App::run` on an app serving nothing, which stops for want of a transport
/// once it booted, under an OTLP endpoint no collector answers — after
/// `OpenTelemetry::init()` when `initialised`, as `main` writes it.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn run_under_an_endpoint(initialised: bool) -> nest_rs_core::anyhow::Result<()> {
    let mut ran = None;
    figment::Jail::expect_with(|jail| {
        for name in console_vars() {
            jail.set_env(name, "");
        }
        for name in EXPORT_VARS {
            jail.set_env(name, "");
        }
        jail.set_env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:9");
        let otel =
            initialised.then(|| OpenTelemetry::init().expect("the process's first subscriber"));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime the app runs on");
        ran = Some(runtime.block_on(async {
            App::builder()
                .module::<ServingNothing>()
                .build()
                .await?
                .run()
                .await
        }));
        drop(otel);
        Ok(())
    });
    ran.expect("the jail ran")
}

/// The production shape: `main` calls `init`, whose subscriber exports, so
/// `App::run` runs under the deployment's endpoint — the kernel's check reads
/// its own console alone, which `init` never installs.
#[cfg(feature = "otlp")]
#[test]
fn app_run_runs_under_an_endpoint_once_init_installed_its_exporter() {
    if let Err(refused) = run_under_an_endpoint(true) {
        panic!("init's subscriber exports, so nothing is refused: {refused:#}");
    }
}

/// A binary linking this crate whose `main` forgot `init` logs through the
/// kernel's console alone: `App::run` refuses the endpoint, naming the line.
#[test]
fn app_run_refuses_an_endpoint_when_main_never_called_init() {
    let Err(refused) = run_under_an_endpoint(false) else {
        panic!("nothing exports to the endpoint the deployment set");
    };
    assert!(
        refused.downcast_ref::<UnservedExportError>().is_some(),
        "{refused:#}"
    );
    assert!(
        format!("{refused:#}").contains("OpenTelemetry::init()"),
        "{refused:#}"
    );
}

/// The service the boot line names under the variables `set`, in a binary with
/// the exporters or without them: the two read the specification's variables
/// alike.
fn service_named_under(set: &[(&str, &str)]) -> String {
    let line = boot_line_in(BOOT_LINE, set);
    logfmt(&line)
        .into_iter()
        .find(|(key, _)| key == "service")
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("the service in {line}"))
}

#[test]
fn the_boot_line_names_the_service_the_specification_s_variables_name() {
    for (set, named) in [
        (vec![("OTEL_SERVICE_NAME", "billing")], "billing"),
        (
            vec![("OTEL_RESOURCE_ATTRIBUTES", "service.name=ledger")],
            "ledger",
        ),
        (
            vec![("OTEL_RESOURCE_ATTRIBUTES", "team=core,service.name=a%2Cb")],
            "a,b",
        ),
        (
            vec![
                ("OTEL_SERVICE_NAME", "billing"),
                ("OTEL_RESOURCE_ATTRIBUTES", "service.name=ledger"),
            ],
            "billing",
        ),
        (
            vec![
                ("OTEL_SERVICE_NAME", ""),
                ("OTEL_RESOURCE_ATTRIBUTES", "service.name=ledger"),
            ],
            "ledger",
        ),
    ] {
        assert_eq!(service_named_under(&set), named, "{set:?}");
    }
}

/// A name in code stands under variables naming none, an empty one among them,
/// which the specification reads as unset.
#[test]
fn the_boot_line_names_the_service_pinned_under_an_empty_variable() {
    let logs = nest_rs_testing::LogCapture::install();
    let otel = init_under(
        &[("OTEL_SERVICE_NAME", "")],
        OpenTelemetryConfig {
            service_name: Some("pinned".into()),
            ..OpenTelemetryConfig::default()
        },
    )
    .expect("the process's first subscriber");
    let event = logs.expect_one(nest_rs_opentelemetry::TARGET, "OpenTelemetry initialised");
    assert_eq!(event.field("service").as_deref(), Some("pinned"));
    drop(otel);
}

/// What names no service fails the boot naming where it is set, in a binary
/// with the exporters or without them.
#[test]
fn a_service_name_that_names_nothing_is_refused_naming_where_it_is_set() {
    let pinned = |name: &str| OpenTelemetryConfig {
        service_name: Some(name.to_owned()),
        ..OpenTelemetryConfig::default()
    };
    for (set, config, named) in [
        (
            vec![("OTEL_SERVICE_NAME", "   ")],
            OpenTelemetryConfig::default(),
            "OTEL_SERVICE_NAME",
        ),
        (
            vec![("OTEL_RESOURCE_ATTRIBUTES", "service.name=%20")],
            OpenTelemetryConfig::default(),
            "OTEL_RESOURCE_ATTRIBUTES",
        ),
        (
            vec![("OTEL_RESOURCE_ATTRIBUTES", "service.name")],
            OpenTelemetryConfig::default(),
            "OTEL_RESOURCE_ATTRIBUTES",
        ),
        (vec![], pinned(""), "OpenTelemetryConfig::service_name"),
        (vec![], pinned("  "), "OpenTelemetryConfig::service_name"),
    ] {
        let said = refused_under(&set, config);
        assert!(said.contains(named), "{set:?}: {said}");
    }
}
