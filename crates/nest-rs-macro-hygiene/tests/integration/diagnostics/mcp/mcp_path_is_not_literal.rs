//! The MCP member of the literal mount paths: an endpoint is one address.

use nest_rs::mcp::mcp;

#[derive(Clone, Default)]
#[mcp(path = "/tools/:tenant")]
pub struct TenantHost;

fn main() {}
