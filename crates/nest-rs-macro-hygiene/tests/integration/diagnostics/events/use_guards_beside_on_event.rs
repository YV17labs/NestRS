//! A guard beside `#[on_event]` is refused by the layer table: an event has no
//! caller to gate.

use nest_rs::core::injectable;
use nest_rs::events::listeners;

struct AllowAll;

#[derive(Clone)]
struct Priced;

#[injectable]
#[derive(Default)]
struct PriceListeners;

#[listeners]
impl PriceListeners {
    #[on_event]
    #[use_guards(AllowAll)]
    async fn on_priced(&self, _event: Priced) {}
}

fn main() {}
