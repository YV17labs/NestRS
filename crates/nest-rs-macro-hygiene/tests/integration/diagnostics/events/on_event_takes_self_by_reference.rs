//! An `#[on_event]` method takes `&self`: the host is shared by every event it
//! handles at once. `&mut self` is refused with that fact — the rule `#[process]`
//! shares — not with what rustc says of the expansion's call.

use nest_rs::core::injectable;
use nest_rs::events::listeners;

#[derive(Clone)]
struct Ping;

#[injectable]
#[derive(Default)]
struct Demo;

#[listeners]
impl Demo {
    #[on_event]
    async fn on_ping(&mut self, _event: Ping) {}
}

fn main() {}
