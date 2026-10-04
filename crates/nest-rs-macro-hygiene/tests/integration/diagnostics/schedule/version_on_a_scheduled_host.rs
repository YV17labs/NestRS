//! A scheduled task has no caller, so there is nothing for `version = "…"` to
//! select. `#[scheduled]` used to ignore its arguments outright — this pins the
//! sentence it gives instead: the clock is the only trigger.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct DemoTasks;

#[scheduled(version = "1")]
impl DemoTasks {}

fn main() {}
