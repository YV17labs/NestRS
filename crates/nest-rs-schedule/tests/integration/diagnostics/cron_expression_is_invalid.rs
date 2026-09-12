//! The expression half of `#[cron]`, pinned the way the `tz` half beside it
//! already is.
//!
//! `validate_cron_literal` is what makes a bad literal a compile error rather
//! than a boot failure, and until this file nothing proved it ran: deleting the
//! call left the suite green, while the docs page quotes this exact diagnostic
//! verbatim. The message is croner's own, so a wording change upstream lands
//! here as a snapshot diff instead of silently making that page a lie.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("every monday")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
