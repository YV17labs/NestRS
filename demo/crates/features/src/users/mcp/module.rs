use nest_rs::core::module;

use super::tool::UsersTool;
use crate::authz::mcp::AuthzMcpModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzMcpModule],
    providers = [UsersTool],
)]
pub struct UsersMcpModule;
