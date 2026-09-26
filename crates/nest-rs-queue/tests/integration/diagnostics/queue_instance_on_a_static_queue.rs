//! `instance` exists on a dynamic queue only: a static queue is its own
//! destination, and asking it for an instance does not compile.

use nest_rs_queue::queue;

#[queue(name = "audio", job = String)]
struct AudioQueue;

fn main() {
    let _ = AudioQueue::instance("acme");
}
