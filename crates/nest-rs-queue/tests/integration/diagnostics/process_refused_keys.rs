//! The worker-job keys a `#[processor]` job cannot take, each refused with the
//! fact that makes it meaningless — the table in `framework.md`, *The impl half*
//! — rather than as an unknown key. One method per cell.

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
    #[process(queue = DemoQueue, replicas = "one")]
    async fn claimed(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }

    #[process(queue = DemoQueue, tz = "Europe/Paris")]
    async fn zoned(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }

    #[process(queue = DemoQueue, key = "billing::Demo::pinned")]
    async fn pinned(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
