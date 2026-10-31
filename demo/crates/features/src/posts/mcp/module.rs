use nest_rs::core::module;
use nest_rs::mcp::McpModule;

use super::tool::PostsTool;
use crate::authz::mcp::AuthzMcpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzMcpModule, McpModule],
    providers = [PostsTool],
)]
pub struct PostsMcpModule;
