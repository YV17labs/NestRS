//! Covers `src/logging.rs` — the console settings the kernel's subscriber reads
//! at boot, and the console it writes to under `#[nest_rs::main]`. Both are
//! process-wide, so each case runs in a [`ChildProcess`] of its own.

use std::time::Duration;

use nest_rs_core::logging::{
    CONSOLE_FLUSH_TIMEOUT, CONSOLE_QUEUE_LINES, LogSettings, install_console, var,
};
use nest_rs_core::{App, EnvPrefix, module};

use crate::ChildProcess;

const CHILD_TEST: &str = "logging::logging_child_process";

/// What a child prints once its boot returned an app.
const BOOTED: &str = "LOGGING-CHILD BOOTED";

/// What a child prints, before the error's chain, once its boot refused.
const REFUSED: &str = "LOGGING-CHILD REFUSED";

#[module]
struct Bare;

/// What a child under `#[nest_rs::main]` says on stderr — its stdout is the
/// console under test.
const CONSOLE_CHILD: &str = "CONSOLE-CHILD";

/// Its lines all filed, and the runtime still answering.
const LOGGED: &str = "CONSOLE-CHILD LOGGED";

/// Its lines not filed within the bound: the runtime waited on its stdout.
const STALLED: &str = "CONSOLE-CHILD STALLED";

/// The event the console files once it dropped lines.
const DROPPED: &str = "log lines dropped while the console was stalled";

/// Far more than a pipe and the console's queue hold together.
const LINES: usize = 100_000;

const TASKS: usize = 8;

/// Half what the console's queue holds, so a stdout nobody reads drops none.
const QUEUED_LINES: usize = CONSOLE_QUEUE_LINES / 2;

/// How long the parent leaves a child's stdout unread once its exit began:
/// well inside the bound the exit waits for it.
fn stdout_stalls_for() -> Duration {
    CONSOLE_FLUSH_TIMEOUT / 5
}

const FIXTURE_TARGET: &str = "nest_rs::fixture";

/// The line a signal exiting at once files last.
const EXITING: &str =
    "shutdown signal received on the way down: exiting at once, abandoning what still runs";

#[test]
fn logging_child_process() {
    match crate::child_role().as_deref() {
        Some("boot") => boot(),
        Some("stalled-exit") => {
            let _ = console_child("stalled-exit".to_owned());
            // libtest's own report would go to the stdout nobody reads.
            std::process::exit(0);
        }
        Some(role @ ("stalled" | "queued-exit" | "queued-signal")) => {
            let _ = console_child(role.to_owned());
        }
        Some("app-less") => {
            let _ = app_less_child();
        }
        Some("app-less-log-logger") => {
            log::set_logger(&STDERR_LOGGER).expect("the child's first `log` logger");
            log::set_max_level(log::LevelFilter::Info);
            let _ = app_less_child();
        }
        _ => {}
    }
}

#[nest_rs_core::main]
async fn console_child(role: String) -> anyhow::Result<()> {
    let app = App::new::<Bare>()?;
    match role.as_str() {
        "queued-exit" => file_lines_the_exit_writes(),
        "queued-signal" => {
            // No app left to drain, so the first signal exits at once.
            drop(app);
            file_lines_the_exit_writes();
            std::future::pending::<()>().await;
        }
        "stalled-exit" => file_lines_nobody_reads().await,
        _ => {
            file_lines_nobody_reads().await;
            wait_for_release().await;
        }
    }
    Ok(())
}

/// File [`QUEUED_LINES`], which the queue holds whatever stdout does, and say
/// so on stderr.
#[expect(
    clippy::print_stderr,
    reason = "the child's stdout is the console under test; stderr is the protocol"
)]
fn file_lines_the_exit_writes() {
    for line in 0..QUEUED_LINES {
        tracing::info!(target: FIXTURE_TARGET, line, "a line the exit writes");
    }
    eprintln!("{LOGGED}");
}

