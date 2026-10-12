//! Two `#[meta]` values of one type on one processor's impl block: refused at
//! the second.

use nest_rs::core::injectable;
use nest_rs::queue::{processor, queue};
use serde::{Deserialize, Serialize};

struct Limit(u32);

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
#[meta(Limit(10))]
#[meta(Limit(20))]
impl Importer {
    #[process(queue = ImportQueue)]
    async fn import(&self, _job: ImportCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
