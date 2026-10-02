//! A repeated `key` is refused through the shared sentence: keeping either would
//! decide by source order which job's occurrences this one claims.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", replicas = "one", key = "billing::a", key = "billing::b")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
