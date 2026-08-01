use nest_rs::core::module;
use nest_rs::mcp::{McpOperationGuard, McpToolContext};
use nest_rs::seaorm::mcp::McpDataContext;

use super::bridge::AppMcpGuard;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppMcpGuard as dyn McpOperationGuard,
        McpDataContext as dyn McpToolContext,
    ],
)]
pub struct AuthzMcpModule;