/// What an app-less child says on stderr once it installed the console twice.
const INSTALLED: &str = "APP-LESS-CHILD INSTALLED";

/// The line an app-less child files through the console it installed.
const APP_LESS_LINE: &str = "a line no app files";

/// The record an app-less child files through the `log` facade.
const LOG_LINE: &str = "a line through the log facade";

/// What a `log` logger set before the console says on stderr, before each
/// record it receives.
const LOG_LOGGER: &str = "LOG-LOGGER";

/// The event the console files when a `log` logger was set before it.
const LOG_KEPT: &str = "a `log` logger was already set: `log` records keep it, and only `tracing` events reach the subscriber installed";

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    #[expect(
        clippy::print_stderr,
        reason = "the child's stdout is the console under test; stderr is the protocol"
    )]
    fn log(&self, record: &log::Record<'_>) {
        eprintln!("{LOG_LOGGER} {}", record.args());
    }

    fn flush(&self) {}
}

static STDERR_LOGGER: StderrLogger = StderrLogger;

/// A binary that runs no `App` — a migration runner — installing the kernel's
/// console from the settings, then trying a second time.
#[nest_rs_core::main]
#[expect(
    clippy::print_stderr,
    reason = "the child's stdout is the console under test; stderr is the protocol"
)]
async fn app_less_child() -> anyhow::Result<()> {
    let settings = LogSettings::from_env()?;
    let first = install_console(&settings);
    tracing::info!(target: FIXTURE_TARGET, step = 1, "{APP_LESS_LINE}");
    log::info!(target: FIXTURE_TARGET, "{LOG_LINE}");
    let second = install_console(&settings);
    eprintln!("{INSTALLED} first={first} second={second}");
    Ok(())
}

/// File [`LINES`] from tokio tasks, then answer a trivial future, and say on
/// stderr whether all of it finished.
#[expect(
    clippy::print_stderr,
    reason = "the child's stdout is the console under test; stderr is the protocol"
)]
async fn file_lines_nobody_reads() {
    let filed = tokio::time::timeout(Duration::from_secs(10), async {
        let tasks: Vec<_> = (0..TASKS)
            .map(|task| {
                tokio::spawn(async move {
                    for line in 0..LINES / TASKS {
                        tracing::info!(target: FIXTURE_TARGET, task, line, "a line the console may drop");
                    }
                })
            })
            .collect();
        for task in tasks {
            let _ = task.await;
        }
        tokio::time::timeout(Duration::from_secs(2), tokio::spawn(async {})).await
    })
    .await;
    match filed {
        Ok(Ok(_)) => eprintln!("{LOGGED}"),
        _ => eprintln!("{STALLED}"),
    }
}

/// Until the parent writes a line to the child's stdin.
async fn wait_for_release() {
    let _ = tokio::task::spawn_blocking(|| std::io::stdin().read_line(&mut String::new())).await;
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

/// Every setting valid, the lines as JSON.
fn console_env() -> Vec<(String, String)> {
    settings(&[(var::FORMAT, "json")])
}

#[test]
fn a_stdout_nobody_reads_holds_neither_the_app_nor_its_exit() {
    let mut child = ChildProcess::spawn_unread(CHILD_TEST, "stalled-exit", &console_env());

    let said = child.expect_stderr_line(CONSOLE_CHILD, Duration::from_secs(30));
    assert_eq!(
        said, LOGGED,
        "the tasks filed every line and the runtime answered"
    );
    let (code, _) = child.exit_within(Duration::from_secs(10));
    assert_eq!(code, Some(0));
    let stderr = child.stderr();
    assert!(
        abandoned(&stderr),
        "the exit says what it could not write: {stderr:#?}"
    );
}

/// What the child's console wrote, each line a JSON event: every other line
/// on its stdout is libtest's own.
fn console_events(printed: &[String]) -> Vec<serde_json::Value> {
    printed
        .iter()
        .filter(|line| {
            !(line.is_empty() || line.starts_with("running ") || line.starts_with("test "))
        })
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|_| panic!("every console line is a JSON event: {line:?}"))
        })
        .collect()
}

