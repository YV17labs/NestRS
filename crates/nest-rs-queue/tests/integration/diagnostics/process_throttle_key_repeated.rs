//! One rate has one limit: the second is refused at the key that repeats it.

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
    #[process(queue = DemoQueue, throttle(limit = 10, limit = 20, window = "1m"))]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
