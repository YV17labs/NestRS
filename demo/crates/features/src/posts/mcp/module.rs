use nest_rs_core::module;

use super::tool::PostsTool;
use crate::authz::mcp::AuthzMcpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzMcpModule],
    providers = [PostsTool],
)]
pub struct PostsMcpModule;
