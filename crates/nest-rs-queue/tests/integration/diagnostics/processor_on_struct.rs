//! One decorator, one item shape: `#[processor]` collects a provider's
//! `#[process]` methods, so on the struct it must point back at
//! `#[injectable]` rather than report the shape it happened to expect.

use nest_rs_queue::processor;

#[processor]
struct DemoProcessor;

fn main() {}
