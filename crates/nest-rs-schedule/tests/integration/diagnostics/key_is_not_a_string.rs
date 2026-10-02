//! `key` takes the job's identity as a string — the path its boot line names —
//! and a value of another kind is refused at both triggers that take it, naming
//! the decorator.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", replicas = "one", key = billing::Tasks::tick)]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 0 * * * *", replicas = "one", key = 42)]
    async fn hourly(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
