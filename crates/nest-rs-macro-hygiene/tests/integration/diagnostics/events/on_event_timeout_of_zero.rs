//! A listener's `timeout` is the job family's duration, above zero: a deadline
//! of nothing would cut every call before it starts.

use nest_rs::core::injectable;
use nest_rs::events::listeners;

#[derive(Clone)]
struct Ping;

#[injectable]
#[derive(Default)]
struct Demo;

#[listeners]
impl Demo {
    #[on_event(timeout = "0s")]
    async fn on_ping(&self, _event: Ping) {}
}

fn main() {}
