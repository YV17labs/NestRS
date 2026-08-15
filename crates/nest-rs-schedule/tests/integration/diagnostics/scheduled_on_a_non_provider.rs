//! The pair's third wrong shape: right item, wrong host. `#[scheduled]` gates
//! on `Container::get::<Host>()` when the trigger fires, so a host nothing
//! registers would have its ticks silently skipped behind a `warn`. The
//! refusal reads `ProviderResidency`, the fact every provider-building
//! decorator records.

use nest_rs_schedule::scheduled;

struct Plain;

#[scheduled]
impl Plain {
    #[every("60s")]
    async fn tick(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
