//! An `#[on_event]` method returns `()`: the bus fires it and reads no answer, so
//! a `Result` would carry an error nobody sees. Refused with that rule rather
//! than with what rustc says of the expansion's call.

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
    async fn on_ping(&self, _event: Ping) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn main() {}
