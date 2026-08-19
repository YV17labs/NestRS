//! `tz` is always a string literal and the IANA name set is closed, so a typo
//! is knowable at expansion — the same fact `#[cron]`'s expression half already
//! acts on. Before this it resolved in `Scheduler::configure`, which made one of
//! two keys in one attribute a deployment failure rather than an underlined
//! line.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 0 * * * *", tz = "Europe/Pariz")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
