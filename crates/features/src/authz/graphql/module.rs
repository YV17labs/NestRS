use nest_rs_core::module;
use nest_rs_graphql::{GraphqlBatchContext, GraphqlOperationGuard};
use nest_rs_seaorm::graphql::LoaderScope;

use super::bridge::AppGraphqlGuard;
use super::guard::GraphqlAuthGuard;
use crate::Claims;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn GraphqlOperationGuard,
        GraphqlAuthGuard,
        LoaderScope as dyn GraphqlBatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

nest_rs_graphql::forward_principal!(Claims, GraphqlAuthGuard);
