//! `name` declares one queue and `prefix` one queue per runtime key; a queue
//! that is both is neither, so the pair is refused at the second.

use nest_rs_queue::queue;

#[queue(name = "tenants", prefix = "tenant", job = String)]
struct TenantQueue;

fn main() {}
