//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[messages]`
//! was accepted and collected nothing. The message declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_ws::messages;

struct DemoGateway;

#[messages]
impl Default for DemoGateway {
    fn default() -> Self {
        Self
    }
}

fn main() {}
