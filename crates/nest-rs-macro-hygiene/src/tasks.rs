//! `#[scheduled]` with all three trigger forms.

use nest_rs::core::injectable;
use nest_rs::schedule::{CronExpression, scheduled};

#[injectable]
pub struct HygieneTasks;

#[scheduled]
impl HygieneTasks {
    /// Interval form, carrying every shared key.
    #[every(
        "60s",
        timeout = "5m",
        transactional = false,
        replicas = "one",
        key = "hygiene::HygieneTasks::tick"
    )]
    async fn tick(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }

    /// One-shot form.
    #[after("1s", transactional = false)]
    async fn warmup(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }

    /// Cron form, with every named argument it takes.
    #[cron(
        CronExpression::EVERY_MINUTE,
        tz = "Europe/Paris",
        transactional = true,
        replicas = "each"
    )]
    async fn heartbeat(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }

    #[every("30s")]
    fn sweep(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }

    /// A tick compiled out takes its schedule entry with it.
    #[cfg(any())]
    #[every("1m")]
    async fn compiled_out(&self) -> crate::does_not_exist::Answer {
        crate::does_not_exist::tick()
    }
}
