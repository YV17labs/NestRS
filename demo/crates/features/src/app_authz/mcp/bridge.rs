use nest_rs::authz::mcp::McpAbilityBridge;

use crate::app_authn::AppAuthnGuard;
use crate::app_authz::http::AppAuthzGuard;

pub type AppMcpGuard = McpAbilityBridge<AppAuthnGuard, AppAuthzGuard>;
