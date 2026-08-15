//! The refusal cannot be waved through by taking the remedy it offers.
//! `#[injectable]` records the residency fact for every scope, so contradicting
//! it by hand is a coherence error rather than a second opinion.

use nest_rs_core::injectable;
use nest_rs_queue::{processor, queue};
use use serde::{Deserialize, Serialize};
use nest_rs_core::ProviderResidency;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue;

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

impl ProviderResidency for PerResolution {
    const SINGLETON: bool = true;
}

#[processor]
impl PerResolution {
    #[process(queue = DemoQueue, retries = 1, transactional = false)]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
