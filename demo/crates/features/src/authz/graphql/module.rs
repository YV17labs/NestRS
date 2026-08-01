use nest_rs::core::module;
use nest_rs::graphql::{GraphqlBatchContext, GraphqlOperationGuard};
use nest_rs::seaorm::graphql::LoaderScope;

use super::bridge::AppGraphqlGuard;
use super::guard::GraphqlAuthnGuard;
use crate::Claims;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn GraphqlOperationGuard,
        GraphqlAuthnGuard,
        LoaderScope as dyn GraphqlBatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

nest_rs::graphql::forward_principal!(Claims, GraphqlAuthnGuard);
