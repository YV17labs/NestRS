//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[hooks]`
//! was accepted and collected nothing. The lifecycle hook declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_core::hooks;

struct DemoProvider;

#[hooks]
impl Default for DemoProvider {
    fn default() -> Self {
        Self
    }
}

fn main() {}
