//! `#[process = …]` names something, and is told the form a `#[process]` takes
//! rather than that its queue is missing.

use nest_rs::core::injectable;
use nest_rs::queue::{processor, queue};
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
    #[process = DemoQueue]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
