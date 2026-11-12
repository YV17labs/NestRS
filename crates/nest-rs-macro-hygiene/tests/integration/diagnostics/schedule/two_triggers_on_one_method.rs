//! The schedule member of the one-role-per-method family: two triggers on one
//! method. A repeated key *inside* one trigger is `nest_rs::codegen::Grammar`'s
//! refusal, held by its unit tests.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s")]
    #[cron("0 0 * * * *")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
