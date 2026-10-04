//! The worker-job keys a `#[scheduled]` interval cannot take, each refused with
//! the fact that makes it meaningless rather than as an unknown key. One method
//! per key.

use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", retries = 3)]
    async fn retried(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s", concurrency = 2)]
    async fn concurrent(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s", throttle(limit = 1, window = "1m"))]
    async fn throttled(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s", tz = "Europe/Paris")]
    async fn zoned(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s", queue = DemoQueue)]
    async fn queued(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
