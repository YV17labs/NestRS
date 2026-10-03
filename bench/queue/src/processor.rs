use std::sync::Arc;

use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::queue::processor;

use crate::command::{C1Queue, C16Queue, NoopCommand};
use crate::probe::{Probe, Run, now_us};

#[injectable]
pub struct C1Processor {
    #[inject]
    probe: Arc<Probe>,
}

#[processor]
impl C1Processor {
    #[process(queue = C1Queue, concurrency = 1)]
    async fn run(&self, job: NoopCommand) -> Result<()> {
        record(&self.probe, &job);
        Ok(())
    }
}

#[injectable]
pub struct C16Processor {
    #[inject]
    probe: Arc<Probe>,
}

#[processor]
impl C16Processor {
    #[process(queue = C16Queue, concurrency = 16)]
    async fn run(&self, job: NoopCommand) -> Result<()> {
        record(&self.probe, &job);
        Ok(())
    }
}

fn record(probe: &Probe, job: &NoopCommand) {
    let started_us = now_us();
    probe.record(Run {
        seq: job.seq,
        pushed_us: job.pushed_us,
        started_us,
        done_us: now_us(),
    });
}
