use nest_rs::core::module;
use nest_rs::mcp::{McpOperationGuard, McpToolContext};
use nest_rs::seaorm::mcp::McpDataContext;

use super::bridge::AppMcpGuard;
use crate::app_authz::http::AppAuthzHttpModule;

#[module(
    imports = [AppAuthzHttpModule],
    providers = [
        AppMcpGuard as dyn McpOperationGuard,
        McpDataContext as dyn McpToolContext,
    ],
)]
pub struct AppAuthzMcpModule;
