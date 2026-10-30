use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::tool::PostsTool;
use crate::authz::mcp::AuthzMcpModule;
use crate::posts::PostsModule;

#[module(
    imports = [PostsModule, AuthzMcpModule, HttpModule],
    providers = [PostsTool],
)]
pub struct PostsMcpModule;
