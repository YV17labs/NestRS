//! A key pins what a job firing once claims its occurrences under; beside a job
//! firing on every replica — the default, or `replicas = "each"` written — it
//! would be a declaration nothing reads, and is refused at the key.

use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

#[injectable]
#[derive(Default)]
struct Tasks;

#[scheduled]
impl Tasks {
    #[every("30s", key = "billing::Tasks::tick")]
    async fn defaulted(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 0 * * * *", replicas = "each", key = "billing::Tasks::hourly")]
    async fn written(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
