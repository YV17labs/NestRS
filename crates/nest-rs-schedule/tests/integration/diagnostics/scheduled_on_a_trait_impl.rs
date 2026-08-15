//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[scheduled]`
//! was accepted and collected nothing. The scheduled tick declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_schedule::scheduled;

struct DemoProvider;

#[scheduled]
impl Default for DemoProvider {
    fn default() -> Self {
        Self
    }
}

fn main() {}
