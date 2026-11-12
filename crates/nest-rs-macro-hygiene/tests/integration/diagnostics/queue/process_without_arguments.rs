//! A bare `#[process]` names no queue, and is told so in the sentence
//! `#[process()]` earns in `process_without_a_queue` — not with syn's
//! `expected attribute arguments in parentheses`.

use nest_rs::core::injectable;
use nest_rs::queue::processor;

#[injectable]
#[derive(Default)]
struct Mailer;

#[processor]
impl Mailer {
    #[process]
    async fn send(&self, _job: String) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {}
