//! A key no member of the job family takes is a misspelling, and each trigger's
//! sentence lists exactly what that trigger takes: `replicas` on the two that
//! share an occurrence across replicas, never on the one-shot. One method per
//! trigger; the refusals of one host arrive together.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", priority = 1)]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 0 * * * *", priority = 1)]
    async fn hourly(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", priority = 1)]
    async fn warmup(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
