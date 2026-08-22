use nest_rs::core::module;
use nest_rs::mcp::{McpOperationGuard, McpToolContext};
use nest_rs::seaorm::mcp::McpDataContext;

use super::bridge::AuthzMcpBridge;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [
        AuthzMcpBridge as dyn McpOperationGuard,
        McpDataContext as dyn McpToolContext,
    ],
)]
pub struct AuthzMcpModule;
