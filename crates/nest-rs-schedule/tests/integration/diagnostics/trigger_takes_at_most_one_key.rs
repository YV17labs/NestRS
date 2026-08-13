//! The same refusal on the trigger half of the family, through the same
//! `nest_rs_codegen::duplicate_argument` sentence — and on the key a trigger
//! owns rather than the shared one, since a repeat is refused per argument and
//! not per key.

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
}

fn main() {}
