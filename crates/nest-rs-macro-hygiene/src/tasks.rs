//! `#[scheduled]` with all three trigger forms.

use nest_rs_core::injectable;
use nest_rs_schedule::{CronExpression, scheduled};

/// Minimal scheduled host.
#[injectable]
pub struct HygieneTasks;

#[scheduled]
impl HygieneTasks {
    /// Interval form. A scheduled method returns `anyhow::Result<()>` by
    /// contract — named here through the surface re-export.
    #[every("60s")]
    async fn tick(&self) -> nest_rs_core::anyhow::Result<()> {
        Ok(())
    }

    /// One-shot form.
    #[after("1s")]
    async fn warmup(&self) -> nest_rs_core::anyhow::Result<()> {
        Ok(())
    }

    /// Cron form.
    #[cron(CronExpression::EVERY_MINUTE)]
    async fn heartbeat(&self) -> nest_rs_core::anyhow::Result<()> {
        Ok(())
    }
}
