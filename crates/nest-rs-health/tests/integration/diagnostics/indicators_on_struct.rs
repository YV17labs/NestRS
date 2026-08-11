//! One decorator, one item shape: `#[indicators]` collects a provider's probe
//! methods, so on the struct it must point back at `#[injectable]` rather than
//! report the shape it happened to expect.

use nest_rs_health::indicators;

#[indicators]
struct DemoIndicator;

fn main() {}
