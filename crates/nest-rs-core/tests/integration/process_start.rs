//! Covers `src/process_start.rs` — the steps `#[nest_rs::main]` runs before the
//! runtime exists. A libtest process has threads of its own, so what is asserted
//! is the runtime's absence, not a thread count.

use std::sync::{Mutex, PoisonError};

use nest_rs_core::__private::ProcessStart;

/// Each step that ran, in order, and whether a runtime existed as it did.
static RAN: Mutex<Vec<(&'static str, bool)>> = Mutex::new(Vec::new());

const FIRST: &str = "nest_rs::fixture::first";
const SECOND: &str = "nest_rs::fixture::second";

fn record(name: &'static str) {
    let runtime = tokio::runtime::Handle::try_current().is_ok();
    RAN.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((name, runtime));
}

fn first() {
    record(FIRST);
}

fn second() {
    record(SECOND);
}

nest_rs_core::inventory::submit! {
    ProcessStart { name: SECOND, run: second }
}

nest_rs_core::inventory::submit! {
    ProcessStart { name: FIRST, run: first }
}

fn ran() -> Vec<(&'static str, bool)> {
    RAN.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

#[nest_rs_core::main]
async fn what_main_finds() -> Vec<(&'static str, bool)> {
    ran()
}

#[test]
fn each_step_runs_once_per_main_by_name_before_the_runtime_exists() {
    let before_main = [(FIRST, false), (SECOND, false)];

    assert_eq!(what_main_finds(), before_main, "before `main`'s first line");
    what_main_finds();

    assert_eq!(ran(), [before_main, before_main].concat(), "once per call");
}
