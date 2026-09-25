//! A `#[on_event]` method takes no type parameter: the expansion calls it with only
//! what its caller carries, so nothing supplies the type — refused naming the
//! parameter, not with rustc's `type annotations needed` at the decorator.

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
    async fn on_ping<T: Default>(&self, _event: Ping) {}
}

fn main() {}
