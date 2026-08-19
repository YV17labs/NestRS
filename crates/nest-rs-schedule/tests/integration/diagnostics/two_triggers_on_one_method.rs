//! The schedule member of the one-role-per-method family. Its sibling
//! `trigger_takes_at_most_one_key` pins a repeated key *inside* one trigger;
//! this pins two triggers on one method, which is the other refusal.

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
