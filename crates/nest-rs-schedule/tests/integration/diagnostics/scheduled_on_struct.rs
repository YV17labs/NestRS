//! One decorator, one item shape: `#[scheduled]` collects a provider's timed
//! methods, so on the struct it must point back at `#[injectable]` rather than
//! report the shape it happened to expect.

use nest_rs_schedule::scheduled;

#[scheduled]
struct DemoTasks;

fn main() {}
