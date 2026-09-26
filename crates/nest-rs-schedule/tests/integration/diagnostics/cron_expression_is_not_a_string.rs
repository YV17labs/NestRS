//! `#[cron]`'s argument is a cron expression — a string literal, validated here,
//! or a `CronExpression` constant, validated at boot. A literal of any other
//! kind is refused at itself, opening with the decorator, rather than surfacing
//! as a type mismatch inside the expansion.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron(3600)]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
