use nest_rs::authz::mcp::McpAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;

pub type AuthzMcpBridge = McpAbilityBridge<AuthnGuard, AuthzGuard>;
