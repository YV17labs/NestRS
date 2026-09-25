//! `#[on_event]` repeated on one method: refused with the family's sentence,
//! counting every copy written — not with rustc's, which recommended
//! `#[listeners]`, the wrong decorator, for the attribute the expansion left
//! behind.

use nest_rs_core::injectable;
use nest_rs_events::listeners;

#[derive(Clone)]
struct Ping;

#[injectable]
#[derive(Default)]
struct Demo;

#[listeners]
impl Demo {
    #[on_event]
    #[on_event]
    #[on_event]
    async fn on_ping(&self, _event: Ping) {}
}

fn main() {}
