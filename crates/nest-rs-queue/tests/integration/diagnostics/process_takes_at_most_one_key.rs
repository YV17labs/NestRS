//! A key written twice has no reading a developer could have meant, and
//! accepting it drops one of the two declarations by source order. The one that
//! disappears here would be `transactional = true` — the default `#[process]`
//! exists to let a developer state — so the repeat is refused rather than
//! resolved. The sentence is `nest_rs_codegen::duplicate_argument`, shared with
//! the trigger decorators, which read a repeat the same way.

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
    #[process(queue = FanQueue, transactional = true, transactional = false)]
    async fn fan(&self, _job: FanCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
