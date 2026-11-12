//! `#[scheduled]` with all three trigger forms, and the layers a tick takes on
//! its block and its trigger.

use nest_rs::core::injectable;
use nest_rs::schedule::{CronExpression, ScheduleView, scheduled};

use crate::pipeline::{HygieneUnitAudit, HygieneUnitRefusal};

#[injectable]
pub struct HygieneTasks;

/// `ScheduleView`'s path through the umbrella.
pub fn job_of(view: &ScheduleView) -> &str {
    view.job()
}

/// A value `#[meta]` declares, read by type.
pub struct HygieneLimit(pub u32);

#[scheduled]
#[meta(HygieneLimit(100))]
#[use_interceptors(HygieneUnitAudit)]
#[use_exception_filters(HygieneUnitRefusal)]
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
    #[meta(HygieneLimit(10))]
    #[use_interceptors(HygieneUnitAudit)]
    #[use_exception_filters(HygieneUnitRefusal)]
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
