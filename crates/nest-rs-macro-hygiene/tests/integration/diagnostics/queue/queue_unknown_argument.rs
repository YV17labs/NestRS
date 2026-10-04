//! One of the four refusals a `key = value` grammar owes, pinned where the
//! compiler says it. Refusals are shared, not per key. One
//! helper, one sentence, every key it covers, **one trybuild snapshot per
//! site**.

use nest_rs::queue::queue;

#[queue(topic = "emails")]
struct Emails;

fn main() {}
