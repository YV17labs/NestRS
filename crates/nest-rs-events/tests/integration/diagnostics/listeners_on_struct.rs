//! One decorator, one item shape: `#[listeners]` collects a provider's
//! `#[on_event]` methods, so on the struct it must point back at
//! `#[injectable]` rather than report the shape it happened to expect.

use nest_rs_events::listeners;

#[listeners]
struct DemoListener;

fn main() {}
