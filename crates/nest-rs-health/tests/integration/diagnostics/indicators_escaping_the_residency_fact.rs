//! The refusal cannot be waved through by taking the remedy it offers.
//! `#[injectable]` records the residency fact for every scope, so contradicting
//! it by hand is a coherence error rather than a second opinion.

use nest_rs_core::injectable;
use nest_rs_health::indicators;
use nest_rs_core::ProviderResidency;

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

impl ProviderResidency for PerResolution {
    const SINGLETON: bool = true;
}

#[indicators]
impl PerResolution {
    #[readiness]
    async fn ready(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
