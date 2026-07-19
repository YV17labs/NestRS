use nest_rs_authz::mcp::McpAbilityBridge;

use crate::authn::AuthGuard;
use crate::authz::http::AuthzGuard;

pub type AppMcpGuard = McpAbilityBridge<AuthGuard, AuthzGuard>;
