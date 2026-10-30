use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::tool::UsersTool;
use crate::authz::mcp::AuthzMcpModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzMcpModule, HttpModule],
    providers = [UsersTool],
)]
pub struct UsersMcpModule;
