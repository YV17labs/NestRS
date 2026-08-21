use nest_rs::core::module;

use super::tool::PostsTool;
use crate::app_authz::mcp::AppAuthzMcpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AppAuthzMcpModule],
    providers = [PostsTool],
)]
pub struct PostsMcpModule;
