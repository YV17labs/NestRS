use nest_rs_authz::mcp::McpAbilityBridge;

use crate::authn::AuthGuard;
use crate::authz::http::AuthzGuard;

/// The app's MCP operation guard: authenticate with `AuthGuard`, then scope the
/// request to the caller's ability with `AuthzGuard`. Wired as
/// `dyn McpOperationGuard` — mirrors `AppGraphqlGuard` on the GraphQL side.
pub type AppMcpGuard = McpAbilityBridge<AuthGuard, AuthzGuard>;
