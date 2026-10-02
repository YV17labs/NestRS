//! The schedule member of the one-role-per-method family: two triggers on one
//! method. A repeated key *inside* one trigger is `nest_rs_codegen::Grammar`'s
//! refusal, held by its unit tests.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

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
