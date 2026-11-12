//! A checkpoint is saved to the queue backend at once, while the default
//! attempt rolls its database work back when it fails — a retry would resume
//! past work that was undone. Declaration and parameter sit on one item, so the
//! decorator refuses the pair at the parameter.

use nest_rs::core::injectable;
use nest_rs::queue::{processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImportCommand {
    rows: u32,
}

#[queue(name = "imports", job = ImportCommand)]
struct ImportQueue;

#[injectable]
#[derive(Default)]
struct Importer;

#[processor]
impl Importer {
    #[process(queue = ImportQueue, retries = 3)]
    async fn import(&self, _job: ImportCommand, _progress: nest_rs::queue::Checkpoint<u32>) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
