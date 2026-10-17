//! `replicas` takes one of two words, and the refusal says what each does —
//! the choice is between two behaviours, not a spelling.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", replicas = "all")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
