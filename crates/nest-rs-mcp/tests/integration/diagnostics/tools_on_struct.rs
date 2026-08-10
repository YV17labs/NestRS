//! The converse: `#[tools]` collects a host's operations, so on the struct it
//! must point back at `#[mcp]`.

use nest_rs_mcp::tools;

#[tools]
#[derive(Clone)]
struct DemoTool;

fn main() {}
