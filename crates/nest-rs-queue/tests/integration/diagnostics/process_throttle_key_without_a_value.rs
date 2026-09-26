//! A bare key inside `throttle(..)` declares nothing — refused where it is written.

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
    #[process(queue = DemoQueue, throttle(limit, window = "1m"))]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
