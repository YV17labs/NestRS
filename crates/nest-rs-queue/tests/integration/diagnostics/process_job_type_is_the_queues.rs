//! The decorator's headline promise: a method whose job argument is not the
//! queue's `Job` is a compile error naming both types, never a job that silently
//! never drains. Nothing exercised it, so the whole assertion could be deleted
//! with every test still green.

use nest_rs_core::injectable;
use nest_rs_queue::{processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue;

#[injectable]
#[derive(Default)]
struct Demo;

#[processor]
impl Demo {
    #[process(queue = DemoQueue)]
    async fn handle(&self, _job: String) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
