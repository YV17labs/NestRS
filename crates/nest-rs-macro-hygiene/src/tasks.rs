//! `#[scheduled]` with all three trigger forms.

use nest_rs::core::injectable;
use nest_rs::schedule::{CronExpression, scheduled};

/// Minimal scheduled host.
#[injectable]
pub struct HygieneTasks;

#[scheduled]
impl HygieneTasks {
    /// Interval form, carrying both shared keys so the trailing named arguments
    /// are proved on a trigger that owns none of its own. `replicas = "one"` is
    /// the one that emits a path — `Replicas::One` — through the schedule
    /// crate's root. A scheduled method returns `anyhow::Result<()>` by
    /// contract, named here through the surface re-export.
    #[every("60s", transactional = false, replicas = "one")]
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

    /// Cron form, with every named argument it takes — `tz` is the trigger's
    /// own, `transactional` and `replicas` the shared ones, and they parse
    /// through one list.
    #[cron(
        CronExpression::EVERY_MINUTE,
        tz = "Europe/Paris",
        transactional = true,
        replicas = "each"
    )]
    async fn heartbeat(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }

    /// A synchronous tick is called without an `.await`.
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
