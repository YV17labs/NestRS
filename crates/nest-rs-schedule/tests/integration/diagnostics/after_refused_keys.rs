//! The worker-job keys a `#[scheduled]` one-shot cannot take, each refused with
//! the fact that makes it meaningless — the table in `macros.md`, *The impl
//! half* — rather than as an unknown key. `replicas` and `key` are among them: a
//! one-shot fires on the replica that booted and claims nothing. One method per
//! cell.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[after("1s", retries = 3)]
    async fn retried(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", concurrency = 2)]
    async fn concurrent(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", throttle(limit = 1, window = "1m"))]
    async fn throttled(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", replicas = "one")]
    async fn claimed(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", key = "billing::Tasks::pinned")]
    async fn pinned(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("1s", tz = "Europe/Paris")]
    async fn zoned(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[after("30s", queue = DemoQueue)]
    async fn queued(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
