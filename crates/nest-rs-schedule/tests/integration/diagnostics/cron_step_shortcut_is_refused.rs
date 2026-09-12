//! The croner 4 break, pinned where an application meets it.
//!
//! `5/5` — "every five minutes from minute 5" — parsed under croner 3 and does
//! not under croner 4, which wants the range the OCPS grammar defines. That is
//! the one change in the bump an application's own cron strings can trip over,
//! so the diagnostic it gets is worth a snapshot: the sibling file pins a
//! field-count error, which both croner majors reject identically and which
//! therefore says nothing about this.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[cron("0 5/5 * * * *")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
