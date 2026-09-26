//! A `#[process]` method answers with a `Result`: `Ok(())` completes the job,
//! `Err` fails the attempt. A method returning nothing is refused with that rule
//! rather than with rustc's `no method named map_err` on the expansion.

use nest_rs_core::injectable;
use nest_rs_queue::{processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue;

#[injectable]
#[derive(Default)]
struct Demo;

#[processor]
impl Demo {
    #[process(queue = DemoQueue)]
    async fn handle(&self, _job: DemoCommand) {}
}

fn main() {}
