//! `throttle(..)` is a key-value grammar of its own, so it owes the same three
//! refusals the outer one does — here, a key it does not take.

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
    #[process(queue = DemoQueue, throttle(burst = 10))]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
