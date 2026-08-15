//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[routes]`
//! was accepted and collected nothing. The route declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_http::routes;

struct DemoController;

#[routes]
impl Default for DemoController {
    fn default() -> Self {
        Self
    }
}

fn main() {}
