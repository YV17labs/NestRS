//! The last of the eight missing-required-key sites. `#[process]` wraps the
//! shared sentence to say what its key names — the queue's `#[queue]` marker
//! type — which is what the module means by "a key with more to say wraps this".
//!
//! Written `#[process()]` with an empty list. The bare `#[process]` earns the
//! same sentence, pinned in `process_without_arguments`.

use nest_rs::core::injectable;
use nest_rs::queue::processor;

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
