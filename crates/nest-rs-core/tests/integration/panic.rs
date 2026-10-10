//! Covers `src/panic.rs` — the process hook `#[nest_rs::main]` installs, and
//! [`contain`], through which a unit of work catches its own unwind.
//!
//! What reaches a process's stdout and stderr is asserted on a [`ChildProcess`].

use std::time::Duration;

use nest_rs_core::panic::{FIELD, LOCATION_FIELD, contain};
use nest_rs_core::{App, EnvPrefix, contained_panic, module};
use nest_rs_testing::LogCapture;

use crate::ChildProcess;

const CHILD_TEST: &str = "panic::panic_child_process";

const TARGET: &str = "nest_rs::fixture";

/// The hook's one sentence for a panic it files itself.
const NO_UNIT: &str = "panicked where no unit of work contains it";

#[module]
struct Bare;

/// `.unwrap()` on a failed decode: the payload quotes the value.
fn unwrap_a_secret() -> u64 {
    serde_json::from_str::<u64>(r#""sk_live_51HsecretTOKEN""#).unwrap()
}

#[nest_rs_core::main]
async fn run_child(role: String) -> anyhow::Result<()> {
    match role.as_str() {
        "subscribed" => {
            let _app = App::new::<Bare>()?;
            unwrap_a_secret();
            Ok(())
        }
        "unsubscribed" => {
            unwrap_a_secret();
            Ok(())
        }
        other => anyhow::bail!("no child role `{other}`"),
    }
}

#[test]
fn panic_child_process() {
    if let Some(role) = crate::child_role() {
        let _ = run_child(role);
    }
}

fn assert_no_secret(stdout: &[String], stderr: &[String]) {
    for line in stdout.iter().chain(stderr) {
        assert!(
            !line.contains("sk_live"),
            "the payload's value reached the process's output: {line}"
        );
    }
}

#[test]
fn a_panic_no_unit_contains_is_one_redacted_error_on_the_app_target() {
    let mut child = ChildProcess::spawn_with(
        CHILD_TEST,
        "subscribed",
        &[
            (
                EnvPrefix::var(nest_rs_core::logging::var::FORMAT),
                "json".to_owned(),
            ),
            (
                EnvPrefix::var(nest_rs_core::logging::var::FILTER),
                "info".to_owned(),
            ),
        ],
    );

    let (code, stdout) = child.exit_within(Duration::from_secs(10));
    let stderr = child.stderr();

    assert_eq!(code, Some(101), "a panic's status: {stdout:#?} {stderr:#?}");
    assert_no_secret(&stdout, &stderr);
    let errors: Vec<serde_json::Value> = stdout
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["level"] == "ERROR")
        .collect();
    assert_eq!(errors.len(), 1, "one error, the hook's: {stdout:#?}");
    let event = &errors[0];
    assert_eq!(event["target"], nest_rs_core::target::APP, "{event}");
    let fields = &event["fields"];
    assert_eq!(fields["message"], NO_UNIT, "{event}");
    assert_eq!(
        fields[FIELD],
        "called `Result::unwrap()` on an `Err` value: Error(\"invalid type: a string, expected \
         u64\", line: 1, column: 24)",
        "{event}",
    );
    let location = fields[LOCATION_FIELD].as_str().unwrap_or_default();
    assert!(
        location.contains("tests/integration/panic.rs:"),
        "where it panicked: {event}"
    );
    assert!(
        fields["thread"].is_string(),
        "the thread it panicked on: {event}"
    );
}

#[test]
fn a_panic_before_any_subscriber_is_one_redacted_line_on_stderr() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "unsubscribed");

    let (code, stdout) = child.exit_within(Duration::from_secs(10));
    let stderr = child.stderr();

    assert_eq!(code, Some(101), "a panic's status: {stdout:#?} {stderr:#?}");
    assert_no_secret(&stdout, &stderr);
    let said: Vec<&String> = stderr
        .iter()
        .filter(|line| line.starts_with("nestrs: panicked at "))
        .collect();
    assert_eq!(said.len(), 1, "one line, the hook's: {stderr:#?}");
    assert!(
        said[0].contains("tests/integration/panic.rs:")
            && said[0].contains("invalid type: a string, expected u64"),
        "where and what, without the value: {}",
        said[0],
    );
}

#[nest_rs_core::main]
async fn contain_a_panic() {
    let unwound = contain(async {
        panic!("deliberate panic");
    })
    .await
    .expect_err("the unit panicked");
    contained_panic!(target: TARGET, unwound.as_ref(), "unit panicked");
}

#[test]
fn a_panic_inside_contain_is_filed_once_by_its_unit_saying_where() {
    let logs = LogCapture::install();

    contain_a_panic();

    let events = logs.events();
    assert_eq!(events.len(), 1, "the unit's line alone: {events:#?}");
    let line = &events[0];
    assert_eq!(
        (line.target.as_str(), line.message.as_str()),
        (TARGET, "unit panicked")
    );
    assert_eq!(line.field(FIELD).as_deref(), Some("deliberate panic"));
    let location = line.field(LOCATION_FIELD).unwrap_or_default();
    assert!(
        location.contains("tests/integration/panic.rs:"),
        "where it panicked: {line:#?}"
    );
}
