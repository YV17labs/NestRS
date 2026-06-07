use nest_rs_core::module;
use nest_rs_graphql::{BatchContext, OperationGuard};
use nest_rs_seaorm::graphql::LoaderScope;

use super::bridge::AppGraphqlGuard;
use super::guard::GraphqlAuthGuard;
use crate::Claims;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn OperationGuard,
        GraphqlAuthGuard,
        LoaderScope as dyn BatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

// Gated on `GraphqlAuthGuard` so the forwarder is silent in apps that do not
// import this module — keeps a second GraphQL app with a different principal
// type from double-forwarding.
nest_rs_graphql::forward_principal!(Claims, GraphqlAuthGuard);
