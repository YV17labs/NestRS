use nest_rs::core::module;
use nest_rs::http::EndpointGuards;
use nest_rs::mcp::McpView;

use crate::authn::AuthnGuard;
use crate::authz::{AuthzGuard, AuthzModule};

#[module(
    imports = [AuthzModule],
    providers = [EndpointGuards<McpView, (AuthnGuard, AuthzGuard)>],
)]
pub struct AuthzMcpModule;
