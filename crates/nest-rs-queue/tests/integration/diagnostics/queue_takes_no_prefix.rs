//! A queue per runtime key is not offered, and the key that declared one in
//! 6.x is refused by name — with the fact that makes it unaffordable and what to
//! write instead — rather than as a key the decorator never heard of.

use nest_rs_queue::queue;

#[queue(prefix = "tenant", job = String)]
struct TenantQueue;

fn main() {}
