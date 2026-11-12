//! A forced guard on a `#[listeners]` block is refused by the layer table: an
//! event has no caller to gate.

use nest_rs::core::injectable;
use nest_rs::events::listeners;

struct AllowAll;

#[derive(Clone)]
struct Priced;

#[injectable]
#[derive(Default)]
struct PriceListeners;

#[listeners]
#[force_guards(AllowAll)]
impl PriceListeners {
    #[on_event]
    async fn on_priced(&self, _event: Priced) {}
}

fn main() {}
