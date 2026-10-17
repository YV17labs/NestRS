//! A checkpoint saves one state, so two type arguments name no state it could save.

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
        _progress: Checkpoint<u32, u32>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
