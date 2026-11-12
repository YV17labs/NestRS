//! A method runs one attempt at a time, so a second attempt parameter would be
//! the same identity twice — refused at the second.

use nest_rs::core::injectable;
use nest_rs::queue::{Attempt, processor, queue};
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
    async fn handle(
        &self,
        _job: DemoCommand,
        _attempt: Attempt,
        _again: Attempt,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
