//! A scoped pipe on a `#[processor]` block is refused by the layer table: a
//! job's payload takes per-argument pipes alone.

use nest_rs::core::injectable;
use nest_rs::queue::{processor, queue};
use serde::{Deserialize, Serialize};

struct Trim;

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
#[use_pipes(Trim)]
impl Importer {
    #[process(queue = ImportQueue)]
    async fn import(&self, _job: ImportCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
