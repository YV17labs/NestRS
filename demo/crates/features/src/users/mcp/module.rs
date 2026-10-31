use nest_rs::core::module;
use nest_rs::mcp::McpModule;

use super::tool::UsersTool;
use crate::authz::mcp::AuthzMcpModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzMcpModule, McpModule],
    providers = [UsersTool],
)]
pub struct UsersMcpModule;
