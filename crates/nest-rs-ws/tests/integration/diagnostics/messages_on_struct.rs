//! One decorator, one item shape: `#[messages]` collects a gateway's message
//! arms, so on the struct it must point back at `#[gateway]` rather than report
//! the shape it happened to expect.

use nest_rs_ws::messages;

#[messages]
struct DemoGateway;

fn main() {}
