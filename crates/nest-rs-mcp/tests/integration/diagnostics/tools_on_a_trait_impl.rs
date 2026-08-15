//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[tools]`
//! was accepted and collected nothing. The tool declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_mcp::tools;

struct DemoHost;

#[tools]
impl Default for DemoHost {
    fn default() -> Self {
        Self
    }
}

fn main() {}