/// How many of `events` carry `message`.
fn saying(events: &[serde_json::Value], message: &str) -> usize {
    events
        .iter()
        .filter(|event| event["fields"]["message"] == message)
        .count()
}

/// Whether the child said on stderr that its exit stopped waiting on stdout.
fn abandoned(stderr: &[String]) -> bool {
    stderr
        .iter()
        .any(|line| line.starts_with("nestrs: the console's last lines were abandoned"))
}

#[test]
fn a_stdout_that_reads_again_within_the_bound_gets_every_line_the_full_queue_held() {
    let mut child = ChildProcess::spawn_unread(CHILD_TEST, "stalled-exit", &console_env());

    let said = child.expect_stderr_line(CONSOLE_CHILD, Duration::from_secs(30));
    assert_eq!(said, LOGGED);
    std::thread::sleep(stdout_stalls_for());
    child.read_stdout();
    let (code, printed) = child.exit_within(Duration::from_secs(10));

    assert_eq!(code, Some(0));
    let written = saying(&console_events(&printed), "a line the console may drop");
    assert!(
        written >= CONSOLE_QUEUE_LINES,
        "the queue was full, and every line it held was written: {written}"
    );
    let stderr = child.stderr();
    assert!(!abandoned(&stderr), "{stderr:#?}");
}

#[test]
fn every_line_still_queued_when_main_returns_is_written_before_the_process_exits() {
    let mut child = ChildProcess::spawn_unread(CHILD_TEST, "queued-exit", &console_env());

    let said = child.expect_stderr_line(CONSOLE_CHILD, Duration::from_secs(30));
    assert_eq!(said, LOGGED);
    std::thread::sleep(stdout_stalls_for());
    child.read_stdout();
    let (code, printed) = child.exit_within(Duration::from_secs(10));

    assert_eq!(code, Some(0));
    let events = console_events(&printed);
    assert_eq!(saying(&events, "a line the exit writes"), QUEUED_LINES);
    let stderr = child.stderr();
    assert!(!abandoned(&stderr), "{stderr:#?}");
}

#[cfg(unix)]
#[test]
fn a_signal_exiting_at_once_writes_every_line_queued_before_it() {
    let mut child = ChildProcess::spawn_unread(CHILD_TEST, "queued-signal", &console_env());

    let said = child.expect_stderr_line(CONSOLE_CHILD, Duration::from_secs(30));
    assert_eq!(said, LOGGED);
    child.signal("TERM");
    child.read_stdout();
    let (code, printed) = child.exit_within(Duration::from_secs(10));

    assert_eq!(code, Some(143), "SIGTERM's code");
    let events = console_events(&printed);
    assert_eq!(saying(&events, "a line the exit writes"), QUEUED_LINES);
    assert_eq!(
        saying(&events, EXITING),
        1,
        "the line it exits on, written too"
    );
}

#[test]
fn lines_dropped_while_stdout_stalled_are_counted_once_it_reads_again() {
    let mut child = ChildProcess::spawn_unread(CHILD_TEST, "stalled", &console_env());

    let said = child.expect_stderr_line(CONSOLE_CHILD, Duration::from_secs(30));
    assert_eq!(said, LOGGED);
    child.read_stdout();
    child.expect_line(DROPPED);
    child.release();
    let (code, printed) = child.exit_within(Duration::from_secs(10));

    assert_eq!(code, Some(0));
    let events = console_events(&printed);
    let reports: Vec<&serde_json::Value> = events
        .iter()
        .filter(|event| event["fields"]["message"] == DROPPED)
        .collect();
    assert_eq!(reports.len(), 1, "said once: {reports:#?}");
    let report = reports[0];
    assert_eq!(report["level"], "WARN", "{report}");
    assert_eq!(report["target"], nest_rs_core::target::APP, "{report}");
    let dropped = report["fields"]["dropped"].as_u64().unwrap_or_default();
    let written = saying(&events, "a line the console may drop");
    assert!(dropped > 0, "{report}");
    assert_eq!(
        usize::try_from(dropped).unwrap_or(usize::MAX) + written,
        LINES,
        "every line filed was written or counted"
    );
}

