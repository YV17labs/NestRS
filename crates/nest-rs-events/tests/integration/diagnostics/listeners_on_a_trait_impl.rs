//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[listeners]`
//! was accepted and collected nothing. The listener declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_events::listeners;

struct DemoProvider;

#[listeners]
impl Default for DemoProvider {
    fn default() -> Self {
        Self
    }
}

fn main() {}
