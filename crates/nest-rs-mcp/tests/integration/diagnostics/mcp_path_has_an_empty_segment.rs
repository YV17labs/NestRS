//! The MCP member. This decorator already refused the empty string; the shared
//! grammar is what gives it the other three ways of writing a mount path wrong.

use nest_rs_mcp::mcp;

#[derive(Clone, Default)]
#[mcp(path = "//tools")]
pub struct DemoHost;

fn main() {}
