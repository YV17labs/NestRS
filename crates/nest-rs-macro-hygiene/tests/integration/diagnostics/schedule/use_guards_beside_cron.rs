//! A guard beside `#[cron]` is refused by the layer table: a tick has no
//! caller to gate.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

struct AllowAll;

#[injectable]
#[derive(Default)]
struct SweepTasks;

#[scheduled]
impl SweepTasks {
    #[cron("0 0 * * * *")]
    #[use_guards(AllowAll)]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
