use nest_rs::core::module;
use nest_rs::graphql::{GraphqlOperationGuard, forward_principal};

use super::bridge::AuthzGraphqlBridge;
use crate::Claims;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [AuthzGraphqlBridge as dyn GraphqlOperationGuard],
)]
pub struct AuthzGraphqlModule;

forward_principal!(Claims);
