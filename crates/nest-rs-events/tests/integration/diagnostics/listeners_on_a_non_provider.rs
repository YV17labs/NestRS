//! The pair's third wrong shape: right item, wrong host. `#[listeners]` gates
//! on `Container::get::<Host>()` when an event is published, so a host nothing
//! registers would have its handlers silently skipped behind a `warn`. The
//! refusal reads `ProviderResidency`, the fact every provider-building
//! decorator records.

use nest_rs_events::listeners;

#[derive(Clone)]
struct Ping;

struct Plain;

#[listeners]
impl Plain {
    #[on_event]
    async fn on_ping(&self, _event: Ping) {}
}

fn main() {}
