//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[indicators]`
//! was accepted and collected nothing. The indicator declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_health::indicators;

struct DemoProvider;

#[indicators]
impl Default for DemoProvider {
    fn default() -> Self {
        Self
    }
}

fn main() {}
