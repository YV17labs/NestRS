//! One of the four refusals a `key = value` grammar owes, pinned where the
//! compiler says it. `CLAUDE.md`: "Refusals are shared, not per key. One
//! helper, one sentence, every key it covers, **one trybuild snapshot per
//! site**."

use nest_rs_core::injectable;
use nest_rs_queue::processor;

#[injectable]
#[derive(Default)]
struct Emails;

#[processor]
impl Emails {
    #[process(topic = "emails")]
    async fn send(&self) {}
}

fn main() {}
