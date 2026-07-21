use nest_rs_core::module;
use nest_rs_mcp::{McpOperationGuard, McpToolContext};
use nest_rs_seaorm::mcp::McpDataContext;

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
