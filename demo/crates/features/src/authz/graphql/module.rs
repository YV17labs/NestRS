use nest_rs::core::module;
use nest_rs::graphql::{GraphqlView, forward_principal};
use nest_rs::http::EndpointGuards;

use crate::Claims;
use crate::authn::AuthnGuard;
use crate::authz::{AuthzGuard, AuthzModule};

#[module(
    imports = [AuthzModule],
    providers = [EndpointGuards<GraphqlView, (AuthnGuard, AuthzGuard)>],
)]
pub struct AuthzGraphqlModule;

forward_principal!(Claims);
