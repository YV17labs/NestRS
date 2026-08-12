//! A queue is addressed by its name, so `version = "…"` has nothing to select
//! here. `#[processor]` used to ignore its arguments outright — this pins the
//! answer it gives instead: version the queue's name if that is really what you
//! mean, and evolve the payload if it is not.

use nest_rs_core::injectable;
use nest_rs_queue::processor;

#[injectable]
#[derive(Default)]
struct DemoProcessor;

#[processor(version = "1")]
impl DemoProcessor {}

fn main() {}
