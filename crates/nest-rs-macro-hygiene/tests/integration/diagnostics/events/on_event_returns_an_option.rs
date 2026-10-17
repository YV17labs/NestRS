//! An `#[on_event]` method answers `()` or `Result<(), E>`, read by its type:
//! an `Err` rolls the listener's unit back and is reported. Any other answer
//! is refused at the return type, naming the two it takes.

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
    async fn on_ping(&self, _event: Ping) -> Option<()> {
        Some(())
    }
}

fn main() {}
