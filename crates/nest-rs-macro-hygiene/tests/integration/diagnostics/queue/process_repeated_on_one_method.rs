//! `#[process]` repeated on one method: refused with the family's sentence,
//! counting every copy written — not with rustc's, which recommended
//! `#[processor]`, the wrong decorator, for the attribute the expansion left
//! behind.

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
    #[process(queue = DemoQueue)]
    #[process(queue = DemoQueue, retries = 5)]
    #[process(queue = DemoQueue, concurrency = 2)]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
