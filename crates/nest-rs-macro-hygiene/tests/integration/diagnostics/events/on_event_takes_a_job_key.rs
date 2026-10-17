//! `#[on_event]` takes two keys, `transactional` and `timeout`; a key of the
//! worker-job family it is no member of is unknown here, and the refusal lists
//! the two.

use nest_rs::core::injectable;
use nest_rs::events::listeners;

#[derive(Clone)]
struct Ping;

#[injectable]
#[derive(Default)]
struct Demo;

#[listeners]
impl Demo {
    #[on_event(retries = 1)]
    async fn on_ping(&self, _event: Ping) {}
}

fn main() {}
