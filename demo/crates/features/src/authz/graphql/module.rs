use nest_rs::core::module;
use nest_rs::graphql::{GraphqlBatchContext, GraphqlOperationGuard, forward_principal};
use nest_rs::seaorm::graphql::LoaderScope;

use super::bridge::AuthzGraphqlBridge;
use crate::Claims;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [
        AuthzGraphqlBridge as dyn GraphqlOperationGuard,
        LoaderScope as dyn GraphqlBatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

forward_principal!(Claims);
