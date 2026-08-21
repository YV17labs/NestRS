use nest_rs::core::module;
use nest_rs::graphql::{GraphqlBatchContext, GraphqlOperationGuard, forward_principal};
use nest_rs::seaorm::graphql::LoaderScope;

use super::bridge::AppGraphqlGuard;
use crate::Claims;
use crate::app_authz::http::AppAuthzHttpModule;

#[module(
    imports = [AppAuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn GraphqlOperationGuard,
        LoaderScope as dyn GraphqlBatchContext,
    ],
)]
pub struct AppAuthzGraphqlModule;

forward_principal!(Claims);
