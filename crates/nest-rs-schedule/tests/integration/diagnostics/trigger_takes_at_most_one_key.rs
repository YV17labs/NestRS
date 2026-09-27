//! The same refusal on the trigger half of the family, through the same
//! `nest_rs_codegen::duplicate_argument` sentence — at every trigger, each on a
//! key its own column holds: `tz`, which only the calendar owns, `replicas` on the
//! interval, and `transactional` on the one-shot, the only key it takes. A repeat
//! is refused per argument and not per key. One method per trigger; the
//! refusals of one host arrive together.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 0 * * * *", tz = "Europe/Paris", tz = "UTC")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s", replicas = "one", replicas = "each")]
    async fn poll(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", transactional = false, transactional = true)]
    async fn warmup(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
