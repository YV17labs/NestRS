//! A repeated `replicas` is refused through the shared sentence: keeping either
//! would decide by source order whether the job fires once or everywhere.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 0 * * * *", replicas = "one", replicas = "each")]
    async fn hourly(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
