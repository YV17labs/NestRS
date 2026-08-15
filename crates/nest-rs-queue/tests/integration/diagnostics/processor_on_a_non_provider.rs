//! The pair's third wrong shape: right item, wrong host. `#[processor]` gates
//! on `Container::get::<Host>()` when a job arrives, so a host nothing
//! registers would have its jobs silently skipped behind a `warn` while the
//! queue fills. The refusal reads `ProviderResidency`, the fact
//! every provider-building decorator records.

use nest_rs_queue::{processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue;

struct Plain;

#[processor]
impl Plain {
    #[process(queue = DemoQueue, retries = 1, transactional = false)]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
