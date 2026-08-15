//! The refusal cannot be waved through by taking the remedy it offers.
//! `#[injectable]` records the residency fact for every scope, so contradicting
//! it by hand is a coherence error rather than a second opinion.

use nest_rs_core::injectable;
use nest_rs_events::listeners;
use nest_rs_core::ProviderResidency;

#[derive(Clone)]
struct Ping;

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

impl ProviderResidency for PerResolution {
    const SINGLETON: bool = true;
}

#[listeners]
impl PerResolution {
    #[on_event]
    async fn on_ping(&self, _event: Ping) {}
}

fn main() {}
