//! A prefix follows the name rule, and `#` is the separator between a prefix
//! and its key — so a prefix carrying one would read as an instance.

use nest_rs_queue::queue;

#[queue(prefix = "tenant#", job = String)]
struct TenantQueue;

fn main() {}