/// A binary with no `App` installs the kernel's console from its settings —
/// their format and source location honoured, not only their filter — and a
/// second install keeps the first.
#[test]
fn a_binary_with_no_app_installs_the_console_its_settings_describe() {
    let env = settings(&[(var::FORMAT, "json"), (var::SOURCE_LOCATION, "true")]);
    let mut child = ChildProcess::spawn_with(CHILD_TEST, "app-less", &env);

    let said = child.expect_stderr_line(INSTALLED, Duration::from_secs(10));
    assert!(said.ends_with("first=true second=false"), "{said}");
    let (code, printed) = child.exit_within(Duration::from_secs(10));
    assert_eq!(code, Some(0), "{printed:#?}");
    let events = console_events(&printed);
    let line = events
        .iter()
        .find(|event| event["fields"]["message"] == APP_LESS_LINE)
        .unwrap_or_else(|| panic!("the line reached the console as JSON: {events:#?}"));
    assert_eq!(line["target"], FIXTURE_TARGET, "{line}");
    assert_eq!(line["fields"]["step"], 1, "{line}");
    assert!(
        line["filename"]
            .as_str()
            .is_some_and(|file| file.ends_with("tests/integration/logging.rs")),
        "the source location is on the line: {line}"
    );
    assert_eq!(
        saying(&events, LOG_LINE),
        1,
        "a `log` record reaches the console: {events:#?}"
    );
}

/// A `log` logger set before the console keeps the `log` records, and the
/// console installed anyway is reported as installed, saying so once at `warn`.
#[test]
fn a_log_logger_set_before_the_console_keeps_its_records_and_the_console_is_installed() {
    let env = settings(&[(var::FORMAT, "json")]);
    let mut child = ChildProcess::spawn_with(CHILD_TEST, "app-less-log-logger", &env);

    let said = child.expect_stderr_line(INSTALLED, Duration::from_secs(10));
    assert!(said.ends_with("first=true second=false"), "{said}");
    let (code, printed) = child.exit_within(Duration::from_secs(10));
    assert_eq!(code, Some(0), "{printed:#?}");
    let events = console_events(&printed);
    assert_eq!(saying(&events, APP_LESS_LINE), 1, "{events:#?}");
    assert_eq!(saying(&events, LOG_LINE), 0, "{events:#?}");
    let kept = events
        .iter()
        .find(|event| event["fields"]["message"] == LOG_KEPT)
        .unwrap_or_else(|| panic!("the console says the `log` logger kept: {events:#?}"));
    assert_eq!(kept["level"], "WARN", "{kept}");
    assert!(
        kept["fields"]["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "{kept}"
    );
    let stderr = child.stderr();
    assert!(
        stderr
            .iter()
            .any(|line| line == &format!("{LOG_LOGGER} {LOG_LINE}")),
        "the `log` record reached the logger set first: {stderr:#?}"
    );
}

/// A filter the settings hold is the one the installed console applies.
#[test]
fn the_installed_console_applies_the_settings_filter() {
    let env = settings(&[(var::FORMAT, "json"), (var::FILTER, "warn")]);
    let mut child = ChildProcess::spawn_with(CHILD_TEST, "app-less", &env);

    child.expect_stderr_line(INSTALLED, Duration::from_secs(10));
    let (code, printed) = child.exit_within(Duration::from_secs(10));
    assert_eq!(code, Some(0), "{printed:#?}");
    assert_eq!(
        saying(&console_events(&printed), APP_LESS_LINE),
        0,
        "an `info` line under `warn`: {printed:#?}"
    );
}
