//! One decorator, one item shape: `#[hooks]` collects a provider's lifecycle
//! methods, so on the struct it must point back at `#[injectable]` rather than
//! report the shape it happened to expect.

use nest_rs_core::hooks;

#[hooks]
struct DemoProvider;

fn main() {}
