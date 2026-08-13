//! The queue half of the same refusal, worded once in `nest_rs_codegen::job` so
//! a developer who wrote a bare `transactional` reads the same sentence
//! whichever of the four job decorators they wrote it on.

use nest_rs_core::injectable;
use nest_rs_queue::{QueueName, processor};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct Command;

struct Q;

impl QueueName for Q {
    const NAME: &'static str = "q";
    type Job = Command;
}

#[injectable]
#[derive(Default)]
struct Jobs;

#[processor]
impl Jobs {
    #[process(queue = Q, transactional)]
    async fn run(&self, _job: Command) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
