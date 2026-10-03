//! The worker-job keys a `#[scheduled]` cron trigger cannot take, each refused
//! with the fact that makes it meaningless rather than as an unknown key. One
//! method per key.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 * * * *", retries = 3)]
    async fn retried(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 * * * *", concurrency = 2)]
    async fn concurrent(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 * * * *", throttle(limit = 1, window = "1m"))]
    async fn throttled(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 * * * *", queue = DemoQueue)]
    async fn queued(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
