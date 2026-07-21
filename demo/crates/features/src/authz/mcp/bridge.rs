use nest_rs_authz::mcp::McpAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::http::AuthzGuard;

pub type AppMcpGuard = McpAbilityBridge<AuthnGuard, AuthzGuard>;
