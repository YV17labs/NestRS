//! Two `#[meta]` values of one type on one trigger: refused at the second.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

struct Limit(u32);

#[injectable]
#[derive(Default)]
struct SweepTasks;

#[scheduled]
impl SweepTasks {
    #[every("1h")]
    #[meta(Limit(10))]
    #[meta(Limit(20))]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
