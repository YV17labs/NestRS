use nest_rs::core::module;

use super::tool::UsersTool;
use crate::app_authz::mcp::AppAuthzMcpModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AppAuthzMcpModule],
    providers = [UsersTool],
)]
pub struct UsersMcpModule;
