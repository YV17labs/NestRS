//! One attempt keeps one progress, so a second checkpoint parameter has no state
//! of its own to save — refused naming both.

use nest_rs::core::injectable;
use nest_rs::queue::{Checkpoint, processor, queue};
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
    #[process(queue = DemoQueue, transactional = false)]
    async fn handle(
        &self,
        _job: DemoCommand,
        _progress: Checkpoint<u32>,
        _other: Checkpoint<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
