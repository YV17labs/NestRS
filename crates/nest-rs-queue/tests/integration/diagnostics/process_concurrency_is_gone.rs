//! `#[process(concurrency = N)]` used to compile and silently bound nothing.
//! It is now refused by name: a process method runs one job at a time, and
//! throughput is a replica count. Anyone reintroducing the key has to delete
//! this snapshot to do it.

use nest_rs_core::injectable;
use nest_rs_queue::{processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct FanCommand {
    seq: usize,
}

#[queue(name = "fan", job = FanCommand)]
struct FanQueue;

#[injectable]
#[derive(Default)]
struct FanProcessor;

#[processor]
impl FanProcessor {
    #[process(queue = FanQueue, concurrency = 2, retries = 1)]
    async fn fan(&self, _job: FanCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
