//! `#[scheduled]` with all three trigger forms.

use nest_rs::core::injectable;
use nest_rs::schedule::{CronExpression, scheduled};

/// Minimal scheduled host.
#[injectable]
pub struct HygieneTasks;

#[scheduled]
impl HygieneTasks {
    /// Interval form, carrying the shared `transactional` key so the trailing
    /// named argument is proved on a trigger that takes one of its own — the
    /// grammar `#[cron]` shares. A scheduled method returns
    /// `anyhow::Result<()>` by contract, named here through the surface
    /// re-export.
    #[every("60s", transactional = false)]
    async fn tick(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }

    /// One-shot form, carrying the shared key too: the witness covered three
    /// of the four sites `transactional` reaches, and the fourth is the one a
    /// developer meets last.
    #[after("1s", transactional = false)]
    async fn warmup(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }

    /// Cron form, with both of its named arguments — `tz` is the trigger's
    /// own, `transactional` the shared one, and they parse through one list.
    #[cron(
        CronExpression::EVERY_MINUTE,
        tz = "Europe/Paris",
        transactional = true
    )]
    async fn heartbeat(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }
}
