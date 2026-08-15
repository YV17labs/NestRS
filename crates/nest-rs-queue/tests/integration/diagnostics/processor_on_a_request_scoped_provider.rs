//! A provider that *is* injectable and still cannot host these methods:
//! `scope = request` registers a factory, so `Container::get` outside a request
//! answers `None` and the work is skipped. The scope is known where
//! `#[injectable]` reads it, so the refusal belongs at compile time.

use nest_rs_core::injectable;
use nest_rs_queue::{processor, queue};
use use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue;

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequest;

#[processor]
impl PerRequest {
    #[process(queue = DemoQueue, retries = 1, transactional = false)]
    async fn handle(&self, _job: DemoCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
