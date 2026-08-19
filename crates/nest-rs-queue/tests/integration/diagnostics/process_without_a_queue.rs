//! The last of the eight missing-required-key sites. `#[process]` wraps the
//! shared sentence to add the second shape its key takes — a `QueueName` type —
//! which is what the module means by "a key with more to say wraps this".
//!
//! Written `#[process(...)]` with an empty list rather than bare: a bare
//! `#[process]` is refused earlier, by the argument grammar itself, and would
//! pin that sentence instead of this one.

use nest_rs_core::injectable;
use nest_rs_queue::processor;

#[injectable]
#[derive(Default)]
struct Mailer;

#[processor]
impl Mailer {
    #[process()]
    async fn send(&self, _job: String) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
