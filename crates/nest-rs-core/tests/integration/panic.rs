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
        "inside_the_subscriber" => {
            let _app = App::new::<Bare>()?;
            tracing::info!(target: TARGET, value = %Unwritable, "never written");
            Ok(())
        }
        other => anyhow::bail!("no child role `{other}`"),
    }
}

/// A value whose text panics as the subscriber writes it.
struct Unwritable;

impl std::fmt::Display for Unwritable {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("the field's text panicked")
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

/// The production shape: the console subscriber is the global one, which the
/// hook files through while it is still writing the line that panicked.
#[test]
fn a_panic_inside_the_subscriber_s_own_dispatch_is_one_event_of_the_hook() {
    let mut child = ChildProcess::spawn_with(
        CHILD_TEST,
        "inside_the_subscriber",
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

    assert_eq!(code, Some(101), "a panic's status: {stdout:#?}");
    let errors: Vec<serde_json::Value> = stdout
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["level"] == "ERROR")
        .collect();
    assert_eq!(errors.len(), 1, "one error, the hook's: {stdout:#?}");
    assert_eq!(errors[0]["fields"]["message"], NO_UNIT, "{}", errors[0]);
    assert_eq!(
        errors[0]["fields"][FIELD], "the field's text panicked",
        "{}",
        errors[0]
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
    crate::rust_panic_hook();

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

/// A value whose drop panics, as a unit's guard can.
struct PanicsOnDrop;

impl Drop for PanicsOnDrop {
    fn drop(&mut self) {
        panic!("dropped mid-flight");
    }
}

#[nest_rs_core::main]
async fn cancel_a_unit_whose_guard_panics_as_it_drops() {
    let unit = contain(async {
        let _guard = PanicsOnDrop;
        std::future::pending::<()>().await;
    });
    tokio::select! {
        biased;
        _ = unit => {}
        () = std::future::ready(()) => {}
    }
}

#[test]
fn a_panic_raised_as_a_cancelled_unit_is_dropped_is_filed_by_the_hook() {
    let logs = LogCapture::install();

    let unwound = std::panic::catch_unwind(cancel_a_unit_whose_guard_panics_as_it_drops);
    crate::rust_panic_hook();

    let unwound = unwound.expect_err("the drop's panic unwinds out of `main`");

    assert_eq!(
        nest_rs_core::panic_message(unwound.as_ref()),
        "dropped mid-flight"
    );

    let events = logs.events();
    assert_eq!(events.len(), 1, "the hook's event alone: {events:#?}");
    let event = &events[0];
    assert_eq!(
        (event.target.as_str(), event.message.as_str()),
        (nest_rs_core::target::APP, NO_UNIT)
    );
    assert_eq!(event.field(FIELD).as_deref(), Some("dropped mid-flight"));
}

#[nest_rs_core::main]
async fn two_units_panic_alike_and_a_third_panic_comes_between() -> Vec<String> {
    let first_at = format!("{}:{}:", file!(), line!() + 1);
    let first = contain(async { panic!("deliberate panic") }).await;
    let second_at = format!("{}:{}:", file!(), line!() + 1);
    let second = contain(async { panic!("deliberate panic") }).await;
    let uncontained = tokio::spawn(async { panic!("deliberate panic") }).await;
    assert!(uncontained.is_err(), "the third panicked outside any unit");

    contained_panic!(
        target: TARGET,
        first.expect_err("the first unit panicked").as_ref(),
        "first unit panicked",
    );
    contained_panic!(
        target: TARGET,
        second.expect_err("the second unit panicked").as_ref(),
        "second unit panicked",
    );
    vec![first_at, second_at]
}

#[test]
fn each_contained_panic_says_where_it_happened_and_not_where_another_did() {
    let logs = LogCapture::install();

    let at = two_units_panic_alike_and_a_third_panic_comes_between();
    crate::rust_panic_hook();

    for (message, at) in ["first unit panicked", "second unit panicked"]
        .into_iter()
        .zip(at)
    {
        let line = logs.expect_one(TARGET, message);
        let location = line.field(LOCATION_FIELD).unwrap_or_default();
        assert!(location.starts_with(&at), "{at}: {line:#?}");
    }
}
