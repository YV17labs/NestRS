//! The queue half of the same refusal, worded once in `nest_rs::codegen::job`:
//! an attempt allowed past a day is a process of its own.

use nest_rs::core::injectable;
use nest_rs::queue::{processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct Command;

#[queue(name = "q", job = Command)]
struct Q;

#[injectable]
#[derive(Default)]
struct Jobs;

#[processor]
impl Jobs {
    #[process(queue = Q, timeout = "25h")]
    async fn run(&self, _job: Command) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
