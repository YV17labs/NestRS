//! The shared `transactional` key states a behaviour, so the refusal states
//! both behaviours rather than the type it wanted — a developer typing this key
//! is choosing between two settlings, not fixing a typo. The sentence is worded
//! once, in `nest_rs_codegen::job`, for all four job decorators.

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
    #[process(queue = FanQueue, transactional = "no")]
    async fn fan(&self, _job: FanCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
