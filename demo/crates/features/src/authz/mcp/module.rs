use nest_rs_core::module;
use nest_rs_mcp::McpOperationGuard;

use super::bridge::AppMcpGuard;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppMcpGuard as dyn McpOperationGuard,
    ],
)]
pub struct AuthzMcpModule;
