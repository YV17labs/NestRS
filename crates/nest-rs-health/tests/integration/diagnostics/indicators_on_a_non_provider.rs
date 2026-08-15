//! The pair's third wrong shape: right item, wrong host. `#[indicators]` gates
//! on `Container::get::<Host>()` when a probe runs, so a host nothing registers
//! would have its probes silently skipped behind a `warn` — a readiness check
//! that never runs reads as healthy. The refusal reads `ProviderResidency`,
//! the fact every provider-building decorator records.

use nest_rs_health::indicators;

struct Plain;

#[indicators]
impl Plain {
    #[readiness]
    async fn ready(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
