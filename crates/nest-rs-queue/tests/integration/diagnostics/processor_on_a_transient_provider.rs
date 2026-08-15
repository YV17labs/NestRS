//! The worst of the shapes: `Container::get` on a `scope = transient`
//! provider **builds a throwaway**, so the methods run against an instance
//! nobody else holds and their effects are dropped — no skip, no warning, no
//! symptom.

use nest_rs_core::injectable;
use nest_rs_queue::{processor, queue};
use use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue;

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

#[processor]
impl PerResolution {
    #[process(queue = DemoQueue, retries = 1, transactional = false)]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
