use nest_rs::core::module;
use nest_rs::mcp::McpOperationGuard;

use super::bridge::AuthzMcpBridge;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [
        AuthzMcpBridge as dyn McpOperationGuard,
    ],
)]
pub struct AuthzMcpModule;
