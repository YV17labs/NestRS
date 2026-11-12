//! A guard beside a `#[process]` method is refused by the layer table: a job
//! has no caller to gate.

use nest_rs::core::injectable;
use nest_rs::queue::{processor, queue};
use serde::{Deserialize, Serialize};

struct AllowAll;

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
    #[process(queue = ImportQueue)]
    #[use_guards(AllowAll)]
    async fn import(&self, _job: ImportCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
