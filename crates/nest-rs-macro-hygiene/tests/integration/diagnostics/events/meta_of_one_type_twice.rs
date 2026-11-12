//! Two `#[meta]` values of one type on one listeners impl block: refused at
//! the second.

use nest_rs::core::injectable;
use nest_rs::events::listeners;

struct Limit(u32);

#[derive(Clone)]
struct Priced;

#[injectable]
#[derive(Default)]
struct PriceListeners;

#[listeners]
#[meta(Limit(10))]
#[meta(Limit(20))]
impl PriceListeners {
    #[on_event]
    async fn on_priced(&self, _event: Priced) {}
}

fn main() {}
